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
//! - `repo`      — W1 (`workflow.flows` + `workflow.flow_versions`)
//! - `refs`      — W1 Task 7 (`WorkflowRefs`, the real `WorkflowRefPort`)
//! - `routes`    — W1 Task 7 (the nine management routes, spec §10)
//! - `runners`   — W2 (expression rendering)
//! - `executor`  — W3 (`advance()`, the pure state machine)
//! - `worker`    — W3b (the claim loop and the reaper) and W4 (the handlers
//!   and the runner). W3b's half shipped with no production caller; **W10**
//!   is the package that calls it from `egret-bin` (spec §9)
//! - `sla`       — W8 Task 6 (`sla:` evaluation; `EXECUTION_ASSERTION` only)
//! - `trigger`   — W6 (the schedule trigger, backfill, the injected clock, and
//!   the flow trigger's window-free arm)
//! - `plugins`   — W5 (handler registry)
//!
//! `models`, `runners`, and `executor` must never touch the database or the
//! network. That boundary holds all the semantic difficulty and is what makes
//! it testable without standing anything up (spec §6.5).

// `pub`, not `mod`: the ratchet that judges it (`tests/executor.rs`) is an
// integration test, exactly as `tests/oracle.rs` is, and an integration test
// sees only the public API.
pub mod executor;
pub mod models;
pub mod publish;
pub mod refs;
pub mod repo;
pub mod routes;
pub mod runners;
/// W8 Task 6: `sla:` — `EXECUTION_ASSERTION` evaluated, `MAX_DURATION` refused
/// by name with the measurement. `pub` so `tests/executor.rs` (an integration
/// test) can drive the predicate, the same reason `executor` and `worker` are.
pub mod sla;
pub mod spec;
// W6 Tasks 5 and 6: the schedule trigger, its backfill, and the flow
// trigger's window-free arm. `pub` because the routes
// that create a backfill and the loop that drives the clock both live outside
// this module -- and because NOTHING in `oracle-exec/` can reach it, so its own
// tests are the whole instrument (see the module doc).
pub mod trigger;

/// The anonymous webhook surface (W6 Task 7). Mounted OUTSIDE `/api` and
/// outside the CSRF layer -- see the module doc.
pub mod webhook_routes;
// W3b Task 2: the claim loop and the reaper -- the FIRST real caller of
// `advance()`. `pub` for the same reason `executor` is: `tests/worker.rs` is an
// integration test and sees only the public API.
pub mod worker;

pub use publish::publish;
pub use refs::WorkflowRefs;
pub use repo::{CreateFlowInput, FlowRepo, FlowRow, FlowVersionRow, ListFilter, UpdateFlowInput};
pub use routes::{management_router, openapi, WorkflowState};
pub use runners::{render_str, rewrite, RenderContext, RenderError, RewriteError};
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
