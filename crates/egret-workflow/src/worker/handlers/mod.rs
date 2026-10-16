//! The four task handlers W3b's oracle needs, and the log sink two of them
//! write to.
//!
//! # The shapes are measured, twice, and the two measurements disagreed
//!
//! Every claim below was taken **from the class** at checkout
//! `96893b589a02e5d5d85c722855f76c08e79085af` and then **re-measured against
//! the running artifact** `kestra/kestra:v2.0.0-rc13`. That was not belt and
//! braces: the two disagree about `core.flow.Sleep`, and the disagreement is
//! the reason this module refuses a duration the checkout's source would
//! accept — see [`sleep`].
//!
//! | type | returns | this module produces |
//! |---|---|---|
//! | `core.debug.Return` | `Return.Output` (`Return.java:87`), one field `private String value` (`:119`), built at `:108` from a `format` rendered `.orElse(null)` (`:99`) | `Some({"value": …})`, or `Some({})` when there is no `format:` |
//! | `core.log.Log` | `VoidOutput` (`Log.java:68`); `run` returns Java `null` (`:109`) | `None` |
//! | `core.execution.Fail` | `VoidOutput` (`Fail.java:118`); throws, **or** returns `null` and succeeds (`:137`) | `None` |
//! | `core.flow.Sleep` | `VoidOutput` (`Sleep.java:47`); returns `null` (`:62`) | `None` |
//!
//! The fifth handler, [`sql`], is **not** in that table and could not be: it is
//! `egret.sql`, egret's own type, which Kestra has no counterpart for and which
//! reaches zero corpus entries. Its shapes are decided rather than measured, and
//! its module doc says which and why.
//!
//! `VoidOutput` is an empty class (`VoidOutput.java:3`) and none of the three
//! ever instantiates it. What an *expression* sees for such a task is not
//! nothing, though — it is `{}` — and that measurement lives on
//! [`Outcome::outputs`](super::Outcome::outputs), because it is the caller's
//! rule and not a handler's.
//!
//! # Rendering
//!
//! Kestra's handlers render through a `RunContext`; these render through
//! [`crate::runners::render_str`] and [`crate::models::Property::render`],
//! against [`TaskInput::render`](super::TaskInput::render) — which W3b Task 5
//! fills and which is **empty today**. A property that references a variable
//! therefore fails to render, and every handler here turns a render failure
//! into `Ok(Outcome::failed_with(…))` rather than `Err`, because that is what
//! Kestra does: a render throws inside `run`, and a task whose `run` throws is
//! `FAILED`, not a worker fault.

pub mod container;
pub mod fail;
pub mod kestra;
pub mod kv;
pub mod log;
pub mod namespace;
pub mod python;
pub mod ret;
pub mod script;
pub mod sleep;
pub mod sql;
pub mod storage;

use crate::logs::{LogLevel, LogLine};

/// The run's own `…/tasks/{task}/{taskRunId}/` prefix, under which a handler
/// that writes a file in egret puts it: the render scope's (the worker's
/// claim computed it), or else built from `flow.namespace` and `flow.id` the
/// way `script.rs`'s `collect_to` does. `writes` names the writer in the
/// message of a run that has none (W9a Tasks 10 and 11: `egret.sql`'s STORE,
/// `core.storage.Write`). `Err` is the author's: a failed task.
pub(crate) fn task_storage_prefix(task: &super::TaskInput, writes: &str) -> Result<String, String> {
    if let Some(prefix) = task.render.files().and_then(|f| f.0.task_prefix()) {
        return Ok(prefix.to_string());
    }
    let flow = (
        crate::runners::render_str("{{ flow.namespace }}", &task.render),
        crate::runners::render_str("{{ flow.id }}", &task.render),
    );
    let (Ok(namespace), Ok(flow_id)) = flow else {
        return Err(format!(
            "this run has no storage prefix for the file {writes} writes: it is not a task \
             run of a flow's execution"
        ));
    };
    crate::storage::uri::task_prefix(
        &namespace,
        &flow_id,
        task.execution_id,
        &task.task_id,
        task.run_id,
    )
}

/// Where a task's log lines go.
///
/// A port rather than a direct `tracing::info!` for one reason: `core.log.Log`
/// **has no output** (`Log.java:68`), so the only thing it produces is its log
/// lines, and a test that could not see them could only assert that the
/// handler *ran*. That is the failure mode this task exists to avoid.
///
/// **Async, and it takes an owned line** (W11 spec §4.1): a durable sink sends
/// into a bounded channel with `send().await`, so a full channel makes the
/// writer WAIT (D3) without blocking a tokio thread. A sync method could only
/// drop the line or block the runtime.
///
/// Nothing writes to a sink directly any more: a handler writes through the
/// attempt's [`RunLog`](crate::logs::RunLog) on
/// [`TaskInput::log`](super::TaskInput::log), which stamps the identity,
/// splits long lines and enforces the per-attempt cap. The registry holds the
/// sink ([`Handlers::with_log_sink`](super::Handlers::with_log_sink)) and
/// `claim::run_one` builds the handle from it.
#[async_trait::async_trait]
pub trait LogSink: Send + Sync + std::fmt::Debug {
    async fn write(&self, line: LogLine);

