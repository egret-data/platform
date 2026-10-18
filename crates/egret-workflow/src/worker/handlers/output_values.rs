//! `core.output.OutputValues` (group C1 Task 2, spec §2.1): render a map and
//! return it as `{"values": …}`.

use serde::Deserialize;

use super::super::{Outcome, TaskHandler, TaskInput};
use crate::models::property::render_map;

/// `io.kestra.plugin.core.output.OutputValues`.
pub const TYPE_ID: &str = "io.kestra.plugin.core.output.OutputValues";

/// `OutputValues.java:66` -- `private Property<Map<String, Object>> values;`,
/// with no `@NotNull`. Read raw rather than as a [`Property`], because
/// [`render_map`] walks the map's own shape (a `Property` would have flattened
/// it to JSON text).
///
/// [`Property`]: crate::models::Property
/// [`render_map`]: crate::models::property::render_map
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Spec {
    values: Option<serde_json::Value>,
}

/// Render `values:` and return it as `{"values": …}`.
#[derive(Debug, Default)]
pub struct OutputValues;

#[async_trait::async_trait]
impl TaskHandler for OutputValues {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    async fn run(&self, task: &TaskInput) -> egret_core::Result<Outcome> {
        let spec: Spec = match serde_json::from_value(task.spec.clone()) {
            Ok(s) => s,
            Err(e) => return Ok(Outcome::failed_with(format!("{TYPE_ID}: {e}"))),
        };
        let values = match spec.values {
            // An absent `values:` (or `values: null`) is an empty map: Kestra's
            // `RunContextProperty.asMap` answers `Collections.emptyMap()` for a
            // null property (`RunContextProperty.java:146-149`).
            None => serde_json::Map::new(),
            Some(raw) => match render_map(&raw, "values", &task.render) {
                Ok(map) => map,
                Err(e) => return Ok(Outcome::failed_with(e)),
            },
        };
        Ok(Outcome::succeeded_with(
            serde_json::json!({ "values": serde_json::Value::Object(values) }),
        ))
    }

    /// Nothing to cancel: this handler renders a map and returns.
    async fn cancel(&self, _task: &TaskInput) -> egret_core::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::input;
    use super::*;
    use crate::executor::RunState;

    /// `Output.values` (`OutputValues.java:82`), the rendered map.
    #[tokio::test]
    async fn the_rendered_map_is_the_values_output() {
        let out = OutputValues
            .run(&input(
                TYPE_ID,
                serde_json::json!({"values": {"a": "x", "b": "{{ 1 + 1 }}"}}),
            ))
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Success);
        assert_eq!(
            out.outputs,
            Some(serde_json::json!({"values": {"a": "x", "b": "2"}}))
        );
        assert_eq!(out.message, None);
    }

    /// No `values:` succeeds with an empty map.
    #[tokio::test]
    async fn no_values_is_an_empty_map() {
        for spec in [serde_json::json!({}), serde_json::json!({"values": null})] {
            let out = OutputValues.run(&input(TYPE_ID, spec)).await.unwrap();
            assert_eq!(out.state, RunState::Success);
            assert_eq!(out.outputs, Some(serde_json::json!({"values": {}})));
        }
    }

    /// A value that cannot render fails the task, naming the key.
    #[tokio::test]
    async fn a_value_that_fails_to_render_fails_the_task() {
        let out = OutputValues
            .run(&input(
                TYPE_ID,
                serde_json::json!({"values": {"a": "{{ nope.nope }}"}}),
            ))
            .await
            .expect("a render failure is the task's, not the worker's");
        assert_eq!(out.state, RunState::Failed);
        assert!(out.outputs.is_none());
        let message = out.message.expect("the failure carries its cause");
        assert!(message.contains("values.a"), "{message}");
    }

    /// `values` that is not a map fails the task.
    #[tokio::test]
    async fn values_that_are_not_a_map_fail_the_task() {
        let out = OutputValues
            .run(&input(TYPE_ID, serde_json::json!({"values": [1, 2]})))
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Failed);
        assert!(out.outputs.is_none());
        assert!(out.message.unwrap().contains("`values`"));
    }
}
