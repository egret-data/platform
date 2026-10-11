//! `core.debug.Return` — the one handler of the four that has an output.

use serde::Deserialize;

use super::super::{Outcome, TaskHandler, TaskInput};
use crate::models::Property;

/// `io.kestra.plugin.core.debug.Return`.
pub const TYPE_ID: &str = "io.kestra.plugin.core.debug.Return";

/// `Return.java:91` — `private Property<String> format;`, with **no**
/// `@NotNull`. An absent `format:` is legal and the run succeeds.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Spec {
    format: Option<Property<String>>,
}

/// Render `format:` and return it as `{"value": …}`.
#[derive(Debug, Default)]
pub struct Return;

#[async_trait::async_trait]
impl TaskHandler for Return {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    async fn run(&self, task: &TaskInput) -> egret_core::Result<Outcome> {
        let spec: Spec = match serde_json::from_value(task.spec.clone()) {
            Ok(s) => s,
            Err(e) => return Ok(Outcome::failed_with(format!("{TYPE_ID}: {e}"))),
        };
        let rendered = match spec.format {
            None => None,
            Some(format) => match format.render(&task.render) {
                Ok(v) => Some(v),
                Err(e) => return Ok(Outcome::failed_with(e.to_string())),
            },
        };
        Ok(Outcome::succeeded_with(match rendered {
            Some(value) => serde_json::json!({ "value": value }),
            // `Output.builder().value(null).build()` still builds an Output —
            // the field is simply dropped on the way out, which is why
            // `{{ outputs.a }}` is `{}` and `{{ outputs.a.value }}` is an
            // error. Measured on the artifact, both ways.
            None => serde_json::json!({}),
        }))
    }

    /// Nothing to cancel: this handler renders a string and returns, and
    /// dropping the future stops all of it. See
    /// [`log`](super::log)'s `cancel` for why `Ok(())` stays right now that the
    /// kill path calls it.
    async fn cancel(&self, _task: &TaskInput) -> egret_core::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::input;
    use super::*;
    use crate::executor::RunState;

    /// The shape, not the fact that it ran: `{"value": <rendered format>}`.
    ///
    /// This is the test the plan's Task 3 Step 4 mutation is aimed at — make
    /// `Return` produce no output and this is what must die.
    #[tokio::test]
    async fn a_return_produces_value_holding_the_rendered_format() {
        let out = Return
            .run(&input(TYPE_ID, serde_json::json!({"format": "hello"})))
            .await
            .expect("a Return is not a worker failure");
        assert_eq!(out.state, RunState::Success);
        assert_eq!(
            out.outputs,
            Some(serde_json::json!({"value": "hello"})),
            "`Return.Output` has exactly one field and it is named `value` \
             (Return.java:119), built from the rendered `format` (:108)"
        );
        assert_eq!(out.message, None);
    }

    /// `format` is rendered `.orElse(null)` (`Return.java:99`), so a `Return`
    /// with no `format:` **succeeds**. Measured on the artifact too: the
    /// execution is `SUCCESS` and `{{ outputs.<id> }}` renders `{}`.
    #[tokio::test]
    async fn a_return_with_no_format_succeeds_with_an_empty_output() {
        let out = Return
            .run(&input(TYPE_ID, serde_json::json!({})))
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Success);
        assert_eq!(
            out.outputs,
            Some(serde_json::json!({})),
            "a null `value` is dropped from the map, not carried as null: on \
             the artifact `{{{{ outputs.a.value }}}}` is `Unable to find \
             `value``, which it could not be if the key were present"
        );
    }

    /// `format: null` is the same as an absent one — measured: SUCCESS.
    #[tokio::test]
    async fn an_explicitly_null_format_is_an_absent_one() {
        let out = Return
            .run(&input(TYPE_ID, serde_json::json!({"format": null})))
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Success);
        assert_eq!(out.outputs, Some(serde_json::json!({})));
    }

    /// A property that cannot render is a **failed task**, not a worker fault:
    /// Kestra's render throws inside `run`, and a task whose `run` throws is
    /// `FAILED`. The empty render context is
    /// [`TaskInput::render`](crate::worker::TaskInput::render)'s hole.
    #[tokio::test]
    async fn an_unrenderable_format_fails_the_run_rather_than_the_worker() {
        let out = Return
            .run(&input(TYPE_ID, serde_json::json!({"format": "{{ nope }}"})))
            .await
            .expect("a render failure must not abort the tick");
        assert_eq!(out.state, RunState::Failed);
        assert!(out.outputs.is_none(), "a failed Return produces no output");
        assert!(
            out.message.is_some(),
            "the failure carries the render error"
        );
    }
}
