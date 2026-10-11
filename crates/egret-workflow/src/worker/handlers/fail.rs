//! `core.execution.Fail` — the one that does not always fail.

use serde::Deserialize;

use super::super::{Outcome, TaskHandler, TaskInput};
use crate::executor::is_truthy;
use crate::logs::LogLevel;
use crate::models::Property;

/// `io.kestra.plugin.core.execution.Fail`.
pub const TYPE_ID: &str = "io.kestra.plugin.core.execution.Fail";

/// `Fail.java:127` — `@Builder.Default private Property<String> errorMessage =
/// Property.ofValue("Task failure");`.
const DEFAULT_ERROR_MESSAGE: &str = "Task failure";

/// `Fail.java:135` — the literal a **conditional** failure throws. It is not
/// the `errorMessage`; that is logged first (`:134`) and then this is thrown.
const CONDITIONAL_MESSAGE: &str = "Fail on a condition";

/// `condition` is optional (`Fail.java:123`); `errorMessage` has a default
/// (`:127`).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Spec {
    condition: Option<Property<String>>,
    #[serde(rename = "errorMessage")]
    error_message: Option<Property<String>>,
}

/// Fail the run — unless a `condition:` is present and falsy.
///
/// **Its log lines, with `claim::run_one`'s** (W11 spec §4.4): the
/// unconditional path writes nothing itself and `run_one` writes the
/// failed-attempt line (`errorMessage`), which is Kestra's one ERROR line; the
/// conditional path writes `errorMessage` here and `run_one` then writes
/// `Fail on a condition`, which are Kestra's two. Adding a line to the
/// unconditional path would store the `errorMessage` twice.
#[derive(Debug, Default)]
pub struct Fail;

impl Fail {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl TaskHandler for Fail {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    async fn run(&self, task: &TaskInput) -> egret_core::Result<Outcome> {
        let spec: Spec = match serde_json::from_value(task.spec.clone()) {
            Ok(s) => s,
            Err(e) => return Ok(Outcome::failed_with(format!("{TYPE_ID}: {e}"))),
        };

        // `if (condition != null)` (`Fail.java:131`) tests the **field**, not
        // the rendered value, so a `condition: null` in the YAML takes the
        // no-condition branch and always fails. Measured on the artifact:
        // FAILED, with `Task failure`.
        let Some(condition) = spec.condition else {
            return Ok(Outcome::failed_with(
                match render_error_message(&spec.error_message, task) {
                    Ok(m) => m,
                    Err(e) => return Ok(Outcome::failed_with(e)),
                },
            ));
        };

        let rendered = match condition.render(&task.render) {
            Ok(v) => v,
            Err(e) => return Ok(Outcome::failed_with(e.to_string())),
        };

        // `TruthUtils.isTruthy` (`TruthUtils.java:11-12`) — the same predicate
        // `If`, `Switch` and `runIf:` use, and it is deliberately imported
        // from `executor::is_truthy` rather than re-spelled here. A second
        // copy of a four-string table is a table that can disagree with the
        // one it copies, and this programme has shipped that shape before.
        if !is_truthy(&rendered) {
            // `return null` (`:137`): the run **SUCCEEDS**, with no output.
            return Ok(Outcome::success());
        }

        let error_message = match render_error_message(&spec.error_message, task) {
            Ok(m) => m,
            Err(e) => return Ok(Outcome::failed_with(e)),
        };
        // `runContext.logger().error(...)` (`:134`) and then
        // `throw new Exception("Fail on a condition")` (`:135`). The two are a
        // pair: the author's message is logged and the exception carries the
        // literal, so a handler that failed with the author's message would be
        // reporting something Kestra never reports.
        task.log.line(LogLevel::Error, &error_message).await;
        Ok(Outcome::failed_with(CONDITIONAL_MESSAGE))
    }

    /// Nothing to cancel: this handler renders at most two strings, and
    /// dropping the future stops all of it. See
    /// [`log`](super::log)'s `cancel` for why `Ok(())` stays right now that the
    /// kill path calls it.
    async fn cancel(&self, _task: &TaskInput) -> egret_core::Result<()> {
        Ok(())
    }
}

fn render_error_message(p: &Option<Property<String>>, task: &TaskInput) -> Result<String, String> {
    match p {
        None => Ok(DEFAULT_ERROR_MESSAGE.to_string()),
        Some(p) => p.render(&task.render).map_err(|e| e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{input, recorded};
    use super::*;
    use crate::executor::RunState;

    /// **A `Fail` does not always fail.** `Fail.java:137` returns null for a
    /// falsy condition and the run SUCCEEDS — measured on the artifact for
    /// `"false"`, `"0"`, `"-0"` and `"  "`.
    #[tokio::test]
    async fn a_falsy_condition_succeeds_and_produces_no_output() {
        for falsy in ["false", "0", "-0", "", "  ", " false "] {
            let (task, sink) = recorded(TYPE_ID, serde_json::json!({"condition": falsy}));
            let out = Fail.run(&task).await.unwrap();
            assert_eq!(
                out.state,
                RunState::Success,
                "condition {falsy:?} is falsy, so the run succeeds"
            );
            assert_eq!(out.outputs, None, "a Fail is a VoidOutput task");
            assert_eq!(out.message, None);
            assert!(sink.lines().is_empty(), "nothing is logged on success");
        }
    }

    /// The truthy path: the `errorMessage` is **logged** (`:134`) and the
    /// failure carries the **literal** (`:135`). Nothing in `golden.json`
    /// records a failure message, so this test is the only instrument that
    /// tells the two throwing paths apart.
    #[tokio::test]
    async fn a_truthy_condition_logs_the_error_message_and_fails_with_the_literal() {
        let (task, sink) = recorded(
            TYPE_ID,
            serde_json::json!({"condition": "true", "errorMessage": "boom"}),
        );
        let out = Fail.run(&task).await.unwrap();
        assert_eq!(out.state, RunState::Failed);
        assert_eq!(
            out.message.as_deref(),
            Some("Fail on a condition"),
            "the thrown message is the literal, not the author's errorMessage"
        );
        assert_eq!(
            sink.texts(),
            vec![(LogLevel::Error, "boom".to_string())],
            "the author's errorMessage is logged at ERROR (Fail.java:134)"
        );
    }

    /// No `condition:` at all throws the **rendered `errorMessage`**
    /// (`:140`), defaulting to `Task failure` (`:127`) — a different message
    /// from the conditional path, and nothing is logged on the way.
    #[tokio::test]
    async fn no_condition_fails_with_the_error_message_and_logs_nothing() {
        let (task, sink) = recorded(TYPE_ID, serde_json::json!({}));
        let out = Fail.run(&task).await.unwrap();
        assert_eq!(out.state, RunState::Failed);
        assert_eq!(out.message.as_deref(), Some("Task failure"));
        assert!(
            sink.lines().is_empty(),
            "the unconditional path throws without logging (Fail.java:140)"
        );

        let out = Fail
            .run(&input(TYPE_ID, serde_json::json!({"errorMessage": "nope"})))
            .await
            .unwrap();
        assert_eq!(out.message.as_deref(), Some("nope"));
    }

    /// `condition: null` sets the Java field to null, so `condition != null`
    /// is false and the task takes the always-fail branch. Measured on the
    /// artifact: FAILED with `Task failure`, **not** SUCCESS.
    #[tokio::test]
    async fn an_explicitly_null_condition_is_an_absent_one_and_fails() {
        let out = Fail
            .run(&input(TYPE_ID, serde_json::json!({"condition": null})))
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Failed);
        assert_eq!(out.message.as_deref(), Some("Task failure"));
    }
}
