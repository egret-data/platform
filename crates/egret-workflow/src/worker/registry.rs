//! `type_id` → handler dispatch.

use std::collections::HashMap;
use std::sync::Arc;

use super::{handlers, Outcome, Registry, TaskHandler, TaskInput, WorkerError};
use egret_core::Result;

/// The handlers this worker can run, keyed by the `type:` a flow writes.
///
/// # An unknown type is a refusal, not a state
///
/// [`Registry::run`] returns `Err(WorkerError::NoHandler)` for a type nothing
/// serves, which aborts the tick for that run and leaves the execution where
/// it was. Neither alternative is acceptable: a silent `SUCCESS` is the
/// over-acceptance shape the spec §7 forbids, and a `FAILED` would report the
/// author's flow as wrong when it is egret that is incomplete.
///
/// # The type-id spellings here and `spec::registry::TASK_TYPES` do not agree
///
/// Publish's table (`crate::spec::registry::TASK_TYPES`) names egret's own
/// short spellings — `core.log`, `egret.sql`, `core.http` — and its own doc
/// says *"W3's handlers are the implementation half and must stay in step with
/// it"*. They are not in step, and this is where that shows:
///
/// - the four registered below are Kestra's FQCNs, because that is what the
///   corpus writes and what `executor::build_plan` puts in
///   `TaskKind::Runnable::type_id` (verbatim from `Task::type`, `plan.rs:742`);
/// - `TASK_TYPES` holds `core.log` and **not** `core.debug.Return`,
///   `core.execution.Fail` or `core.flow.Sleep`, so a flow using three of the
///   four is unpublishable through egret's own publish path, while a flow
///   using `core.log` publishes and then finds no handler.
///
/// **This is reported rather than papered over**, because deciding whether
/// egret's public task vocabulary is Kestra's FQCN, a short alias, or both is
/// a spec-level choice and not a handler's to make. [`Handlers::register`]
/// takes an arbitrary key so that whichever answer wins is one line here.
#[derive(Default)]
pub struct Handlers {
    by_type: HashMap<String, Arc<dyn TaskHandler>>,
}

impl std::fmt::Debug for Handlers {
    /// The keys, not the handlers: a `dyn TaskHandler` has no `Debug` and
    /// requiring one would put a bound on every future handler for the sake of
    /// one line of diagnostics.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handlers")
            .field("type_ids", &self.type_ids())
            .finish()
    }
}

impl Handlers {
    /// An empty registry. Nothing is registered implicitly.
    pub fn new() -> Self {
        Self::default()
    }

    /// The four W3b implements, each under the FQCN a corpus flow writes.
    pub fn with_core_tasks() -> Self {
        let mut r = Self::new();
        r.add(Arc::new(handlers::log::Log::new()));
        r.add(Arc::new(handlers::ret::Return));
        r.add(Arc::new(handlers::fail::Fail::new()));
        r.add(Arc::new(handlers::sleep::Sleep::new()));
        r
    }

    /// Register a handler under its own [`TaskHandler::type_id`].
    pub fn add(&mut self, handler: Arc<dyn TaskHandler>) -> &mut Self {
        let key = handler.type_id().to_string();
        self.register(key, handler)
    }

    /// Register a handler under an arbitrary key — an alias, or a spelling the
    /// handler does not name itself.
    pub fn register(
        &mut self,
        type_id: impl Into<String>,
        handler: Arc<dyn TaskHandler>,
    ) -> &mut Self {
        self.by_type.insert(type_id.into(), handler);
        self
    }

    /// The handler for `type_id`, if any.
    pub fn get(&self, type_id: &str) -> Option<&Arc<dyn TaskHandler>> {
        self.by_type.get(type_id)
    }

    /// Every registered key, sorted. For a test or a diagnostic.
    pub fn type_ids(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self.by_type.keys().map(String::as_str).collect();
        out.sort_unstable();
        out
    }
}

#[async_trait::async_trait]
impl Registry for Handlers {
    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        match self.get(&task.type_id) {
            Some(h) => h.run(task).await,
            None => Err(egret_core::Error::Internal(anyhow::anyhow!(
                WorkerError::NoHandler(task.type_id.clone())
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::handlers::testing::input;
    use super::*;
    use crate::executor::RunState;

    /// The four are registered under the spellings `build_plan` produces, and
    /// the list is asserted whole rather than probed one at a time — a
    /// `contains` check passes just as well with a fifth entry nobody meant.
    #[test]
    fn the_core_registry_holds_exactly_the_four_kestra_type_ids() {
        assert_eq!(
            Handlers::with_core_tasks().type_ids(),
            vec![
                "io.kestra.plugin.core.debug.Return",
                "io.kestra.plugin.core.execution.Fail",
                "io.kestra.plugin.core.flow.Sleep",
                "io.kestra.plugin.core.log.Log",
            ]
        );
    }

    /// Dispatch reaches the right handler — asserted on the **output**, which
    /// only `Return` has, so this cannot pass by reaching a different one.
    #[tokio::test]
    async fn dispatch_reaches_the_handler_that_owns_the_type() {
        let reg = Handlers::with_core_tasks();
        let out = reg
            .run(&input(
                "io.kestra.plugin.core.debug.Return",
                serde_json::json!({"format": "hi"}),
            ))
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Success);
        assert_eq!(out.outputs, Some(serde_json::json!({"value": "hi"})));
    }

    /// An unknown type is a worker refusal, and the message names the type so
    /// the reader is not sent looking for a broken flow.
    #[tokio::test]
    async fn an_unknown_type_is_a_worker_error_naming_the_type() {
        let err = Handlers::with_core_tasks()
            .run(&input(
                "io.kestra.plugin.core.output.OutputValues",
                serde_json::json!({}),
            ))
            .await
            .expect_err("an unregistered type must not produce an Outcome");
        assert!(
            err.to_string()
                .contains("io.kestra.plugin.core.output.OutputValues"),
            "got {err}"
        );
    }
}
