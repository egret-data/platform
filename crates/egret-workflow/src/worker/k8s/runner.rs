//! [`KubernetesRunner`]: the lifecycle of one task pod (spec §5).
//!
//! ```text
//! run ─ build ─ get-or-create ─┬─ wait for files-in running ─ upload + verify + `ready`
//!                              └─ wait for main started
//!       follow main's log (timestamps; sinceTime on re-attach) ─ ingest_line ─ RunLog
//!       main terminated → exit code ─ download outputFiles (exec + tar)
//! commit the outcome ─ release: delete / `ended`
//! ```
//!
//! # What ends a pod, and what never does
//!
//! * **`release`, after the run's outcome COMMITTED** (final review I3) —
//!   deletes the pod with grace 0 when `taskRunner.delete` (default `true`);
//!   with `delete: false` it uploads the `ended` marker instead, so the sidecar
//!   exits and the pod completes rather than holding its resources for ever.
//!   **`run` returning deletes nothing**: if it did, a commit that failed
//!   after the task finished would leave no pod, and the next claim would run
//!   the task again. As it is, the next claim re-attaches to the finished pod
//!   and collects the same outcome; a crash after the commit leaves a pod of a
//!   terminal run, which the orphan sweep deletes.
//! * **`cancel`** (a timeout, a kill) — delete with grace 0.
//! * **Dropping the `run` future deletes NOTHING.** That is the carry from W5a
//!   Task 3: a worker that loses its lease has its handler future dropped, and
//!   the next holder re-attaches to the same pod by name. There is no `Drop`
//!   impl anywhere on this path, deliberately.
//! * **A worker failure** (`Err(Error::Internal)`: the API server did not
//!   answer, a helper image is missing) keeps the pod, for the same reason:
//!   the run stays leased, the reaper returns it, and the next claim
//!   re-attaches.
//! * **The orphan sweep** ([`sweep_orphans`]) deletes what is left of a run
//!   that ended while nobody was attached.
//!
//! # Two streams
//!
//! `pods/log` merges a container's stdout and stderr, so `main`'s stderr is
//! carried separately (fix round 1, Important 1): when the interpreter is a
//! shell, `main`'s joined script starts with `exec 2>>/egret/streams/stderr`
//! ([`pod_spec::main_argv`]), `files-out` streams that file to its own log
//! with `tail -F`, and egret follows `files-out`'s log as stderr
//! (`is_err = true`, ERROR, counted in `stdErrLineCount`), ending on a
//! per-call sentinel it appends once `main` has terminated. A non-shell
//! interpreter keeps the merged stream ([`pod_spec::stderr_is_separable`]).
//!
//! # Re-attach and `sinceTime`
//!
//! `get-or-create` looks the pod up by [`pod_name`] — **(run, attempt)** —
//! first. Found: this call adopts it and follows both streams from
//! `sinceTime` = ⌊newest `task_logs.ts` stored for this attempt⌋ −
//! [`REATTACH_MARGIN`] ([`reattach_since`]); the stored lines of the overlap
//! are dropped by exact text ([`Replayed`]). See [`REATTACH_MARGIN`] for
//! exactly what is guaranteed: no line is lost while read latency plus clock
//! skew stays under the margin; a few lines may repeat (spec §11).
//!
//! Within one call a dropped log stream is re-followed from one second before
//! the newest kubelet timestamp read (`timestamps=true`), and exactly the
//! `(timestamp, line)` pairs already read are skipped ([`Seen`]) — not every
//! line before a high-water mark, because the kubelet's timestamps are not
//! ordered across stdout and stderr (measured; see [`Seen`]).

use std::collections::BTreeMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use egret_core::{Error, Result};
use futures_util::io::AsyncBufReadExt as _;
use k8s_openapi::api::core::v1::{ContainerStatus, Pod};
use kube::api::{Api, AttachParams, DeleteParams, ListParams, LogParams, PostParams};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

use super::pod_spec::{
    self, attempt_of, pod_name, PodConfig, DELETE_ANNOTATION, ENDED_MARKER, FILES_IN, FILES_OUT,
    MAIN, MARKERS_DIR, READY_MARKER, STDERR_FILE,
};
use crate::logs::{LogLevel, LogRepo, RunIdentity};
use crate::scripts::RunnerSpec;
use crate::storage::transfer::{write_tar, TarError, TarSource};
use crate::worker::commands::{
    collection_target, expand_outputs, listing_too_big, output_problem, pattern_dirs, run_liveness,
    staged_name_problem, staging_error, stream_replayed_line, sweep_decision, CommandsResult,
    LineObserver, OrphanSweep, StreamState, TaskCommands, TaskRunner, CANCEL_WAIT_MARGIN,
    MAX_LISTING_BYTES, MAX_OUTPUT_NAMES, MAX_VARS_BYTES, OUTPUT_DIR, OUTPUT_DIR_REL,
    RUN_NAME_PREFIX, TASKRUN_LABEL, WORKING_DIR,
};
use crate::worker::TaskInput;

/// How long one exec (a tar in or out, a verification, a marker) may take
/// before egret calls its own transfer failed.
const EXEC_TIMEOUT: Duration = Duration::from_secs(120);
/// How many times an upload is attempted when the exec itself fails (Kestra's
/// `UPLOAD_RETRY_MAX_ATTEMPTS` is 5; egret's pod is fresh and local).
const UPLOAD_ATTEMPTS: u32 = 3;
/// How long a container may sit in `ErrImagePull` before the pull is judged
/// to have failed for good (final review I4). `ErrImagePull` is the first,
/// retried, failure of a pull and normally turns into `ImagePullBackOff`
/// (terminal below) within seconds; this bounds the case where it does not.
const PULL_FAILURE_BOUND: Duration = Duration::from_secs(90);
/// A waiting reason after which a container will not start without a change
/// to its spec. `ErrImagePull` is left out: it is the first, retried, failure
/// of a pull and becomes `ImagePullBackOff` when it persists
/// ([`PULL_FAILURE_BOUND`] covers it otherwise).
const START_FAILURES: &[&str] = &[
    "ImagePullBackOff",
    "ErrImageNeverPull",
    "InvalidImageName",
    "CreateContainerConfigError",
    "CreateContainerError",
    "RunContainerError",
];

/// `io.kestra.plugin.ee.kubernetes.runner.Kubernetes`: one pod per task-run
/// attempt. See the module doc for the lifecycle and [`super`] for the Kestra
/// mapping.
#[derive(Clone)]
pub struct KubernetesRunner {
    client: kube::Client,
    cfg: PodConfig,
    /// Where a re-attach reads its `sinceTime` from.
    logs: LogRepo,
    max_vars_bytes: usize,
    poll: Duration,
    /// Set once the first `pods/exec` 403 has been logged at ERROR (final
    /// review I4): the operator gets one loud line with the RBAC hint, and
    /// every task that meets the same refusal fails naming it.
    exec_forbidden_logged: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Internal storage, which stored inputs are streamed from (W9a Task 5)
    /// and output files into (Task 6).
    files: Option<std::sync::Arc<crate::storage::FileStore>>,
    /// How long a kept (`delete: false`) pod outlives its run before the
    /// orphan sweep deletes it (workflow debt D3). `None`: kept for ever.
    /// [`DockerRunner::with_kept_retention`]'s field.
    ///
    /// [`DockerRunner::with_kept_retention`]: crate::worker::commands::DockerRunner::with_kept_retention
    kept_retention: Option<Duration>,
}

impl std::fmt::Debug for KubernetesRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KubernetesRunner")
            .field("namespace", &self.cfg.namespace)
            .finish_non_exhaustive()
    }
}

impl KubernetesRunner {
    /// `logs` is required, not optional: without it a re-attach would replay
    /// the whole of `main`'s log into the attempt a second time.
    pub fn new(client: kube::Client, cfg: PodConfig, logs: LogRepo) -> Self {
        Self {
            client,
            cfg,
            logs,
            max_vars_bytes: MAX_VARS_BYTES,
            poll: Duration::from_millis(500),
            exec_forbidden_logged: Default::default(),
            files: None,
            kept_retention: None,
        }
    }

    /// The same runner, whose orphan sweep deletes a kept (`delete: false`)
    /// pod once its run has been terminal longer than `retention`; `None`
    /// keeps them for ever ([`DockerRunner::with_kept_retention`]).
    ///
    /// [`DockerRunner::with_kept_retention`]: crate::worker::commands::DockerRunner::with_kept_retention
    pub fn with_kept_retention(mut self, retention: Option<Duration>) -> Self {
        self.kept_retention = retention;
        self
    }

    /// The same runner, streaming stored inputs from `files` -- what
    /// production builds when `[storage]` is configured (`runtime::handlers_for`).
    pub fn with_files(mut self, files: std::sync::Arc<crate::storage::FileStore>) -> Self {
        self.files = Some(files);
        self
    }

    /// A smaller markers cap, for a test ([`DockerRunner::with_vars_cap`]'s
    /// reason). The output files' caps are the [`FileStore`]'s.
    ///
    /// [`DockerRunner::with_vars_cap`]: crate::worker::commands::DockerRunner::with_vars_cap
    /// [`FileStore`]: crate::storage::FileStore
    pub fn with_vars_cap(mut self, max_vars_bytes: usize) -> Self {
        self.max_vars_bytes = max_vars_bytes;
        self
    }

    pub fn config(&self) -> &PodConfig {
        &self.cfg
    }

    fn pods(&self) -> Api<Pod> {
        Api::namespaced(self.client.clone(), &self.cfg.namespace)
    }

    /// The pod by name; `Ok(false)` when it already existed (re-attach),
    /// `Ok(true)` when this call created it.
    async fn get_or_create(&self, name: &str, pod: &Pod) -> Result<bool> {
        if let Some(existing) = self
            .pods()
            .get_opt(name)
            .await
            .map_err(|e| internal("reading the task pod", e))?
        {
            if existing.metadata.deletion_timestamp.is_some() {
                return Err(Error::InvalidInput(format!(
                    "the task's pod {name} is being deleted (the run was cancelled)"
                )));
            }
            return Ok(false);
        }
        match self.pods().create(&PostParams::default(), pod).await {
            Ok(_) => Ok(true),
            // Another holder created it between the read and the create.
            Err(kube::Error::Api(ae)) if ae.code == 409 => Ok(false),
            // The API refused the pod as written: an author's value (an env
            // var name, an image reference) is the likely cause.
            Err(kube::Error::Api(ae)) if ae.code == 400 || ae.code == 422 => Err(
                Error::InvalidInput(format!("Kubernetes refused the task's pod: {}", ae.message)),
            ),
            Err(e) => Err(internal("creating the task pod", e)),
        }
    }

    async fn get(&self, name: &str) -> Result<Pod> {
        match self.pods().get_opt(name).await {
            Ok(Some(p)) => Ok(p),
            Ok(None) => Err(deleted(name)),
            Err(e) => Err(internal("reading the task pod", e)),
        }
    }

    async fn delete_pod(&self, name: &str) -> Result<()> {
        match self
            .pods()
            .delete(name, &DeleteParams::default().grace_period(0))
            .await
        {
            Ok(_) => Ok(()),
            Err(kube::Error::Api(ae)) if ae.code == 404 => Ok(()),
            Err(e) => Err(internal("deleting the task pod", e)),
        }
    }

