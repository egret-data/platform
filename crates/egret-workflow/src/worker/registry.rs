//! `type_id` → handler dispatch.

use std::collections::HashMap;
use std::sync::Arc;

use super::commands::OrphanSweep;
use super::handlers::{LogSink, TracingSink};
use super::{handlers, Outcome, Registry, TaskHandler, TaskInput, WorkerError};
use crate::logs::LogLimits;
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
/// # The type-id spellings here and `spec::registry::TASK_TYPES` agree, and a
/// # single table is why
///
/// **They did not, for four packages.** This doc used to end by reporting the
/// disagreement and declining to settle it: the four handlers registered
/// Kestra's FQCNs, because that is what the corpus writes and what
/// `executor::build_plan` puts in `TaskKind::Runnable::type_id` (verbatim from
/// `Task::type`, `plan.rs:742`), while `TASK_TYPES` held `core.log` and none of
/// the four FQCNs. The intersection was **empty**, and W10 Task 6's end-to-end
/// test — the first thing in this repository to drive publish and the worker in
/// one process — is what made that have a consequence: `POST …/publish`
/// answered **400 `unknown type 'io.kestra.plugin.core.log.Log'`** for the only
/// flow the shipped worker can run, and **no Kestra flow pasted into egret
/// could be published at all**.
///
/// **It was never an open question.** Programme spec §8 item 4 had already
/// decided: *"`type:` accepts Kestra's full FQCN as the canonical form
/// (`io.kestra.plugin.core.log.Log`); short aliases are secondary. A Kestra
/// flow pasted into egret runs, and looking a task up in Kestra's docs lands on
/// the right page."* `TASK_TYPES` was the side that was wrong, and W10 Task 6
/// fix round 1 moved it: the four FQCNs are canonical rows there, `core.log` is
/// an **alias** on the `Log` row, and this registry reads that alias list
/// rather than repeating it.
///
/// **The agreement is held by construction in the direction that matters.**
/// [`Handlers::add_publishable`] takes the alias list from `TASK_TYPES` and
/// **panics if the handler's `type_id()` has no row there**, so a handler that
/// publish would refuse cannot reach [`Handlers::with_core_tasks`] and
/// `egret-bin` will not start. An incomplete edit fails loudly at startup and
/// in every test that builds the registry, rather than waiting for someone to
/// paste a Kestra flow.
///
/// [`Handlers::register`] still takes an arbitrary key, for a handler wired in
/// by a deployment under a spelling of its own; it is [`add_publishable`] that
/// carries the invariant.
///
/// W3b Task 6 added [`handlers::sql`], which serves `egret.sql` — the one type
/// where the two halves agreed all along, and the shape the other four now
/// have.
///
/// [`add_publishable`]: Handlers::add_publishable
#[derive(Default)]
pub struct Handlers {
    by_type: HashMap<String, Arc<dyn TaskHandler>>,
    /// Where each attempt's [`RunLog`](crate::logs::RunLog) writes, and its
    /// cap. `None` is `tracing` under the default limits.
    log: Option<(Arc<dyn LogSink>, LogLimits)>,
    /// What the reaper's tick sweeps for work left behind by ended runs
    /// (W5a): the configured container runner, or nothing.
    sweep: Option<Arc<dyn OrphanSweep>>,
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
    ///
    /// # `egret.sql` is in NEITHER registry, and that is the answer to Task 5's
    /// hand-off
    ///
    /// Task 5 left the question open: the ratchet's registry is
    /// `oracle-exec/driver.rs`'s `registry()` and deliberately not this one, so
    /// a fifth handler had to be added "to both or to neither". **Neither**, and
    /// it is structural rather than a preference: [`handlers::sql::Sql`] is
    /// constructed from an `Arc<dyn QueryExecPort>`, and neither constructor has
    /// one — this function takes no arguments, and the oracle's registry must
    /// never reach an engine at all, because `egret.sql` appears in **zero**
    /// corpus flows and a handler the corpus cannot call has nothing to
    /// contribute to a comparison.
    ///
    /// A caller that wants it wires it in one line beside these four:
    ///
    /// ```ignore
    /// let mut reg = Handlers::with_core_tasks(None);
    /// // `add_publishable`, not `add`: `egret.sql` HAS a `TASK_TYPES` row, and
    /// // this is the invariant documented above — a handler wired in by a
    /// // deployment is still a handler whose type publish has to accept.
    /// reg.add_publishable(std::sync::Arc::new(handlers::sql::Sql::new(port)));
    /// ```
    ///
    /// **This example said `reg.add(…)` until W10 Task 6 fix round 2**, which
    /// is the one handler deployments actually wire — so the snippet directly
    /// under the invariant's own paragraph was the one place showing how to
    /// bypass it.
    ///
    /// `oracle-exec/README.md`'s coverage statement says the same thing, so a
    /// green ratchet is not read as covering a handler it never ran.
    pub fn with_core_tasks(files: Option<Arc<crate::storage::FileStore>>) -> Self {
        let mut r = Self::new();
        // `add_publishable`, not `add`: each of these must be a type publish
        // accepts, and each of publish's alias spellings must dispatch here.
        // Before W10 Task 6 fix round 1 that was true of none of them.
        r.add_publishable(Arc::new(handlers::log::Log));
        r.add_publishable(Arc::new(handlers::ret::Return));
        r.add_publishable(Arc::new(handlers::fail::Fail));
        r.add_publishable(Arc::new(handlers::sleep::Sleep::new()));
        // W9a Task 11: the two core storage tasks that need no connection.
        // `Write` takes the store its CALLER passes (spec §4.6: the store is a
        // parameter, never read from a global here; fix round 1) -- `runtime`
        // passes the one `egret-bin` installed, a test its own or `None`.
        // `Delete` and `PurgeCurrentExecutionFiles` need the pool and are
        // added by `handlers::storage::register`.
        r.add_publishable(Arc::new(handlers::storage::Write::new(files.clone())));
        r.add_publishable(Arc::new(handlers::storage::Size));
        // W9a Task 12: the five whole-file storage tasks, over the same store.
        r.add_publishable(Arc::new(handlers::storage::Concat::new(files.clone())));
        r.add_publishable(Arc::new(handlers::storage::Split::new(files.clone())));
        r.add_publishable(Arc::new(handlers::storage::Reverse::new(files.clone())));
        r.add_publishable(Arc::new(handlers::storage::DeduplicateItems::new(
            files.clone(),
        )));
        r.add_publishable(Arc::new(handlers::storage::FilterItems::new(files)));
        r
    }

