//! `egret-workflow-api` — the Workflow seam's contract crate (W0).
//!
//! Mirrors `egret-dataapi-api` / `egret-query-api` / `egret-catalog-api`: a pure
//! leaf so any cross-seam consumer can depend on the Workflow wire vocabulary
//! cheaply.
//!
//! Empty at W0 by design. `WorkflowPort::workspace_of` — the "which workspace
//! owns this flow" question `egret-automation` will ask in order to alert on a
//! failed run — lands with that caller, not before it.
