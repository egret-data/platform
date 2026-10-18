//! `core.execution.Assert` (group C1 Task 3, spec §2.2): render each condition,
//! and fail the run when any of them is falsy.

use serde::Deserialize;

use super::super::{Outcome, TaskHandler, TaskInput};
use crate::executor::is_truthy;
use crate::logs::LogLevel;
use crate::models::Property;
use crate::runners::render_str;

/// `io.kestra.plugin.core.execution.Assert`.
pub const TYPE_ID: &str = "io.kestra.plugin.core.execution.Assert";

/// `Assert.java:71-78`.
///
/// `conditions` is `@NotNull @PluginProperty(dynamic = true) List<String>`: a
/// raw list of templates, each rendered with [`render_str`], not a
/// [`Property`]. It is read as values and each entry turned to text by
/// [`condition_texts`], as Jackson coerces a scalar to `String`.
/// `errorMessage` is a real `Property<String>`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Spec {
    conditions: Option<Vec<serde_json::Value>>,
    #[serde(rename = "errorMessage")]
    error_message: Option<Property<String>>,
}

/// Each entry of `conditions` as the `String` Jackson's `StringDeserializer`
/// makes of it: a string as it is, a boolean or a number as its text
/// (`true`, `1`, `1.5`). A null, list or map entry is an error naming
/// `conditions[i]` (Jackson refuses a list or map; a null entry would reach
/// Kestra's renderer as `null` and fail there).
fn condition_texts(raw: &[serde_json::Value]) -> Result<Vec<String>, String> {
    use serde_json::Value;
    raw.iter()
        .enumerate()
        .map(|(i, v)| match v {
            Value::String(s) => Ok(s.clone()),
            Value::Bool(b) => Ok(b.to_string()),
            Value::Number(n) => Ok(n.to_string()),
            Value::Null => Err(format!("`conditions[{i}]`: must not be null")),
            Value::Array(_) => Err(format!("`conditions[{i}]`: expected a string, got a list")),
            Value::Object(_) => Err(format!("`conditions[{i}]`: expected a string, got a map")),
        })
        .collect()
}

/// Evaluate every condition; any falsy one fails the run.
#[derive(Debug, Default)]
pub struct Assert;

#[async_trait::async_trait]
impl TaskHandler for Assert {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    async fn run(&self, task: &TaskInput) -> egret_core::Result<Outcome> {
        let spec: Spec = match serde_json::from_value(task.spec.clone()) {
            Ok(s) => s,
            Err(e) => return Ok(Outcome::failed_with(format!("{TYPE_ID}: {e}"))),
        };
        // `@NotNull` (`Assert.java:75`), which Kestra enforces at publish;
        // egret's publish does not carry that rule, so the run fails here, as
        // `Log`'s absent `message` does.
        let Some(conditions) = spec.conditions else {
            return Ok(Outcome::failed_with("conditions: must not be null"));
        };
        let conditions = match condition_texts(&conditions) {
            Ok(c) => c,
            Err(e) => return Ok(Outcome::failed_with(e)),
        };

        // `Assert.java:82-101`: every condition is evaluated, each failure
        // logged as it happens. A render error counts as a failure.
        let (mut succeeded, mut failed) = (0usize, 0usize);
        for c in &conditions {
            match render_str(c, &task.render) {
                Ok(rendered) if is_truthy(&rendered) => succeeded += 1,
                Ok(_) => {
                    task.log
                        .line(LogLevel::Error, &format!("Assertion `{c}` failed!"))
                        .await;
                    failed += 1;
                }
                Err(e) => {
                    task.log
                        .line(
                            LogLevel::Error,
                            &format!("Assertion `{c}` failed, failed to render `{e}`"),
                        )
                        .await;
                    failed += 1;
                }
            }
        }

        // Kestra emits `success`/`failed` counters here (`:103-104`); egret
        // carries no task metrics (spec D1), so the counts are one INFO line.
        task.log
            .line(
                LogLevel::Info,
                &format!("{succeeded} succeeded, {failed} failed"),
            )
            .await;

        if failed == 0 {
            // `return null` (`:115`): a `VoidOutput`, no outputs.
            return Ok(Outcome::success());
        }

        // `:106-111`. `errorMessage` is rendered only when something failed,
        // and a render error replaces the assertion message.
        let suffix = match spec.error_message {
            None => String::new(),
            Some(p) => match p.render(&task.render) {
                Ok(m) => format!("\n{m}"),
                Err(e) => return Ok(Outcome::failed_with(e.to_string())),
            },
        };
        let plural = if failed > 1 { "s" } else { "" };
        Ok(Outcome::failed_with(format!(
            "{failed} assertion{plural} failed!{suffix}"
        )))
    }

    /// Nothing to cancel: this handler renders strings and returns.
    async fn cancel(&self, _task: &TaskInput) -> egret_core::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::recorded;
    use super::*;
    use crate::executor::RunState;

    async fn run(spec: serde_json::Value) -> (Outcome, Vec<(LogLevel, String)>) {
        let (task, rec) = recorded(TYPE_ID, spec);
        let out = Assert.run(&task).await.unwrap();
        (out, rec.texts())
    }

