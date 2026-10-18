/// What kind of egret object a task field names.
///
/// Moved to `egret-workflow-api` in Task 6 (publish), so the port trait that
/// names it (`WorkflowRefPort::workspace_of`) can live in that leaf crate
/// without pulling this crate's engine dependencies along with it.
/// Re-exported here under its old path so nothing importing
/// `spec::registry::RefKind` had to change.
pub use egret_workflow_api::RefKind;

#[derive(Debug, Clone, Copy)]
pub struct RefField {
    pub field: &'static str,
    pub kind: RefKind,
}

#[derive(Debug, Clone, Copy)]
pub struct TaskTypeSpec {
    /// **The canonical spelling, and for a ported Kestra task that is the
    /// FQCN** — programme spec section 8 rule 4: *"`type:` accepts Kestra's
    /// full FQCN as the canonical form (`io.kestra.plugin.core.log.Log`);
    /// short aliases are secondary. A Kestra flow pasted into egret runs, and
    /// looking a task up in Kestra's docs lands on the right page."*
    pub r#type: &'static str,
    /// Other spellings [`known_type`] accepts for the same task. Secondary by
    /// the rule above, and **the single source of truth for the alias**: the
    /// worker registry reads this list rather than repeating it, so a spelling
    /// publish accepts for a ported task is a spelling the worker dispatches by
    /// construction (`worker::Handlers::add_publishable`).
    pub aliases: &'static [&'static str],
    pub references: &'static [RefField],
}