    /// The graceful half of [`TaskRunner::cancel`]: delete `name` with
    /// `gracePeriodSeconds = secs`, then poll until it is gone or `main` has
    /// terminated. **Unbounded here; the caller bounds the whole of it**, so
    /// that no single API call -- a delete or a read the API server never
    /// answers, which the kube client would wait out for its own read
    /// timeout -- can hold `cancel` past `grace` + [`CANCEL_WAIT_MARGIN`]. A
    /// read that fails is retried: the delete has been issued, and an early
    /// return would let `release`'s grace-0 delete cut the grace short.
    async fn delete_and_wait(&self, name: &str, secs: u32) -> Result<()> {
        match self
            .pods()
            .delete(name, &DeleteParams::default().grace_period(secs))
            .await
        {
            Ok(_) => {}
            Err(kube::Error::Api(ae)) if ae.code == 404 => return Ok(()),
            Err(e) => return Err(internal("deleting the task pod", e)),
        }
        loop {
            match self.pods().get_opt(name).await {
                Ok(None) => return Ok(()),
                Ok(Some(p)) if matches!(phase(status_of(&p, MAIN)), Phase::Terminated { .. }) => {
                    return Ok(())
                }
                Ok(Some(_)) => {}
                Err(e) => {
                    tracing::debug!(pod = %name, "waiting for the cancelled task pod to stop: {e}")
                }
            }
            tokio::time::sleep(self.poll).await;
        }
    }

    /// Let `files-out` exit, so a `delete: false` pod completes.
    async fn signal_ended(&self, name: &str) {
        let marker = format!("{MARKERS_DIR}/{ENDED_MARKER}");
        match self
            .exec(name, FILES_OUT, vec!["touch".into(), marker], None, 4096)
            .await
        {
            Ok(o) if o.success => {}
            Ok(o) => {
                tracing::warn!(pod = %name, "could not signal files-out to exit: {}", o.stderr)
            }
            Err(e) => tracing::warn!(pod = %name, "could not signal files-out to exit: {e}"),
        }
    }

    /// Everything between having a pod and returning a result.
    async fn drive(
        &self,
        task: &TaskInput,
        cmds: &TaskCommands,
        observer: &dyn LineObserver,
        name: &str,
        reattached: bool,
    ) -> Result<CommandsResult> {
        // ---- start: upload the inputs, wait for main ---------------------
        let mut uploaded = !cmds.has_inputs();
        let mut pulling_since: Option<tokio::time::Instant> = None;
        loop {
            let p = self.get(name).await?;
            if let Some(e) = start_failure(&p) {
                return Err(e);
            }
            match pull_error(&p) {
                Some(cs) => {
                    let since = *pulling_since.get_or_insert_with(tokio::time::Instant::now);
                    if since.elapsed() >= PULL_FAILURE_BOUND {
                        return Err(start_error(
                            cs,
                            "ErrImagePull",
                            &format!(
                                "the image could not be pulled for {PULL_FAILURE_BOUND:?}: {}",
                                waiting_message(cs)
                            ),
                        ));
                    }
                }
                None => pulling_since = None,
            }
            if matches!(
                phase(status_of(&p, MAIN)),
                Phase::Running | Phase::Terminated { .. }
            ) {
                break;
            }
            if !uploaded {
                match phase(status_of(&p, FILES_IN)) {
                    Phase::Running => {
                        self.upload(name, cmds).await?;
                        uploaded = true;
                    }
                    // A previous holder uploaded and `files-in` has exited.
                    Phase::Terminated { exit: 0, .. } => uploaded = true,
                    _ => {}
                }
            }
            tokio::time::sleep(self.poll).await;
        }

        // ---- follow main's stdout, and its stderr through files-out --------
        //
        // Two producers, one consumer: every line of both streams reaches
        // `stream_replayed_line` from ONE place, so the `StreamState` (the markers' vars
        // cap, the `##kestra:log:debug##` bracket) is shared across the streams
        // as W11 requires. The order BETWEEN the streams is best-effort: each
        // stream is in order, but a stderr line reaches egret up to `tail -F`'s
        // one-second poll after the stdout lines around it (Kestra's Docker
        // runner does not order across its two streams either).
        // **The re-attach window is read BEFORE egret writes anything to this
        // attempt's log** (fix round 2). `last_ts` is "the newest stored line of
        // the attempt", whoever wrote it: a line this call writes first — the
        // WARN below — can be flushed by the batching writer before the query
        // runs, and `sinceTime` would then be THIS holder's now − margin,
        // dropping every line `main` wrote while nobody was attached. Nothing
        // on the path from `run` to here writes to `task.log`.
        let (since, mut replayed) = if reattached {
            self.reattach_window(task).await?
        } else {
            (None, Replayed::default())
        };
        let split = pod_spec::stderr_is_separable(cmds);
        if !split {
            task.log
                .line(
                    LogLevel::Warn,
                    "The interpreter is not a shell, so under the Kubernetes task runner this \
                     task's stderr cannot be separated from its stdout: its stderr lines are \
                     logged at INFO and counted as stdout.",
                )
                .await;
        }
        let mut result = CommandsResult::default();
        let mut state = StreamState::default();
        let sentinel = format!("{STDERR_END_PREFIX}{}##", uuid::Uuid::new_v4().simple());
        let (tx, mut rx) = tokio::sync::mpsc::channel::<(bool, String)>(1024);
        let tx_err = split.then(|| tx.clone());
        let tx_out = tx;
        let (done_tx, done_rx) = tokio::sync::watch::channel(Pump::Running);

        let stdout = async {
            let r = self.pump_stdout(name, since, &tx_out).await;
            drop(tx_out);
            let (mut evicted, mut ended) = (None, None);
            if r.is_ok() {
                // An evicted pod's helpers are gone too: no exec can reach
                // them, so this is decided BEFORE any exec (final review I5).
                evicted = self.eviction(name).await;
                if evicted.is_none() && split {
                    // Everything `main` wrote is in the file now: close the
                    // stream.
                    ended = self.end_stderr(name, &sentinel).await;
                }
            }
            let fine = r.is_ok() && evicted.is_none() && ended.is_none();
            let _ = done_tx.send(if fine { Pump::Done } else { Pump::Failed });
            (r, evicted, ended)
        };
        let stderr = async {
            let Some(tx_err) = tx_err else { return };
            let mut done_rx = done_rx;
            tokio::select! {
                _ = self.pump_stderr(name, since, &sentinel, tx_err) => {}
                _ = async {
                    loop {
                        let now = *done_rx.borrow();
                        match now {
                            Pump::Failed => return,
                            Pump::Done => break,
                            Pump::Running => {}
                        }
                        if done_rx.changed().await.is_err() {
                            return;
                        }
                    }
                    tokio::time::sleep(STDERR_DRAIN).await;
                    tracing::warn!(pod = %name, "main's stderr stream did not end within {STDERR_DRAIN:?}");
                } => {}
            }
        };
        let consume = async {
            while let Some((is_err, line)) = rx.recv().await {
                // Compared as stored, not as read (W5b fix round 1, M1).
                stream_replayed_line(
                    task,
                    observer,
                    &mut result,
                    &mut state,
                    self.max_vars_bytes,
                    &line,
                    is_err,
                    cmds.host_protocol,
                    &mut replayed,
                )
                .await;
            }
        };
        let ((out, evicted, ended), (), ()) = tokio::join!(stdout, stderr, consume);
        let (exit, reason) = out?;
        result.exit_code = exit;
        if let Some(why) = evicted {
            result.failure = Some(why);
            return Ok(result);
        }
        if let Some(e) = ended {
            return Err(e);
        }

        // ---- outputs --------------------------------------------------------
        if cmds.has_outputs() && result.failure.is_none() {
            self.download(name, cmds, &mut result).await?;
        }
        if exit != 0 && reason.as_deref() == Some("OOMKilled") && result.failure.is_none() {
            result.failure = Some(format!(
                "`main` was OOMKilled (exit code {exit}): the command used more memory than \
                 its limit (`taskRunner.resources.limit.memory`, capped by egret's maximum)"
            ));
        }
        Ok(result)
    }

    /// Follow `main`'s log until `main` has terminated; its exit code and
    /// reason.
    async fn pump_stdout(
        &self,
        name: &str,
        mut since: Option<DateTime<Utc>>,
        tx: &tokio::sync::mpsc::Sender<(bool, String)>,
    ) -> Result<(i32, Option<String>)> {
        let mut seen = Seen::default();
        loop {
            self.follow(name, MAIN, since, &mut seen, tx, false, None)
                .await?;
            let p = self.get(name).await?;
            if let Phase::Terminated { exit, reason } = phase(status_of(&p, MAIN)) {
                return Ok((exit, reason));
            }
            if let Some(e) = start_failure(&p) {
                return Err(e);
            }
            // The stream ended with main still running: a dropped connection.
            since = seen.resume_from().or(since);
            tokio::time::sleep(self.poll).await;
        }
    }

    /// Follow `files-out`'s log — `main`'s stderr, streamed by `tail -F` —
    /// until this call's sentinel arrives. Bounded by the caller.
    async fn pump_stderr(
        &self,
        name: &str,
        mut since: Option<DateTime<Utc>>,
        sentinel: &str,
        tx: tokio::sync::mpsc::Sender<(bool, String)>,
    ) {
        let mut seen = Seen::default();
        loop {
            match self
                .follow(name, FILES_OUT, since, &mut seen, &tx, true, Some(sentinel))
                .await
            {
                Ok(true) => return,
                Ok(false) => {}
                Err(e) => {
                    tracing::debug!(pod = %name, "following main's stderr: {e}");
                }
            }
            since = seen.resume_from().or(since);
            tokio::time::sleep(self.poll).await;
        }
    }

    /// Append this call's end sentinel to the stderr file, after `main` has
    /// terminated, so the stderr follower knows it has read everything.
    ///
    /// `Some` is a failure no later claim can get past -- the exec was refused
    /// (a `pods/exec` 403, final review I4) or the pod is gone -- and the task
    /// fails with it. Anything else is a WARN, and the stderr follower's
    /// drain bound ends the stream.
    async fn end_stderr(&self, name: &str, sentinel: &str) -> Option<Error> {
        let cmd = vec![
            "sh".to_string(),
            "-c".to_string(),
            format!("printf '%s\\n' \"$1\" >> {STDERR_FILE}"),
            "sh".to_string(),
            sentinel.to_string(),
        ];
        match self.exec(name, FILES_OUT, cmd, None, 4096).await {
            Ok(o) if o.success => None,
            Ok(o) => {
                tracing::warn!(pod = %name, "could not end main's stderr stream: {}", o.stderr);
                None
            }
            Err(e @ Error::InvalidInput(_)) => Some(e),
            Err(e) => {
                tracing::warn!(pod = %name, "could not end main's stderr stream: {e}");
                None
            }
        }
    }

    /// Why the pod was evicted, if the kubelet evicted it -- the pod's own
    /// message, e.g. *Pod ephemeral local storage usage exceeds the total
    /// limit of containers 10Gi* or *Usage of EmptyDir volume "working-dir"
    /// exceeds the limit* (final review I5). A read that fails is `None`: the
    /// next step meets the same API and decides.
    async fn eviction(&self, name: &str) -> Option<String> {
        match self.pods().get_opt(name).await {
            Ok(Some(p)) => eviction_of(&p),
            _ => None,
        }
    }

