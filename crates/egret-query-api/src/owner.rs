//! `DatabaseOwnerPort` — which workspace owns a registered database.
//!
//! The Workflow seam resolves an `egret.sql` task's `database:` at publish
//! (`egret_workflow::refs::WorkflowRefs`, `RefKind::Database`) and refuses a
//! reference another workspace owns. [`crate::QueryExecPort`] executes SQL; it
//! does not answer this question, so this is its own port, the same leaf-port
//! shape as `egret_dataapi_api::DataApiEndpointPort`: `egret-workflow` holds an
//! `Arc<dyn DatabaseOwnerPort>`, and `egret-query` implements it over its
//! `DatabaseRepo`.

use uuid::Uuid;

#[async_trait::async_trait]
pub trait DatabaseOwnerPort: Send + Sync {
    /// The workspace `id` belongs to, or `None` if no `query.databases` row
    /// has that id.
    ///
    /// Deliberately UNSCOPED by a caller-supplied workspace: the caller
    /// compares the answer with its own workspace and treats a mismatch
    /// exactly like `None`, so a foreign id and a fictional one are refused
    /// the same way.
    async fn workspace_of(&self, id: Uuid) -> Result<Option<Uuid>, egret_core::Error>;
}