/// Every task type egret knows, and which of its fields name egret objects.
///
/// This table is the declaration half of a seam: W3's handlers are the
/// implementation half and must stay in step with it. Publish refuses a type
/// absent from here, so the table is also what makes an unknown type
/// unpublishable rather than silently unresolvable (spec section 4.2, 8).
///
/// # "Must stay in step with it" is now held by construction, not by memory
///
/// The sentence above stood for four packages while the two halves were
/// **disjoint** (see the comment on the four core rows below). It is no longer
/// a request: [`worker::Handlers::add_publishable`](crate::worker::Handlers::add_publishable)
/// registers a handler under this table's canonical spelling **and** its
/// aliases, and **panics if the handler's `type_id()` has no row here at all**
/// — so a handler that is not publishable cannot reach
/// [`with_core_tasks`](crate::worker::Handlers::with_core_tasks), and
/// `egret-bin` will not start. An alias added here is dispatched without
/// anyone remembering to add it twice.
///
/// **The converse does not hold, is not meant to, and does NOT fail the way its
/// mirror did.** A row here with no handler is a type egret publishes and does
/// not run: `core.http`, `core.slack`, and every `egret.*` task a given
/// deployment does not wire in.
///
/// It would be easy to read that as "the same defect facing the other way".
/// The missing-row case was a **400 at publish** — loud, immediate, and about a
/// flow nobody had started. **This case was a hang until the no-silent-stall
/// package**: `claim` took the run and wrote its lease, `Registry::run`
/// returned `WorkerError::NoHandler`, the error propagated out of `run_one`
/// and `tick`, the run kept its lease, the reaper cleared it
/// `working_timeout` later, and the next tick refused it again, for ever.
///
/// **It is now refused before it starts and failed if it slips through.**
/// The set of types this deployment serves is installed at start-up
/// (`runtime::served_for`, from the same configuration that builds the
/// handlers); `spec::unrunnable` names each task outside it
/// (`task_type_not_served`), and publish and every start refuse it
/// (no-silent-stall Tasks 2–3). An execution that reaches the worker anyway
/// -- started on an older binary, or under a configuration that served the
/// type -- has its run FAILED by `worker::run_one` with a line saying the
/// deployment, not the flow, is at fault (Task 4, spec §5), and settles.
///
/// **Not guarding it HERE is still the right call**, and the reason is the panic in
/// [`add_publishable`](crate::worker::Handlers::add_publishable): the same
/// mechanism facing this way would refuse to start any deployment that declines
/// to wire `egret.sql`, `core.http` or the container tasks — which is every
/// deployment, since `with_core_tasks` is eleven handlers and this table is
/// twenty-four rows (5 `egret.*`, 2 short `core.*`, 17 Kestra FQCNs -- W5a replaced
/// the `egret.container`/`egret.script.python` rows with four script-task
/// FQCNs that carry them as aliases, W9a Task 11 added the four core
/// storage tasks, two of them in `with_core_tasks`, and Task 12 the five
/// whole-file ones, all in it). *(2026-10-17, kill-and-exit Task 3 fix round 1:
/// those counts were already stale; counted then, the table is 34 rows, the
/// last added being `io.kestra.plugin.core.execution.Exit`, a row with no
/// handler on purpose -- see [`EXECUTOR_LEVEL`].)* The
/// asymmetry is deliberate, `main.rs` documents the shipping set, and what a
/// flow using an unwired type gets is the per-deployment refusal above, not a
/// startup panic.
pub const TASK_TYPES: &[TaskTypeSpec] = &[
    TaskTypeSpec {
        r#type: "egret.sql",
        aliases: &[],
        references: &[RefField {
            field: "database",
            kind: RefKind::Database,
        }],
    },
    // ── W5a: Kestra's four script tasks (`crate::scripts`) ─────────────────
    //
    // FQCN canonical (rule 4). W4's `egret.container` and `egret.script.python`
    // had rows of their own until W5a; they are now ALIASES of `shell.Commands`
    // and `python.Script` (W5a spec §3), so a flow saved with them still
    // publishes -- `scripts::parse` accepts W4's `image` under those two
    // spellings. No `RefField`: a script and an image name are not egret
    // objects.
    //
    // HISTORICAL (W4 Task 5 fix round 1): `egret.script.python` took a
    // different name from the container-backed `script:` task rather than
    // colliding with a row named `egret.python` one line above this comment
    // used to be -- that row was the notebook-kernel task (a required
    // `notebook` `RefField`), removed in Data Apps P6 along with the rest of
    // the notebook. The name `egret.script.python` stays as it is; renaming
    // it back would be churn for a collision that can no longer happen.
    TaskTypeSpec {
        r#type: crate::scripts::SHELL_COMMANDS,
        aliases: &[crate::scripts::ALIAS_CONTAINER],
        references: &[],
    },
    TaskTypeSpec {
        r#type: crate::scripts::SHELL_SCRIPT,
        aliases: &[],
        references: &[],
    },
    TaskTypeSpec {
        r#type: crate::scripts::PYTHON_COMMANDS,
        aliases: &[],
        references: &[],
    },
    TaskTypeSpec {
        r#type: crate::scripts::PYTHON_SCRIPT,
        aliases: &[crate::scripts::ALIAS_SCRIPT_PYTHON],
        references: &[],
    },
    TaskTypeSpec {
        r#type: "egret.dataapi.call",
        aliases: &[],
        references: &[RefField {
            field: "endpoint",
            kind: RefKind::DataApiEndpoint,
        }],
    },
    TaskTypeSpec {
        r#type: "egret.dataset.refresh",
        aliases: &[],
        references: &[RefField {
            field: "dataset",
            kind: RefKind::Dataset,
        }],
    },
    TaskTypeSpec {
        r#type: "egret.report.send",
        aliases: &[],
        references: &[RefField {
            field: "chart",
            kind: RefKind::Chart,
        }],
    },
    TaskTypeSpec {
        r#type: "core.http",
        aliases: &[],
        references: &[],
    },
    TaskTypeSpec {
        r#type: "core.slack",
        aliases: &[],
        references: &[],
    },
    // ── The four tasks `worker::Handlers::with_core_tasks` implements ────
    //
    // **FQCN canonical, short form as an alias, per programme spec section 8
    // rule 4.** Until W10 Task 6 fix round 1 this table held `core.log` and
    // nothing else of the four, while the handlers registered Kestra's FQCNs:
    // the intersection was EMPTY, so a flow using `core.log` published and then
    // found no handler, a flow using the FQCN ran and could not be published,
    // and **no Kestra flow pasted into egret could be published at all** —
    // measured as `each of the four handler TYPE_IDs appears in this table
    // exactly zero times`. That is a violation of a decided rule, not an open
    // question, and this table was the side that was wrong.
    //
    // `core.log` is kept as an alias because it is a spelling this table has
    // published since W1. **The other three get no short alias**, because they
    // have never had one and inventing three spellings is a decision nobody
    // has made: `core.log` maps to the PACKAGE segment (`…core.log.Log`), which
    // does not generalise to `…core.debug.Return` without picking between
    // `core.debug` and `core.return`.
    TaskTypeSpec {
        r#type: "io.kestra.plugin.core.log.Log",
        aliases: &["core.log"],
        references: &[],
    },
    TaskTypeSpec {
        r#type: "io.kestra.plugin.core.debug.Return",
        aliases: &[],
        references: &[],
    },
    TaskTypeSpec {
        r#type: "io.kestra.plugin.core.execution.Fail",
        aliases: &[],
        references: &[],
    },
    // Kill-and-exit Task 3 (spec §6). **A row with no worker handler on
    // purpose**: the executor runs it (`executor::TaskKind::Exit`), so it is
    // in [`EXECUTOR_LEVEL`] and every deployment serves it.
    TaskTypeSpec {
        r#type: crate::executor::exit::TYPE_ID,
        aliases: &[],
        references: &[],
    },
    // Group C1 Task 2 (`worker::handlers::output_values`): FQCN only, no
    // alias, and no reference -- it renders `values:` and nothing else.
    TaskTypeSpec {
        r#type: crate::worker::handlers::output_values::TYPE_ID,
        aliases: &[],
        references: &[],
    },
    TaskTypeSpec {
        r#type: "io.kestra.plugin.core.flow.Sleep",
        aliases: &[],
        references: &[],
    },
    // ── W9a Task 11: the core storage tasks (`worker::handlers::storage`) ──
    //
    // FQCN only, no short alias, for the reason given above for the three
    // that have none. `Write` and `Size` are in `with_core_tasks`; `Delete`
    // and `PurgeCurrentExecutionFiles` need a pool and are registered by
    // `handlers::storage::register`, which `runtime::handlers_for` calls.
    TaskTypeSpec {
        r#type: crate::worker::handlers::storage::WRITE,
        aliases: &[],
        references: &[],
    },
    TaskTypeSpec {
        r#type: crate::worker::handlers::storage::SIZE,
        aliases: &[],
        references: &[],
    },
    TaskTypeSpec {
        r#type: crate::worker::handlers::storage::DELETE,
        aliases: &[],
        references: &[],
    },
    TaskTypeSpec {
        r#type: crate::worker::handlers::storage::PURGE_CURRENT_EXECUTION_FILES,
        aliases: &[],
        references: &[],
    },
    // ── W9a Task 12: the whole-file core storage tasks, all five in
    // `with_core_tasks` (they need the store and no pool). ──
    TaskTypeSpec {
        r#type: crate::worker::handlers::storage::CONCAT,
        aliases: &[],
        references: &[],
    },
    TaskTypeSpec {
        r#type: crate::worker::handlers::storage::SPLIT,
        aliases: &[],
        references: &[],
    },
    TaskTypeSpec {
        r#type: crate::worker::handlers::storage::REVERSE,
        aliases: &[],
        references: &[],
    },
    TaskTypeSpec {
        r#type: crate::worker::handlers::storage::DEDUPLICATE_ITEMS,
        aliases: &[],
        references: &[],
    },
    TaskTypeSpec {
        r#type: crate::worker::handlers::storage::FILTER_ITEMS,
        aliases: &[],
        references: &[],
    },
    // ── W9b Task 6: the core namespace-file tasks (`worker::handlers::namespace`),
    // FQCN only, as the storage rows above. They need the pool and are
    // registered by `handlers::namespace::register`, which
    // `runtime::handlers_for` calls. ──
    TaskTypeSpec {
        r#type: crate::worker::handlers::namespace::UPLOAD_FILES,
        aliases: &[],
        references: &[],
    },
    TaskTypeSpec {
        r#type: crate::worker::handlers::namespace::DOWNLOAD_FILES,
        aliases: &[],
        references: &[],
    },
    // ── W9b Task 7: `DeleteFiles` and `PurgeFiles`, registered by the same
    // `handlers::namespace::register`. ──
    TaskTypeSpec {
        r#type: crate::worker::handlers::namespace::DELETE_FILES,
        aliases: &[],
        references: &[],
    },
    TaskTypeSpec {
        r#type: crate::worker::handlers::namespace::PURGE_FILES,
        aliases: &[],
        references: &[],
    },
    // ── W9c Task 5: the core KV tasks (`worker::handlers::kv`), FQCN only,
    // as the namespace rows above. They need the pool and are registered by
    // `handlers::kv::register`, which `runtime::handlers_for` calls. ──
    TaskTypeSpec {
        r#type: crate::worker::handlers::kv::SET,
        aliases: &[],
        references: &[],
    },
    TaskTypeSpec {
        r#type: crate::worker::handlers::kv::GET,
        aliases: &[],
        references: &[],
    },
    TaskTypeSpec {
        r#type: crate::worker::handlers::kv::GET_KEYS,
        aliases: &[],
        references: &[],
    },
    TaskTypeSpec {
        r#type: crate::worker::handlers::kv::DELETE,
        aliases: &[],
        references: &[],
    },
    TaskTypeSpec {
        r#type: crate::worker::handlers::kv::PUT,
        aliases: &[],
        references: &[],
    },
    TaskTypeSpec {
        r#type: crate::worker::handlers::kv::PURGE_KV,
        aliases: &[],
        references: &[],
    },
];

/// The registered types `advance()` runs itself, with no worker handler:
/// `build_plan` plans them as executor-level kinds, so no run of one is ever
/// claimed, and every deployment serves them (`runtime::Served::serves`).
pub const EXECUTOR_LEVEL: &[&str] = &[crate::executor::exit::TYPE_ID];

/// Whether `t` is one of [`EXECUTOR_LEVEL`].
pub fn executor_level(t: &str) -> bool {
    EXECUTOR_LEVEL.contains(&t)
}

/// The row for `t`, matching the canonical spelling **or any alias**.
///
/// The alias arm is why `core.log` and `io.kestra.plugin.core.log.Log` are one
/// task to publish rather than two: see [`TaskTypeSpec::aliases`].
pub fn known_type(t: &str) -> Option<&'static TaskTypeSpec> {
    TASK_TYPES
        .iter()
        .find(|s| s.r#type == t || s.aliases.contains(&t))
}