    /// One `pods/log?follow` stream of `container`, to its end, each new line
    /// sent as `(is_err, line)`. `Ok(true)`: `sentinel` was read.
    #[allow(clippy::too_many_arguments)]
    async fn follow(
        &self,
        name: &str,
        container: &str,
        since: Option<DateTime<Utc>>,
        seen: &mut Seen,
        tx: &tokio::sync::mpsc::Sender<(bool, String)>,
        is_err: bool,
        sentinel: Option<&str>,
    ) -> Result<bool> {
        let lp = LogParams {
            container: Some(container.to_string()),
            follow: true,
            timestamps: true,
            since_time: since,
            ..Default::default()
        };
        let stream = match self.pods().log_stream(name, &lp).await {
            Ok(s) => s,
            Err(kube::Error::Api(ae)) if ae.code == 404 => return Err(deleted(name)),
            // "container … is waiting to start" / "terminated": the caller
            // re-reads the pod and decides.
            Err(kube::Error::Api(ae)) if ae.code == 400 => return Ok(false),
            Err(e) => return Err(internal("following a task pod's log", e)),
        };
        let mut stream = Box::pin(stream);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match stream.read_until(b'\n', &mut buf).await {
                Ok(0) => return Ok(false),
                Ok(_) => {}
                Err(e) => {
                    tracing::debug!(pod = %name, "the log stream broke, re-following: {e}");
                    return Ok(false);
                }
            }
            let raw = String::from_utf8_lossy(&buf);
            let raw = raw.strip_suffix('\n').unwrap_or(&raw);
            let (ts, line) = split_timestamp(raw);
            if !seen.admit(ts, line) {
                continue;
            }
            if let Some(end) = sentinel {
                if line == end {
                    return Ok(true);
                }
                // A previous holder's sentinel (another nonce): not a line.
                if line.starts_with(STDERR_END_PREFIX) {
                    continue;
                }
            }
            if tx.send((is_err, line.to_string())).await.is_err() {
                return Ok(false);
            }
        }
    }

    /// Where a re-attach resumes `main`'s streams, and which of the lines it
    /// will see again were already stored by the previous holder.
    ///
    /// `sinceTime` = ⌊newest stored `ts`⌋ − [`REATTACH_MARGIN`]
    /// ([`reattach_since`]); the stored lines whose `ts` is at or after
    /// ⌊newest⌋ are the overlap that is dropped by exact text
    /// ([`Replayed`]). No stored line: from the container's start.
    async fn reattach_window(&self, task: &TaskInput) -> Result<(Option<DateTime<Utc>>, Replayed)> {
        reattach_window(&self.logs, task).await
    }

    /// Upload the input files into `files-in`, verify them, then `ready`.
    async fn upload(&self, name: &str, cmds: &TaskCommands) -> Result<()> {
        let mut tries = 0;
        loop {
            tries += 1;
            match self.upload_once(name, cmds).await {
                Err(Error::Internal(e)) if tries < UPLOAD_ATTEMPTS => {
                    tracing::warn!(pod = %name, "uploading the input files failed, retrying: {e}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                other => return other,
            }
        }
    }

    async fn upload_once(&self, name: &str, cmds: &TaskCommands) -> Result<()> {
        let sources = cmds.tar_sources()?;
        let out = self
            .exec_within(
                name,
                FILES_IN,
                extract_command(),
                Some(&sources),
                64 * 1024,
                Limit::Idle(UPLOAD_IDLE),
            )
            .await?;
        extract_verdict(&out)?;

        // Kestra's `verifyFileUpload`: `wc -c < <file>` per file, compared
        // with what was sent.
        // A directory (`outputDirectory`'s) has no size to verify.
        let files: Vec<(&String, &TarSource<'_>)> = sources
            .iter()
            .filter(|(_, s)| !matches!(s, TarSource::Directory))
            .collect();
        let script: String = files
            .iter()
            .map(|(rel, _)| format!("wc -c < {}\n", shell_quote(&format!("{WORKING_DIR}/{rel}"))))
            .collect();
        let out = self
            .exec(
                name,
                FILES_IN,
                vec!["sh".into(), "-c".into(), script],
                None,
                64 * 1024,
            )
            .await?;
        let found: Vec<u64> = String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .filter_map(|n| n.parse().ok())
            .collect();
        let expected: Vec<u64> = files
            .iter()
            .map(|(_, s)| match s {
                TarSource::Contents(c) => c.len() as u64,
                TarSource::Stored(st) => st.size,
                TarSource::Directory => 0,
            })
            .collect();
        if !out.success || found != expected {
            return Err(internal(
                "verifying the input files",
                format!(
                    "expected {expected:?} bytes but found {found:?} in files-in — the tar \
                     transfer was likely truncated ({})",
                    out.stderr.trim()
                ),
            ));
        }

        let marker = format!("{MARKERS_DIR}/{READY_MARKER}");
        let out = self
            .exec(name, FILES_IN, vec!["touch".into(), marker], None, 4096)
            .await?;
        if !out.success {
            return Err(internal("uploading the ready marker", out.stderr.trim()));
        }
        Ok(())
    }

    /// Each declared output, one `tar -c` out of `files-out` apiece, read as
    /// a stream ([`TarReader`]) straight into internal storage
    /// ([`collect_declared`]); then, with `outputDirectory: true`, the whole
    /// [`OUTPUT_DIR`] the same way ([`collect_directory`]). W9a Task 6
    /// replaced W5a's in-memory `unpack_one` + `judge` (the archive held whole
    /// under the 1 MiB cap): nothing is held whole now, and a link entry has
    /// no contents to take, so it is refused by name.
    ///
    /// An author's problem lands in `result.failure`, exit code and vars
    /// intact. So does a `tar -c` that **exited non-zero** part-way through a
    /// file -- the pod's failure, naming the file -- but only that exit: a
    /// lost status, a kubelet-side failure or a stalled stream is egret's
    /// (`Err`), and so is the store not answering (spec §11).
    ///
    /// [`TarReader`]: crate::storage::transfer::TarReader
    async fn download(
        &self,
        name: &str,
        cmds: &TaskCommands,
        result: &mut CommandsResult,
    ) -> Result<()> {
        use crate::storage::collect::{collect_declared, collect_directory, CollectError};
        use crate::storage::transfer::TarReader;
        for rel in &cmds.output_files {
            if let Some(problem) = output_problem(rel) {
                result.failure = Some(problem);
                return Ok(());
            }
        }
        let (fs, to) = match collection_target(self.files.as_ref(), cmds) {
            Ok(both) => both,
            Err(problem) => {
                result.failure = Some(problem);
                return Ok(());
            }
        };
        // Patterns (W5a Task 5 fix round 1): list each directory a pattern
        // names in `files-out` -- `ls -A1p`, so a directory carries a `/` and
        // is dropped (Kestra's glob visits files only) -- then read every
        // match back like a literal name.
        let mut listed = std::collections::BTreeMap::new();
        for dir in pattern_dirs(&cmds.output_files) {
            let path = if dir.is_empty() {
                WORKING_DIR.to_string()
            } else {
                format!("{WORKING_DIR}/{dir}")
            };
            let out = self
                .exec(
                    name,
                    FILES_OUT,
                    vec!["ls".into(), "-A1p".into(), "--".into(), path],
                    None,
                    MAX_LISTING_BYTES + 1,
                )
                .await?;
            if out.stdout.len() > MAX_LISTING_BYTES {
                result.failure = Some(listing_too_big(MAX_LISTING_BYTES));
                return Ok(());
            }
            // A directory that does not exist lists nothing: its pattern
            // matches nothing, as Kestra's.
            let names: Vec<String> = if out.success {
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .filter(|l| !l.is_empty() && !l.ends_with('/'))
                    .map(str::to_string)
                    .collect()
            } else {
                Vec::new()
            };
            listed.insert(dir, names);
        }
        let names = match expand_outputs(&cmds.output_files, &listed, MAX_OUTPUT_NAMES) {
            Ok(n) => n,
            Err(problem) => {
                result.failure = Some(problem);
                return Ok(());
            }
        };
        let mut so_far = 0u64;
        let mut names_left = MAX_OUTPUT_NAMES.saturating_sub(names.len());
        for (rel, from_glob) in &names {
            // An EXPANDED name is re-validated like a declared one (fix round
            // 2): a file named `x\n..` lists as the line `..`, which `*`
            // matches, and must never reach `tar -C <wd> -- ..`.
            if let Some(problem) = output_problem(rel) {
                result.failure = Some(problem);
                return Ok(());
            }
            let mut out = self.exec_out(name, tar_create(rel)).await?;
            let got = {
                let mut reader = TarReader::new(&mut out.stdout);
                until_idle(
                    collect_declared(&fs, &to, rel, *from_glob, &mut reader, &mut so_far),
                    &out.progress,
                    EXEC_TIMEOUT,
                )
                .await
            };
            match got {
                None => {
                    out.abort().await;
                    return Err(stalled(rel));
                }
                Some(Ok(file)) => {
                    let exit = out.finish().await;
                    match file {
                        Some(file) => {
                            result.output_files.insert(rel.clone(), file);
                        }
                        // Nothing archived: the file is not there (tar -c
                        // EXITED non-zero saying so) -- IGNORED, as Kestra.
                        None if exit.ended() => {}
                        None => return Err(exit.lost(rel)),
                    }
                }
                Some(Err(CollectError::Author(problem))) => {
                    out.abort().await;
                    result.failure = Some(problem);
                    return Ok(());
                }
                Some(Err(CollectError::Store(m))) => {
                    out.abort().await;
                    return Err(Error::Internal(anyhow::anyhow!("{m}")));
                }
                Some(Err(CollectError::Source(m))) => {
                    let exit = out.finish().await;
                    if exit.exited_non_zero() {
                        result.failure = Some(tar_died(rel, &m, &exit.stderr));
                        return Ok(());
                    }
                    return Err(internal(&format!("reading {rel:?} out of {FILES_OUT}"), m));
                }
            }
        }
        if cmds.output_directory {
            let mut out = self.exec_out(name, tar_create(OUTPUT_DIR_REL)).await?;
            let got = {
                let mut reader = TarReader::new(&mut out.stdout);
                until_idle(
                    collect_directory(
                        &fs,
                        &to,
                        &mut reader,
                        &mut so_far,
                        &mut names_left,
                        &mut result.output_files,
                    ),
                    &out.progress,
                    EXEC_TIMEOUT,
                )
                .await
            };
            match got {
                None => {
                    out.abort().await;
                    return Err(stalled(OUTPUT_DIR));
                }
                Some(Ok(())) => {
                    let exit = out.finish().await;
                    if !exit.ended() {
                        return Err(exit.lost(OUTPUT_DIR));
                    }
                }
                Some(Err(CollectError::Author(problem))) => {
                    out.abort().await;
                    result.failure = Some(problem);
                }
                Some(Err(CollectError::Store(m))) => {
                    out.abort().await;
                    return Err(Error::Internal(anyhow::anyhow!("{m}")));
                }
                Some(Err(CollectError::Source(m))) => {
                    let exit = out.finish().await;
                    if exit.exited_non_zero() {
                        result.failure = Some(tar_died(OUTPUT_DIR, &m, &exit.stderr));
                        return Ok(());
                    }
                    return Err(internal(
                        &format!("reading {OUTPUT_DIR} out of {FILES_OUT}"),
                        m,
                    ));
                }
            }
        }
        Ok(())
    }

    /// `cmd` in `files-out`, its stdout handed back as a stream (W9a Task 6)
    /// with every byte recorded as [`Progress`], and its stderr read beside
    /// it (16 KiB kept).
    async fn exec_out(&self, name: &str, cmd: Vec<String>) -> Result<OutExec> {
        let ap = AttachParams::default()
            .container(FILES_OUT)
            .stdin(false)
            .stdout(true)
            .stderr(true)
            .max_stdout_buf_size(64 * 1024)
            .max_stderr_buf_size(64 * 1024);
        let mut proc = match self.pods().exec(name, cmd, &ap).await {
            Ok(p) => p,
            Err(kube::Error::Api(ae)) if ae.code == 404 => return Err(deleted(name)),
            Err(e) if is_forbidden(&e) => return Err(self.exec_forbidden(FILES_OUT, e)),
            Err(e) => return Err(internal(&format!("exec in {FILES_OUT}"), e)),
        };
        let status = proc
            .take_status()
            .ok_or_else(|| internal("exec", "no status channel"))?;
        let stdout = proc
            .stdout()
            .ok_or_else(|| internal("exec", "no stdout channel"))?;
        let stderr = proc.stderr();
        let errs = tokio::spawn(async move { read_capped(stderr, 16 * 1024, true).await.0 });
        let progress = Progress::new();
        Ok(OutExec {
            stdout: ProgressReader {
                inner: Box::new(stdout),
                progress: progress.clone(),
            },
            status: Box::pin(status),
            errs,
            proc,
            progress,
        })
    }
    /// Run `cmd` in `container`, feed it `stdin`, and collect its output.
    async fn exec(
        &self,
        name: &str,
        container: &str,
        cmd: Vec<String>,
        stdin: Option<&BTreeMap<String, TarSource<'_>>>,
        stdout_cap: usize,
    ) -> Result<ExecOut> {
        self.exec_within(
            name,
            container,
            cmd,
            stdin,
            stdout_cap,
            Limit::Total(EXEC_TIMEOUT),
        )
        .await
    }

    /// [`exec`](Self::exec) under a given [`Limit`] -- the upload's is an
    /// IDLE limit (fix round 1, I2): it runs as long as bytes keep moving.
    async fn exec_within(
        &self,
        name: &str,
        container: &str,
        cmd: Vec<String>,
        stdin: Option<&BTreeMap<String, TarSource<'_>>>,
        stdout_cap: usize,
        limit: Limit,
    ) -> Result<ExecOut> {
        let timed_out = |what: String| internal(&format!("exec in {container}"), what);
        match limit {
            Limit::Total(total) => match tokio::time::timeout(
                total,
                self.exec_inner(name, container, cmd, stdin, stdout_cap, None),
            )
            .await
            {
                Ok(r) => r,
                Err(_) => Err(timed_out(format!("no answer within {total:?}"))),
            },
            Limit::Idle(idle) => {
                let progress = Progress::new();
                let run = self.exec_inner(name, container, cmd, stdin, stdout_cap, Some(&progress));
                match until_idle(run, &progress, idle).await {
                    Some(r) => r,
                    None => Err(timed_out(format!(
                        "no byte moved for {idle:?} (the upload stalled)"
                    ))),
                }
            }
        }
    }

    async fn exec_inner(
        &self,
        name: &str,
        container: &str,
        cmd: Vec<String>,
        stdin: Option<&BTreeMap<String, TarSource<'_>>>,
        stdout_cap: usize,
        progress: Option<&Progress>,
    ) -> Result<ExecOut> {
        let ap = AttachParams::default()
            .container(container)
            .stdin(stdin.is_some())
            .stdout(true)
            .stderr(true)
            .max_stdin_buf_size(64 * 1024)
            .max_stdout_buf_size(64 * 1024)
            .max_stderr_buf_size(64 * 1024);
        let mut proc = match self.pods().exec(name, cmd, &ap).await {
            Ok(p) => p,
            Err(kube::Error::Api(ae)) if ae.code == 404 => return Err(deleted(name)),
            Err(e) if is_forbidden(&e) => return Err(self.exec_forbidden(container, e)),
            Err(e) => return Err(internal(&format!("exec in {container}"), e)),
        };
        let status = proc
            .take_status()
            .ok_or_else(|| internal("exec", "no status channel"))?;
        let stdout = proc.stdout();
        let writer = proc.stdin();
        let stderr = proc.stderr();
        let errs = tokio::spawn(async move { read_capped(stderr, 16 * 1024, true).await });
        // stdin is written beside the stdout read, so neither can stall the
        // other. The archive streams from the store as it is written (W9a
        // Task 5); a failure part-way aborts the exec, or `tar -x` would wait
        // for the rest of an archive that is never coming.
        let store = self.files.as_deref();
        let proc_ref = &proc;
        let write = async move {
            let (Some(w), Some(files)) = (writer, stdin) else {
                return Ok(());
            };
            let mut w = ProgressWriter {
                inner: w,
                progress: progress.cloned(),
            };
            let wrote = match write_tar(&mut w, files, store).await {
                Ok(()) => w.shutdown().await.map_err(TarError::Write),
                Err(e) => Err(e),
            };
            if let Some(p) = progress {
                p.touch();
            }
            // Our side stopped part-way (the author's object, or the store):
            // `tar -x` would wait for the rest of the archive for ever. A
            // WRITE failure is the other side ending -- it is not aborted, so
            // its exit status still arrives and decides (fix round 1, I2).
            if matches!(wrote, Err(TarError::Author(_) | TarError::Store(_))) {
                proc_ref.abort();
            }
            wrote
        };
        let read = async move {
            let (out, overflowed) = read_capped(stdout, stdout_cap, false).await;
            if overflowed {
                proc_ref.abort();
            }
            (out, overflowed)
        };
        let (wrote, (out, overflowed)) = tokio::join!(write, read);
        let stderr = errs.await.map(|(b, _)| b).unwrap_or_default();
        let status = if overflowed { None } else { status.await };
        let _ = proc.join().await;
        if !overflowed {
            judge_write(wrote, status.as_ref())?;
        }
        let success = status.as_ref().and_then(|s| s.status.as_deref()) == Some("Success");
        let mut stderr = String::from_utf8_lossy(&stderr).into_owned();
        if let Some(message) = status.as_ref().and_then(|s| s.message.as_deref()) {
            if !success && !message.is_empty() {
                if !stderr.is_empty() {
                    stderr.push_str("; ");
                }
                stderr.push_str(message);
            }
        }
        Ok(ExecOut {
            stdout: out,
            stderr,
            success,
            reason: status.as_ref().and_then(|s| s.reason.clone()),
        })
    }
}

impl KubernetesRunner {
    /// A `pods/exec` 403: the task fails naming the missing permission, and
    /// the first one in this process is logged at ERROR with the same hint
    /// (final review I4). Classed as the task's failure (`InvalidInput`), not
    /// egret's outage (`Internal`): no later claim can get past a Role that
    /// refuses it, so re-attaching would only hold the pod for ever.
    fn exec_forbidden(&self, container: &str, e: kube::Error) -> Error {
        let msg = exec_forbidden_message(&self.cfg.namespace, container, &e);
        if !self
            .exec_forbidden_logged
            .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            tracing::error!(namespace = %self.cfg.namespace, "{msg}");
        }
        Error::InvalidInput(msg)
    }
}

/// Whether the API server refused the request as FORBIDDEN -- a plain API
/// error, or (WebSocket exec) the upgrade answered 403 instead of 101.
fn is_forbidden(e: &kube::Error) -> bool {
    match e {
        kube::Error::Api(ae) => ae.code == 403,
        kube::Error::UpgradeConnection(kube::client::UpgradeConnectionError::ProtocolSwitch(
            status,
        )) => status.as_u16() == 403,
        _ => false,
    }
}

/// The message a `pods/exec` 403 fails the task with: what was refused, and
/// what the operator must grant.
fn exec_forbidden_message(namespace: &str, container: &str, e: &dyn std::fmt::Display) -> String {
    format!(
        "egret may not exec into the task pod's {container} container: the task namespace \
         {namespace:?} Role lacks `pods/exec` `get` (WebSocket exec on Kubernetes >= 1.30 is \
         authorised as `get`) or `create` ({e}). The Role must grant `pods/exec` get and \
         create, as the chart's rbac-tasks.yaml does; an operator must fix it, and retrying \
         the task will not help"
    )
}

/// How the stdout follower ended, for the stderr follower's bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pump {
    Running,
    Done,
    Failed,
}

