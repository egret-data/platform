//! `NotebookDocsPort` implementation over the concrete [`NotebookState`].
//!
//! This is the notebook-side of the E5c-1 seam: `egret-kernel` depends on
//! `Arc<dyn egret_analytics_api::NotebookDocsPort>`, and egret-bin hands it an
//! `Arc<NotebookState>` via this impl. Each method delegates to the concrete
//! store / `build_notebook_runtime`, mapping the seam's `NotebookApiError` down
//! to its inner `egret_core::Error` (the leaf's error type).
use egret_analytics_api::{
    CellOutput, EngineOption, NotebookDocsPort, NotebookRuntime, NotebookSummary,
};
use uuid::Uuid;

use crate::routes::NotebookState;
use crate::runtime::build_notebook_runtime;

#[async_trait::async_trait]
impl NotebookDocsPort for NotebookState {
    async fn get_summary(
        &self,
        id: Uuid,
    ) -> Result<Option<(NotebookSummary, Uuid)>, egret_core::Error> {
        self.store.get_summary(id).await.map_err(|e| e.0)
    }

    async fn list_engine_options(&self, ws: Uuid) -> Result<Vec<EngineOption>, egret_core::Error> {
        self.store.list_engine_options(ws).await.map_err(|e| e.0)
    }

    async fn record_output(
        &self,
        nb: Uuid,
        cid: Uuid,
        outputs: &[CellOutput],
        execution_count: Option<i32>,
        elapsed_ms: Option<i32>,
    ) -> Result<(), egret_core::Error> {
        self.store
            .record_output(nb, cid, outputs, execution_count, elapsed_ms)
            .await
            .map_err(|e| e.0)
    }

    async fn build_runtime(
        &self,
        ws: Uuid,
        user_id: egret_core::UserId,
    ) -> Result<NotebookRuntime, egret_core::Error> {
        build_notebook_runtime(self, ws, user_id)
            .await
            .map_err(|e| e.0)
    }
}
