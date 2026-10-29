// kestra: core/src/main/java/io/kestra/core/runners/pebble/functions/ErrorLogsFunction.java
// kestra: core/src/main/java/io/kestra/core/models/executions/LogEntry.java

//! `errorLogs()` (package #5b Task B7, spec D11).
//!
//! # What Kestra does
//!
//! `ErrorLogsFunction.execute` answers only on the worker
//! (`PebbleUtils.calledOnWorker`); anywhere else it throws
//! [`OFF_THE_WORKER`]. On the worker it reads
//! `ExecutionLogMetaStore.errorLogs(flow.tenantId, execution.id)` -- the
//! first page of 25 ERROR `LogEntry`s of the execution, oldest first -- and,
//! while that list is empty, reads again (exponential, 100 ms to 1 s apart)
//! for at most 5 s, then answers `[]`. Pebble checks the arguments first: the
//! function declares none, so any argument fails before the worker check.
//!
//! # What egret does
//!
//! minijinja functions are synchronous, so the database is never read here.
//! The worker reads the list **before** the render (`worker::claim`'s
//! `render_context`), and only when the run's text calls `errorLogs`
//! ([`calls_error_logs`]); it attaches the answer to the render context
//! ([`ErrorLogs`]), and both render threads bind it as the
//! [`ERROR_LOGS_SCOPE`] global, as they bind the file scope. So:
//!
//! - no global (an executor-side render, a trigger, an input, the oracle): Kestra's
//!   [`OFF_THE_WORKER`] sentence;
//! - [`ErrorLogs::NotLoaded`] (a worker render whose text did not call the
//!   function when the worker looked: the name was assembled at render, by
//!   `~` or a filter): egret's [`NOT_LOADED`] sentence, the rule a `kestra://`
//!   URI assembled at render follows (`storage::scope::not_in_snapshot`);
//! - [`ErrorLogs::Loaded`]: the list, each line a map in `LogEntry`'s shape.
//!
//! The wait is egret's 1 s, not Kestra's 5 s (spec D11's divergence); see
//! `worker::claim::ERROR_LOGS_WAIT` for why it is needed at all.
//!
//! # A log message is data
//!
//! egret's top-level render runs to a fixpoint (`env::render_str`); Kestra's
//! default (`recursiveRendering = false`) runs one pass. A message holding
//! `{{ … }}` printed by one pass would be evaluated by the next. So a pass
//! that called `errorLogs` is the last one: [`take_handed_out`], read by the
//! loop. An author who asks for a message to be rendered (`render(…)`) gets
//! what Kestra's `render()` does.

use std::cell::Cell;
use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};
use minijinja::value::{Object, Rest, Value as MjValue};
use minijinja::{Error as MjError, State};
use uuid::Uuid;

use super::env::OrderedMap;
use super::filters::{pebble_error, Args};

/// Kestra's sentence off the worker (`ErrorLogsFunction.java:45`).
pub(crate) const OFF_THE_WORKER: &str =
    "The 'errorLogs' function can only be used in the Worker as it access logs from the database.";

/// egret's sentence for a worker render that did not load the list.
pub(crate) const NOT_LOADED: &str = "The 'errorLogs' function was not loaded for this render: \
     the worker reads the execution's error logs before a task renders, and only when the \
     task's source or a value of its render context (an input, a task's outputs, the loop \
     item) calls errorLogs; a call assembled while rendering, by `~` or a filter, is not seen";

/// The render-time global the function looks up. A space in it, like
/// `FILE_SCOPE`, so no template can name it.
pub(crate) const ERROR_LOGS_SCOPE: &str = "egret error logs";

/// One stored ERROR line, as `errorLogs()` hands it out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorLogEntry {
    pub namespace: String,
    pub flow_id: String,
    pub task_id: Option<String>,
    pub execution_id: Uuid,
    pub task_run_id: Option<Uuid>,
    /// 0-based, as stored and as Kestra's `attemptNumber`.
    pub attempt: Option<i32>,
    pub ts: DateTime<Utc>,
    pub message: String,
}

