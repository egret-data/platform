//! Kernel lifecycle HTTP routes: `POST /ws/{workspace}/kernel` (start),
//! `GET` (status), `DELETE` (stop).
//!
//! All handlers are gated by `authz.require_workspace(&ctx, "notebook:read")`
//! BEFORE any provisioning work. The kernel key is always keyed to the
//! caller's own `(user_id, workspace_id)` — never a path/body-supplied user.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{FromRef, FromRequestParts, Path, State};
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use egret_auth::session::SessionService;
use egret_notebook::build_notebook_runtime;
use egret_notebook::NotebookState;
use egret_workspace::context::WsContext;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use serde::Serialize;
use uuid::Uuid;

use crate::env::manifest_to_env;
use crate::error::KernelApiError;
use crate::launcher::KernelPhase;
use crate::manager::KernelManager;
use crate::notebook_kernels::NotebookKernelStore;
use crate::proxy::KernelProxy;
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
    pub notebook_kernels: NotebookKernelStore,
    pub proxy: Arc<dyn KernelProxy>,
    /// Injected engine-execution seam for `POST /kernel/query`. The concrete
    /// runner (egret-engine + egret-database) is wired in from egret-bin.
    pub query_runner: Arc<dyn crate::query::NotebookQueryRunner>,
    /// Mints fresh per-user engine JWTs for `POST /ws/{ws}/notebook-runtime/token`.
    /// Shares the SAME `EngineTokenBroker` Arc as `NotebookState` (wired in egret-bin).
    pub engine_minter: std::sync::Arc<dyn egret_notebook::engine_token::EngineTokenMinter>,
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

/// Browser/session-cookie routes: the kernel lifecycle and notebook channels.
///
/// These are authenticated by the user's session cookie and MUST be mounted
/// behind the CSRF layer in egret-server (they are mutating, browser-driven).
pub fn kernel_router() -> Router<KernelState> {
    Router::new()
        .route(
            "/ws/{workspace}/kernel",
            post(start).get(status).delete(stop),
        )
        .route(
            "/ws/{workspace}/notebooks/{notebook}/kernel/connect",
            post(connect),
        )
        .route(
            "/ws/{workspace}/notebooks/{notebook}/kernel/channels",
            get(channels),
        )
}

