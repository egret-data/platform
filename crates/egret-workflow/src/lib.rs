//! `egret-workflow` — the workflow engine.
//!
//! The module tree mirrors Kestra's Java packages on purpose, so a reader can
//! move between this crate and the Kestra checkout without translating names.
//! See `docs/en/specs/2026-09-01-workflow-programme-design.md` §7.
//!
//! Source reference: `data-portal/kestra` at `96893b589` (2.0.0-SNAPSHOT).
//!
//! Modules arrive with their package:
//!
//! - `models`    — W0 (this crate's first type) and W1
//! - `repo`      — W1 (`workflow.flows` + `workflow.flow_versions`) and W10
//!   Task 3 (`ExecutionRepo`, the READ layer over `workflow.executions` +
//!   `workflow.task_runs`; the writes stay in `worker::claim`, which holds the
//!   `FOR NO KEY UPDATE` this one deliberately does not)
//! - `refs`      — W1 Task 7 (`WorkflowRefs`, the real `WorkflowRefPort`)
//! - `routes`    — W1 Task 7 (the nine management routes, spec §10)
//! - `runners`   — W2 (expression rendering)
//! - `executor`  — W3 (`advance()`, the pure state machine)
//! - `worker`    — W3b (the claim loop and the reaper) and W4 (the handlers
//!   and the runner). W3b's half shipped with no production caller; **W10**
//!   is the package that calls it from `egret-bin` (spec §9)
//! - `sla`       — W8 Task 6 (`sla:` evaluation; `EXECUTION_ASSERTION` only)
//! - `trigger`   — W6 (the schedule trigger, backfill, the injected clock, and
//!   the flow trigger's window-free arm). W6 built the predicate as a pure
//!   function and no table, so nothing here fired in production until **W12**
//!   built `workflow.triggers` and the loop that claims it (`scheduler::fire_due`,
//!   spec §9) -- behind `scheduler_enabled`, default `false`
//! - `scheduler` — W12 (`workflow.triggers`, `sync`; the database half that
//!   `trigger` must not contain)
//! - `flow_trigger` — W13a (the outbox processor: the Flow trigger's database
//!   half, run from the scheduler ticker)
//! - `plugins`   — W5 (handler registry)
//! - `logs`      — W11 (`workflow.task_logs` and `LogRepo`)
//! - `storage`   — W9a (execution files: URIs, the index, the readable set)
//!
//! `models`, `runners`, and `executor` must never touch the database or the
//! network. That boundary holds all the semantic difficulty and is what makes
//! it testable without standing anything up (spec §6.5).

