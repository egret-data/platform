//! `GET /ws/{workspace}/notebook-runtime` — per-user kernel manifest (F1→D1).
//!
//! Returns the caller's stable username and the workspace's enabled
//! engines so the Daft kernel can wire itself up in one round-trip.

use axum::extract::State;
use axum::Json;
use egret_workspace::context::WsContext;

use crate::error::NotebookApiError;
use crate::model::EngineOption;
use crate::routes::NotebookState;

// `NotebookRuntime` moved verbatim to the leaf contract crate
// `egret-analytics-api` (E5c-1); re-exported so callers keep using
// `egret_notebook::NotebookRuntime`.
pub use egret_analytics_api::NotebookRuntime;

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

    // 5. Load enabled engines (filter to enabled==true).
    let engines: Vec<EngineOption> = st
        .store
        .list_engine_options(ws)
        .await?
        .into_iter()
        .filter(|e| e.enabled)
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
        engine_jwt,
        engine_jwt_expires_at,
    })
}

/// `GET /ws/{workspace}/notebook-runtime`
///
/// Steps:
/// 1. Require `notebook:read`.
///    2–6. Delegate to [`build_notebook_runtime`].
#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/notebook-runtime",
    tag = "notebooks",
    params(egret_core::WorkspacePath),
    responses(
        (status = 200, description = "Per-user kernel runtime manifest (username, engines, engine JWT)", body = NotebookRuntime),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing notebook:read permission", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
pub async fn runtime(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
) -> Result<Json<NotebookRuntime>, NotebookApiError> {
    // 1. Authorization check.
    egret_core::authz::require(&*st.pdp, &ctx, "notebook:read")
        .await
        .map_err(NotebookApiError::from)?;

    let manifest = build_notebook_runtime(&st, ctx.workspace_id.0, ctx.principal.user_id).await?;

    Ok(Json(manifest))
}
