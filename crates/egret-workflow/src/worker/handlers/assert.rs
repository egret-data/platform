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
/// [`Property`]. `errorMessage` is a real `Property<String>`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Spec {
    conditions: Option<Vec<String>>,
    #[serde(rename = "errorMessage")]
    error_message: Option<Property<String>>,
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
