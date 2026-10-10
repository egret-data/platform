//! `egret-workflow-api` — the Workflow seam's contract crate (W0/W1).
//!
//! Mirrors `egret-dataapi-api` / `egret-query-api` / `egret-catalog-api`: a
//! pure leaf (no sqlx, axum, or heavy egret-* deps) so any cross-seam
//! consumer can depend on the Workflow vocabulary cheaply.
//!
//! `RefKind` and `WorkflowRefPort` land together in Task 6: `RefKind` moved
//! here from `egret-workflow`'s task-type registry (and is re-exported from
//! there so existing paths keep working) so the port trait -- which names
//! `RefKind` in its own signature -- can live in this leaf crate without
//! pulling `egret-workflow`'s engine dependencies along with it.

mod port;
pub use port::WorkflowRefPort;

/// What kind of egret object a task field names.
///
/// Moved here from `egret-workflow::spec::registry` (Task 6): `RefField`
/// still lives in that registry and still carries a `RefKind`, but the kind
/// itself has to be nameable from this leaf crate's `WorkflowRefPort`
/// trait, which cannot depend on `egret-workflow` (the engine) without
/// making this crate stop being a leaf.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RefKind {
    Database,
    Dataset,
    Chart,
    DataApiEndpoint,
    Subflow,
}