/// Kernel-token bearer callback routes invoked by the kernel SUBPROCESS, not a
/// browser. They authenticate via `Authorization: Bearer <EGRET_KERNEL_TOKEN>`
/// (the `KernelIdentity` extractor) and have NO browser session / CSRF cookie,
/// so in egret-server they MUST be mounted CSRF-exempt (like `/catalog/v1` and
/// the Ranger `/service` routes). Mounting these behind `csrf_layer` would 403
/// every request before `KernelIdentity` runs.
pub fn kernel_callback_router() -> Router<KernelState> {
    Router::new()
        .route("/kernel/query", post(kernel_query))
        .route(
            "/ws/{workspace}/notebook-runtime/token",
            post(notebook_runtime_token),
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

/// `POST /ws/{workspace}/notebooks/{notebook}/kernel/connect` — ensure the gateway
/// and resolve/create the per-notebook kernel id.
///
/// Gate: `notebook:read`; the notebook must belong to this workspace and be
/// readable by the caller (owner or public). The kernel is always the caller's own.
async fn connect(
    State(st): State<KernelState>,
    WsContext(ctx): WsContext,
    Path((_ws, nb)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, KernelApiError> {
    st.authz
        .require_workspace(&ctx, "notebook:read")
        .await
        .map_err(KernelApiError::from)?;

    // Notebook must belong to this workspace AND be readable by the caller.
    let (summary, nb_ws) = st
        .notebook
        .store
        .get_summary(nb)
        .await
        .map_err(|e| KernelApiError(e.0))?
        .ok_or_else(|| KernelApiError(egret_core::Error::NotFound("notebook not found".into())))?;
    if nb_ws != ctx.workspace_id.0 {
        return Err(KernelApiError(egret_core::Error::NotFound(
            "notebook not found".into(),
        )));
    }
    if !(summary.is_public || summary.owner_user_id == ctx.principal.user_id.0) {
        return Err(KernelApiError(egret_core::Error::Forbidden));
    }

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

    if status.phase != KernelPhase::Ready {
        return Ok(Json(serde_json::json!({ "phase": status.phase.as_str() })));
    }

    let existing = st
        .notebook_kernels
        .get(nb, ctx.principal.user_id.0)
        .await
        .map_err(KernelApiError::from)?;
    let kid = crate::gateway::ensure_kernel(&status.url, existing.as_deref())
        .await
        .map_err(KernelApiError::from)?;
    st.notebook_kernels
        .upsert(nb, ctx.principal.user_id.0, &kid)
        .await
        .map_err(KernelApiError::from)?;

    Ok(Json(
        serde_json::json!({ "phase": "ready", "kernel_id": kid }),
    ))
}

/// `GET /ws/{workspace}/notebooks/{notebook}/kernel/channels` — authenticate via the
/// session cookie + `notebook:read`, require a Ready kernel, resolve the per-notebook
/// kernel id, then upgrade and proxy the Jupyter channels WS to the gateway. Touches
/// the kernel (throttled) per message so the idle reaper keeps it alive.
async fn channels(
    State(st): State<KernelState>,
    WsContext(ctx): WsContext,
    Path((_ws, nb)): Path<(String, Uuid)>,
    upgrade: WebSocketUpgrade,
) -> Result<axum::response::Response, KernelApiError> {
    st.authz
        .require_workspace(&ctx, "notebook:read")
        .await
        .map_err(KernelApiError::from)?;

    // Notebook must belong to this workspace AND be readable by the caller.
    let (summary, nb_ws) = st
        .notebook
        .store
        .get_summary(nb)
        .await
        .map_err(|e| KernelApiError(e.0))?
        .ok_or_else(|| KernelApiError(egret_core::Error::NotFound("notebook not found".into())))?;
    if nb_ws != ctx.workspace_id.0 {
        return Err(KernelApiError(egret_core::Error::NotFound(
            "notebook not found".into(),
        )));
    }
    if !(summary.is_public || summary.owner_user_id == ctx.principal.user_id.0) {
        return Err(KernelApiError(egret_core::Error::Forbidden));
    }

    let user = ctx.principal.user_id.0;
    let key = KernelKey {
        user_id: user,
        workspace_id: ctx.workspace_id.0,
    };

    // The gateway must already be Ready (the FE calls `connect` first).
    let status = st
        .manager
        .status(key)
        .await
        .map_err(KernelApiError::from)?
        .filter(|s| s.phase == KernelPhase::Ready)
        .ok_or_else(|| {
            KernelApiError(egret_core::Error::Conflict(
                "kernel not started; call connect first".into(),
            ))
        })?;

    // Resolve the per-notebook kernel id (self-heal if connect was skipped).
    let existing = st
        .notebook_kernels
        .get(nb, user)
        .await
        .map_err(KernelApiError::from)?;
    let kid = crate::gateway::ensure_kernel(&status.url, existing.as_deref())
        .await
        .map_err(KernelApiError::from)?;
    st.notebook_kernels
        .upsert(nb, user, &kid)
        .await
        .map_err(KernelApiError::from)?;

    // Throttled touch: at most once per 15s, off the hot path.
    let last: Arc<std::sync::Mutex<Option<Instant>>> = Arc::new(std::sync::Mutex::new(None));
    let on_activity: Arc<dyn Fn() + Send + Sync> = {
        let st = st.clone();
        let last = last.clone();
        Arc::new(move || {
            let now = Instant::now();
            let due = {
                let mut g = last.lock().unwrap();
                let due = g
                    .map(|t| now.duration_since(t) >= Duration::from_secs(15))
                    .unwrap_or(true);
                if due {
                    *g = Some(now);
                }
                due
            };
            if due {
                let st = st.clone();
                tokio::spawn(async move {
                    let _ = st.manager.touch(key).await;
                    let _ = st.notebook_kernels.touch(nb, user).await;
                });
            }
        })
    };

    let proxy = st.proxy.clone();
    let url = status.url.clone();
    Ok(upgrade.on_upgrade(move |socket| async move {
        let _ = proxy.proxy_channels(socket, &url, &kid, on_activity).await;
    }))
}

// ── kernel query callback ─────────────────────────────────────────────────────

/// Identity resolved from an `EGRET_KERNEL_TOKEN` bearer (the kernel-callback plane).
pub struct KernelIdentity {
    pub user_id: Uuid,
    pub workspace_id: Uuid,
}

impl FromRequestParts<KernelState> for KernelIdentity {
    type Rejection = KernelApiError;
    async fn from_request_parts(
        parts: &mut Parts,
        st: &KernelState,
    ) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .ok_or(KernelApiError(egret_core::Error::Unauthorized))?;
        let hash = crate::kernel_token::hash_token(token);
        let key = st
            .manager
            .registry()
            .resolve_token(&hash)
            .await
            .map_err(KernelApiError::from)?
            .ok_or(KernelApiError(egret_core::Error::Unauthorized))?;
        Ok(KernelIdentity {
            user_id: key.user_id,
            workspace_id: key.workspace_id,
        })
    }
}

#[derive(serde::Deserialize)]
struct QueryReq {
    engine: String,
    sql: String,
    #[serde(default)]
    max_rows: Option<usize>,
}

/// `POST /kernel/query` — run SQL on an enabled notebook engine as the
/// token's user (Ranger `X-Trino-User`). Auth = `EGRET_KERNEL_TOKEN` bearer.
async fn kernel_query(
    State(st): State<KernelState>,
    ident: KernelIdentity,
    Json(req): Json<QueryReq>,
) -> Result<Json<serde_json::Value>, KernelApiError> {
    // Engine must be enabled for notebooks in the token's workspace.
    let engines = st
        .notebook
        .store
        .list_engine_options(ident.workspace_id)
        .await
        .map_err(|e| KernelApiError(e.0))?;
    let eng = engines
        .into_iter()
        .find(|e| e.enabled && (e.id.to_string() == req.engine || e.name == req.engine))
        .ok_or(KernelApiError(egret_core::Error::Forbidden))?;

    let username = st
        .notebook
        .users
        .ensure_username(egret_core::ids::UserId(ident.user_id))
        .await
        .map_err(KernelApiError::from)?;

    let bearer = match st.engine_minter.mint(ident.user_id).await {
        Ok(tok) => Some(tok.access_token),
        Err(egret_notebook::engine_token::BrokerError::NoRefreshToken) => {
            return Err(KernelApiError(egret_core::Error::Unauthorized));
        }
        Err(e) => {
            return Err(KernelApiError(egret_core::Error::Internal(
                anyhow::anyhow!(e),
            )))
        }
    };
    let max_rows = req.max_rows.unwrap_or(10_000).min(100_000);
    let rows = st
        .query_runner
        .run(eng.id, username, bearer, req.sql, max_rows)
        .await
        .map_err(|e| match e {
            crate::query::QueryRunError::NotFound(m) => {
                KernelApiError(egret_core::Error::NotFound(m))
            }
            crate::query::QueryRunError::Engine(m) => {
                KernelApiError(egret_core::Error::InvalidInput(m))
            }
        })?;

    Ok(Json(serde_json::json!({
        "columns": rows.columns,
        "rows": rows.rows,
        "truncated": rows.truncated,
    })))
}

// ── notebook-runtime token refresh ────────────────────────────────────────────

/// `POST /ws/{workspace}/notebook-runtime/token` — mint a fresh short-lived engine
/// JWT for the kernel-token's user. Auth = `EGRET_KERNEL_TOKEN` bearer; identity
/// comes ONLY from `KernelIdentity` — the `{workspace}` path segment is accepted
/// for URL symmetry but never trusted for auth. Returns ONLY the caller's token.
async fn notebook_runtime_token(
    id: KernelIdentity,
    State(st): State<KernelState>,
) -> Result<Json<egret_notebook::engine_token::EngineToken>, KernelApiError> {
    let tok = st
        .engine_minter
        .mint(id.user_id)
        .await
        .map_err(|e| KernelApiError(egret_core::Error::Internal(anyhow::anyhow!(e))))?;
    Ok(Json(tok))
}

// ── bring IntoResponse into scope for the status handler ─────────────────────

use axum::response::IntoResponse;
