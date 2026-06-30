//! Kernel lifecycle HTTP routes: `POST /ws/{workspace}/kernel` (start),
//! `GET` (status), `DELETE` (stop).
//!
//! All handlers are gated by `authz.require_workspace(&ctx, "notebook:read")`
//! BEFORE any provisioning work. The kernel key is always keyed to the
//! caller's own `(user_id, workspace_id)` — never a path/body-supplied user.

use std::sync::Arc;

use axum::extract::{FromRef, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use egret_auth::session::SessionService;
use egret_notebook::build_notebook_runtime;
use egret_notebook::NotebookState;
use egret_workspace::context::WsContext;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use serde::Serialize;

use crate::env::manifest_to_env;
use crate::error::KernelApiError;
use crate::manager::KernelManager;
use crate::registry::KernelKey;

// ── State ─────────────────────────────────────────────────────────────────────

/// Application state threaded through the kernel router.
///
/// `KernelManager` is stored behind an `Arc` so `KernelState` can be `Clone`
/// (required by axum's `with_state`). The manager internally holds an
/// `Arc<dyn KernelLauncher>` and a `KernelRegistry` (also `Clone`), so
/// cloning the `Arc<KernelManager>` is cheap and correct.
#[derive(Clone)]
pub struct KernelState {
    pub manager: Arc<KernelManager>,
    pub notebook: NotebookState,
    pub sessions: SessionService,
    pub workspaces: WorkspaceRepo,
    pub authz: Authorizer,
    /// Base URL of the egret HTTP API forwarded to the kernel process as
    /// `EGRET_API_URL`.  Example: `"http://localhost:3000"`.
    pub api_url: String,
}

impl FromRef<KernelState> for SessionService {
    fn from_ref(s: &KernelState) -> Self {
        s.sessions.clone()
    }
}

impl FromRef<KernelState> for WorkspaceRepo {
    fn from_ref(s: &KernelState) -> Self {
        s.workspaces.clone()
    }
}

impl FromRef<KernelState> for Authorizer {
    fn from_ref(s: &KernelState) -> Self {
        s.authz.clone()
    }
}

// ── Router ────────────────────────────────────────────────────────────────────

pub fn kernel_router() -> Router<KernelState> {
    Router::new().route(
        "/ws/{workspace}/kernel",
        post(start).get(status).delete(stop),
    )
}

// ── "No kernel" sentinel ──────────────────────────────────────────────────────

/// Returned by `GET` when there is no kernel row for the caller.
/// Using 200 + `{"phase":"none"}` so the FE can poll without treating a
/// missing kernel as an error.
#[derive(Serialize)]
struct NoKernel {
    phase: &'static str,
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `POST /ws/{workspace}/kernel` — start (or reuse) the caller's kernel.
///
/// Gate: `notebook:read`.
/// Key: `(ctx.principal.user_id, ctx.workspace_id)` — always the caller's own.
async fn start(
    State(st): State<KernelState>,
    WsContext(ctx): WsContext,
) -> Result<Json<crate::manager::KernelStatus>, KernelApiError> {
    st.authz
        .require_workspace(&ctx, "notebook:read")
        .await
        .map_err(KernelApiError::from)?;

    let key = KernelKey {
        user_id: ctx.principal.user_id.0,
        workspace_id: ctx.workspace_id.0,
    };

    let manifest = build_notebook_runtime(&st.notebook, ctx.workspace_id.0, ctx.principal.user_id)
        .await
        .map_err(|e| KernelApiError(e.0))?;

    let env = manifest_to_env(&manifest, &st.api_url, "");

    let status = st
        .manager
        .ensure_running(key, &env)
        .await
        .map_err(KernelApiError::from)?;

    Ok(Json(status))
}

/// `GET /ws/{workspace}/kernel` — current kernel status for the caller.
///
/// Returns 200 `{phase, url}` if a row exists, or 200 `{phase:"none"}` if not.
async fn status(
    State(st): State<KernelState>,
    WsContext(ctx): WsContext,
) -> Result<axum::response::Response, KernelApiError> {
    st.authz
        .require_workspace(&ctx, "notebook:read")
        .await
        .map_err(KernelApiError::from)?;

    let key = KernelKey {
        user_id: ctx.principal.user_id.0,
        workspace_id: ctx.workspace_id.0,
    };

    match st.manager.status(key).await.map_err(KernelApiError::from)? {
        Some(s) => Ok(Json(s).into_response()),
        None => Ok(Json(NoKernel { phase: "none" }).into_response()),
    }
}

/// `DELETE /ws/{workspace}/kernel` — stop the caller's kernel and remove its row.
///
/// Returns 204 No Content.
async fn stop(
    State(st): State<KernelState>,
    WsContext(ctx): WsContext,
) -> Result<StatusCode, KernelApiError> {
    st.authz
        .require_workspace(&ctx, "notebook:read")
        .await
        .map_err(KernelApiError::from)?;

    let key = KernelKey {
        user_id: ctx.principal.user_id.0,
        workspace_id: ctx.workspace_id.0,
    };

    st.manager.stop(key).await.map_err(KernelApiError::from)?;

    Ok(StatusCode::NO_CONTENT)
}

// ── bring IntoResponse into scope for the status handler ─────────────────────

use axum::response::IntoResponse;
