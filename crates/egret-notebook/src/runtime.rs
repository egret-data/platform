//! `GET /ws/{workspace}/notebook-runtime` — per-user kernel manifest (F1→D1).
//!
//! Returns the caller's stable username and the workspace's enabled
//! engines/catalogs so the Daft kernel can wire itself up in one round-trip.

use axum::extract::State;
use axum::Json;
use egret_workspace::context::WsContext;

use crate::error::NotebookApiError;
use crate::model::{CatalogOption, EngineOption};
use crate::routes::NotebookState;

/// Full kernel manifest returned by `GET /ws/{workspace}/notebook-runtime`.
#[derive(serde::Serialize)]
pub struct NotebookRuntime {
    /// The caller's stable username (email local-part, collision-suffixed).
    pub username: String,
    /// Workspace engines (DB connections) enabled for notebooks.
    pub engines: Vec<EngineOption>,
    /// Workspace catalogs (warehouses) enabled for notebooks.
    pub catalogs: Vec<CatalogOption>,
    /// Short-lived per-user engine JWT the kernel presents to query engines.
    /// `None` when the caller has no stored refresh token (e.g. password/local
    /// users or OIDC logins without an offline token) — minting is best-effort
    /// since no P1 engine consumes this yet.
    pub engine_jwt: Option<String>,
    /// Expiry of `engine_jwt` (RFC3339); kernels re-mint before this time.
    /// `None` whenever `engine_jwt` is `None`.
    pub engine_jwt_expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Build the [`NotebookRuntime`] manifest for `(ws, user_id)` without performing
/// any HTTP authorization check. Steps 2–6 of the `runtime` handler.
///
/// The HTTP handler is responsible for calling `require_workspace` before
/// invoking this function. This split allows `egret-kernel` (and future
/// callers) to reuse manifest construction without going through the HTTP
/// layer.
pub async fn build_notebook_runtime(
    st: &NotebookState,
    ws: uuid::Uuid,
    user_id: egret_core::UserId,
) -> Result<NotebookRuntime, NotebookApiError> {
    // 2. Resolve stable username.
    let username = st
        .users
        .ensure_username(user_id)
        .await
        .map_err(NotebookApiError)?;

    // 5. Load enabled engines and catalogs (filter to enabled==true).
    let engines: Vec<EngineOption> = st
        .store
        .list_engine_options(ws)
        .await?
        .into_iter()
        .filter(|e| e.enabled)
        .collect();

    let catalogs: Vec<CatalogOption> = st
        .store
        .list_catalog_options(ws)
        .await?
        .into_iter()
        .filter(|c| c.enabled)
        .collect();

    // 5b. Best-effort mint of a short-lived per-user engine JWT for the kernel.
    // Users without a stored refresh token (password/local logins, OIDC logins
    // without an offline token) cannot mint one; that's not an error here since
    // no P1 engine consumes this JWT yet — leave the field empty in that case.
    let (engine_jwt, engine_jwt_expires_at) = match st.engine_minter.mint(user_id.0).await {
        Ok(tok) => (Some(tok.access_token), Some(tok.expires_at)),
        Err(e) => {
            tracing::warn!("notebook engine token mint skipped: {e}");
            (None, None)
        }
    };

    // 6. Build and return the manifest.
    Ok(NotebookRuntime {
        username,
        engines,
        catalogs,
        engine_jwt,
        engine_jwt_expires_at,
    })
}

/// `GET /ws/{workspace}/notebook-runtime`
///
/// Steps:
/// 1. Require `notebook:read`.
///    2–6. Delegate to [`build_notebook_runtime`].
pub async fn runtime(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
) -> Result<Json<NotebookRuntime>, NotebookApiError> {
    // 1. Authorization check.
    st.authz
        .require_workspace(&ctx, "notebook:read")
        .await
        .map_err(NotebookApiError::from)?;

    let manifest = build_notebook_runtime(&st, ctx.workspace_id.0, ctx.principal.user_id).await?;

    Ok(Json(manifest))
}