/// The start of the line egret appends to the stderr file once `main` has
/// terminated; the rest is a per-call nonce, so neither the author nor a
/// previous holder can end this call's stream early. A stderr line starting
/// with this prefix is never logged.
const STDERR_END_PREFIX: &str = "##egret:stderr:end:";
/// How long the stderr follower may take, after `main` terminated, to read
/// the sentinel: `tail -F` polls once a second, so this is ample.
const STDERR_DRAIN: Duration = Duration::from_secs(15);

/// How far before the newest stored line a re-attach resumes.
///
/// A stored line's `ts` is **egret's** clock when the line was read
/// (`RunLog::line`), while `sinceTime` is compared with the **node's** clock
/// at the moment the line was written. The gap is the read latency
/// (kubelet → API server → egret, normally milliseconds, up to seconds under
/// load) plus the skew between the two clocks (NTP-synced hosts: well under a
/// second). 5 s bounds their sum with headroom — plus the floor to the
/// second that `sinceTime`'s resolution forces.
///
/// **What is guaranteed:** no line is lost across a re-attach as long as
/// read latency + (egret clock − node clock) < 5 s. Lines inside the margin
/// that were stored are dropped by exact text ([`Replayed`]); a stored line
/// split into pieces by `RunLog` (over `LOG_PIECE_CHARS`) does not match and
/// is repeated, and so is a line the previous holder read but had not yet
/// stored — the repeats spec §11 accepts.
const REATTACH_MARGIN: chrono::TimeDelta = chrono::TimeDelta::seconds(5);
/// The most stored lines a re-attach loads to drop the overlap with.
const REPLAY_DEDUPE_MAX: i64 = 10_000;

/// `sinceTime` for a re-attach whose newest stored line has `ts = last`.
pub(crate) fn reattach_since(last: DateTime<Utc>) -> DateTime<Utc> {
    floor_to_second(last) - REATTACH_MARGIN
}

/// [`KubernetesRunner`]'s re-attach window, shared with the Docker runner's
/// (W5a Task 5): `since` = ⌊newest stored `ts` of this attempt⌋ −
/// [`REATTACH_MARGIN`], and the stored lines at or after ⌊newest⌋ as the
/// overlap to drop. No stored line: `(None, empty)`, i.e. from the start.
pub(crate) async fn reattach_window(
    logs: &LogRepo,
    task: &TaskInput,
) -> Result<(Option<DateTime<Utc>>, Replayed)> {
    let attempt = RunIdentity::stored_attempt(task.attempt);
    let Some(last) = logs
        .last_ts(task.execution_id, task.run_id, attempt)
        .await?
    else {
        return Ok((None, Replayed::default()));
    };
    let stored = logs
        .messages_since(
            task.execution_id,
            task.run_id,
            attempt,
            floor_to_second(last),
            REPLAY_DEDUPE_MAX,
        )
        .await?;
    Ok((Some(reattach_since(last)), Replayed::new(stored)))
}

/// The stored lines a re-attach will see again, as a multiset of their text:
/// each replayed line equal to one of them consumes it and is dropped.
///
/// The window is the lines stored at or after ⌊newest stored `ts`⌋, which
/// [`REATTACH_MARGIN`] puts wholly inside the replay (their node time is at
/// most the margin earlier), so each one really is replayed once and cannot
/// consume a later, genuinely new line with the same text.
#[derive(Debug, Default)]
pub(crate) struct Replayed {
    left: std::collections::HashMap<String, usize>,
}

impl Replayed {
    pub(crate) fn new(stored: Vec<String>) -> Self {
        let mut left = std::collections::HashMap::new();
        for m in stored {
            *left.entry(m).or_insert(0) += 1;
        }
        Self { left }
    }

    /// True (and consumed) when `line` is one of the stored overlap. `line`
    /// must be in its STORED form (`commands::stream_replayed_line`): level
    /// prefix stripped, masked -- the raw line re-read from a plugin
    /// container is neither (W5b fix round 1, M1).
    pub(crate) fn already_stored(&mut self, line: &str) -> bool {
        match self.left.get_mut(line.trim_end_matches(['\r', '\n'])) {
            Some(n) if *n > 0 => {
                *n -= 1;
                true
            }
            _ => false,
        }
    }
}

