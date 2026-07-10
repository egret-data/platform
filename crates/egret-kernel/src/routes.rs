//! Kernel lifecycle HTTP routes: `POST /ws/{workspace}/kernel` (start),
//! `GET` (status), `DELETE` (stop).
//!
//! All handlers are gated by `egret_core::authz::require(&*st.pdp, &ctx, "notebook:read")`
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
use egret_analytics_api::{CellOutput, NotebookDocsPort};
use egret_auth::session::SessionService;
use egret_core::http::ErrorBody;
use egret_core::WorkspacePath;
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
    /// Notebook-docs Port (E5c-1): kernel depends on this leaf trait, not
    /// concrete `egret-notebook`. Backs the summary/IDOR gate, engine-option
    /// listing, output persistence, and runtime-manifest build.
    pub notebook_docs: Arc<dyn NotebookDocsPort>,
    pub sessions: SessionService,
    pub workspaces: WorkspaceRepo,
    pub authz: Authorizer,
    pub pdp: std::sync::Arc<dyn egret_core::authz::Pdp>,
    /// Base URL of the egret HTTP API forwarded to the kernel process as
    /// `EGRET_API_URL`.  Example: `"http://localhost:3000"`.
    pub api_url: String,
    pub notebook_kernels: NotebookKernelStore,
    pub proxy: Arc<dyn KernelProxy>,
    /// Mints fresh per-user engine JWTs for `POST /ws/{ws}/notebook-runtime/token`.
    /// Shares the SAME `EngineTokenBroker` Arc as `NotebookState` (wired in egret-bin).
    pub engine_minter: std::sync::Arc<dyn egret_analytics_api::EngineTokenMinter>,
    /// Resolves an engine's database id to its connection coordinates for
    /// `POST /kernel/engine-token`. Concrete impl injected from egret-bin.
    pub engine_conn: std::sync::Arc<dyn crate::engine_conn::EngineConnResolver>,
    /// Shared registry of currently-running cells per (notebook, user), fed by the
    /// output recorder in `channels` and read by the active-executions endpoint.
    pub active: crate::recorder::ActiveExecutions,
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
        .route(
            "/ws/{workspace}/notebooks/{notebook}/kernel/interrupt",
            post(kernel_interrupt),
        )
        .route(
            "/ws/{workspace}/notebooks/{notebook}/kernel/restart",
            post(kernel_restart),
        )
        .route(
            "/ws/{workspace}/notebooks/{notebook}/kernel/active-executions",
            get(active_executions),
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
        .route("/kernel/engine-token", post(engine_token))
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
#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/kernel",
    tag = "kernel",
    params(WorkspacePath),
    responses(
        (status = 200, description = "Kernel started (or the running one reused)", body = crate::manager::KernelStatus),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing notebook:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn start(
    State(st): State<KernelState>,
    WsContext(ctx): WsContext,
) -> Result<Json<crate::manager::KernelStatus>, KernelApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, "notebook:read")
        .await
        .map_err(KernelApiError::from)?;

    let key = KernelKey {
        user_id: ctx.principal.user_id.0,
        workspace_id: ctx.workspace_id.0,
    };

    let manifest = st
        .notebook_docs
        .build_runtime(ctx.workspace_id.0, ctx.principal.user_id)
        .await
        .map_err(KernelApiError)?;

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
#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/kernel",
    tag = "kernel",
    params(WorkspacePath),
    responses(
        (status = 200, description = "Current kernel status; when no kernel exists the body is the sentinel `{ \"phase\": \"none\" }`", body = crate::manager::KernelStatus),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing notebook:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn status(
    State(st): State<KernelState>,
    WsContext(ctx): WsContext,
) -> Result<axum::response::Response, KernelApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, "notebook:read")
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
#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/kernel",
    tag = "kernel",
    params(WorkspacePath),
    responses(
        (status = 204, description = "Kernel stopped and its row removed"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing notebook:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn stop(
    State(st): State<KernelState>,
    WsContext(ctx): WsContext,
) -> Result<StatusCode, KernelApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, "notebook:read")
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
#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/notebooks/{notebook}/kernel/connect",
    tag = "kernel",
    params(
        WorkspacePath,
        ("notebook" = Uuid, Path, description = "Notebook id"),
    ),
    responses(
        (status = 200, description = "Gateway ensured; body is `{ phase, kernel_id }` when ready, else `{ phase }`", body = Object),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Notebook is private and not owned by the caller", body = ErrorBody),
        (status = 404, description = "No such notebook in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn connect(
    State(st): State<KernelState>,
    WsContext(ctx): WsContext,
    Path((_ws, nb)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, KernelApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, "notebook:read")
        .await
        .map_err(KernelApiError::from)?;

    // Notebook must belong to this workspace AND be readable by the caller.
    let (summary, nb_ws) = st
        .notebook_docs
        .get_summary(nb)
        .await
        .map_err(KernelApiError)?
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
    let manifest = st
        .notebook_docs
        .build_runtime(ctx.workspace_id.0, ctx.principal.user_id)
        .await
        .map_err(KernelApiError)?;
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

/// Persistence sink for the output recorder: delegates to the notebook-docs Port
/// so the recorder owns output persistence (the FE never PATCHes outputs).
struct StoreSink(Arc<dyn NotebookDocsPort>);
#[async_trait::async_trait]
impl crate::recorder::OutputSink for StoreSink {
    async fn record(
        &self,
        nb: uuid::Uuid,
        cid: uuid::Uuid,
        outs: Vec<CellOutput>,
        ec: Option<i32>,
        ms: Option<i32>,
    ) {
        if let Err(e) = self.0.record_output(nb, cid, &outs, ec, ms).await {
            tracing::warn!("recorder persist failed: {e:?}");
        }
    }
}

/// Composite tap: on every forwarded frame it runs the throttled kernel-touch
/// (D1c liveness) AND feeds the byte-verbatim frame to the output recorder.
struct ChannelsTap {
    rec: crate::recorder::Recorder,
    touch: std::sync::Arc<dyn Fn() + Send + Sync>,
}
impl crate::recorder::ChannelTap for ChannelsTap {
    fn on_frame(&self, dir: crate::recorder::FrameDir, bytes: &[u8]) {
        (self.touch)();
        self.rec.on_frame(dir, bytes);
    }
}

/// `GET /ws/{workspace}/notebooks/{notebook}/kernel/channels` — authenticate via the
/// session cookie + `notebook:read`, require a Ready kernel, resolve the per-notebook
/// kernel id, then upgrade and proxy the Jupyter channels WS to the gateway. Touches
/// the kernel (throttled) per message so the idle reaper keeps it alive, and records
/// kernel outputs to the DB (the recorder owns output persistence).
#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/notebooks/{notebook}/kernel/channels",
    tag = "kernel",
    params(
        WorkspacePath,
        ("notebook" = Uuid, Path, description = "Notebook id"),
    ),
    responses(
        // This endpoint is NOT a REST/JSON call: the frontend opens a WebSocket,
        // not a fetch. On success the server replies 101 Switching Protocols and
        // proxies the Jupyter channels WS to the kernel gateway — there is no JSON
        // response body. The error statuses below are returned before the upgrade.
        (status = 101, description = "WebSocket upgrade for the Jupyter channels stream"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Notebook is private and not owned by the caller", body = ErrorBody),
        (status = 404, description = "No such notebook in this workspace", body = ErrorBody),
        (status = 409, description = "Kernel not started; call connect first", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn channels(
    State(st): State<KernelState>,
    WsContext(ctx): WsContext,
    Path((_ws, nb)): Path<(String, Uuid)>,
    upgrade: WebSocketUpgrade,
) -> Result<axum::response::Response, KernelApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, "notebook:read")
        .await
        .map_err(KernelApiError::from)?;

    // Notebook must belong to this workspace AND be readable by the caller.
    let (summary, nb_ws) = st
        .notebook_docs
        .get_summary(nb)
        .await
        .map_err(KernelApiError)?
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
    let touch: Arc<dyn Fn() + Send + Sync> = {
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

    // Output recorder: correlates execute_request → iopub and persists on idle.
    // Owns output persistence — the FE never PATCHes outputs. `nb` + `user` are
    // already captured in scope (before the on_upgrade move closure).
    let rec = crate::recorder::Recorder::new(
        nb,
        user,
        std::sync::Arc::new(StoreSink(st.notebook_docs.clone())),
        st.active.clone(),
    );
    let tap: std::sync::Arc<dyn crate::recorder::ChannelTap> =
        std::sync::Arc::new(ChannelsTap { rec, touch });

    let proxy = st.proxy.clone();
    let url = status.url.clone();
    Ok(upgrade.on_upgrade(move |socket| async move {
        let _ = proxy.proxy_channels(socket, &url, &kid, tap).await;
    }))
}

/// Shared auth + IDOR gate for any notebook-scoped endpoint: requires
/// `notebook:read`, the notebook to belong to this workspace, and the caller to
/// be the owner or the notebook to be public.  Used by `resolve_running_notebook_kernel`,
/// `active_executions`, and any future endpoint that needs the same check but
/// does NOT need a Ready kernel.
async fn authorize_notebook(
    st: &KernelState,
    ctx: &egret_core::WorkspaceContext,
    nb: Uuid,
) -> Result<(), KernelApiError> {
    egret_core::authz::require(&*st.pdp, ctx, "notebook:read")
        .await
        .map_err(KernelApiError::from)?;

    // Notebook must belong to this workspace AND be readable by the caller.
    let (summary, nb_ws) = st
        .notebook_docs
        .get_summary(nb)
        .await
        .map_err(KernelApiError)?
        .ok_or_else(|| KernelApiError(egret_core::Error::NotFound("notebook not found".into())))?;
    if nb_ws != ctx.workspace_id.0 {
        return Err(KernelApiError(egret_core::Error::NotFound(
            "notebook not found".into(),
        )));
    }
    if !(summary.is_public || summary.owner_user_id == ctx.principal.user_id.0) {
        return Err(KernelApiError(egret_core::Error::Forbidden));
    }
    Ok(())
}

/// Shared auth + IDOR + resolution for `interrupt`/`restart`. Requires
/// `notebook:read`, the notebook to belong to this workspace and be readable,
/// and a Ready kernel with a resolved per-notebook kernel id. Returns
/// `(gateway_url, kernel_id)`. 409 Conflict if no Ready kernel / no kernel id yet.
async fn resolve_running_notebook_kernel(
    st: &KernelState,
    ctx: &egret_core::WorkspaceContext,
    nb: Uuid,
) -> Result<(String, String), KernelApiError> {
    authorize_notebook(st, ctx, nb).await?;

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

    // The per-notebook kernel must already exist (created by connect/channels).
    let kid = st
        .notebook_kernels
        .get(nb, user)
        .await
        .map_err(KernelApiError::from)?
        .ok_or_else(|| {
            KernelApiError(egret_core::Error::Conflict(
                "no running kernel for this notebook; call connect first".into(),
            ))
        })?;

    Ok((status.url, kid))
}

/// `POST /ws/{workspace}/notebooks/{notebook}/kernel/interrupt` — interrupt the
/// per-notebook kernel via the Jupyter Kernel Gateway REST API. Returns 204.
#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/notebooks/{notebook}/kernel/interrupt",
    tag = "kernel",
    params(
        WorkspacePath,
        ("notebook" = Uuid, Path, description = "Notebook id"),
    ),
    responses(
        (status = 204, description = "Interrupt sent to the per-notebook kernel"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Notebook is private and not owned by the caller", body = ErrorBody),
        (status = 404, description = "No such notebook in this workspace", body = ErrorBody),
        (status = 409, description = "No running kernel for this notebook; call connect first", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn kernel_interrupt(
    State(st): State<KernelState>,
    WsContext(ctx): WsContext,
    Path((_ws, nb)): Path<(String, Uuid)>,
) -> Result<StatusCode, KernelApiError> {
    let (gateway_url, kernel_id) = resolve_running_notebook_kernel(&st, &ctx, nb).await?;
    crate::gateway::interrupt_kernel(&gateway_url, &kernel_id)
        .await
        .map_err(KernelApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /ws/{workspace}/notebooks/{notebook}/kernel/restart` — restart the
/// per-notebook kernel via the gateway REST API. Returns `{kernel_id}`.
#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/notebooks/{notebook}/kernel/restart",
    tag = "kernel",
    params(
        WorkspacePath,
        ("notebook" = Uuid, Path, description = "Notebook id"),
    ),
    responses(
        (status = 200, description = "Kernel restarted; body is `{ kernel_id }`", body = Object),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Notebook is private and not owned by the caller", body = ErrorBody),
        (status = 404, description = "No such notebook in this workspace", body = ErrorBody),
        (status = 409, description = "No running kernel for this notebook; call connect first", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn kernel_restart(
    State(st): State<KernelState>,
    WsContext(ctx): WsContext,
    Path((_ws, nb)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, KernelApiError> {
    let (gateway_url, kernel_id) = resolve_running_notebook_kernel(&st, &ctx, nb).await?;
    let id = crate::gateway::restart_kernel(&gateway_url, &kernel_id)
        .await
        .map_err(KernelApiError::from)?;
    Ok(Json(serde_json::json!({ "kernel_id": id })))
}

/// `GET /ws/{workspace}/notebooks/{notebook}/kernel/active-executions` — returns
/// the cell_ids currently executing for the caller's notebook from the in-memory
/// `ActiveExecutions` registry, so the frontend can restore in-flight cells on
/// reopen.
///
/// Gate: `notebook:read`; the notebook must belong to this workspace and be
/// readable by the caller (owner or public). No kernel resolution needed — this
/// is a pure registry lookup; even if the kernel has since died, the registry
/// will be empty (the `Recorder` drop impl clears stale entries).
#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/notebooks/{notebook}/kernel/active-executions",
    tag = "kernel",
    params(
        WorkspacePath,
        ("notebook" = Uuid, Path, description = "Notebook id"),
    ),
    responses(
        (status = 200, description = "Cell ids currently executing for the caller's notebook: `{ running: [uuid] }`", body = Object),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Notebook is private and not owned by the caller", body = ErrorBody),
        (status = 404, description = "No such notebook in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn active_executions(
    State(st): State<KernelState>,
    WsContext(ctx): WsContext,
    Path((_ws, nb)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, KernelApiError> {
    authorize_notebook(&st, &ctx, nb).await?;
    let running = st.active.running(nb, ctx.principal.user_id.0);
    Ok(Json(serde_json::json!({ "running": running })))
}

// ── kernel-callback identity ──────────────────────────────────────────────────

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

// ── engine-token callback ─────────────────────────────────────────────────────

#[derive(serde::Deserialize, utoipa::ToSchema)]
struct EngineTokenReq {
    engine: String,
}

#[derive(serde::Serialize, utoipa::ToSchema)]
struct EngineTokenResp {
    jwt: String,
    expires_at: chrono::DateTime<chrono::Utc>,
    kind: String,
    host: String,
    port: i32,
}

/// `POST /kernel/engine-token` — mint a fresh per-user engine JWT and return the
/// engine's connection coordinates so the Daft kernel can open a direct
/// connection. Auth = `EGRET_KERNEL_TOKEN` bearer; identity + workspace come ONLY
/// from `KernelIdentity`. The engine must be enabled for notebooks in the token's
/// workspace; unknown/disabled engines → 404.
#[utoipa::path(
    post,
    path = "/api/kernel/engine-token",
    tag = "kernel-callback",
    request_body = EngineTokenReq,
    responses(
        (status = 200, description = "Fresh per-user engine JWT + the engine's connection coordinates", body = EngineTokenResp),
        (status = 401, description = "Missing/invalid EGRET_KERNEL_TOKEN bearer, or the user has no stored refresh token", body = ErrorBody),
        (status = 404, description = "Engine not enabled for notebooks in the token's workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn engine_token(
    State(st): State<KernelState>,
    id: KernelIdentity,
    Json(req): Json<EngineTokenReq>,
) -> Result<Json<EngineTokenResp>, KernelApiError> {
    let engines = st
        .notebook_docs
        .list_engine_options(id.workspace_id)
        .await
        .map_err(KernelApiError)?;
    let engine = engines
        .into_iter()
        .filter(|e| e.enabled)
        .find(|e| e.id.to_string() == req.engine || e.name == req.engine)
        .ok_or_else(|| {
            KernelApiError(egret_core::Error::NotFound(format!(
                "engine not enabled for notebooks: {}",
                req.engine
            )))
        })?;
    let conn = st
        .engine_conn
        .resolve(engine.id)
        .await
        .map_err(|e| match e {
            crate::engine_conn::EngineConnError::NotFound(m) => {
                KernelApiError(egret_core::Error::NotFound(m))
            }
            crate::engine_conn::EngineConnError::Other(m) => {
                KernelApiError(egret_core::Error::Internal(anyhow::anyhow!(m)))
            }
        })?;
    let tok = match st.engine_minter.mint(id.user_id).await {
        Ok(tok) => tok,
        Err(egret_analytics_api::BrokerError::NoRefreshToken) => {
            return Err(KernelApiError(egret_core::Error::Unauthorized));
        }
        Err(e) => {
            return Err(KernelApiError(egret_core::Error::Internal(
                anyhow::anyhow!(e),
            )));
        }
    };
    Ok(Json(EngineTokenResp {
        jwt: tok.access_token,
        expires_at: tok.expires_at,
        kind: conn.kind,
        host: conn.host,
        port: conn.port,
    }))
}

// ── notebook-runtime token refresh ────────────────────────────────────────────

/// `POST /ws/{workspace}/notebook-runtime/token` — mint a fresh short-lived engine
/// JWT for the kernel-token's user. Auth = `EGRET_KERNEL_TOKEN` bearer; identity
/// comes ONLY from `KernelIdentity` — the `{workspace}` path segment is accepted
/// for URL symmetry but never trusted for auth. Returns ONLY the caller's token.
#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/notebook-runtime/token",
    tag = "kernel-callback",
    params(WorkspacePath),
    responses(
        (status = 200, description = "Fresh short-lived engine JWT for the kernel-token's user", body = egret_analytics_api::EngineToken),
        (status = 401, description = "Missing/invalid EGRET_KERNEL_TOKEN bearer", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn notebook_runtime_token(
    id: KernelIdentity,
    State(st): State<KernelState>,
) -> Result<Json<egret_analytics_api::EngineToken>, KernelApiError> {
    let tok = st
        .engine_minter
        .mint(id.user_id)
        .await
        .map_err(|e| KernelApiError(egret_core::Error::Internal(anyhow::anyhow!(e))))?;
    Ok(Json(tok))
}

// ── bring IntoResponse into scope for the status handler ─────────────────────

use axum::response::IntoResponse;

// ── OpenAPI fragments (E0·P4b-8) ─────────────────────────────────────────────
//
// Two `#[derive(utoipa::OpenApi)]` doc structs, merged into `egret_kernel::openapi()`:
//   - `KernelApi` — the browser/session kernel-lifecycle + notebook-channels
//     router (mounted `/api`, CSRF-protected). `channels` is a WebSocket upgrade,
//     documented as a `GET` returning `101 Switching Protocols` with no JSON body.
//   - `KernelCallbackApi` — the kernel-token bearer callbacks (mounted at the
//     root, CSRF-exempt) invoked by the kernel subprocess, not a browser.
// Free-form `{ … }` JSON bodies are documented as `Object` (the codebase-wide
// convention for `Json<serde_json::Value>` responses).

/// OpenAPI fragment for the session-cookie kernel router
/// (`/api/ws/{workspace}/kernel`, `/notebooks/{notebook}/kernel/*`).
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        start,
        status,
        stop,
        connect,
        channels,
        kernel_interrupt,
        kernel_restart,
        active_executions,
    ),
    components(schemas(
        crate::manager::KernelStatus,
        ErrorBody,
    )),
    tags((name = "kernel", description = "Per-(user×workspace) Jupyter kernel lifecycle + per-notebook channels (WebSocket), interrupt/restart, active executions")),
)]
pub struct KernelApi;

/// OpenAPI fragment for the kernel-token bearer callback router
/// (`/kernel/engine-token`, `/ws/{workspace}/notebook-runtime/token`).
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        engine_token,
        notebook_runtime_token,
    ),
    components(schemas(
        EngineTokenReq,
        EngineTokenResp,
        egret_analytics_api::EngineToken,
        ErrorBody,
    )),
    tags((name = "kernel-callback", description = "Machine-facing kernel-subprocess callbacks (EGRET_KERNEL_TOKEN bearer, CSRF-exempt): engine-token + runtime-token refresh")),
)]
pub struct KernelCallbackApi;
