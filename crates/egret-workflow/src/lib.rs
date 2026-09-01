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
//! - `runners`   — W2 (expression rendering)
//! - `executor`  — W3 (`advance()`, the pure state machine)
//! - `worker`    — W4 (claim loops, handlers, reaper)
//! - `plugins`   — W5 (handler registry)
//!
//! `models`, `runners`, and `executor` must never touch the database or the
//! network. That boundary holds all the semantic difficulty and is what makes
//! it testable without standing anything up (spec §6.5).

pub mod models;
pub mod publish;
pub mod repo;
pub mod spec;

pub use publish::publish;
pub use repo::{CreateFlowInput, FlowRepo, FlowRow, FlowVersionRow, ListFilter, UpdateFlowInput};
// Task 6: `RefKind` moved to `egret-workflow-api` so `WorkflowRefPort` (also
// re-exported here) can name it without this crate's engine dependencies.
// Re-exported at the crate root -- not just via `spec::registry::RefKind` --
// because `publish` and its port live at this level too.
pub use egret_workflow_api::{RefKind, WorkflowRefPort};