/// Which lines of `main`'s log this call has already ingested, so a
/// re-follow after a dropped connection repeats none of them.
///
/// # Why not "skip every line at or before the last timestamp"
///
/// That was the first version, and the live suite caught it dropping lines:
/// the kubelet timestamps stdout and stderr **per stream**, and the CRI log
/// file is not ordered by timestamp across the two. Measured on kind 1.33 (the
/// `a_shell_command_…` case, 2 of 5 runs):
///
/// ```text
/// …10.520330457Z to-stderr
/// …10.520330454Z hello from 1000 in /egret/working-dir   <- 3 ns EARLIER, read second
/// ```
///
/// so a high-water mark skips a line nobody has seen. This keeps the exact
/// `(timestamp, line)` pairs read in the window a re-follow can return, and
/// skips only those.
#[derive(Debug, Default)]
pub(crate) struct Seen {
    /// The newest timestamp read.
    max: Option<DateTime<Utc>>,
    /// Every `(ts, line)` read at or after [`Seen::resume_from`].
    window: std::collections::BTreeSet<(DateTime<Utc>, String)>,
}

impl Seen {
    /// Whether this line is new; records it if so. A line without a
    /// timestamp cannot be matched and is always admitted.
    pub(crate) fn admit(&mut self, ts: Option<DateTime<Utc>>, line: &str) -> bool {
        let Some(ts) = ts else { return true };
        if !self.window.insert((ts, line.to_string())) {
            return false;
        }
        if self.max.is_none_or(|m| ts > m) {
            self.max = Some(ts);
            if let Some(from) = self.resume_from() {
                self.window = self.window.split_off(&(from, String::new()));
            }
        }
        true
    }

    /// The `sinceTime` of a re-follow: the newest timestamp floored to the
    /// second (the resolution `sinceTime` carries), less one second more for
    /// the cross-stream skew above, so an unread line just before the newest
    /// is still returned.
    pub(crate) fn resume_from(&self) -> Option<DateTime<Utc>> {
        self.max
            .map(|m| floor_to_second(m) - chrono::Duration::seconds(1))
    }
}

struct ExecOut {
    stdout: Vec<u8>,
    stderr: String,
    success: bool,
    /// The exec status's `reason`, `None` when no status arrived (fix round
    /// 2, N1): only `NonZeroExitCode` is the process's own exit.
    reason: Option<String>,
}

/// Read to EOF, keeping at most `cap` bytes. `drain`: keep reading (and
/// discarding) past the cap instead of returning, so the writer never blocks.
async fn read_capped<R: AsyncRead + Unpin>(
    r: Option<R>,
    cap: usize,
    drain: bool,
) -> (Vec<u8>, bool) {
    let Some(mut r) = r else {
        return (Vec::new(), false);
    };
    let mut out = Vec::new();
    let mut over = false;
    let mut chunk = vec![0u8; 16 * 1024];
    loop {
        match r.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if out.len() + n > cap {
                    let room = cap.saturating_sub(out.len());
                    out.extend_from_slice(&chunk[..room]);
                    over = true;
                    if !drain {
                        break;
                    }
                } else if !over {
                    out.extend_from_slice(&chunk[..n]);
                }
            }
        }
    }
    (out, over)
}

#[async_trait::async_trait]
impl TaskRunner for KubernetesRunner {
    /// See the module doc. `spec.pull_policy`, `spec.resources` and
    /// `spec.delete` are read; `cpu`/`memory` are the Docker runner's.
    async fn run(
        &self,
        task: &TaskInput,
        commands: &TaskCommands,
        spec: &RunnerSpec,
        observer: &dyn LineObserver,
    ) -> Result<CommandsResult> {
        // `Docker.run`'s first act and message, kept for this runner too.
        if commands.container_image.trim().is_empty() {
            return Err(Error::InvalidInput(
                "This task runner needs the `containerImage` property to be set".to_string(),
            ));
        }
        for rel in commands
            .input_files
            .keys()
            .chain(commands.stored_inputs.keys())
        {
            // W9b Task 5 fix round 1, I2: the staged-name rule, as Docker's.
            if let Some(problem) = staged_name_problem(rel) {
                return Err(Error::InvalidInput(problem));
            }
        }
        // egret's configuration first, so a bad `run_as_user` is egret's
        // failure and not blamed on the author's pod.
        self.cfg
            .validate()
            .map_err(|e| internal("the task pod configuration", e))?;
        let pod = pod_spec::build(task, commands, spec, &self.cfg).map_err(Error::InvalidInput)?;
        let name = pod_name(task.run_id, attempt_of(task));

        let created = self.get_or_create(&name, &pod).await?;
        if !created {
            tracing::info!(run = %task.run_id, pod = %name, "re-attaching to the task's existing pod");
        }
        let outcome = self.drive(task, commands, observer, &name, !created).await;
        kept_on_internal(task, &name, &outcome);
        // Nothing is deleted or signalled here, whatever the outcome:
        // `release`, after the outcome commits (final review I3).
        outcome
    }

    /// The attempt's pod, only if it already exists: followed to its end as
    /// a re-attach. `None` when there is no pod, or it is being deleted.
    async fn adoptable(&self, task: &TaskInput) -> Result<bool> {
        let existing = self
            .pods()
            .get_opt(&pod_name(task.run_id, attempt_of(task)))
            .await
            .map_err(|e| internal("reading the task pod", e))?;
        Ok(matches!(existing, Some(p) if p.metadata.deletion_timestamp.is_none()))
    }

    async fn adopt(
        &self,
        task: &TaskInput,
        commands: &TaskCommands,
        _spec: &RunnerSpec,
        observer: &dyn LineObserver,
    ) -> Result<Option<CommandsResult>> {
        let name = pod_name(task.run_id, attempt_of(task));
        let existing = self
            .pods()
            .get_opt(&name)
            .await
            .map_err(|e| internal("reading the task pod", e))?;
        match existing {
            Some(p) if p.metadata.deletion_timestamp.is_none() => {
                tracing::info!(
                    run = %task.run_id,
                    pod = %name,
                    "re-attaching to the task's existing pod, which passed the gate when it \
                     was created"
                );
                let outcome = self.drive(task, commands, observer, &name, true).await;
                kept_on_internal(task, &name, &outcome);
                outcome.map(Some)
            }
            _ => Ok(None),
        }
    }

    /// Delete the attempt's pod. A pod that is not there is `Ok(())`.
    ///
    /// **Grace 0** is W5a's cancel exactly: delete with `gracePeriodSeconds`
    /// 0 -- Kestra's `killGracePeriod` default -- and return.
    ///
    /// **A grace (W5b, spec §7.1)** deletes with `gracePeriodSeconds =
    /// grace` (whole seconds, rounded up): the kubelet sends SIGTERM to every
    /// container and SIGKILL at the grace. Then this waits, polling the pod,
    /// until it is gone or `main` shows terminated. **In practice that is the
    /// pod gone**: measured on kind (1.33.1), the kubelet reports no
    /// container terminated until all of them have stopped, which is why a
    /// plugin pod's `files-out` exits on SIGTERM
    /// ([`pod_spec::files_out_script`]) -- the pod then goes when `main`
    /// does. A script pod's does not, so a graceful cancel of one would wait
    /// out its grace; the script handler passes 0. The delete AND the wait
    /// are bounded together by `grace` + [`CANCEL_WAIT_MARGIN`]; a pod still
    /// there then (or an API server that never answered) is a WARN and
    /// `Ok(())`, since the outcome is the budget's either way (`cancel.rs`
    /// fact 2). [`release`](TaskRunner::release) then deletes with grace 0.
    ///
    /// **`delete: false` is deleted too** (debt D2, 2026-10-19): a pod cannot
    /// be stopped without being deleted; `spec.delete` is not read here. The
    /// only other way, an exec of `kill` inside the author's image, is
    /// fragile; the divergence from Docker's kept container (D1) is written
    /// in the workflow kill spec §4.2.
    async fn cancel(&self, task: &TaskInput, grace: Duration, _spec: &RunnerSpec) -> Result<()> {
        let name = pod_name(task.run_id, attempt_of(task));
        if grace.is_zero() {
            return self.delete_pod(&name).await;
        }
        let secs = grace.as_secs() + u64::from(grace.subsec_nanos() > 0);
        let secs = u32::try_from(secs).unwrap_or(u32::MAX);
        let bound = grace + CANCEL_WAIT_MARGIN;
        match tokio::time::timeout(bound, self.delete_and_wait(&name, secs)).await {
            Ok(done) => done,
            Err(_) => {
                tracing::warn!(
                    run = %task.run_id,
                    pod = %name,
                    "the task pod had not stopped {bound:?} after it was deleted with a \
                     grace (or the API server did not answer); it is left to the kubelet"
                );
                Ok(())
            }
        }
    }

    /// After the commit: delete the pod (grace 0) when `spec.delete`, else
    /// upload the `ended` marker so `files-out` exits and the kept pod
    /// completes instead of holding its resources.
    async fn release(&self, task: &TaskInput, spec: &RunnerSpec) -> Result<()> {
        let name = pod_name(task.run_id, attempt_of(task));
        if spec.delete {
            self.delete_pod(&name).await
        } else {
            self.signal_ended(&name).await;
            Ok(())
        }
    }
}

/// The WARN a kept pod gets: a worker failure leaves the pod for the next
/// claim to re-attach to.
fn kept_on_internal(task: &TaskInput, name: &str, outcome: &Result<CommandsResult>) {
    if let Err(Error::Internal(e)) = outcome {
        tracing::warn!(
            run = %task.run_id,
            pod = %name,
            "the task pod is kept for a later claim to re-attach to: {e}"
        );
    }
}

#[async_trait::async_trait]
impl OrphanSweep for KubernetesRunner {
    async fn sweep_orphans(&self, pool: &sqlx::PgPool) -> Result<u64> {
        sweep_orphans(&self.client, &self.cfg, pool, self.kept_retention).await
    }

    fn kept_retention(&self) -> Option<Duration> {
        self.kept_retention
    }
}

