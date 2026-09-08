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
pub mod log;
pub mod python;
pub mod ret;
pub mod sleep;
pub mod sql;

use std::sync::Arc;

use super::TaskInput;

/// Where a task's log lines go.
///
/// A port rather than a direct `tracing::info!` for one reason: `core.log.Log`
/// **has no output** (`Log.java:68`), so the only thing it produces is its log
/// lines, and a test that could not see them could only assert that the
/// handler *ran*. That is the failure mode this task exists to avoid.
///
/// egret has no durable workflow-log store yet — Kestra writes to one and W3b
/// builds no table for it — so [`TracingSink`] is the shipping implementation
/// and the gap is stated rather than papered over.
pub trait LogSink: Send + Sync + std::fmt::Debug {
    fn log(&self, task: &TaskInput, level: Level, message: &str);
}

/// `Log.java`'s `level:`, which is `org.slf4j.event.Level`.
///
/// The five names are slf4j's own and are compared **case-sensitively**, as
/// Jackson compares an enum: measured, `level: SHOUT` fails the run on the
/// artifact rather than falling back to `INFO`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    /// `None` for anything outside the five — the caller turns that into a
    /// failed run, exactly as Jackson's enum conversion does.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "TRACE" => Level::Trace,
            "DEBUG" => Level::Debug,
            "INFO" => Level::Info,
            "WARN" => Level::Warn,
            "ERROR" => Level::Error,
            _ => return None,
        })
    }
}

/// The shipping [`LogSink`]: the workspace's own `tracing`.
#[derive(Clone, Copy, Debug, Default)]
pub struct TracingSink;

impl LogSink for TracingSink {
    fn log(&self, task: &TaskInput, level: Level, message: &str) {
        let execution = task.execution_id;
        let task_id = &task.task_id;
        match level {
            Level::Trace => tracing::trace!(%execution, %task_id, "{message}"),
            Level::Debug => tracing::debug!(%execution, %task_id, "{message}"),
            Level::Info => tracing::info!(%execution, %task_id, "{message}"),
            Level::Warn => tracing::warn!(%execution, %task_id, "{message}"),
            Level::Error => tracing::error!(%execution, %task_id, "{message}"),
        }
    }
}

/// The sink a handler built with `::new()` writes to.
pub fn default_sink() -> Arc<dyn LogSink> {
    Arc::new(TracingSink)
}

#[cfg(test)]
pub(crate) mod testing {
    //! A recording [`LogSink`], shared by `log`'s and `fail`'s tests.

    use std::sync::{Arc, Mutex};

    use super::{Level, LogSink, TaskInput};

    #[derive(Debug, Default)]
    pub struct Recorder {
        lines: Mutex<Vec<(Level, String)>>,
    }

    impl Recorder {
        pub fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }
        pub fn lines(&self) -> Vec<(Level, String)> {
            self.lines.lock().unwrap().clone()
        }
    }

    impl LogSink for Recorder {
        fn log(&self, _task: &TaskInput, level: Level, message: &str) {
            self.lines
                .lock()
                .unwrap()
                .push((level, message.to_string()));
        }
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
            // `None`, and that is right for the four handlers that share this
            // helper: not one of them reaches an egret object, so an identity
            // they cannot use must not be there to be quietly relied on.
            // `handlers::sql` builds its own input for that reason.
            principal: None,
            // A fixed instant rather than `Utc::now()`: this helper's callers
            // assert on values derived from the input, and a clock reading
            // would make `commands::container_name`'s output differ between
            // two runs of the same test. `handlers::container`'s tests build
            // their own input when they need two DIFFERENT leases.
            claimed_at: chrono::DateTime::UNIX_EPOCH,
        }
    }
}