    /// The flush barrier (W11 spec §4.3): return once every line this sink
    /// accepted before the call is stored, or once the sink's own bound has
    /// passed. **Never unbounded and never an error**: `RunLog::finish` awaits
    /// it before the run's outcome is committed, so it may delay an outcome by
    /// at most its bound and must never change one. A sink that stores
    /// synchronously (this default) has nothing to wait for.
    async fn flush(&self) {}
}

/// A [`LogSink`] over the workspace's own `tracing`: the default when no
/// durable sink is configured, and what [`RunLog::discard`] writes to.
///
/// [`RunLog::discard`]: crate::logs::RunLog::discard
#[derive(Clone, Copy, Debug, Default)]
pub struct TracingSink;

#[async_trait::async_trait]
impl LogSink for TracingSink {
    async fn write(&self, line: LogLine) {
        let execution = line.execution_id;
        let task_id = line.task_id.as_deref().unwrap_or("");
        let attempt = line.attempt.unwrap_or(-1);
        let message = &line.message;
        match line.level {
            LogLevel::Trace => tracing::trace!(%execution, %task_id, attempt, "{message}"),
            LogLevel::Debug => tracing::debug!(%execution, %task_id, attempt, "{message}"),
            LogLevel::Info => tracing::info!(%execution, %task_id, attempt, "{message}"),
            LogLevel::Warn => tracing::warn!(%execution, %task_id, attempt, "{message}"),
            LogLevel::Error => tracing::error!(%execution, %task_id, attempt, "{message}"),
        }
    }
}

#[cfg(test)]
pub(crate) mod testing {
    //! A recording [`LogSink`], shared by the handlers' and `RunLog`'s tests.

    use std::sync::{Arc, Mutex};

    use super::super::TaskInput;
    use super::LogSink;
    use crate::logs::{LogLevel, LogLimits, LogLine, RunIdentity, RunLog};

    #[derive(Debug, Default)]
    pub struct Recorder {
        lines: Mutex<Vec<LogLine>>,
    }

    impl Recorder {
        pub fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }
        pub fn lines(&self) -> Vec<LogLine> {
            self.lines.lock().unwrap().clone()
        }
        /// `(level, message)` per line, in write order.
        pub fn texts(&self) -> Vec<(LogLevel, String)> {
            self.lines()
                .into_iter()
                .map(|l| (l.level, l.message))
                .collect()
        }
    }

    #[async_trait::async_trait]
    impl LogSink for Recorder {
        async fn write(&self, line: LogLine) {
            self.lines.lock().unwrap().push(line);
        }
    }

    /// `input(type_id, spec)` whose `log` records into the returned
    /// [`Recorder`], under default limits.
    pub fn recorded(type_id: &str, spec: serde_json::Value) -> (TaskInput, Arc<Recorder>) {
        let rec = Recorder::new();
        let mut task = input(type_id, spec);
        task.log = recording(&task, &rec);
        (task, rec)
    }

    /// A [`RunLog`] for `task`'s identity writing into `rec`.
    pub fn recording(task: &TaskInput, rec: &Arc<Recorder>) -> RunLog {
        RunLog::new(
            RunIdentity {
                workspace_id: task.workspace_id,
                execution_id: task.execution_id,
                task_run_id: task.run_id,
                task_id: task.task_id.clone(),
                attempt: RunIdentity::stored_attempt(task.attempt),
            },
            rec.clone(),
            LogLimits::default(),
        )
    }

    /// A [`TaskInput`] carrying `spec`, against an empty render context.
    pub fn input(type_id: &str, spec: serde_json::Value) -> TaskInput {
        TaskInput {
            workspace_id: uuid::Uuid::nil(),
            execution_id: uuid::Uuid::nil(),
            run_id: uuid::Uuid::nil(),
            task_id: "t".to_string(),
            type_id: type_id.to_string(),
            value: None,
            attempt: 1,
            spec,
            render: crate::runners::RenderContext::new(),
            // No budget, which is what a task with no `timeout:` gets. A
            // handler test that wants one sets it on the returned value, so a
            // default here can never make a bounded test look unbounded.
            timeout: None,
            working_dir: None,
            // `None`, and that is right for the four handlers that share this
            // helper: not one of them reaches an egret object, so an identity
            // they cannot use must not be there to be quietly relied on.
            // `handlers::sql` builds its own input for that reason.
            principal: None,
            // A fixed instant rather than `Utc::now()`, so two runs of the
            // same test build the same input. It bounds only `Delete`'s and
            // `PurgeCurrentExecutionFiles`'s row `DELETE` (W9a final review
            // I1), and no test of this helper reaches a database with one.
            claimed_at: chrono::DateTime::UNIX_EPOCH,
            // `tracing`, under a nil identity. A test that reads the lines
            // uses `recorded` instead.
            log: RunLog::discard(),
        }
    }
}