/// **The pod half of the orphan sweep** (spec §5, *Orphans*): delete every
/// task pod in the task namespace whose run is **terminal**; one the author
/// asked to keep (`delete: false`) only once its run has been terminal longer
/// than `kept_retention`, never with `None` (workflow debt D3). The rule is
/// [`sweep_decision`], shared with the Docker half.
///
/// # A pod whose run has no row is left alone, exactly as in the Docker half
///
/// Fix round 1, Important 3. The first version deleted those too, on the
/// argument that the task namespace is egret's alone. It is not guaranteed
/// to be: two egret installs, or egret and a live test suite (each with its
/// own database), pointed at one namespace would each delete the other's
/// RUNNING pods on every reaper tick. Only a row this database can see as
/// terminal authorises a delete; an unknown run is logged at debug. The cost,
/// stated: a pod whose execution was deleted while it ran is not swept.
///
/// **Debt D15, the one exception:** a pod whose run has no row, but whose
/// `egret.io/execution-id` names an execution this database holds as
/// terminal, is a superseded `LoopUntil` iteration's (debt D12 renamed its
/// row). That execution is this database's, so the pod is too, and it is
/// judged as a terminal run's ([`pod_sweep_deletes`]): deleted at once when
/// not kept (PR-2 final review I1), a kept pod's retention from the
/// execution's end.
pub async fn sweep_orphans(
    client: &kube::Client,
    cfg: &PodConfig,
    pool: &sqlx::PgPool,
    kept_retention: Option<Duration>,
) -> Result<u64> {
    let pods: Api<Pod> = Api::namespaced(client.clone(), &cfg.namespace);
    let listed = pods
        .list(&ListParams::default().labels(TASKRUN_LABEL))
        .await
        .map_err(|e| internal("listing task pods", e))?;
    let mut candidates = Vec::new();
    for p in listed.items {
        let Some(name) = p.metadata.name.clone() else {
            continue;
        };
        if !name.starts_with(RUN_NAME_PREFIX) || p.metadata.deletion_timestamp.is_some() {
            continue;
        }
        let keep = p
            .metadata
            .annotations
            .as_ref()
            .and_then(|a| a.get(DELETE_ANNOTATION))
            .is_some_and(|v| v == "false");
        let label = |key: &str| {
            p.metadata
                .labels
                .as_ref()
                .and_then(|l| l.get(key))
                .and_then(|v| uuid::Uuid::parse_str(v).ok())
        };
        let Some(id) = label(TASKRUN_LABEL) else {
            continue;
        };
        // Debt D15: what links a superseded run's pod to its execution.
        let execution = label(super::pod_spec::EXECUTION_LABEL);
        candidates.push((name, id, keep, execution));
    }
    if candidates.is_empty() {
        return Ok(0);
    }
    let work: Vec<_> = candidates.iter().map(|(_, id, _, ex)| (*id, *ex)).collect();
    let live = run_liveness(pool, &work).await?;
    let now = live.now.unwrap_or_else(Utc::now);
    let mut deleted = 0u64;
    for (name, id, keep, _) in candidates {
        if !pod_sweep_deletes(&live, id, keep, kept_retention, now) {
            if live.missing.contains(&id) && !live.superseded.contains_key(&id) {
                tracing::debug!(pod = %name, "orphan sweep: run {id} is unknown to this database; left alone");
            }
            continue;
        }
        match pods
            .delete(&name, &DeleteParams::default().grace_period(0))
            .await
        {
            Ok(_) => deleted += 1,
            Err(kube::Error::Api(ae)) if ae.code == 404 => {}
            Err(e) => tracing::warn!(pod = %name, "orphan sweep could not delete the pod: {e}"),
        }
    }
    Ok(deleted)
}

/// **Whether [`sweep_orphans`] deletes one pod**: [`sweep_decision`], and
/// nothing else. A run this database has no row for, and whose execution it
/// does not hold as terminal, is never deleted; a superseded `LoopUntil`
/// iteration's pod (debt D15) is judged as a terminal run's -- at once when
/// not kept, after the retention when kept.
pub(crate) fn pod_sweep_deletes(
    live: &crate::worker::commands::RunLiveness,
    id: uuid::Uuid,
    keep: bool,
    retention: Option<Duration>,
    now: chrono::DateTime<Utc>,
) -> bool {
    if live.missing.contains(&id) && !live.superseded.contains_key(&id) {
        return false;
    }
    sweep_decision(live, id, keep, retention, now)
}

/// How long the upload may go without a byte moving before it is judged
/// stalled (fix round 1, I2). Idle, not total: a 1 GiB stored input through
/// the API server's exec takes as long as it takes, and a total limit would
/// turn a slow-but-moving upload into a worker error and a re-claim for ever.
const UPLOAD_IDLE: Duration = EXEC_TIMEOUT;

/// An exec's time limit.
#[derive(Clone, Copy, Debug)]
enum Limit {
    /// The whole exec.
    Total(Duration),
    /// No progress (a byte written to its stdin) for this long.
    Idle(Duration),
}

/// When bytes last moved.
#[derive(Clone, Debug)]
struct Progress(std::sync::Arc<std::sync::Mutex<tokio::time::Instant>>);

impl Progress {
    fn new() -> Self {
        Self(std::sync::Arc::new(std::sync::Mutex::new(
            tokio::time::Instant::now(),
        )))
    }
    fn touch(&self) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = tokio::time::Instant::now();
    }
    fn last(&self) -> tokio::time::Instant {
        *self.0.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// An `AsyncWrite` that records [`Progress`] for every byte it accepts.
struct ProgressWriter<W> {
    inner: W,
    progress: Option<Progress>,
}

impl<W: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for ProgressWriter<W> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let r = std::pin::Pin::new(&mut self.inner).poll_write(cx, buf);
        if let std::task::Poll::Ready(Ok(n)) = &r {
            if *n > 0 {
                if let Some(p) = &self.progress {
                    p.touch();
                }
            }
        }
        r
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// `fut`'s output, or `None` once `progress` has not moved for `idle`.
async fn until_idle<F: std::future::Future>(
    fut: F,
    progress: &Progress,
    idle: Duration,
) -> Option<F::Output> {
    tokio::pin!(fut);
    loop {
        let deadline = progress.last() + idle;
        tokio::select! {
            out = &mut fut => return Some(out),
            _ = tokio::time::sleep_until(deadline) => {
                if progress.last() + idle <= tokio::time::Instant::now() {
                    return None;
                }
            }
        }
    }
}

/// The exec status `reason` the API server gives a process that EXITED
/// non-zero -- the only failure that is the process's own (fix round 2, N1).
const EXITED: &str = "NonZeroExitCode";

/// What a failed write of the archive means, given how the exec ended (fix
/// round 1, I2). Our own side stopping is [`staging_error`]'s split. A WRITE
/// failure whose status says the process EXITED non-zero ([`EXITED`]) is the
/// process in the pod ending (a `tar -x` that died): `Ok`, and its exit
/// decides, as for any command. No status, or any other reason (a
/// kubelet-side `InternalError`), is egret's outage (fix round 2, N1).
fn judge_write(
    wrote: std::result::Result<(), TarError>,
    status: Option<&k8s_openapi::apimachinery::pkg::apis::meta::v1::Status>,
) -> Result<()> {
    match wrote {
        Ok(()) => Ok(()),
        Err(TarError::Write(_)) if status.and_then(|s| s.reason.as_deref()) == Some(EXITED) => {
            Ok(())
        }
        Err(TarError::Write(e)) => Err(internal("writing to the exec's stdin", e)),
        Err(e) => Err(staging_error(e)),
    }
}

/// The upload's command in `files-in`: `tar -x`, then **drain the rest of
/// stdin**, then exit with tar's status (fix round 1, I2).
///
/// Measured on kind (Kubernetes 1.33): when the exec's process dies with its
/// stdin still open, the kubelet stops reading stdin and sends no status
/// until stdin closes, so the upload stalls and only the idle limit ends it --
/// as an outage. With the drain, a `tar -x` that dies part-way (a full disk)
/// leaves `cat` reading to the end, and the status that follows carries
/// tar's failure, which is the task's ([`extract_verdict`]).
fn extract_command() -> Vec<String> {
    vec![
        "sh".into(),
        "-c".into(),
        "tar -x -f - -C \"$1\"; rc=$?; cat > /dev/null; exit $rc".into(),
        "sh".into(),
        WORKING_DIR.into(),
    ]
}

/// The verdict on `tar -x` in `files-in` (fix round 1, I2): the API server
/// answered with the process's exit, so a failure is in the POD -- a full
/// node disk, a name the helper's `tar` refuses -- and is the task's, as
/// Docker's `classify` makes a `docker cp` the daemon answered. Calling it
/// an outage would re-upload the same archive for ever, spending no attempt.
fn extract_verdict(out: &ExecOut) -> Result<()> {
    if out.success {
        return Ok(());
    }
    // Fix round 2, N1: only the process's own exit is the task's. A lost
    // status (the connection dropped after the write) or a kubelet-side
    // failure is egret's: `upload` retries it, then the run is re-claimed.
    if out.reason.as_deref() != Some(EXITED) {
        return Err(internal(
            "extracting the input files in files-in",
            format!(
                "no exit status from tar -x ({}): {}",
                out.reason.as_deref().unwrap_or("the status was lost"),
                out.stderr.trim()
            ),
        ));
    }
    Err(Error::InvalidInput(format!(
        "the input files could not be extracted in the task pod ({FILES_IN}: tar -x failed): {}",
        out.stderr.trim()
    )))
}

/// `tar -c` of one path relative to the working directory, to stdout.
fn tar_create(rel: &str) -> Vec<String> {
    vec![
        "tar".into(),
        "-c".into(),
        "-f".into(),
        "-".into(),
        "-C".into(),
        WORKING_DIR.into(),
        "--".into(),
        rel.to_string(),
    ]
}

fn stalled(what: &str) -> Error {
    internal(
        &format!("reading {what:?} out of {FILES_OUT}"),
        format!("no byte moved for {EXEC_TIMEOUT:?} (the download stalled)"),
    )
}

/// `tar -c` in the pod EXITED non-zero part-way through: the pod's failure
/// (a file that vanished or shrank as it was read), the task's, naming it.
fn tar_died(what: &str, read: &str, stderr: &str) -> String {
    format!(
        "{what:?} could not be read out of the task pod ({FILES_OUT}: tar -c failed part-way: \
         {}; {read})",
        stderr.trim()
    )
}

/// A `files-out` exec whose stdout is read as a stream ([`KubernetesRunner::exec_out`]).
struct OutExec {
    stdout: ProgressReader,
    status: std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Option<k8s_openapi::apimachinery::pkg::apis::meta::v1::Status>,
                > + Send,
        >,
    >,
    errs: tokio::task::JoinHandle<Vec<u8>>,
    proc: kube::api::AttachedProcess,
    progress: Progress,
}

/// How a `files-out` exec ended.
struct OutExit {
    status: Option<k8s_openapi::apimachinery::pkg::apis::meta::v1::Status>,
    stderr: String,
}

impl OutExit {
    fn success(&self) -> bool {
        self.status.as_ref().and_then(|s| s.status.as_deref()) == Some("Success")
    }
    /// The process's OWN non-zero exit ([`EXITED`]), never a lost status.
    fn exited_non_zero(&self) -> bool {
        self.status.as_ref().and_then(|s| s.reason.as_deref()) == Some(EXITED)
    }
    /// The process ended by itself, either way.
    fn ended(&self) -> bool {
        self.success() || self.exited_non_zero()
    }
    fn lost(&self, what: &str) -> Error {
        internal(
            &format!("reading {what:?} out of {FILES_OUT}"),
            format!(
                "no exit status from tar -c ({}): {}",
                self.status
                    .as_ref()
                    .and_then(|s| s.reason.as_deref())
                    .unwrap_or("the status was lost"),
                self.stderr.trim()
            ),
        )
    }
}

impl OutExec {
    /// Read what is left of the archive (its end blocks; at most 1 MiB, within
    /// [`EXEC_TIMEOUT`]) and wait for the exit status.
    async fn finish(mut self) -> OutExit {
        use tokio::io::AsyncReadExt;
        let drained = tokio::time::timeout(
            EXEC_TIMEOUT,
            tokio::io::copy(
                &mut (&mut self.stdout).take(1 << 20),
                &mut tokio::io::sink(),
            ),
        )
        .await;
        if !matches!(drained, Ok(Ok(n)) if n < 1 << 20) {
            self.proc.abort();
        }
        let status = tokio::time::timeout(EXEC_TIMEOUT, self.status)
            .await
            .ok()
            .flatten();
        let _ = self.proc.join().await;
        let stderr = String::from_utf8_lossy(&self.errs.await.unwrap_or_default()).into_owned();
        OutExit { status, stderr }
    }

    /// Stop reading: the exec is aborted (a refusal part-way, an outage).
    async fn abort(self) {
        self.proc.abort();
        let _ = self.proc.join().await;
    }
}

/// An `AsyncRead` that records [`Progress`] for every byte it yields.
struct ProgressReader {
    inner: Box<dyn tokio::io::AsyncRead + Send + Unpin>,
    progress: Progress,
}

impl tokio::io::AsyncRead for ProgressReader {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let r = std::pin::Pin::new(&mut self.inner).poll_read(cx, buf);
        if buf.filled().len() > before {
            self.progress.touch();
        }
        r
    }
}

