//! `core.log.Log` — writes messages and produces nothing.

use serde::Deserialize;

use super::super::{Outcome, TaskHandler, TaskInput};
use crate::logs::LogLevel;
use crate::models::Property;
use crate::runners::render_str;

/// `io.kestra.plugin.core.log.Log`.
pub const TYPE_ID: &str = "io.kestra.plugin.core.log.Log";

/// `Log.java:79` — `private Object message;`, `@NotNull` (`:77`),
/// `@PluginProperty(dynamic = true)`, declared `oneOf = {String, String[]}`.
///
/// It is an `Object` rather than a `Property<…>`, so it is rendered as a raw
/// template ([`render_str`]) and not through [`Property::render`]; `level:` is
/// a real `Property<Level>` defaulting to `INFO`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Spec {
    message: Option<serde_json::Value>,
    level: Option<Property<String>>,
}

/// Render every message and write it to the attempt's
/// [`RunLog`](crate::logs::RunLog) — `task.log`.
#[derive(Debug, Default)]
pub struct Log;

impl Log {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl TaskHandler for Log {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    async fn run(&self, task: &TaskInput) -> egret_core::Result<Outcome> {
        let spec: Spec = match serde_json::from_value(task.spec.clone()) {
            Ok(s) => s,
            Err(e) => return Ok(Outcome::failed_with(format!("{TYPE_ID}: {e}"))),
        };

        // `runContext.render(this.level).as(Level.class).orElseThrow()`
        // (`Log.java:93`), and Jackson compares an enum name exactly:
        // measured, `level: SHOUT` fails the run rather than defaulting.
        let level = match spec.level {
            None => LogLevel::Info,
            Some(p) => {
                let rendered = match p.render(&task.render) {
                    Ok(v) => v,
                    Err(e) => return Ok(Outcome::failed_with(e.to_string())),
                };
                match LogLevel::parse(&rendered) {
                    Some(l) => l,
                    None => {
                        return Ok(Outcome::failed_with(format!(
                            "level: {rendered:?} is not one of TRACE, DEBUG, INFO, WARN, ERROR"
                        )))
                    }
                }
            }
        };

        // `Log.java:95-107`: a String is rendered once; a Collection is
        // rendered element by element; **anything else throws**
        // `IllegalArgumentException` and the run fails. An absent `message` is
        // `@NotNull`, which Kestra enforces at publish (measured: `POST
        // /flows` is 422); egret's publish does not carry that rule, so the
        // run fails here instead of the flow being unpublishable. Failing is
        // the safe direction — a `Log` that quietly succeeded having logged
        // nothing is the over-acceptance shape spec §7 forbids.
        let messages: Vec<&serde_json::Value> = match spec.message.as_ref() {
            None => return Ok(Outcome::failed_with("message: must not be null")),
            Some(serde_json::Value::Array(items)) => items.iter().collect(),
            Some(one) => vec![one],
        };

        for m in messages {
            let Some(text) = m.as_str() else {
                return Ok(Outcome::failed_with(format!(
                    "Invalid message type '{}'",
                    json_type_name(m)
                )));
            };
            match render_str(text, &task.render) {
                Ok(rendered) => task.log.line(level, &rendered).await,
                Err(e) => return Ok(Outcome::failed_with(e.to_string())),
            }
        }

        // `return null` (`Log.java:109`) — no `Output` object at all.
        Ok(Outcome::success())
    }

    /// Nothing to cancel: this handler renders strings and writes them, and
    /// dropping the future stops all of it.
    ///
    /// **`Ok(())` is now an answer somebody hears** — W4 Task 3 wired
    /// [`cancel`](super::super::cancel) to the budget's expiry — and it is
    /// still the truthful one: there is no remote statement, no pod and no
    /// handle outliving the drop, so "stopped" and "was already stopped" are
    /// the same fact here.
    async fn cancel(&self, _task: &TaskInput) -> egret_core::Result<()> {
        Ok(())
    }
}

fn json_type_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::recorded;
    use super::*;
    use crate::executor::RunState;
    use crate::logs::fixture;

