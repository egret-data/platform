//! `QueryExecPort` — the only thing the Data API seam knows about query
//! execution.
//!
//! `egret-dataapi` holds an `Arc<dyn QueryExecPort>` and never depends on
//! `egret-query`, so the Data API can move to its own binary by swapping the
//! implementation. This mirrors `NotebookDocsPort` in `egret-analytics-api`.
//!
//! Deliberately minimal: it carries `describe_as` and nothing else, because
//! that is all P3 has a caller for. `execute_as` (and the row-rendering surface
//! beside it) arrives in P4 with the serving path that calls it — a trait
//! method with no consumer is untested surface, so it is not here yet. Its
//! absence is a decision, not an oversight.

use uuid::Uuid;

use crate::ColumnMeta;

#[async_trait::async_trait]
pub trait QueryExecPort: Send + Sync {
    /// Resolve the output column schema of `sql` without fetching rows, running
    /// as `principal` so a delegated connection resolves under that identity's
    /// Ranger policy rather than the service account's.
    ///
    /// Used by validate-before-publish to freeze the output shape into the
    /// snapshot, so the published contract cannot drift when the underlying
    /// table changes.
    async fn describe_as(
        &self,
        ws: Uuid,
        database_id: Uuid,
        principal: egret_core::UserId,
        sql: &str,
    ) -> Result<Vec<ColumnMeta>, egret_core::Error>;
}