fn internal(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Internal(anyhow::anyhow!("kubernetes task runner: {what}: {e}"))
}

fn deleted(name: &str) -> Error {
    Error::InvalidInput(format!(
        "the task's pod {name} was deleted before its command finished"
    ))
}

/// Kestra's `AbstractPod.shellQuote`: `'` … `'`, with `'` as `'\''`.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn floor_to_second(t: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp(t.timestamp(), 0).unwrap_or(t)
}

/// `timestamps=true` prefixes each line with an RFC 3339 instant and a space.
pub(crate) fn split_timestamp(raw: &str) -> (Option<DateTime<Utc>>, &str) {
    let (head, rest) = raw.split_once(' ').unwrap_or((raw, ""));
    match DateTime::parse_from_rfc3339(head) {
        Ok(t) => (Some(t.with_timezone(&Utc)), rest),
        Err(_) => (None, raw),
    }
}

/// Where one container is in its life.
#[derive(Debug, PartialEq, Eq)]
enum Phase {
    Unknown,
    Waiting,
    Running,
    Terminated { exit: i32, reason: Option<String> },
}

fn status_of<'a>(p: &'a Pod, name: &str) -> Option<&'a ContainerStatus> {
    let s = p.status.as_ref()?;
    s.init_container_statuses
        .iter()
        .flatten()
        .chain(s.container_statuses.iter().flatten())
        .find(|c| c.name == name)
}

fn phase(cs: Option<&ContainerStatus>) -> Phase {
    let Some(state) = cs.and_then(|c| c.state.as_ref()) else {
        return Phase::Unknown;
    };
    if let Some(t) = &state.terminated {
        return Phase::Terminated {
            exit: t.exit_code,
            reason: t.reason.clone(),
        };
    }
    if state.running.is_some() {
        return Phase::Running;
    }
    if state.waiting.is_some() {
        return Phase::Waiting;
    }
    Phase::Unknown
}

/// Why this pod will never run `main` to completion, if it will not.
///
/// **Every such failure fails the task** (final review I4), because no later
/// claim can get past it: a start failure of `main` is the **author's** (the
/// image is theirs); one of a helper is **egret's configuration** (its image,
/// `workflow.kubernetes.helper_image`), and the message says an operator
/// must fix it. It used to be a worker failure, which kept the pod and
/// re-attached every `working_timeout` for ever -- with the chart's default
/// `egret-task-helper:dev`, which is not pullable from a registry, that was
/// every script task with files.
fn start_failure(p: &Pod) -> Option<Error> {
    // `start_failure` sees the pod after `main` may have finished: an
    // eviction is decided by [`eviction_of`], not here.

    let s = p.status.as_ref()?;
    for cs in s
        .init_container_statuses
        .iter()
        .flatten()
        .chain(s.container_statuses.iter().flatten())
    {
        let Some(w) = cs.state.as_ref().and_then(|st| st.waiting.as_ref()) else {
            continue;
        };
        let Some(reason) = w.reason.as_deref() else {
            continue;
        };
        if START_FAILURES.contains(&reason) {
            return Some(start_error(cs, reason, &waiting_message(cs)));
        }
    }
    if let Phase::Terminated { exit, .. } = phase(status_of(p, FILES_IN)) {
        if exit != 0 {
            return Some(Error::InvalidInput(format!(
                "egret's files-in helper container exited {exit} before the input files were \
                 ready; its image is egret's `workflow.kubernetes.helper_image`, which an \
                 operator must fix (retrying the task will not help)"
            )));
        }
    }
    if s.phase.as_deref() == Some("Failed")
        && !matches!(phase(status_of(p, MAIN)), Phase::Terminated { .. })
    {
        return Some(Error::InvalidInput(format!(
            "the task's pod failed before `main` finished: {}: {}",
            s.reason.as_deref().unwrap_or("?"),
            s.message.as_deref().unwrap_or("")
        )));
    }
    None
}

/// The waiting container stuck in `ErrImagePull`, if one is.
fn pull_error(p: &Pod) -> Option<&ContainerStatus> {
    let s = p.status.as_ref()?;
    s.init_container_statuses
        .iter()
        .flatten()
        .chain(s.container_statuses.iter().flatten())
        .find(|cs| {
            cs.state
                .as_ref()
                .and_then(|st| st.waiting.as_ref())
                .and_then(|w| w.reason.as_deref())
                == Some("ErrImagePull")
        })
}

fn waiting_message(cs: &ContainerStatus) -> String {
    cs.state
        .as_ref()
        .and_then(|st| st.waiting.as_ref())
        .and_then(|w| w.message.clone())
        .unwrap_or_default()
}

/// A container that will not start: the author's on `main`, egret's
/// configuration on a helper. Both fail the task, by name.
fn start_error(cs: &ContainerStatus, reason: &str, detail: &str) -> Error {
    if cs.name == MAIN {
        Error::InvalidInput(format!(
            "the main container could not start: {reason}: {detail}"
        ))
    } else {
        Error::InvalidInput(format!(
            "egret's {} helper container could not start: {reason}: {detail}. Its image \
             ({:?}) is egret's `workflow.kubernetes.helper_image`: an operator must make it \
             pullable from the task namespace (build deploy/docker/task-helper.Dockerfile and \
             push it, or point the setting at a published copy); retrying the task will not \
             help",
            cs.name, cs.image
        ))
    }
}

