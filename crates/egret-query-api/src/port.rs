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
    ///
    /// # Scoping — an implementation MUST honour this
    ///
    /// `database_id` typically arrives from a request body, so it is
    /// attacker-controlled. Every implementation **must** verify that
    /// `database_id` belongs to `ws` before connecting, and report a
    /// non-belonging or absent id as [`egret_core::Error::NotFound`] — absence
    /// and denial look identical, so the call cannot be used to probe which
    /// connections exist in other workspaces. `ws` is a scoping argument, not a
    /// hint. The port is fail-closed on this by design: a caller that forgets its
    /// own check is still safe.
    ///
    /// The *permission* half is the caller's: the port has no `WorkspaceContext`
    /// and does not evaluate `query:run`.
    ///
    /// # Errors
    ///
    /// The distinction matters to the endpoint author and must be preserved:
    ///
    /// * [`Error::Forbidden`] — the caller's policy denies the table. This is the
    ///   reason `principal` exists; never flatten it into `InvalidInput`.
    /// * [`Error::NotFound`] — the database is not in `ws`, or the relation the
    ///   SQL names does not exist.
    /// * [`Error::InvalidInput`] — the SQL is the problem, with the engine's
    ///   message attached so the author can fix it.
    /// * [`Error::Internal`] — infrastructure. Never carries engine addresses to
    ///   the client.
    ///
    /// [`Error::Forbidden`]: egret_core::Error::Forbidden
    /// [`Error::NotFound`]: egret_core::Error::NotFound
    /// [`Error::InvalidInput`]: egret_core::Error::InvalidInput
    /// [`Error::Internal`]: egret_core::Error::Internal
    async fn describe_as(
        &self,
        ws: Uuid,
        database_id: Uuid,
        principal: egret_core::UserId,
        sql: &str,
    ) -> Result<Vec<ColumnMeta>, egret_core::Error>;
}