    /// The shape: a `Log` produces **no output**, and what it *does* produce
    /// is a line at the right level. Both halves are asserted — the first
    /// alone would pass against a handler that logged nothing.
    #[tokio::test]
    async fn a_log_writes_its_message_and_produces_no_output() {
        let (task, sink) = recorded(TYPE_ID, serde_json::json!({"message": "hi"}));
        let out = Log.run(&task).await.unwrap();
        assert_eq!(out.state, RunState::Success);
        assert_eq!(
            out.outputs, None,
            "`Log` returns Java null (Log.java:109) and `VoidOutput` is never \
             instantiated — there is no output object"
        );
        assert_eq!(
            sink.texts(),
            vec![(LogLevel::Info, "hi".to_string())],
            "one line, at the default level INFO (Log.java:85-86)"
        );
        assert_eq!(sink.lines()[0].attempt, Some(0), "attempt 1 is stored as 0");
    }

    /// `message:` is `Object` and a list is the second declared shape
    /// (`oneOf = {String.class, String[].class}`): every element is rendered
    /// and logged in order. Measured on the artifact: SUCCESS.
    #[tokio::test]
    async fn a_list_message_logs_every_element_in_order() {
        let (task, sink) = recorded(
            TYPE_ID,
            serde_json::json!({"message": ["one", "two"], "level": "WARN"}),
        );
        let out = Log.run(&task).await.unwrap();
        assert_eq!(out.state, RunState::Success);
        assert_eq!(
            sink.texts(),
            vec![
                (LogLevel::Warn, "one".to_string()),
                (LogLevel::Warn, "two".to_string())
            ]
        );
    }

    /// Task 1's `log_task_cases`, typed: what a `Log` stores, per declared
    /// level, and nothing else (no framing lines).
    #[tokio::test]
    async fn the_log_task_cases_store_what_task_1_measured() {
        let e = fixture::expected("no_worker_framing");
        assert_eq!(e["framing_line_count"], 0);
        let cases = e["log_task_cases"].as_array().unwrap();
        assert!(!cases.is_empty());
        for case in cases {
            let mut spec = serde_json::json!({"message": case["message"]});
            if !case["level_declared"].is_null() {
                spec["level"] = case["level_declared"].clone();
            } else {
                assert_eq!(
                    fixture::level(&fixture::w11()["level_table"]["log_task_default"]),
                    LogLevel::Info
                );
            }
            let (task, sink) = recorded(TYPE_ID, spec);
            let out = Log.run(&task).await.unwrap();
            assert_eq!(out.state, RunState::Success, "{case}");
            let want: Vec<(LogLevel, String)> = case["stored"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| (fixture::level(&p[0]), p[1].as_str().unwrap().to_string()))
                .collect();
            assert_eq!(sink.texts(), want, "{case}");
        }
    }

    /// `Log.java:106` throws `IllegalArgumentException` for anything that is
    /// neither a String nor a Collection. Measured: `message: 42` gives
    /// *"Invalid message type 'class java.lang.Integer'"* and the run FAILS.
    #[tokio::test]
    async fn a_message_that_is_not_a_string_fails_the_run() {
        let (task, sink) = recorded(TYPE_ID, serde_json::json!({"message": 42}));
        let out = Log.run(&task).await.unwrap();
        assert_eq!(out.state, RunState::Failed);
        assert!(
            out.message.as_deref().unwrap_or("").contains("number"),
            "the failure names the type it refused, got {:?}",
            out.message
        );
        assert!(sink.lines().is_empty(), "nothing is logged when it refuses");
    }

    /// Measured: `level: SHOUT` fails the run rather than falling back to
    /// INFO, because Jackson compares the enum name exactly.
    #[tokio::test]
    async fn a_level_outside_the_five_fails_the_run() {
        let (task, sink) = recorded(
            TYPE_ID,
            serde_json::json!({"message": "hi", "level": "SHOUT"}),
        );
        let out = Log.run(&task).await.unwrap();
        assert_eq!(out.state, RunState::Failed);
        assert!(sink.lines().is_empty());
    }
}