/// The kubelet's eviction of the pod, as the task's failure message (final
/// review I5): the pod's phase is `Failed` with reason `Evicted`, and its
/// message says which bound was crossed -- the pod's ephemeral-storage limit
/// or an `emptyDir`'s `sizeLimit`.
fn eviction_of(p: &Pod) -> Option<String> {
    let s = p.status.as_ref()?;
    if s.reason.as_deref() != Some("Evicted") {
        return None;
    }
    Some(format!(
        "the task's pod was evicted by the kubelet: {} (the working directory and the \
         container's own disk are bounded by egret's `workflow.kubernetes` ephemeral-storage \
         limit)",
        s.message.as_deref().unwrap_or("no reason given")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::core::v1::{
        ContainerState, ContainerStateTerminated, ContainerStateWaiting, PodStatus,
    };

    /// **Final review I1 (PR-2): the pod sweep's rule.** A terminal run's
    /// pod and a superseded `LoopUntil` iteration's pod (debt D12 renamed its
    /// row; its execution is this database's and terminal) are deleted at
    /// once when not kept, after the retention when kept; a live run's pod,
    /// and one whose run and execution are unknown here, are never deleted.
    #[test]
    fn the_pod_sweep_deletes_a_superseded_iterations_pod_like_a_terminal_runs() {
        use crate::worker::commands::RunLiveness;
        use chrono::Duration as D;
        let now = Utc::now();
        let (live_run, done, sup_old, sup_new, unknown) = (
            uuid::Uuid::from_u128(1),
            uuid::Uuid::from_u128(2),
            uuid::Uuid::from_u128(3),
            uuid::Uuid::from_u128(4),
            uuid::Uuid::from_u128(5),
        );
        let mut l = RunLiveness::default();
        l.terminal.insert(done);
        l.ended.insert(done, now - D::minutes(1));
        l.missing.extend([sup_old, sup_new, unknown]);
        l.superseded.insert(sup_old, now - D::days(8));
        l.superseded.insert(sup_new, now - D::minutes(1));
        let week = Some(Duration::from_secs(7 * 86_400));
        let del = |id, keep| pod_sweep_deletes(&l, id, keep, week, now);
        assert!(!del(live_run, false), "a live run's pod stays");
        assert!(del(done, false), "terminal, not kept: deleted at once");
        assert!(
            del(sup_new, false),
            "superseded, not kept: deleted at once, as a terminal run's"
        );
        assert!(!del(sup_new, true), "superseded, kept, 1 minute: stays");
        assert!(del(sup_old, true), "superseded, kept, 8 days: deleted");
        assert!(!del(unknown, false), "unknown here: never ours");
        assert!(!del(unknown, true), "unknown here: never ours");
    }

    #[test]
    fn a_log_line_loses_its_timestamp_and_keeps_its_spaces() {
        let (ts, line) = split_timestamp("2026-09-28T10:00:01.123456789Z hello  world");
        assert_eq!(line, "hello  world");
        assert_eq!(
            ts.unwrap()
                .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
            "2026-09-28T10:00:01.123456789Z"
        );
        assert_eq!(
            split_timestamp("2026-09-28T10:00:01Z ").1,
            "",
            "an empty line"
        );
        assert_eq!(
            split_timestamp("no timestamp here"),
            (None, "no timestamp here")
        );
    }

    #[test]
    fn since_time_is_floored_to_the_second_so_nothing_is_skipped() {
        let t = DateTime::parse_from_rfc3339("2026-09-28T10:00:01.999Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(floor_to_second(t).to_rfc3339(), "2026-09-28T10:00:01+00:00");
    }

    #[test]
    fn lines_out_of_timestamp_order_are_all_admitted_and_a_replay_repeats_none() {
        let t = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc);
        let mut seen = Seen::default();
        // The measured order: stderr first, stdout 3 ns earlier read second.
        assert!(seen.admit(Some(t("2026-09-28T05:29:10.520330457Z")), "to-stderr"));
        assert!(seen.admit(Some(t("2026-09-28T05:29:10.520330454Z")), "hello"));
        assert!(seen.admit(Some(t("2026-09-28T05:29:10.520426684Z")), "::{}::"));
        assert_eq!(
            seen.resume_from().unwrap().to_rfc3339(),
            "2026-09-28T05:29:09+00:00"
        );
        // A re-follow returns the same lines, then new ones.
        assert!(!seen.admit(Some(t("2026-09-28T05:29:10.520330454Z")), "hello"));
        assert!(!seen.admit(Some(t("2026-09-28T05:29:10.520330457Z")), "to-stderr"));
        assert!(seen.admit(Some(t("2026-09-28T05:29:10.520330999Z")), "unread before"));
        assert!(seen.admit(Some(t("2026-09-28T05:29:12.000000001Z")), "after"));
        // The same text at another instant is another line.
        assert!(seen.admit(Some(t("2026-09-28T05:29:12.000000002Z")), "after"));
        assert!(seen.admit(None, "no timestamp"));
        assert!(
            seen.admit(None, "no timestamp"),
            "cannot be matched, never dropped"
        );
    }

    #[test]
    fn a_reattach_resumes_a_margin_before_the_newest_stored_line() {
        let last = DateTime::parse_from_rfc3339("2026-09-28T10:00:07.300Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            reattach_since(last).to_rfc3339(),
            "2026-09-28T10:00:02+00:00",
            "⌊ts⌋ − 5 s: a line written on the node up to 5 s before egret stored \
             the newest line is still replayed"
        );
    }

    #[test]
    fn the_stored_overlap_is_dropped_once_per_stored_copy() {
        let mut r = Replayed::new(vec!["one".into(), "retry".into()]);
        assert!(r.already_stored("one"));
        assert!(!r.already_stored("one"), "a second `one` is new");
        assert!(r.already_stored("retry"));
        assert!(
            !r.already_stored("retry"),
            "the retry printed after the reclaim is kept"
        );
        assert!(!r.already_stored("almost"));
        assert!(!Replayed::default().already_stored("one"));
    }

    #[test]
    fn a_quote_in_a_file_name_cannot_break_out_of_the_verification_script() {
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }

    fn pod_with(name: &str, state: ContainerState) -> Pod {
        Pod {
            status: Some(PodStatus {
                container_statuses: Some(vec![ContainerStatus {
                    name: name.into(),
                    state: Some(state),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// **A container that will not start FAILS the task, whoever's image it
    /// is** (final review I4): `main`'s is the author's; a helper's names
    /// egret's `helper_image` setting and says an operator must fix it.
    /// Neither is `Internal`, which kept the pod and re-attached for ever.
    #[test]
    fn an_image_that_will_not_pull_is_the_authors_on_main_and_egrets_on_a_helper() {
        let waiting = |reason: &str| ContainerState {
            waiting: Some(ContainerStateWaiting {
                reason: Some(reason.into()),
                message: Some("nope".into()),
            }),
            ..Default::default()
        };
        match start_failure(&pod_with(MAIN, waiting("ImagePullBackOff"))) {
            Some(Error::InvalidInput(m)) => {
                assert!(m.contains("main container could not start"), "{m}");
                assert!(!m.contains("helper_image"), "{m}");
            }
            other => panic!("{other:?}"),
        }
        for helper in [FILES_OUT, FILES_IN] {
            for reason in ["ImagePullBackOff", "ErrImageNeverPull", "InvalidImageName"] {
                match start_failure(&pod_with(helper, waiting(reason))) {
                    Some(Error::InvalidInput(m)) => {
                        assert!(m.contains(helper) && m.contains(reason), "{m}");
                        assert!(m.contains("workflow.kubernetes.helper_image"), "{m}");
                        assert!(m.contains("nope"), "the kubelet's own words: {m}");
                    }
                    other => panic!("{helper} {reason}: {other:?}"),
                }
            }
        }
        // `files-in` exiting non-zero is egret's too, and just as final.
        let exited = ContainerState {
            terminated: Some(ContainerStateTerminated {
                exit_code: 2,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(matches!(
            start_failure(&pod_with(FILES_IN, exited)),
            Some(Error::InvalidInput(m)) if m.contains("files-in") && m.contains("exited 2")
        ));
        assert_eq!(
            pull_error(&pod_with(FILES_OUT, waiting("ErrImagePull"))).map(|c| c.name.as_str()),
            Some(FILES_OUT),
            "ErrImagePull is watched against PULL_FAILURE_BOUND"
        );
        assert!(pull_error(&pod_with(FILES_OUT, waiting("ContainerCreating"))).is_none());
        assert!(
            start_failure(&pod_with(MAIN, waiting("ErrImagePull"))).is_none(),
            "the first pull failure is retried by the kubelet"
        );
        assert!(start_failure(&pod_with(MAIN, waiting("ContainerCreating"))).is_none());
        let done = ContainerState {
            terminated: Some(ContainerStateTerminated {
                exit_code: 3,
                reason: Some("Error".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            phase(status_of(&pod_with(MAIN, done), MAIN)),
            Phase::Terminated {
                exit: 3,
                reason: Some("Error".into())
            }
        );
    }

    /// **A `pods/exec` 403 is recognised in both of the shapes kube returns
    /// it** -- a plain API error, and (WebSocket exec, measured in W5a Task 6:
    /// `failed to switch protocol: 403 Forbidden`) an upgrade answered 403 --
    /// and nothing else is: a 404, a 500 or another upgrade status stays the
    /// runner's to classify. The message names the missing verbs.
    #[test]
    fn an_exec_403_is_forbidden_in_both_shapes_and_names_the_role() {
        use kube::client::UpgradeConnectionError::ProtocolSwitch;
        let api = |code: u16| {
            kube::Error::Api(kube::core::ErrorResponse {
                status: "Failure".into(),
                message: "m".into(),
                reason: "r".into(),
                code,
            })
        };
        let upgrade = |code: u16| {
            kube::Error::UpgradeConnection(ProtocolSwitch(
                axum::http::StatusCode::from_u16(code).expect("a status"),
            ))
        };
        assert!(is_forbidden(&api(403)));
        assert!(is_forbidden(&upgrade(403)));
        for other in [api(404), api(500), upgrade(400), upgrade(500)] {
            assert!(!is_forbidden(&other), "{other}");
        }
        let m = exec_forbidden_message("egret-tasks", FILES_OUT, &upgrade(403));
        for needle in [
            "`pods/exec` `get`",
            "`create`",
            "\"egret-tasks\"",
            "files-out",
            "403",
        ] {
            assert!(m.contains(needle), "{needle}: {m}");
        }
    }

    /// **An evicted pod is the task's failure, carrying the kubelet's reason**
    /// (final review I5); a pod that merely failed, or succeeded, is not an
    /// eviction.
    #[test]
    fn an_eviction_is_named_with_the_kubelets_message() {
        let pod = |reason: Option<&str>, message: Option<&str>| Pod {
            status: Some(PodStatus {
                phase: Some("Failed".into()),
                reason: reason.map(Into::into),
                message: message.map(Into::into),
                ..Default::default()
            }),
            ..Default::default()
        };
        let why = eviction_of(&pod(
            Some("Evicted"),
            Some("Usage of EmptyDir volume \"working-dir\" exceeds the limit \"10Gi\"."),
        ))
        .expect("an eviction");
        assert!(
            why.contains("evicted") && why.contains("working-dir"),
            "{why}"
        );
        assert!(eviction_of(&pod(None, None)).is_none());
        assert!(eviction_of(&pod(Some("DeadlineExceeded"), Some("x"))).is_none());
    }

    /// **The whole graceful cancel is bounded by `grace` + the margin, even
    /// against an API server that never answers** (W5b Task 5 fix round 1):
    /// the kube client would otherwise wait out its own read timeout (295 s)
    /// on the delete or on any one read.
    #[tokio::test]
    async fn a_graceful_cancel_is_bounded_even_when_the_api_server_never_answers() {
        // kube 2 and axum share `http` 1.x.
        use axum::http;
        let silent = tower::service_fn(|_req: http::Request<kube::client::Body>| async {
            std::future::pending::<
                std::result::Result<http::Response<kube::client::Body>, std::convert::Infallible>,
            >()
            .await
        });
        let client = kube::Client::new(silent, "egret-tasks");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://nobody@localhost:1/none")
            .expect("a lazy pool never connects");
        let runner = KubernetesRunner::new(
            client,
            PodConfig {
                namespace: "egret-tasks".into(),
                run_as_user: 1000,
                helper_image: "h".into(),
                default_resources: Default::default(),
                max_resources: Default::default(),
                image_pull_secrets: vec![],
            },
            LogRepo::new(pool),
        );
        let task = crate::worker::handlers::testing::input("t", serde_json::Value::Null);
        let grace = Duration::from_secs(1);
        let started = tokio::time::Instant::now();
        let out = tokio::time::timeout(
            Duration::from_secs(30),
            runner.cancel(
                &task,
                grace,
                &RunnerSpec::defaults(crate::scripts::RunnerKind::Kubernetes),
            ),
        )
        .await
        .expect("cancel returned: it is bounded, not the kube client's 295 s");
        let took = started.elapsed();
        assert!(out.is_ok(), "past the bound it is a WARN and Ok: {out:?}");
        assert!(
            took >= grace + CANCEL_WAIT_MARGIN - Duration::from_millis(100)
                && took < grace + CANCEL_WAIT_MARGIN + Duration::from_secs(2),
            "{took:?}"
        );
    }

    // ---- W9a Task 5 fix round 1, I2 ----------------------------------------

    fn exec_status(
        status: &str,
        reason: Option<&str>,
    ) -> k8s_openapi::apimachinery::pkg::apis::meta::v1::Status {
        k8s_openapi::apimachinery::pkg::apis::meta::v1::Status {
            status: Some(status.to_string()),
            reason: reason.map(str::to_string),
            ..Default::default()
        }
    }

    fn failed_out(reason: Option<&str>) -> ExecOut {
        ExecOut {
            stdout: Vec::new(),
            stderr: "tar: write error: No space left on device".into(),
            success: false,
            reason: reason.map(str::to_string),
        }
    }

    /// A `tar -x` that died in the pod -- its own non-zero exit, reason
    /// `NonZeroExitCode` -- is the TASK's error (fix round 1, I2). Fix round
    /// 2, N1: a LOST status (the connection dropped) and a kubelet-side
    /// Failure (`InternalError`) are not the process's exit: egret's outage,
    /// retried, never a permanently failed task.
    #[test]
    fn only_tars_own_non_zero_exit_is_the_tasks_failure() {
        let broke = || TarError::Write(std::io::Error::from(std::io::ErrorKind::BrokenPipe));
        let nonzero = exec_status("Failure", Some("NonZeroExitCode"));
        let internal_error = exec_status("Failure", Some("InternalError"));

        // The process's own exit decides.
        judge_write(Err(broke()), Some(&nonzero)).expect("the exit decides");
        let err = extract_verdict(&failed_out(Some("NonZeroExitCode"))).expect_err("tar failed");
        assert!(
            matches!(&err, Error::InvalidInput(m) if m.contains("No space left on device")),
            "{err:?}"
        );
        extract_verdict(&ExecOut {
            stdout: Vec::new(),
            stderr: String::new(),
            success: true,
            reason: None,
        })
        .expect("a successful extract");

        // No status, or a kubelet-side failure: egret's.
        assert!(matches!(
            judge_write(Err(broke()), None),
            Err(Error::Internal(_))
        ));
        assert!(matches!(
            judge_write(Err(broke()), Some(&internal_error)),
            Err(Error::Internal(_))
        ));
        assert!(
            matches!(extract_verdict(&failed_out(None)), Err(Error::Internal(_))),
            "a lost status is not tar failing"
        );
        assert!(
            matches!(
                extract_verdict(&failed_out(Some("InternalError"))),
                Err(Error::Internal(_))
            ),
            "a kubelet error is not tar failing"
        );

        // Our own side stopping keeps staging_error's split.
        assert!(matches!(
            judge_write(Err(TarError::Author("x".into())), Some(&nonzero)),
            Err(Error::InvalidInput(_))
        ));
        assert!(matches!(
            judge_write(Err(TarError::Store("x".into())), None),
            Err(Error::Internal(_))
        ));
    }

    /// A writer that accepts everything, slowly enough to be observed.
    struct Sink;
    impl tokio::io::AsyncWrite for Sink {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(Ok(buf.len()))
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    /// The upload's limit is IDLE (scaled down here to real milliseconds): an
    /// upload that keeps moving a byte every 100 ms runs 2 s -- nearly seven
    /// times the 300 ms limit -- and finishes; one that stops moving is cut
    /// once the limit passes with no byte.
    #[tokio::test]
    async fn a_slow_but_progressing_upload_is_not_timed_out_and_a_stalled_one_is() {
        let idle = Duration::from_millis(300);
        let progress = Progress::new();
        let mut w = ProgressWriter {
            inner: Sink,
            progress: Some(progress.clone()),
        };
        let started = tokio::time::Instant::now();
        let slow = async {
            for _ in 0..20 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                w.write_all(b"x").await.unwrap();
            }
            "finished"
        };
        assert_eq!(until_idle(slow, &progress, idle).await, Some("finished"));
        assert!(started.elapsed() >= Duration::from_secs(2));

        let progress = Progress::new();
        let mut w = ProgressWriter {
            inner: Sink,
            progress: Some(progress.clone()),
        };
        let started = tokio::time::Instant::now();
        let stalled = async {
            w.write_all(b"x").await.unwrap();
            tokio::time::sleep(Duration::from_secs(30)).await;
            "never"
        };
        assert_eq!(until_idle(stalled, &progress, idle).await, None);
        let took = started.elapsed();
        assert!(
            took >= idle && took < Duration::from_secs(5),
            "cut after the idle limit: {took:?}"
        );
    }
}