impl ErrorLogEntry {
    /// `LogEntry`'s fields in declaration order (Lombok `@Value`, which is
    /// the order Jackson writes them in). A field egret does not record
    /// (`tenantId`, `triggerId`, `thread`, `executionKind`, `progress`) is
    /// `null`, so a member read answers `null` as the Java getter does, and
    /// printing omits it as Kestra's `NON_NULL` mapper does.
    fn to_value(&self) -> MjValue {
        let opt = |s: Option<String>| s.map(MjValue::from).unwrap_or(MjValue::from(()));
        MjValue::from_object(OrderedMap::new(vec![
            ("tenantId".into(), MjValue::from(())),
            ("namespace".into(), MjValue::from(self.namespace.clone())),
            ("flowId".into(), MjValue::from(self.flow_id.clone())),
            ("taskId".into(), opt(self.task_id.clone())),
            (
                "executionId".into(),
                MjValue::from(self.execution_id.to_string()),
            ),
            (
                "taskRunId".into(),
                opt(self.task_run_id.map(|id| id.to_string())),
            ),
            (
                "attemptNumber".into(),
                self.attempt.map(MjValue::from).unwrap_or(MjValue::from(())),
            ),
            ("triggerId".into(), MjValue::from(())),
            // `Instant.toString()` / Jackson's `ISO_INSTANT`: the fraction in
            // groups of three digits, none when zero.
            (
                "timestamp".into(),
                MjValue::from(self.ts.to_rfc3339_opts(SecondsFormat::AutoSi, true)),
            ),
            ("level".into(), MjValue::from("ERROR")),
            ("thread".into(), MjValue::from(())),
            ("message".into(), MjValue::from(self.message.clone())),
            ("executionKind".into(), MjValue::from(())),
            ("progress".into(), MjValue::from(())),
        ]))
    }
}

/// What a worker render knows about `errorLogs()`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorLogs {
    /// The execution's ERROR lines, read before the render.
    Loaded(Arc<[ErrorLogEntry]>),
    /// A worker render whose text did not call `errorLogs` ([`NOT_LOADED`]).
    NotLoaded,
}

/// [`ERROR_LOGS_SCOPE`]'s value: opaque, so `fetchContext` keeps it out of
/// its answer by identity.
#[derive(Debug)]
pub(crate) struct ErrorLogsObject(pub(crate) ErrorLogs);

impl Object for ErrorLogsObject {}

thread_local! {
    /// Set by a call that handed out log text; taken by the fixpoint loop.
    static HANDED_OUT: Cell<bool> = const { Cell::new(false) };
}

/// Whether a call on this thread handed out log text since the last take.
pub(super) fn take_handed_out() -> bool {
    HANDED_OUT.with(|h| h.replace(false))
}

/// Whether `text` calls `errorLogs` as code (`env::name_in_code`): what the
/// worker reads the list for. A cheap substring test first.
pub fn text_calls_error_logs(text: &str) -> bool {
    text.contains("errorLogs") && super::env::name_in_code(text, "errorLogs")
}

/// [`text_calls_error_logs`] over every text and every string (keys too) of
/// every JSON value -- the texts a `render()` can reach at run time.
pub fn calls_error_logs(texts: &[&str], json: &[&serde_json::Value]) -> bool {
    fn walk(v: &serde_json::Value) -> bool {
        match v {
            serde_json::Value::String(s) => text_calls_error_logs(s),
            serde_json::Value::Array(items) => items.iter().any(walk),
            serde_json::Value::Object(map) => {
                map.iter().any(|(k, v)| text_calls_error_logs(k) || walk(v))
            }
            _ => false,
        }
    }
    texts.iter().any(|t| text_calls_error_logs(t)) || json.iter().any(|v| walk(v))
}