    /// The same registry, with every attempt's log going to `sink` under
    /// `limits`. W11 Task 4 passes the Postgres-backed sink here.
    pub fn with_log_sink(mut self, sink: Arc<dyn LogSink>, limits: LogLimits) -> Self {
        self.log = Some((sink, limits));
        self
    }

    /// The same registry, with the reaper's tick sweeping `sweep`'s orphans
    /// (`OrphanSweep`, W5a Task 4) -- set by `runtime::handlers_for` when a
    /// container runner is configured.
    pub fn with_orphan_sweep(mut self, sweep: Arc<dyn OrphanSweep>) -> Self {
        self.sweep = Some(sweep);
        self
    }

    /// The sink and limits [`Registry::log_sink`] answers with.
    pub fn log_sink(&self) -> (Arc<dyn LogSink>, LogLimits) {
        match &self.log {
            Some((sink, limits)) => (sink.clone(), *limits),
            None => (Arc::new(TracingSink), LogLimits::default()),
        }
    }

    /// Register `handler` under its own [`TaskHandler::type_id`] **and under
    /// every spelling `spec::registry::TASK_TYPES` accepts for its row** (the
    /// canonical one and every alias; the row is found by `type_id`, which may
    /// itself be an alias) — the one
    /// call that keeps publish's vocabulary and the worker's from drifting.
    ///
    /// # Panics
    ///
    /// If `type_id()` has **no row** in `TASK_TYPES`, i.e. publish would refuse
    /// a flow using this handler's own canonical spelling. That is a
    /// programming error in the pair of tables, not a runtime condition, and it
    /// is a panic rather than a `Result` deliberately: this runs at process
    /// start (`egret-bin` builds the registry before the server listens) and in
    /// every test that builds one, so an incomplete edit is loud immediately
    /// instead of surfacing as a 400 the first time somebody pastes a flow —
    /// which is exactly how the four-package divergence survived. A handler a
    /// deployment wants under a spelling of its own uses [`Self::register`].
    pub fn add_publishable(&mut self, handler: Arc<dyn TaskHandler>) -> &mut Self {
        let canonical = handler.type_id();
        let row = crate::spec::registry::known_type(canonical).unwrap_or_else(|| {
            panic!(
                "handler type_id `{canonical}` has no row in spec::registry::TASK_TYPES, \
                 so publish would refuse every flow that names it. Add the row (canonical \
                 spelling = the FQCN, per programme spec section 8 rule 4) or register the \
                 handler with `Handlers::register` if it is deliberately unpublishable."
            )
        });
        // The aliases come FROM the publish table, so a spelling publish
        // accepts for this task is a spelling this registry serves, without
        // anybody having to write it twice.
        for alias in row.aliases {
            self.register(*alias, handler.clone());
        }
        // W5a: the row's CANONICAL spelling too. Until W5a every handler's
        // `type_id()` was its row's canonical spelling, so `add` below covered
        // it; W4's `egret.container`/`egret.script.python` handlers now name
        // an ALIAS (their rows became `io.kestra.plugin.scripts.*`), and
        // without this line publish would accept the FQCN and this registry
        // would not serve it -- the NoHandler loop `spec::registry`'s doc
        // describes.
        self.register(row.r#type, handler.clone());
        self.add(handler)
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

    /// The same lookup as [`Registry::run`], and the same refusal on a miss.
    ///
    /// A miss cannot be `Ok(())`: the kill path would then report a task as
    /// stopped by a registry that has never heard of its type, which is
    /// [`WorkerError::NoHandler`]'s own argument applied to the other method.
    async fn cancel(&self, task: &TaskInput) -> Result<()> {
        match self.get(&task.type_id) {
            Some(h) => h.cancel(task).await,
            None => Err(egret_core::Error::Internal(anyhow::anyhow!(
                WorkerError::NoHandler(task.type_id.clone())
            ))),
        }
    }
    /// The same lookup; a type with no handler has nothing to free.
    async fn release(&self, task: &TaskInput) -> Result<()> {
        match self.get(&task.type_id) {
            Some(h) => h.release(task).await,
            None => Ok(()),
        }
    }
    fn log_sink(&self) -> (Arc<dyn LogSink>, LogLimits) {
        Handlers::log_sink(self)
    }
    fn orphan_sweep(&self) -> Option<Arc<dyn OrphanSweep>> {
        self.sweep.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::super::handlers::testing::input;
    use super::*;
    use crate::executor::RunState;

    /// The four are registered under the spellings `build_plan` produces —
    /// **plus the one alias publish accepts** — and the list is asserted whole
    /// rather than probed one at a time: a `contains` check passes just as well
    /// with a fifth entry nobody meant.
    ///
    /// `core.log` is here because `TASK_TYPES` names it an alias of
    /// `io.kestra.plugin.core.log.Log`, not because this function mentions it.
    /// Delete that alias and this list loses an entry without anybody editing
    /// `with_core_tasks`.
    ///
    /// W9a Task 11 adds `core.storage.Write` and `Size`, the two storage tasks
    /// that need no pool, and Task 12 the five whole-file storage tasks.
    #[test]
    fn the_core_registry_holds_the_eleven_kestra_type_ids_and_the_one_alias() {
        assert_eq!(
            Handlers::with_core_tasks(None).type_ids(),
            vec![
                "core.log",
                "io.kestra.plugin.core.debug.Return",
                "io.kestra.plugin.core.execution.Fail",
                "io.kestra.plugin.core.flow.Sleep",
                "io.kestra.plugin.core.log.Log",
                "io.kestra.plugin.core.storage.Concat",
                "io.kestra.plugin.core.storage.DeduplicateItems",
                "io.kestra.plugin.core.storage.FilterItems",
                "io.kestra.plugin.core.storage.Reverse",
                "io.kestra.plugin.core.storage.Size",
                "io.kestra.plugin.core.storage.Split",
                "io.kestra.plugin.core.storage.Write",
            ]
        );
    }

    /// **Every spelling the shipped registry dispatches is a spelling publish
    /// accepts** — the invariant that was false for four packages, asserted in
    /// the direction `add_publishable` enforces.
    ///
    /// Held by construction: the aliases come from `TASK_TYPES` and a canonical
    /// id with no row panics. This test is the *witness* rather than the
    /// mechanism, and it is worth having because a future
    /// `with_core_tasks` that went back to `add` would compile and pass
    /// everything else in this file.
    #[test]
    fn every_type_the_shipped_registry_serves_is_publishable() {
        let reg = Handlers::with_core_tasks(None);
        let ids = reg.type_ids();
        assert!(!ids.is_empty(), "the registry must not be empty");
        for id in &ids {
            assert!(
                crate::spec::registry::known_type(id).is_some(),
                "the worker dispatches `{id}` and publish refuses it: {ids:?}"
            );
        }
        // The control for the loop above: a spelling NOTHING serves is also a
        // spelling publish refuses, so the assertion is not vacuously true of
        // every string.
        assert!(crate::spec::registry::known_type("io.kestra.plugin.core.log.NotAThing").is_none());
    }

    /// **Both spellings of `Log` reach the same handler**, which is what
    /// "aliases are secondary" has to mean at run time: the FQCN a pasted
    /// Kestra flow writes, and the short form egret has published since W1.
    ///
    /// Asserted on dispatch rather than on the key list, because a key present
    /// in the map and a key that reaches the right handler are different
    /// claims.
    #[tokio::test]
    async fn both_spellings_of_log_dispatch_to_the_log_handler() {
        let reg = Handlers::with_core_tasks(None);
        for spelling in ["io.kestra.plugin.core.log.Log", "core.log"] {
            let out = reg
                .run(&input(spelling, serde_json::json!({"message": "hi"})))
                .await
                .unwrap_or_else(|e| panic!("`{spelling}` must dispatch: {e}"));
            assert_eq!(out.state, RunState::Success, "{spelling}");
            // `Log` has no output at all; `Return`'s `{"value": …}` is how a
            // wrong handler would show itself.
            assert_eq!(out.outputs, None, "{spelling} reached the wrong handler");
        }
    }

    /// W5a: a handler whose `type_id()` is an ALIAS (`egret.container`, W4's
    /// spelling, now an alias of `io.kestra.plugin.scripts.shell.Commands`)
    /// still passes `add_publishable`'s guard, and is registered under every
    /// spelling publish accepts for its row -- the canonical FQCN included.
    #[test]
    fn an_alias_typed_handler_is_served_under_every_spelling_of_its_row() {
        struct AliasTyped;
        #[async_trait::async_trait]
        impl TaskHandler for AliasTyped {
            fn type_id(&self) -> &'static str {
                "egret.container"
            }
            async fn run(&self, _: &TaskInput) -> Result<Outcome> {
                Ok(Outcome::success())
            }
            async fn cancel(&self, _: &TaskInput) -> Result<()> {
                Ok(())
            }
        }
        let mut reg = Handlers::default();
        reg.add_publishable(Arc::new(AliasTyped));
        assert_eq!(
            reg.type_ids(),
            vec!["egret.container", "io.kestra.plugin.scripts.shell.Commands"]
        );
        for id in reg.type_ids() {
            assert!(crate::spec::registry::known_type(id).is_some(), "{id}");
        }
    }

    /// Dispatch reaches the right handler — asserted on the **output**, which
    /// only `Return` has, so this cannot pass by reaching a different one.
    #[tokio::test]
    async fn dispatch_reaches_the_handler_that_owns_the_type() {
        let reg = Handlers::with_core_tasks(None);
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
        let err = Handlers::with_core_tasks(None)
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
