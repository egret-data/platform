//! [`KubernetesRunner`]: the lifecycle of one task pod (spec §5).
//!
//! ```text
//! run ─ build ─ get-or-create ─┬─ wait for files-in running ─ upload + verify + `ready`
//!                              └─ wait for main started
//!       follow main's log (timestamps; sinceTime on re-attach) ─ ingest_line ─ RunLog
//!       main terminated → exit code ─ download outputFiles (exec + tar) ─ delete / `ended`
//! ```
//!
//! # What ends a pod, and what never does
//!
//! * **`run` returning** — `Ok` (whatever the exit code) or an author error —
//!   deletes the pod with grace 0 when `taskRunner.delete` (default `true`);
//!   with `delete: false` it uploads the `ended` marker instead, so the sidecar
//!   exits and the pod completes rather than holding its resources for ever.
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
    relative_problem, run_liveness, stream_line, CommandsResult, LineObserver, OrphanSweep,
    StreamState, TaskCommands, TaskRunner, MAX_OUTPUT_FILE_BYTES, MAX_VARS_BYTES, RUN_NAME_PREFIX,
    TASKRUN_LABEL, WORKING_DIR,
};
use crate::worker::TaskInput;

/// How long one exec (a tar in or out, a verification, a marker) may take
/// before egret calls its own transfer failed.
const EXEC_TIMEOUT: Duration = Duration::from_secs(120);
/// How many times an upload is attempted when the exec itself fails (Kestra's
/// `UPLOAD_RETRY_MAX_ATTEMPTS` is 5; egret's pod is fresh and local).
const UPLOAD_ATTEMPTS: u32 = 3;
/// A waiting reason after which a container will not start without a change
/// to its spec. `ErrImagePull` is left out: it is the first, retried, failure
/// of a pull and becomes `ImagePullBackOff` when it persists.
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
        loop {
            let p = self.get(name).await?;
            if let Some(e) = start_failure(&p) {
                return Err(e);
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
            if r.is_ok() && split {
                // Everything `main` wrote is in the file now: close the stream.
                self.end_stderr(name, &sentinel).await;
            }
            let _ = done_tx.send(if r.is_ok() { Pump::Done } else { Pump::Failed });
            r
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
                )
                .await;
            }
        };
        let (out, (), ()) = tokio::join!(stdout, stderr, consume);
        let (exit, reason) = out?;
        result.exit_code = exit;

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
    async fn end_stderr(&self, name: &str, sentinel: &str) {
        let cmd = vec![
            "sh".to_string(),
            "-c".to_string(),
            format!("printf '%s\\n' \"$1\" >> {STDERR_FILE}"),
            "sh".to_string(),
            sentinel.to_string(),
        ];
        match self.exec(name, FILES_OUT, cmd, None, 4096).await {
            Ok(o) if o.success => {}
            Ok(o) => {
                tracing::warn!(pod = %name, "could not end main's stderr stream: {}", o.stderr)
            }
            Err(e) => tracing::warn!(pod = %name, "could not end main's stderr stream: {e}"),
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
        let attempt = RunIdentity::stored_attempt(task.attempt);
        let Some(last) = self
            .logs
            .last_ts(task.execution_id, task.run_id, attempt)
            .await?
        else {
            return Ok((None, Replayed::default()));
        };
        let stored = self
            .logs
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
            if let Some(problem) = relative_problem(rel) {
                result.failure = Some(problem);
                return Ok(());
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
            let bytes = match transfer::judge(rel, entry, self.max_output_bytes) {
                Ok(b) => b,
                Err(problem) => {
                    result.failure = Some(problem);
                    break;
                }
            };
            total += bytes.len();
            if total > self.max_output_bytes {
                result.failure = Some(format!(
                    "the output files declared by this task come to {total} bytes, over the \
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

        match &outcome {
            Err(Error::Internal(e)) => tracing::warn!(
                run = %task.run_id,
                pod = %name,
                "the task pod is kept for a later claim to re-attach to: {e}"
            ),
            _ if spec.delete => {
                if let Err(e) = self.delete_pod(&name).await {
                    tracing::warn!(run = %task.run_id, pod = %name, "{e}");
                }
            }
            _ => self.signal_ended(&name).await,
        }
        outcome
    }

    /// Delete the attempt's pod with grace 0 — Kestra's `killGracePeriod`
    /// default of zero. A pod that is not there is `Ok(())`.
    async fn cancel(&self, task: &TaskInput) -> Result<()> {
        self.delete_pod(&pod_name(task.run_id, attempt_of(task)))
            .await
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
/// A start failure of `main` is the **author's** (the image is theirs);
/// one of a helper is **egret's** (its image, its configuration), so that one
/// is a worker failure and the pod is kept.
fn start_failure(p: &Pod) -> Option<Error> {
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
            let msg = format!(
                "the {} container could not start: {reason}: {}",
                cs.name,
                w.message.as_deref().unwrap_or("")
            );
            return Some(if cs.name == MAIN {
                Error::InvalidInput(msg)
            } else {
                internal("starting a helper container", msg)
            });
        }
    }
    if let Phase::Terminated { exit, .. } = phase(status_of(p, FILES_IN)) {
        if exit != 0 {
            return Some(internal(
                "the files-in container",
                format!("exited {exit} before the input files were ready"),
            ));
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

    #[test]
    fn an_image_that_will_not_pull_is_the_authors_on_main_and_egrets_on_a_helper() {
        let waiting = |reason: &str| ContainerState {
            waiting: Some(ContainerStateWaiting {
                reason: Some(reason.into()),
                message: Some("nope".into()),
            }),
            ..Default::default()
        };
        assert!(matches!(
            start_failure(&pod_with(MAIN, waiting("ImagePullBackOff"))),
            Some(Error::InvalidInput(_))
        ));
        assert!(matches!(
            start_failure(&pod_with(FILES_OUT, waiting("ImagePullBackOff"))),
            Some(Error::Internal(_))
        ));
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
}