    #[tokio::test]
    async fn all_true_succeeds_with_no_outputs_and_one_count_line() {
        let (out, lines) = run(serde_json::json!({"conditions": ["{{ 1 == 1 }}", "yes"]})).await;
        assert_eq!(out.state, RunState::Success);
        assert_eq!(out.outputs, None);
        assert_eq!(out.message, None);
        assert_eq!(
            lines,
            vec![(LogLevel::Info, "2 succeeded, 0 failed".to_string())]
        );
    }

    #[tokio::test]
    async fn one_false_fails_with_the_singular_message() {
        let (out, lines) = run(serde_json::json!({"conditions": ["{{ 1 == 2 }}", "x"]})).await;
        assert_eq!(out.state, RunState::Failed);
        assert_eq!(out.message.as_deref(), Some("1 assertion failed!"));
        assert_eq!(
            lines,
            vec![
                (
                    LogLevel::Error,
                    "Assertion `{{ 1 == 2 }}` failed!".to_string()
                ),
                (LogLevel::Info, "1 succeeded, 1 failed".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn two_false_fail_with_the_plural_message_and_the_error_message() {
        let (out, lines) = run(serde_json::json!({
            "conditions": ["{{ 1 == 2 }}", "false"],
            "errorMessage": "because {{ 'x' }}",
        }))
        .await;
        assert_eq!(out.state, RunState::Failed);
        assert_eq!(
            out.message.as_deref(),
            Some("2 assertions failed!\nbecause x")
        );
        assert_eq!(
            lines.last(),
            Some(&(LogLevel::Info, "0 succeeded, 2 failed".to_string()))
        );
    }

    #[tokio::test]
    async fn a_condition_that_fails_to_render_counts_as_failed() {
        let (out, lines) = run(serde_json::json!({"conditions": ["{{ nope.nope }}"]})).await;
        assert_eq!(out.state, RunState::Failed);
        assert_eq!(out.message.as_deref(), Some("1 assertion failed!"));
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(lines[0].0, LogLevel::Error);
        assert!(
            lines[0]
                .1
                .starts_with("Assertion `{{ nope.nope }}` failed, failed to render `"),
            "{lines:?}"
        );
        assert_eq!(
            lines[1],
            (LogLevel::Info, "0 succeeded, 1 failed".to_string())
        );
    }

    /// `is_truthy`'s falsy set: `"-0"` and whitespace-only are falsy, and the
    /// comparison is exact, so `"False"` is truthy.
    #[tokio::test]
    async fn the_falsy_set_is_is_truthys() {
        let (out, _) = run(serde_json::json!({"conditions": ["-0", "  "]})).await;
        assert_eq!(out.message.as_deref(), Some("2 assertions failed!"));
        let (out, lines) = run(serde_json::json!({"conditions": ["False"]})).await;
        assert_eq!(out.state, RunState::Success);
        assert_eq!(
            lines,
            vec![(LogLevel::Info, "1 succeeded, 0 failed".to_string())]
        );
    }

    #[tokio::test]
    async fn an_error_message_that_fails_to_render_fails_with_that_error() {
        let (out, _) = run(serde_json::json!({
            "conditions": ["0"],
            "errorMessage": "{{ nope.nope }}",
        }))
        .await;
        assert_eq!(out.state, RunState::Failed);
        let render_error = render_str("{{ nope.nope }}", &crate::runners::RenderContext::new())
            .unwrap_err()
            .to_string();
        assert_eq!(out.message, Some(render_error));
    }

    /// Jackson coerces a scalar to the `String` of `List<String>`, so
    /// `conditions: [true, 1, 1.5]` reads as `["true", "1", "1.5"]`: all
    /// truthy.
    #[tokio::test]
    async fn scalar_conditions_are_read_as_their_text() {
        let (out, lines) = run(serde_json::json!({"conditions": [true, 1, 1.5]})).await;
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);
        assert_eq!(
            lines,
            vec![(LogLevel::Info, "3 succeeded, 0 failed".to_string())]
        );
        let (out, lines) = run(serde_json::json!({"conditions": [false, 0]})).await;
        assert_eq!(out.message.as_deref(), Some("2 assertions failed!"));
        assert_eq!(
            lines[..2],
            [
                (LogLevel::Error, "Assertion `false` failed!".to_string()),
                (LogLevel::Error, "Assertion `0` failed!".to_string()),
            ]
        );
    }

    /// A null, list or map entry is not a string: the run fails naming the
    /// entry, and nothing is evaluated.
    #[tokio::test]
    async fn a_null_list_or_map_condition_fails_naming_its_index() {
        for bad in [
            serde_json::json!(null),
            serde_json::json!([1]),
            serde_json::json!({"a": 1}),
        ] {
            let (out, lines) = run(serde_json::json!({"conditions": ["1", bad]})).await;
            assert_eq!(out.state, RunState::Failed, "{bad}");
            assert!(
                out.message
                    .as_deref()
                    .unwrap_or("")
                    .starts_with("`conditions[1]`"),
                "{bad}: {:?}",
                out.message
            );
            assert!(lines.is_empty(), "{bad}: {lines:?}");
        }
    }

    #[tokio::test]
    async fn no_conditions_fails_naming_conditions() {
        let (out, lines) = run(serde_json::json!({})).await;
        assert_eq!(out.state, RunState::Failed);
        assert!(
            out.message.as_deref().unwrap_or("").contains("conditions"),
            "{:?}",
            out.message
        );
        assert!(lines.is_empty(), "{lines:?}");
    }
}