/// `ErrorLogsFunction`.
pub(super) fn error_logs(state: &State, rest: Rest<MjValue>) -> Result<MjValue, MjError> {
    let args = Args::new(rest);
    // Pebble's `ArgumentsNode` checks, `getArgumentNames()` being empty.
    if !args.positional_only().is_empty() {
        return Err(pebble_error(
            "The argument at position 1 is not allowed. Only 0 argument(s) are allowed.",
        ));
    }
    if let Some(unknown) = args.named_keys().next() {
        return Err(pebble_error(format!(
            "The following named argument does not exist: {unknown}"
        )));
    }
    let held = state.lookup(ERROR_LOGS_SCOPE).and_then(|v| {
        v.downcast_object_ref::<ErrorLogsObject>()
            .map(|held| held.0.clone())
    });
    match held {
        None => Err(pebble_error(OFF_THE_WORKER)),
        Some(ErrorLogs::NotLoaded) => Err(pebble_error(NOT_LOADED)),
        Some(ErrorLogs::Loaded(lines)) => {
            if !lines.is_empty() {
                HANDED_OUT.with(|h| h.set(true));
            }
            Ok(MjValue::from(
                lines
                    .iter()
                    .map(ErrorLogEntry::to_value)
                    .collect::<Vec<_>>(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runners::{render_str, RenderContext};
    use chrono::TimeZone;

    fn line(task: &str, message: &str, secs: i64) -> ErrorLogEntry {
        ErrorLogEntry {
            namespace: "io.kestra.tests".into(),
            flow_id: "errors".into(),
            task_id: Some(task.into()),
            execution_id: Uuid::from_u128(7),
            task_run_id: Some(Uuid::from_u128(8)),
            attempt: Some(0),
            ts: Utc
                .timestamp_opt(1_800_000_000 + secs, 123_000_000)
                .unwrap(),
            message: message.into(),
        }
    }

    fn cx(logs: Option<ErrorLogs>) -> RenderContext {
        let mut cx = RenderContext::new();
        cx.insert("outputs", serde_json::json!({"t": {"tpl": "{{ 1 + 1 }}"}}));
        match logs {
            Some(logs) => cx.with_error_logs(logs),
            None => cx,
        }
    }

    fn loaded(lines: Vec<ErrorLogEntry>) -> Option<ErrorLogs> {
        Some(ErrorLogs::Loaded(lines.into()))
    }

    /// Kestra's `ErrorLogsFunctionTest.shouldReturnNothingWhenNoErrors`: `[]`.
    #[test]
    fn an_empty_log_is_an_empty_list() {
        assert_eq!(
            render_str("{{ errorLogs() }}", &cx(loaded(vec![]))).unwrap(),
            "[]"
        );
        assert_eq!(
            render_str("{{ errorLogs() | length }}", &cx(loaded(vec![]))).unwrap(),
            "0"
        );
    }

    /// The lines, in the order handed in (the read's: oldest first), in
    /// `LogEntry`'s shape, null fields omitted when printed.
    #[test]
    fn the_error_lines_are_listed_in_log_entry_shape() {
        let cx = cx(loaded(vec![line("a", "first", 0), line("b", "second", 1)]));
        assert_eq!(
            render_str(
                "{% for log in errorLogs() %}{{ log.taskId }}:{{ log.message }};{% endfor %}",
                &cx
            )
            .unwrap(),
            "a:first;b:second;"
        );
        let first = render_str("{{ errorLogs()[0] }}", &cx).unwrap();
        assert_eq!(
            first,
            format!(
                "{{\"namespace\":\"io.kestra.tests\",\"flowId\":\"errors\",\"taskId\":\"a\",\
                 \"executionId\":\"{}\",\"taskRunId\":\"{}\",\"attemptNumber\":0,\
                 \"timestamp\":\"2027-01-15T08:00:00.123Z\",\"level\":\"ERROR\",\
                 \"message\":\"first\"}}",
                Uuid::from_u128(7),
                Uuid::from_u128(8)
            )
        );
        // A field egret does not record reads as null, as the getter does.
        assert_eq!(
            render_str("{{ errorLogs()[0].thread is null }}", &cx).unwrap(),
            "true"
        );
        assert_eq!(
            render_str("{{ errorLogs()[0]['message'] }}", &cx).unwrap(),
            "first"
        );
    }

    /// Off the worker (no list attached): Kestra's sentence.
    #[test]
    fn outside_the_worker_it_is_kestras_refusal() {
        let err = render_str("{{ errorLogs() }}", &cx(None)).unwrap_err();
        assert!(err.to_string().contains(OFF_THE_WORKER), "{err}");
        // Not the unknown-name error a missing registration would give.
        assert!(!err.to_string().contains("is unknown"), "{err}");
        let err = render_str("{{ errorLogs() }}", &cx(Some(ErrorLogs::NotLoaded))).unwrap_err();
        assert!(err.to_string().contains(NOT_LOADED), "{err}");
    }

    /// Pebble's argument checks come first, on and off the worker.
    #[test]
    fn errorlogs_takes_no_argument() {
        for logs in [None, loaded(vec![])] {
            let err = render_str("{{ errorLogs('x') }}", &cx(logs.clone())).unwrap_err();
            assert!(
                err.to_string()
                    .contains("The argument at position 1 is not allowed. Only 0 argument(s)"),
                "{err}"
            );
            let err = render_str("{{ errorLogs(n=1) }}", &cx(logs)).unwrap_err();
            assert!(
                err.to_string()
                    .contains("The following named argument does not exist: n"),
                "{err}"
            );
        }
    }

    /// A message holding an expression is data: printed as it is, never
    /// evaluated by a later pass of the fixpoint loop. The control: the same
    /// text from the render context IS evaluated by the loop, so the stop is
    /// what makes the difference.
    #[test]
    fn a_message_containing_braces_is_not_rendered() {
        let cx = cx(loaded(vec![line("a", "{{ 1 + 1 }} {{ outputs }}", 0)]));
        assert_eq!(
            render_str("{{ errorLogs()[0].message }}", &cx).unwrap(),
            "{{ 1 + 1 }} {{ outputs }}"
        );
        assert_eq!(
            render_str("{% for l in errorLogs() %}{{ l.message }}{% endfor %}", &cx).unwrap(),
            "{{ 1 + 1 }} {{ outputs }}"
        );
        assert_eq!(render_str("{{ outputs.t.tpl }}", &cx).unwrap(), "2");
        // The flag does not outlive the render that set it.
        assert!(!take_handed_out());
    }

    /// Inside `render()` on the worker it answers, as Kestra's nested render
    /// on the worker does; off the worker it is refused with Kestra's text.
    #[test]
    fn a_nested_render_reaches_the_same_list() {
        let mut on = cx(loaded(vec![line("a", "boom", 0)]));
        on.insert(
            "inputs",
            serde_json::json!({"x": "{{ errorLogs()[0].message }}"}),
        );
        assert_eq!(render_str("{{ render(inputs.x) }}", &on).unwrap(), "boom");
        let mut off = cx(None);
        off.insert(
            "inputs",
            serde_json::json!({"x": "{{ errorLogs()[0].message }}"}),
        );
        let err = render_str("{{ render(inputs.x) }}", &off).unwrap_err();
        assert!(err.to_string().contains(OFF_THE_WORKER), "{err}");
    }

    /// What the worker reads the list for: code, not data or a comment.
    #[test]
    fn only_a_call_in_code_asks_for_the_list() {
        assert!(text_calls_error_logs("{{ errorLogs() }}"));
        assert!(text_calls_error_logs(
            "{%- for log in errorLogs() -%}{{ log.message }}{%- endfor -%}"
        ));
        assert!(!text_calls_error_logs("see errorLogs() in the docs"));
        assert!(!text_calls_error_logs("{# errorLogs() #}"));
        assert!(!text_calls_error_logs("{{ 'errorLogs()' }}"));
        assert!(!text_calls_error_logs("{{ x.errorLogs }}"));
        assert!(calls_error_logs(
            &["no call"],
            &[&serde_json::json!({"a": ["{{ errorLogs() }}"]})]
        ));
        assert!(!calls_error_logs(
            &["no call"],
            &[&serde_json::json!({"a": ["errorLogs"]})]
        ));
    }
}
