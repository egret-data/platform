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
use super::transfer;
use crate::logs::{LogLevel, LogRepo, RunIdentity};
use crate::scripts::RunnerSpec;
use crate::worker::commands::{
    expand_outputs, listing_too_big, output_problem, pattern_dirs, relative_problem, run_liveness,
    stream_line, CommandsResult, LineObserver, OrphanSweep, StreamState, TaskCommands, TaskRunner,
    CANCEL_WAIT_MARGIN, MAX_LISTING_BYTES, MAX_OUTPUT_FILE_BYTES, MAX_OUTPUT_NAMES, MAX_VARS_BYTES,
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
    max_output_bytes: usize,
    max_vars_bytes: usize,
    poll: Duration,
    /// Set once the first `pods/exec` 403 has been logged at ERROR (final
    /// review I4): the operator gets one loud line with the RBAC hint, and
    /// every task that meets the same refusal fails naming it.
    exec_forbidden_logged: std::sync::Arc<std::sync::atomic::AtomicBool>,
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
            max_output_bytes: MAX_OUTPUT_FILE_BYTES,
            max_vars_bytes: MAX_VARS_BYTES,
            poll: Duration::from_millis(500),
            exec_forbidden_logged: Default::default(),
        }
    }

    /// Smaller caps, for a test ([`DockerRunner::with_limits`]'s reason).
    ///
    /// [`DockerRunner::with_limits`]: crate::worker::commands::DockerRunner::with_limits
    pub fn with_limits(mut self, max_output_bytes: usize, max_vars_bytes: usize) -> Self {
        self.max_output_bytes = max_output_bytes;
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

    /// Poll `name` until it is gone or `main` has terminated, for at most
    /// `bound`; past it, a WARN. A read that fails is retried until the
    /// bound: the delete has been issued, and an early return would let
    /// `release`'s grace-0 delete cut the grace short.
    async fn wait_main_stopped(&self, task: &TaskInput, name: &str, bound: Duration) {
        let deadline = tokio::time::Instant::now() + bound;
        loop {
            match self.pods().get_opt(name).await {
                Ok(None) => return,
                Ok(Some(p)) if matches!(phase(status_of(&p, MAIN)), Phase::Terminated { .. }) => {
                    return
                }
                Ok(Some(_)) => {}
                Err(e) => {
                    tracing::debug!(pod = %name, "waiting for the cancelled task pod to stop: {e}")
                }
            }
            if tokio::time::Instant::now() >= deadline {
                tracing::warn!(
                    run = %task.run_id,
                    pod = %name,
                    "the task pod had not stopped {bound:?} after it was deleted with a \
                     grace; it is left to the kubelet's SIGKILL"
                );
                return;
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
        let mut uploaded = cmds.input_files.is_empty();
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
        // `stream_line` from ONE place, so the `StreamState` (the markers' vars
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
                if replayed.already_stored(&line) {
                    continue;
                }
                stream_line(
                    task,
                    observer,
                    &mut result,
                    &mut state,
                    self.max_vars_bytes,
                    &line,
                    is_err,
                    cmds.host_protocol,
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
        if !cmds.output_files.is_empty() && result.failure.is_none() {
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
        let archive = transfer::pack(&cmds.input_files).map_err(Error::InvalidInput)?;
        let out = self
            .exec(
                name,
                FILES_IN,
                vec![
                    "tar".into(),
                    "-x".into(),
                    "-f".into(),
                    "-".into(),
                    "-C".into(),
                    WORKING_DIR.into(),
                ],
                Some(archive),
                64 * 1024,
            )
            .await?;
        if !out.success {
            return Err(internal(
                "extracting the input files in files-in",
                out.stderr.trim(),
            ));
        }

        // Kestra's `verifyFileUpload`: `wc -c < <file>` per file, compared
        // with what was sent.
        let script: String = cmds
            .input_files
            .keys()
            .map(|rel| format!("wc -c < {}\n", shell_quote(&format!("{WORKING_DIR}/{rel}"))))
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
        let expected: Vec<u64> = cmds.input_files.values().map(|c| c.len() as u64).collect();
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

    /// Each declared output, one `tar -c` out of `files-out` apiece, judged by
    /// [`transfer::judge`] under the W4 caps. An author's problem lands in
    /// `result.failure`, exit code and vars intact; `Err` is the transfer.
    async fn download(
        &self,
        name: &str,
        cmds: &TaskCommands,
        result: &mut CommandsResult,
    ) -> Result<()> {
        let mut total = 0usize;
        for rel in &cmds.output_files {
            if let Some(problem) = output_problem(rel) {
                result.failure = Some(problem);
                return Ok(());
            }
        }
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
        for (rel, from_glob) in names {
            let rel = &rel;
            // An EXPANDED name is re-validated like a declared one (fix round
            // 2): a file named `x\n..` lists as the line `..`, which `*`
            // matches, and must never reach `tar -C <wd> -- ..`.
            if let Some(problem) = output_problem(rel) {
                result.failure = Some(problem);
                break;
            }
            // The contents, their header, a possible long-name entry and the
            // end blocks. Past this the stream is cut and the header decides.
            let cap = self.max_output_bytes + 8 * 512;
            let out = self
                .exec(
                    name,
                    FILES_OUT,
                    vec![
                        "tar".into(),
                        "-c".into(),
                        "-f".into(),
                        "-".into(),
                        "-C".into(),
                        WORKING_DIR.into(),
                        "--".into(),
                        rel.clone(),
                    ],
                    None,
                    cap,
                )
                .await?;
            let entry = transfer::unpack_one(&out.stdout, self.max_output_bytes)
                .map_err(|e| internal(&format!("reading {rel:?} out of files-out"), e))?;
            // A glob visits files only: a directory it matched is skipped.
            if from_glob && matches!(entry, Some(transfer::Entry::Directory)) {
                continue;
            }
            let bytes = match transfer::judge(rel, entry, self.max_output_bytes) {
                Ok(Some(b)) => b,
                // Not there: ignored, as Kestra (`missing_declared_file`).
                Ok(None) => continue,
                Err(problem) => {
                    result.failure = Some(problem);
                    break;
                }
            };
            // The key counts toward the total too (fix round 2).
            total += bytes.len() + rel.len();
            if total > self.max_output_bytes {
                result.failure = Some(format!(
                    "the output files declared by this task come to {total} bytes (names and \
                     contents), over the \
                     {} one task may carry. `outputFiles:` is an unbounded list, so a per-file \
                     cap alone bounds nothing",
                    self.max_output_bytes
                ));
                break;
            }
            match String::from_utf8(bytes) {
                Ok(text) => {
                    result.output_files.insert(rel.clone(), text);
                }
                Err(_) => {
                    result.failure = Some(format!(
                        "the output file {rel:?} is not UTF-8, and an output is JSON. egret has \
                         no internal storage to hand back a URI to (W9 owns it), so a binary \
                         artefact cannot be an output of this task today"
                    ));
                    break;
                }
            }
        }
        Ok(())
    }

    /// Run `cmd` in `container`, feed it `stdin`, and collect its output.
    async fn exec(
        &self,
        name: &str,
        container: &str,
        cmd: Vec<String>,
        stdin: Option<Vec<u8>>,
        stdout_cap: usize,
    ) -> Result<ExecOut> {
        match tokio::time::timeout(
            EXEC_TIMEOUT,
            self.exec_inner(name, container, cmd, stdin, stdout_cap),
        )
        .await
        {
            Ok(r) => r,
            Err(_) => Err(internal(
                &format!("exec in {container}"),
                format!("no answer within {EXEC_TIMEOUT:?}"),
            )),
        }
    }

    async fn exec_inner(
        &self,
        name: &str,
        container: &str,
        cmd: Vec<String>,
        stdin: Option<Vec<u8>>,
        stdout_cap: usize,
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
        // stdin and stderr run beside the stdout read, so neither can stall
        // it; both end when the process (or the abort below) ends the stream.
        let writer = proc.stdin();
        let write = tokio::spawn(async move {
            if let (Some(mut w), Some(bytes)) = (writer, stdin) {
                w.write_all(&bytes).await?;
                w.shutdown().await?;
            }
            Ok::<(), std::io::Error>(())
        });
        let stderr = proc.stderr();
        let errs = tokio::spawn(async move { read_capped(stderr, 16 * 1024, true).await });
        let (out, overflowed) = read_capped(stdout, stdout_cap, false).await;
        if overflowed {
            proc.abort();
        }
        let stderr = errs.await.map(|(b, _)| b).unwrap_or_default();
        let wrote = write.await;
        let status = if overflowed { None } else { status.await };
        let _ = proc.join().await;
        if !overflowed {
            if let Ok(Err(e)) = wrote {
                return Err(internal("writing to the exec's stdin", e));
            }
        }
        let success = status.as_ref().and_then(|s| s.status.as_deref()) == Some("Success");
        Ok(ExecOut {
            stdout: out,
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            success,
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

    /// True (and consumed) when `line` is one of the stored overlap.
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
        for rel in commands.input_files.keys() {
            if let Some(problem) = relative_problem(rel) {
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
    /// container terminated until all of them have stopped, which is why
    /// `files-out` exits on SIGTERM ([`pod_spec::files_out_script`]) -- the
    /// pod then goes when `main` does. The wait is bounded by `grace` +
    /// [`CANCEL_WAIT_MARGIN`]; a pod still there then is a WARN and
    /// `Ok(())`, since the outcome is the budget's either way (`cancel.rs`
    /// fact 2). [`release`](TaskRunner::release) then deletes with grace 0.
    async fn cancel(&self, task: &TaskInput, grace: Duration) -> Result<()> {
        let name = pod_name(task.run_id, attempt_of(task));
        if grace.is_zero() {
            return self.delete_pod(&name).await;
        }
        let secs = grace.as_secs() + u64::from(grace.subsec_nanos() > 0);
        let secs = u32::try_from(secs).unwrap_or(u32::MAX);
        match self
            .pods()
            .delete(&name, &DeleteParams::default().grace_period(secs))
            .await
        {
            Ok(_) => {}
            Err(kube::Error::Api(ae)) if ae.code == 404 => return Ok(()),
            Err(e) => return Err(internal("deleting the task pod", e)),
        }
        self.wait_main_stopped(task, &name, grace + CANCEL_WAIT_MARGIN)
            .await;
        Ok(())
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
        sweep_orphans(&self.client, &self.cfg, pool).await
    }
}

/// **The pod half of the orphan sweep** (spec §5, *Orphans*): delete every
/// task pod in the task namespace whose run is **terminal**, unless the author
/// asked to keep it (`delete: false`).
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
pub async fn sweep_orphans(
    client: &kube::Client,
    cfg: &PodConfig,
    pool: &sqlx::PgPool,
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
        if keep {
            continue;
        }
        let Some(id) = p
            .metadata
            .labels
            .as_ref()
            .and_then(|l| l.get(TASKRUN_LABEL))
            .and_then(|v| uuid::Uuid::parse_str(v).ok())
        else {
            continue;
        };
        candidates.push((name, id));
    }
    if candidates.is_empty() {
        return Ok(0);
    }
    let ids: Vec<uuid::Uuid> = candidates.iter().map(|(_, id)| *id).collect();
    let live = run_liveness(pool, &ids).await?;
    let mut deleted = 0u64;
    for (name, id) in candidates {
        if live.missing.contains(&id) {
            tracing::debug!(pod = %name, "orphan sweep: run {id} is unknown to this database; left alone");
            continue;
        }
        if !live.terminal.contains(&id) {
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
}