// `pub`, not `mod`: the ratchet that judges it (`tests/executor.rs`) is an
// integration test, exactly as `tests/oracle.rs` is, and an integration test
// sees only the public API.
pub mod executor;
/// W13a: `process_events`, the Flow-trigger outbox processor. `pub` because
/// `tests/flow_trigger_fire.rs` drives it directly.
pub mod flow_trigger;
/// W7c: the read-only topology graph of a flow (`flow_graph`), pure like
/// `executor`. `pub` because the routes (Task 2) and the corpus sweep use it.
pub mod graph;
/// W7f: execution-label merge and the backfill's label checks. Crate-private.
pub(crate) mod labels;
/// W11: `workflow.task_logs` -- the log vocabulary and `LogRepo`. `pub`
/// because `tests/task_logs.rs` drives the repository directly.
pub mod logs;
pub mod models;
/// W7d: what the namespace pages read beyond the repository -- the
/// dependencies graph built from flow drafts.
pub(crate) mod namespaces;
/// W7e: reading triggers -- a flow's and the workspace's, with their state
/// (spec §3.5, §3.6).
pub(crate) mod triggers;
/// §3.6's last-execution statement, `pub` so `tests/disabled_triggers.rs`
/// `EXPLAIN`s the text the routes run.
#[doc(hidden)]
pub use triggers::state::LAST_EXECUTION_SQL;
/// W5b: real Kestra plugins run by the plugin host (`plugin-host/`). Task 2
/// holds only the embedded catalog and its lockfile check.
pub mod plugins;
pub mod publish;
pub mod refs;
pub mod repo;
pub mod routes;
pub mod runners;
/// W10 Task 2: the two background tickers that give
/// [`worker::tick`](worker::tick) and [`worker::reap`](worker::reap) their
/// first production caller, and the per-statement concurrency argument the
/// `SKIP LOCKED` sentence does not cover. `pub` because `egret-bin` starts it
/// and `tests/runtime.rs` drives it.
pub mod runtime;
/// W12: `workflow.triggers` and the code that keeps it (the trigger loop's
/// I/O half; `trigger` stays pure). `pub` because `scheduler::sync` is the
/// call `FlowRepo`'s three writers that can change what fires make (`create`,
/// `update`, `soft_delete` -- `set_live` is exempt) and later W12 tasks drive
/// it from integration tests.
pub mod scheduler;
/// W5a: Kestra's script tasks (`io.kestra.plugin.scripts.{shell,python}.*`)
/// and their `taskRunner:` -- the model, the save-time refusals, and the ONE
/// governance gate (`scripts::gate::check`) publish and the handler call.
pub mod scripts;
/// KI-112: `type: SECRET` inputs, masked in the row and sealed beside it.
/// `pub` because `egret-bin` installs the cipher and runs the backfill.
pub mod secret_inputs;
/// W8 Task 6: `sla:` — `EXECUTION_ASSERTION` evaluated, `MAX_DURATION` refused
/// by name with the measurement. `pub` so `tests/executor.rs` (an integration
/// test) can drive the predicate, the same reason `executor` and `worker` are.
pub mod sla;
pub mod spec;
/// W9a: internal storage -- `kestra://` URIs, the file index, the ONE
/// readability decision (`storage::resolve_readable_many`) and the
/// process-wide `FileStore`. `pub` because `egret-bin` installs the store and
/// `tests/storage_index.rs` drives the index.
pub mod storage;
// W6 Tasks 5 and 6: the schedule trigger, its backfill, and the flow
// trigger's window-free arm. `pub` because the loop that drives the clock
// lives in `scheduler`, not here, and backfill's routes are
// `triggers::backfill_routes` (W7f) -- and
// because NOTHING in `oracle-exec/` can reach it, so its own tests are the
// whole instrument (see the module doc).
pub mod trigger;

/// The anonymous webhook surface (W6 Task 7). Mounted OUTSIDE `/api` and
/// outside the CSRF layer -- see the module doc.
pub mod webhook_routes;
// W3b Task 2: the claim loop and the reaper -- the FIRST real caller of
// `advance()`. `pub` for the same reason `executor` is: `tests/worker.rs` is an
// integration test and sees only the public API.
pub mod worker;

pub use publish::{publish, publish_gated, ScriptGate};
pub use refs::WorkflowRefs;
pub use repo::{
    CreateFlowInput, ExecutionDetail, ExecutionFilter, ExecutionLogStatus, ExecutionPage,
    ExecutionRepo, ExecutionRow, FlowRepo, FlowRow, FlowVersionRow, ListFilter, NamespaceRow, Page,
    TaskRunRow, UpdateFlowInput,
};
pub use routes::{management_router, openapi, WorkflowState};
pub use runners::{render_str, render_typed, rewrite, RenderContext, RenderError, RewriteError};
pub use runtime::RuntimeConfig;
pub use webhook_routes::{
    webhook_router, webhook_router_with_guard, WebhookGuard, WebhookResponse,
    MAX_WEBHOOK_BODY_BYTES, WEBHOOK_ATTEMPTS_PER_WINDOW, WEBHOOK_ATTEMPT_WINDOW,
    WEBHOOK_MAX_CONCURRENCY,
};
// Task 6: `RefKind` moved to `egret-workflow-api` so `WorkflowRefPort` (also
// re-exported here) can name it without this crate's engine dependencies.
// Re-exported at the crate root -- not just via `spec::registry::RefKind` --
// because `publish` and its port live at this level too.
pub use egret_workflow_api::{RefKind, WorkflowRefPort};
