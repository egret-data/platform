//! The `NotebookDocsPort` reader/writer trait — the exact notebook-docs surface
//! `egret-kernel` needs from the Analytics seam.
//!
//! `egret-kernel` holds an `Arc<dyn NotebookDocsPort>` (never concrete
//! `egret-notebook`); `egret-notebook` implements this over its concrete
//! `NotebookState`, delegating to its `NotebookStore` + `build_notebook_runtime`.
//!
//! Methods return `egret_core::Error` directly (the seam's inner error), keeping
//! this leaf free of `egret-notebook`'s `NotebookApiError`.
use uuid::Uuid;

use crate::dto::{CellOutput, EngineOption, NotebookRuntime, NotebookSummary};

#[async_trait::async_trait]
pub trait NotebookDocsPort: Send + Sync {
    /// Fetch a notebook summary (no ws/ownership scoping) + its workspace id,
    /// so the caller can enforce the ws-scope + owner/public IDOR gate.
    async fn get_summary(
        &self,
        id: Uuid,
    ) -> Result<Option<(NotebookSummary, Uuid)>, egret_core::Error>;

    /// List all engine (DB connection) options for a workspace with enabled flag.
    async fn list_engine_options(&self, ws: Uuid) -> Result<Vec<EngineOption>, egret_core::Error>;

    /// Persist a cell's captured outputs (capped), execution count, elapsed ms.
    async fn record_output(
        &self,
        nb: Uuid,
        cid: Uuid,
        outputs: &[CellOutput],
        execution_count: Option<i32>,
        elapsed_ms: Option<i32>,
    ) -> Result<(), egret_core::Error>;

    /// Build the per-user kernel runtime manifest for `(ws, user_id)`.
    async fn build_runtime(
        &self,
        ws: Uuid,
        user_id: egret_core::UserId,
    ) -> Result<NotebookRuntime, egret_core::Error>;
}
