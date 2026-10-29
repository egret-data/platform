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
    /// printing omits it as Kestra's `NON_NULL` mapper does -- for every field
    /// but `attemptNumber`, which `LogEntry.java:51-53` marks
    /// `@JsonInclude(ALWAYS)`, so Kestra prints a null one as `null` where
    /// egret omits it (spec §6.2 recorded divergence 5).
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

/// The most bytes of candidate text [`calls_error_logs`] parses for one
/// claim (fix wave #5b, I3). egret's.
///
/// A candidate is a string that holds both `{` and `errorLogs`; past this
/// many bytes of them the scan stops and answers `true`, the over-reading
/// side (one read, and at most `worker::claim::ERROR_LOGS_WAIT` on an empty
/// log), rather than parse more run-time data. 4 MiB is four times the
/// output cap of one render pass; the parse it bounds is linear
/// ([`super::env::name_in_code`]).
pub const MAX_ERROR_LOGS_SCAN_BYTES: usize = 4 << 20;

/// The cheap filter: code needs a `{` (`{{`, `{%`), and the name is written
/// out in the source.
fn is_candidate(text: &str) -> bool {
    text.contains('{') && text.contains("errorLogs")
}

/// Whether `text` calls `errorLogs` as code (`env::name_in_code`): what the
/// worker reads the list for. A cheap substring test first; a text that does
/// not parse calls nothing (fix wave #5b, I3: it used to answer `true`, so
/// `"errorLogs {{"` in an input made every task of the execution wait).
pub fn text_calls_error_logs(text: &str) -> bool {
    is_candidate(text) && super::env::name_in_code(text, "errorLogs")
}

/// [`text_calls_error_logs`] over every text and every string (keys too) of
/// every JSON value -- the texts a `render()` can reach at run time.
///
/// **Bounded and off the runtime (fix wave #5b, I3).** The walk only
/// collects the candidates ([`is_candidate`], linear); at most
/// [`MAX_ERROR_LOGS_SCAN_BYTES`] of them are parsed, past which the answer
/// is `true`; and the parsing runs on a thread of its own with a render's
/// stack (`env::on_a_parse_thread`), never on a tokio worker. A thread that
/// cannot start answers `true`, the over-reading side.
pub fn calls_error_logs(texts: &[&str], json: &[&serde_json::Value]) -> bool {
    fn walk<'v>(v: &'v serde_json::Value, out: &mut Vec<&'v str>) {
        match v {
            serde_json::Value::String(s) => {
                if is_candidate(s) {
                    out.push(s);
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|v| walk(v, out)),
            serde_json::Value::Object(map) => map.iter().for_each(|(k, v)| {
                if is_candidate(k) {
                    out.push(k);
                }
                walk(v, out);
            }),
            _ => {}
        }
    }
    let mut candidates: Vec<&str> = texts.iter().copied().filter(|t| is_candidate(t)).collect();
    json.iter().for_each(|v| walk(v, &mut candidates));
    if candidates.is_empty() {
        return false;
    }
    if candidates.iter().map(|t| t.len()).sum::<usize>() > MAX_ERROR_LOGS_SCAN_BYTES {
        return true;
    }
    super::env::on_a_parse_thread(|| {
        candidates
            .iter()
            .any(|t| super::env::name_in_code(t, "errorLogs"))
    })
    .unwrap_or(true)
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
        // One copy probes every occurrence: a call after data still counts,
        // and so does an assignment target.
        assert!(text_calls_error_logs(
            "{{ 'errorLogs' }} errorLogs {# errorLogs #}{{ errorLogs() }}"
        ));
        assert!(text_calls_error_logs("{% set errorLogs = 1 %}"));
    }

    /// Fix wave #5b, I3: a string that does not parse cannot be rendered, so
    /// it calls nothing. It used to answer `true`, and `"errorLogs {{"` in an
    /// input made every task of the execution read and wait.
    #[test]
    fn a_text_that_does_not_parse_calls_nothing() {
        assert!(!text_calls_error_logs("errorLogs {{"));
        assert!(!text_calls_error_logs("{{ errorLogs( }}"));
        assert!(!calls_error_logs(
            &["no call"],
            &[&serde_json::json!({"inputs": {"x": "errorLogs {{"}})]
        ));
    }

    /// Fix wave #5b, I3: run-time data full of the word is cheap. Without a
    /// `{` nothing is parsed; with one, every occurrence is probed in one
    /// parse (it was one parse of the whole text per occurrence: 20 000 x
    /// 360 KB here, minutes; the bound below is generous for a debug build).
    #[test]
    fn detection_is_linear_in_the_text() {
        let started = std::time::Instant::now();
        let plain = "errorLogs ".repeat(100_000);
        assert_eq!(plain.len(), 1_000_000);
        assert!(!text_calls_error_logs(&plain));
        assert!(!calls_error_logs(&[], &[&serde_json::json!({"x": plain})]));
        // Lines, not one line: minijinja's lexer keeps a column in a `u16`
        // and its debug build overflows reporting an error past column
        // 65 535 (the release build wraps; not this fix's concern).
        let data = "{{ 1 }} errorLogs\n".repeat(20_000);
        assert!(!text_calls_error_logs(&data));
        assert!(text_calls_error_logs(&format!(
            "{data}{{{{ errorLogs() }}}}"
        )));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "{:?}",
            started.elapsed()
        );
    }

    /// Fix wave #5b, I3: past [`MAX_ERROR_LOGS_SCAN_BYTES`] of candidate text
    /// the scan stops and answers `true` (the over-reading side) without
    /// parsing; at or under it, the same data parses and answers `false`.
    #[test]
    fn the_scan_is_capped() {
        let piece = "{ errorLogs ".repeat(MAX_ERROR_LOGS_SCAN_BYTES / 12 / 4);
        let under: Vec<serde_json::Value> = (0..4).map(|_| serde_json::json!(piece)).collect();
        let refs: Vec<&serde_json::Value> = under.iter().collect();
        assert!(!calls_error_logs(&[], &refs));
        let over = serde_json::json!("{ errorLogs ".repeat(MAX_ERROR_LOGS_SCAN_BYTES / 12 + 1));
        assert!(calls_error_logs(&[], &[&over]));
    }
}
