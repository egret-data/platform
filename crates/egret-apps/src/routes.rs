//! The cookie-session API for data apps (`/api/ws/{workspace}/apps…`): CRUD,
//! folders, trash, bookmarks, the revisioned draft, publish, and versions.
//!
//! Authorization is role-based with per-object shares, exactly as notebooks: list
//! and create gate on the workspace (`authz::require`), and every `{id}` route
//! first resolves the app IN THE CALLER'S WORKSPACE ([`app_object_ref`], 404 when
//! it is absent or belongs to another workspace) and then gates on
//! [`require_object`], so a folder / workspace / object grant cascades to it.
//! Only after both does a handler call an [`AppStore`] function — which is what
//! lets the store's app-id-only functions (`draft`, `save_draft`, `publish`, …)
//! skip the workspace.
//!
//! A trashed app is read-only: every route that changes it, other than
//! `POST …/restore` itself, answers 409 until it is restored.

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, FromRef, Path, Query, State};
use axum::http::header::{ETAG, IF_MATCH};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use axum_extra::extract::cookie::CookieJar;
use chrono::{DateTime, Utc};
use egret_audit::{mutation_for, AuditSink};
use egret_auth::cookie::{read_session, secure_cookies};
use egret_auth::session::SessionService;
use egret_core::authz::{require_object, ObjectRef, Pdp};
use egret_core::http::{ApiError, ErrorBody};
use egret_core::{Error, WorkspaceContext, WorkspacePath};
use egret_metastore::Db;
use egret_workspace::context::WsContext;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::grant::{GrantRepo, NewGrant};
use crate::session::AppMode;
use crate::store::{
    AppRow, AppStore, PublishOutcome, SaveOutcome, VersionSummary, MAX_SOURCE_BYTES,
};
use crate::template::new_app_source;

const APP_READ: &str = "app:read";
const APP_WRITE: &str = "app:write";
/// Edit mode also needs arbitrary SQL, as SQL Lab does (spec §4.2).
const QUERY_RUN: &str = "query:run";

/// The draft PUT carries up to 1 MiB of source inside JSON; escaping can grow it,
/// so the body may be larger than the source (1 MiB of `"` is 2 MiB of JSON before
/// the envelope). Explicit rather than axum's 2 MB default so the limit cannot
/// move under this route; the handler's own 1 MiB check on the decoded source is
/// what answers a JSON 413.
pub(crate) const MAX_BODY_BYTES: usize = 3 * 1024 * 1024;

const TRASHED: &str = "app is in the trash; restore it first";

#[derive(Clone)]
pub struct AppsApiState {
    pub store: AppStore,
    /// `query.databases` lookup for `POST /apps {database}`.
    pub db: Db,
    pub sessions: SessionService,
    pub workspaces: WorkspaceRepo,
    pub authz: Authorizer,
    pub pdp: Arc<dyn Pdp>,
    pub audit: AuditSink,
    /// One-time tickets for the apps-origin iframe (spec §6.1).
    pub grants: GrantRepo,
    /// `apps.origin`, no trailing slash. Empty: the ticket endpoint answers 503.
    pub apps_origin: String,
    /// `apps.ticket_ttl_secs`.
    pub ticket_ttl: Duration,
}

impl FromRef<AppsApiState> for SessionService {
    fn from_ref(s: &AppsApiState) -> Self {
        s.sessions.clone()
    }
}
impl FromRef<AppsApiState> for WorkspaceRepo {
    fn from_ref(s: &AppsApiState) -> Self {
        s.workspaces.clone()
    }
}
impl FromRef<AppsApiState> for Authorizer {
    fn from_ref(s: &AppsApiState) -> Self {
        s.authz.clone()
    }
}

pub fn apps_router() -> Router<AppsApiState> {
    Router::new()
        .route("/ws/{workspace}/apps", get(list).post(create))
        .route("/ws/{workspace}/apps/{id}", get(get_one).patch(update))
        .route("/ws/{workspace}/apps/{id}/trash", post(trash))
        .route("/ws/{workspace}/apps/{id}/restore", post(restore))
        .route("/ws/{workspace}/apps/{id}/folder", patch(set_folder))
        .route(
            "/ws/{workspace}/apps/{id}/bookmark",
            post(bookmark_add).delete(bookmark_remove),
        )
        .route(
            "/ws/{workspace}/apps/{id}/draft",
            get(draft_get).put(draft_put),
        )
        .route("/ws/{workspace}/apps/{id}/publish", post(publish))
        .route("/ws/{workspace}/apps/{id}/ticket", post(ticket))
        .route("/ws/{workspace}/apps/{id}/versions", get(versions))
        .route(
            "/ws/{workspace}/apps/{id}/versions/{version_id}",
            get(version_get),
        )
        .route(
            "/ws/{workspace}/apps/{id}/versions/{version_id}/restore",
            post(version_restore),
        )
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
}

// ── Wire types ──────────────────────────────────────────────────────────────

/// One app in a list.
#[derive(Debug, Serialize, ToSchema)]
pub struct AppSummary {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub folder_id: Option<Uuid>,
    pub owner_user_id: Option<Uuid>,
    /// The caller created this app (metadata only; access is role/share based).
    pub is_owner: bool,
    pub updated_at: DateTime<Utc>,
    /// Set while the app is in the trash.
    pub deleted_at: Option<DateTime<Utc>>,
    /// Bookmarked by the caller.
    pub bookmarked: bool,
    pub draft_revision: i64,
    /// `None` until the first publish.
    pub latest_version_no: Option<i32>,
}

/// One app, as returned by get / create / update.
#[derive(Debug, Serialize, ToSchema)]
pub struct AppDetail {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub folder_id: Option<Uuid>,
    pub owner_user_id: Option<Uuid>,
    pub is_owner: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub bookmarked: bool,
    pub draft_revision: i64,
    pub latest_version_no: Option<i32>,
}

impl AppSummary {
    fn of(r: AppRow, ctx: &WorkspaceContext) -> Self {
        Self {
            is_owner: r.owner_user_id == Some(ctx.principal.user_id.0),
            id: r.id,
            name: r.name,
            description: r.description,
            folder_id: r.folder_id,
            owner_user_id: r.owner_user_id,
            updated_at: r.updated_at,
            deleted_at: r.deleted_at,
            bookmarked: r.bookmarked,
            draft_revision: r.draft_revision,
            latest_version_no: r.latest_version_no,
        }
    }
}

impl AppDetail {
    fn of(r: AppRow, ctx: &WorkspaceContext) -> Self {
        Self {
            is_owner: r.owner_user_id == Some(ctx.principal.user_id.0),
            id: r.id,
            name: r.name,
            description: r.description,
            folder_id: r.folder_id,
            owner_user_id: r.owner_user_id,
            created_at: r.created_at,
            updated_at: r.updated_at,
            deleted_at: r.deleted_at,
            bookmarked: r.bookmarked,
            draft_revision: r.draft_revision,
            latest_version_no: r.latest_version_no,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AppCreate {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// An `app` folder of this workspace.
    #[serde(default)]
    pub folder_id: Option<Uuid>,
    /// A database (by name) of this workspace; the template's connection cell
    /// is pre-filled with it.
    #[serde(default)]
    pub database: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AppUpdate {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// Move into a folder, or back to Uncategorized with `folder_id: null`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AppSetFolder {
    pub folder_id: Option<Uuid>,
}

/// The editable draft. Its revision is also the response `ETag`.
#[derive(Debug, Serialize, ToSchema)]
pub struct AppDraft {
    pub source: String,
    /// marimo's grid-layout companion file, verbatim JSON; omitted for the default
    /// vertical arrangement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<serde_json::Value>,
    pub revision: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct AppSaveDraft {
    /// marimo source, at most 1 MiB (1 048 576 bytes).
    pub source: String,
    /// The complete new layout state; omit or send `null` to clear it.
    #[serde(default)]
    pub layout: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AppDraftSaved {
    pub revision: i64,
}

/// Open an app in an iframe on the apps origin.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AppTicketRequest {
    pub mode: AppTicketMode,
    /// Run mode only: the version to open. Omitted = the latest published version.
    #[serde(default)]
    pub version_id: Option<Uuid>,
}

#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AppTicketMode {
    /// Author the draft. Needs `app:write` on the app and `query:run` in the workspace.
    Edit,
    /// View a published version. Needs `app:read` on the app.
    Run,
}

/// Where to point the iframe. The URL carries a one-time ticket that expires in
/// seconds: open it at once, never cache or reload it.
#[derive(Debug, Serialize, ToSchema)]
pub struct AppTicket {
    pub url: String,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct AppPublish {
    /// Publish only if the draft is still at this revision (else 409).
    #[serde(default)]
    pub expected_revision: Option<i64>,
}

/// The draft moved on since the caller read it: reload, then retry.
#[derive(Debug, Serialize, ToSchema)]
pub struct AppStaleRevision {
    pub error: String,
    pub current_revision: i64,
}

/// The 409 body of draft PUT and publish: either the draft moved on
/// ([`AppStaleRevision`], carrying `current_revision`) or the app is in the trash
/// ([`ErrorBody`]). Documentation only — the handlers return one of the two.
#[derive(Debug, Serialize, ToSchema)]
#[serde(untagged)]
pub enum AppConflict {
    Stale(AppStaleRevision),
    Trashed(ErrorBody),
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AppVersionSummary {
    pub id: Uuid,
    pub version_no: i32,
    /// The draft revision this version was published from; `None` for a restore.
    pub from_revision: Option<i64>,
    /// The version this one republished; `None` for a publish.
    pub restored_from: Option<Uuid>,
    pub published_by: Option<Uuid>,
    pub published_at: DateTime<Utc>,
}

impl From<VersionSummary> for AppVersionSummary {
    fn from(v: VersionSummary) -> Self {
        Self {
            id: v.id,
            version_no: v.version_no,
            from_revision: v.from_revision,
            restored_from: v.restored_from,
            published_by: v.published_by,
            published_at: v.published_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AppVersion {
    #[serde(flatten)]
    pub summary: AppVersionSummary,
    pub source: String,
}

/// `?trashed=true` selects the trash bin instead of the live list.
#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub trashed: bool,
}

// ── Errors this API adds to the shared `Error` taxonomy ─────────────────────

/// Failures with a status or body `egret_core::Error` has no variant for.
#[derive(Debug)]
pub enum AppsHttpError {
    /// 428: a draft save without `If-Match`.
    PreconditionRequired,
    /// 413: source over [`MAX_SOURCE_BYTES`].
    SourceTooLarge,
    /// 409 with the revision to reload.
    StaleRevision { current_revision: i64 },
    /// 409: a change to a trashed app.
    Trashed,
    /// 409: a run ticket for an app that has never been published.
    NoPublishedVersion,
    /// 503: `apps.origin` is not configured, so there is nowhere to send an iframe.
    OriginNotConfigured,
}

impl IntoResponse for AppsHttpError {
    fn into_response(self) -> Response {
        let body = |status: StatusCode, msg: &str| {
            (
                status,
                Json(ErrorBody {
                    error: msg.to_string(),
                    code: None,
                }),
            )
                .into_response()
        };
        match self {
            Self::PreconditionRequired => body(
                StatusCode::PRECONDITION_REQUIRED,
                "If-Match with the draft revision is required",
            ),
            Self::SourceTooLarge => body(StatusCode::PAYLOAD_TOO_LARGE, "source exceeds 1 MiB"),
            Self::StaleRevision { current_revision } => (
                StatusCode::CONFLICT,
                Json(AppStaleRevision {
                    error: "the draft changed since it was read".into(),
                    current_revision,
                }),
            )
                .into_response(),
            Self::Trashed => body(StatusCode::CONFLICT, TRASHED),
            Self::NoPublishedVersion => body(
                StatusCode::CONFLICT,
                "the app has no published version to run; publish it first",
            ),
            Self::OriginNotConfigured => body(
                StatusCode::SERVICE_UNAVAILABLE,
                "the apps origin is not configured on this deployment",
            ),
        }
    }
}

pub(crate) type AppsResult<T> = Result<T, ApiError<AppsHttpError>>;

pub(crate) fn domain(e: AppsHttpError) -> ApiError<AppsHttpError> {
    ApiError::Domain(e)
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Resolve `id` in the caller's workspace: `NotFound` when the app is absent OR in
/// another workspace (identical body, so existence does not leak). Returns the
/// [`ObjectRef`] for [`require_object`] and the row (trash state, response).
async fn app_object_ref(
    st: &AppsApiState,
    ctx: &WorkspaceContext,
    id: Uuid,
) -> AppsResult<(ObjectRef, AppRow)> {
    let row = st
        .store
        .get(ctx.workspace_id, ctx.principal.user_id, id)
        .await?
        .ok_or_else(|| Error::NotFound("app".into()))?;
    let obj = ObjectRef {
        kind: "app".into(),
        id,
        folder_id: row.folder_id,
        workspace_id: ctx.workspace_id,
    };
    Ok((obj, row))
}

fn refuse_if_trashed(row: &AppRow) -> AppsResult<()> {
    if row.deleted_at.is_some() {
        return Err(domain(AppsHttpError::Trashed));
    }
    Ok(())
}

/// A folder must be an `app` folder of this workspace; anything else is 404 so a
/// foreign folder's existence does not leak.
async fn check_folder(st: &AppsApiState, ctx: &WorkspaceContext, fid: Uuid) -> AppsResult<()> {
    match st.store.folder_kind(fid).await? {
        Some((w, t)) if w == ctx.workspace_id.0 && t == "app" => Ok(()),
        _ => Err(Error::NotFound("folder".into()).into()),
    }
}

/// Re-read the app after a change (it was resolved in this workspace already).
async fn detail(st: &AppsApiState, ctx: &WorkspaceContext, id: Uuid) -> AppsResult<AppDetail> {
    let row = st
        .store
        .get(ctx.workspace_id, ctx.principal.user_id, id)
        .await?
        .ok_or_else(|| Error::NotFound("app".into()))?;
    Ok(AppDetail::of(row, ctx))
}

fn emit(
    st: &AppsApiState,
    ctx: &WorkspaceContext,
    action: &str,
    id: Uuid,
    detail: serde_json::Value,
) {
    st.audit.emit(mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        action,
        "app",
        Some(id.to_string()),
        detail,
    ));
}

pub(crate) fn etag(revision: i64) -> HeaderValue {
    HeaderValue::from_str(&format!("\"{revision}\"")).expect("digits and quotes are a valid header")
}

/// `If-Match`: a quoted or bare integer revision. Absent → 428; anything else → 400.
pub(crate) fn if_match(headers: &HeaderMap) -> AppsResult<i64> {
    let raw = headers
        .get(IF_MATCH)
        .ok_or_else(|| domain(AppsHttpError::PreconditionRequired))?;
    let bad = || Error::InvalidInput("If-Match must be a draft revision, e.g. \"3\"".into());
    let s = raw.to_str().map_err(|_| bad())?.trim();
    let s = s
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(s);
    Ok(s.parse::<i64>().map_err(|_| bad())?)
}

// ── Handlers ────────────────────────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/apps",
    tag = "apps",
    params(
        WorkspacePath,
        ("trashed" = Option<bool>, Query, description = "List the trash bin instead of live apps when true"),
    ),
    responses(
        (status = 200, description = "Apps in the workspace, most recently updated first", body = Vec<AppSummary>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing app:read, or the apps feature is disabled", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list(
    State(st): State<AppsApiState>,
    WsContext(ctx): WsContext,
    Query(q): Query<ListQuery>,
) -> AppsResult<Json<Vec<AppSummary>>> {
    egret_core::authz::require(&*st.pdp, &ctx, APP_READ).await?;
    let rows = st
        .store
        .list(ctx.workspace_id, ctx.principal.user_id, q.trashed)
        .await?;
    Ok(Json(
        rows.into_iter().map(|r| AppSummary::of(r, &ctx)).collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/apps",
    tag = "apps",
    params(WorkspacePath),
    request_body = AppCreate,
    responses(
        (status = 201, description = "App created with a revision-1 draft from the template", body = AppDetail),
        (status = 400, description = "Empty name", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing app:write, or the apps feature is disabled", body = ErrorBody),
        (status = 404, description = "The folder or database is not in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn create(
    State(st): State<AppsApiState>,
    WsContext(ctx): WsContext,
    Json(b): Json<AppCreate>,
) -> AppsResult<(StatusCode, Json<AppDetail>)> {
    egret_core::authz::require(&*st.pdp, &ctx, APP_WRITE).await?;
    let name = b.name.trim();
    if name.is_empty() {
        return Err(Error::InvalidInput("name must not be empty".into()).into());
    }
    if let Some(fid) = b.folder_id {
        check_folder(&st, &ctx, fid).await?;
    }
    if let Some(database) = b.database.as_deref() {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM query.databases WHERE workspace_id = $1 AND name = $2)",
        )
        .bind(ctx.workspace_id.0)
        .bind(database)
        .fetch_one(&st.db.pool)
        .await
        .map_err(|e| Error::Internal(e.into()))?;
        if !exists {
            return Err(Error::NotFound("database".into()).into());
        }
    }
    let source = new_app_source(b.database.as_deref());
    let row = st
        .store
        .create(
            ctx.workspace_id,
            ctx.principal.user_id,
            name,
            b.description.as_deref().unwrap_or(""),
            b.folder_id,
            &source,
        )
        .await?;
    emit(&st, &ctx, "app:create", row.id, json!({ "name": row.name }));
    Ok((StatusCode::CREATED, Json(AppDetail::of(row, &ctx))))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/apps/{id}",
    tag = "apps",
    params(WorkspacePath, ("id" = Uuid, Path, description = "App id")),
    responses(
        (status = 200, description = "The app (also when trashed; see deleted_at)", body = AppDetail),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing app:read on this app", body = ErrorBody),
        (status = 404, description = "No such app in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn get_one(
    State(st): State<AppsApiState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> AppsResult<Json<AppDetail>> {
    let (obj, row) = app_object_ref(&st, &ctx, id).await?;
    require_object(&*st.pdp, &ctx, APP_READ, obj).await?;
    Ok(Json(AppDetail::of(row, &ctx)))
}

#[utoipa::path(
    patch,
    path = "/api/ws/{workspace}/apps/{id}",
    tag = "apps",
    params(WorkspacePath, ("id" = Uuid, Path, description = "App id")),
    request_body = AppUpdate,
    responses(
        (status = 200, description = "App renamed / re-described", body = AppDetail),
        (status = 400, description = "Empty name", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing app:write on this app", body = ErrorBody),
        (status = 404, description = "No such app in this workspace", body = ErrorBody),
        (status = 409, description = "The app is in the trash", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn update(
    State(st): State<AppsApiState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Json(b): Json<AppUpdate>,
) -> AppsResult<Json<AppDetail>> {
    let (obj, row) = app_object_ref(&st, &ctx, id).await?;
    require_object(&*st.pdp, &ctx, APP_WRITE, obj).await?;
    refuse_if_trashed(&row)?;
    let name = b.name.as_deref().map(str::trim);
    if name.is_some_and(str::is_empty) {
        return Err(Error::InvalidInput("name must not be empty".into()).into());
    }
    st.store
        .update(ctx.workspace_id, id, name, b.description.as_deref())
        .await?;
    emit(
        &st,
        &ctx,
        "app:update",
        id,
        json!({ "name": name, "description_changed": b.description.is_some() }),
    );
    Ok(Json(detail(&st, &ctx, id).await?))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/apps/{id}/trash",
    tag = "apps",
    params(WorkspacePath, ("id" = Uuid, Path, description = "App id")),
    responses(
        (status = 204, description = "App moved to the trash"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing app:write on this app", body = ErrorBody),
        (status = 404, description = "No such app in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn trash(
    State(st): State<AppsApiState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> AppsResult<StatusCode> {
    let (obj, _row) = app_object_ref(&st, &ctx, id).await?;
    require_object(&*st.pdp, &ctx, APP_WRITE, obj).await?;
    st.store.set_trashed(ctx.workspace_id, id, true).await?;
    emit(&st, &ctx, "app:trash", id, json!({}));
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/apps/{id}/restore",
    tag = "apps",
    params(WorkspacePath, ("id" = Uuid, Path, description = "App id")),
    responses(
        (status = 204, description = "App restored from the trash"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing app:write on this app", body = ErrorBody),
        (status = 404, description = "No such app in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn restore(
    State(st): State<AppsApiState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> AppsResult<StatusCode> {
    let (obj, _row) = app_object_ref(&st, &ctx, id).await?;
    require_object(&*st.pdp, &ctx, APP_WRITE, obj).await?;
    st.store.set_trashed(ctx.workspace_id, id, false).await?;
    emit(&st, &ctx, "app:restore", id, json!({}));
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    patch,
    path = "/api/ws/{workspace}/apps/{id}/folder",
    tag = "apps",
    params(WorkspacePath, ("id" = Uuid, Path, description = "App id")),
    request_body = AppSetFolder,
    responses(
        (status = 204, description = "App moved to the folder (or back to Uncategorized)"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing app:write on this app", body = ErrorBody),
        (status = 404, description = "No such app, or the folder is not an app folder of this workspace", body = ErrorBody),
        (status = 409, description = "The app is in the trash", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn set_folder(
    State(st): State<AppsApiState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Json(b): Json<AppSetFolder>,
) -> AppsResult<StatusCode> {
    let (obj, row) = app_object_ref(&st, &ctx, id).await?;
    require_object(&*st.pdp, &ctx, APP_WRITE, obj).await?;
    refuse_if_trashed(&row)?;
    if let Some(fid) = b.folder_id {
        check_folder(&st, &ctx, fid).await?;
    }
    st.store
        .set_folder(ctx.workspace_id, id, b.folder_id)
        .await?;
    emit(
        &st,
        &ctx,
        "app:folder",
        id,
        json!({ "folder_id": b.folder_id }),
    );
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/apps/{id}/bookmark",
    tag = "apps",
    params(WorkspacePath, ("id" = Uuid, Path, description = "App id")),
    responses(
        (status = 204, description = "App bookmarked for the caller"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing app:read on this app", body = ErrorBody),
        (status = 404, description = "No such app in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn bookmark_add(
    State(st): State<AppsApiState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> AppsResult<StatusCode> {
    let (obj, _row) = app_object_ref(&st, &ctx, id).await?;
    require_object(&*st.pdp, &ctx, APP_READ, obj).await?;
    st.store
        .set_bookmark(ctx.principal.user_id, id, true)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/apps/{id}/bookmark",
    tag = "apps",
    params(WorkspacePath, ("id" = Uuid, Path, description = "App id")),
    responses(
        (status = 204, description = "The caller's bookmark removed"),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing app:read on this app", body = ErrorBody),
        (status = 404, description = "No such app in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn bookmark_remove(
    State(st): State<AppsApiState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> AppsResult<StatusCode> {
    let (obj, _row) = app_object_ref(&st, &ctx, id).await?;
    require_object(&*st.pdp, &ctx, APP_READ, obj).await?;
    st.store
        .set_bookmark(ctx.principal.user_id, id, false)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/apps/{id}/draft",
    tag = "apps",
    params(WorkspacePath, ("id" = Uuid, Path, description = "App id")),
    responses(
        (status = 200, description = "The draft; `ETag` is its quoted revision", body = AppDraft,
         headers(("ETag" = String, description = "Quoted draft revision, e.g. \"3\""))),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing app:write on this app", body = ErrorBody),
        (status = 404, description = "No such app in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn draft_get(
    State(st): State<AppsApiState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> AppsResult<Response> {
    let (obj, _row) = app_object_ref(&st, &ctx, id).await?;
    require_object(&*st.pdp, &ctx, APP_WRITE, obj).await?;
    let d = st
        .store
        .draft(id)
        .await?
        .ok_or_else(|| Error::NotFound("draft".into()))?;
    Ok((
        [(ETAG, etag(d.revision))],
        Json(AppDraft {
            source: d.source,
            layout: d.layout_json.and_then(|s| serde_json::from_str(&s).ok()),
            revision: d.revision,
        }),
    )
        .into_response())
}

#[utoipa::path(
    put,
    path = "/api/ws/{workspace}/apps/{id}/draft",
    tag = "apps",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "App id"),
        ("If-Match" = String, Header, description = "The revision the edit is based on, quoted or bare"),
    ),
    request_body = AppSaveDraft,
    responses(
        (status = 200, description = "Saved; `ETag` is the new quoted revision", body = AppDraftSaved,
         headers(("ETag" = String, description = "Quoted new draft revision"))),
        (status = 400, description = "If-Match is not a revision", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing app:write on this app", body = ErrorBody),
        (status = 404, description = "No such app in this workspace", body = ErrorBody),
        (status = 409, description = "Stale revision (reload current_revision), or the app is in the trash", body = AppConflict),
        (status = 413, description = "Source exceeds 1 MiB", body = ErrorBody),
        (status = 428, description = "If-Match is missing", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn draft_put(
    State(st): State<AppsApiState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    headers: HeaderMap,
    Json(b): Json<AppSaveDraft>,
) -> AppsResult<Response> {
    let (obj, row) = app_object_ref(&st, &ctx, id).await?;
    require_object(&*st.pdp, &ctx, APP_WRITE, obj).await?;
    refuse_if_trashed(&row)?;
    let expected = if_match(&headers)?;
    if b.source.len() > MAX_SOURCE_BYTES {
        return Err(domain(AppsHttpError::SourceTooLarge));
    }
    let layout_json = b.layout.as_ref().map(|v| v.to_string());
    // Deliberately not audited: drafts save on every edit; publish is the event.
    match st
        .store
        .save_draft(
            id,
            ctx.principal.user_id,
            expected,
            &b.source,
            layout_json.as_deref(),
        )
        .await?
    {
        SaveOutcome::Saved { revision } => {
            Ok(([(ETAG, etag(revision))], Json(AppDraftSaved { revision })).into_response())
        }
        SaveOutcome::Stale { current_revision } => {
            Err(domain(AppsHttpError::StaleRevision { current_revision }))
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/apps/{id}/ticket",
    tag = "apps",
    params(WorkspacePath, ("id" = Uuid, Path, description = "App id")),
    request_body = AppTicketRequest,
    responses(
        (status = 200, description = "A one-time URL for the apps-origin iframe", body = AppTicket),
        (status = 400, description = "An edit ticket cannot name a version", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing app:read (run) or app:write + query:run (edit)", body = ErrorBody),
        (status = 404, description = "No such app or version in this workspace", body = ErrorBody),
        (status = 409, description = "The app is in the trash, or (run) has no published version", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
        (status = 503, description = "The apps origin is not configured", body = ErrorBody),
    ),
)]
async fn ticket(
    State(st): State<AppsApiState>,
    WsContext(ctx): WsContext,
    jar: CookieJar,
    Path((_ws, id)): Path<(String, Uuid)>,
    Json(b): Json<AppTicketRequest>,
) -> AppsResult<Json<AppTicket>> {
    if st.apps_origin.is_empty() {
        return Err(domain(AppsHttpError::OriginNotConfigured));
    }
    let (obj, row) = app_object_ref(&st, &ctx, id).await?;
    refuse_if_trashed(&row)?;
    let (mode, version_id) = match b.mode {
        AppTicketMode::Edit => {
            if b.version_id.is_some() {
                return Err(Error::InvalidInput(
                    "an edit ticket opens the draft; it cannot name a version".into(),
                )
                .into());
            }
            require_object(&*st.pdp, &ctx, APP_WRITE, obj).await?;
            egret_core::authz::require(&*st.pdp, &ctx, QUERY_RUN).await?;
            (AppMode::Edit, None)
        }
        AppTicketMode::Run => {
            require_object(&*st.pdp, &ctx, APP_READ, obj).await?;
            let version = match b.version_id {
                // `version()` is scoped to the app by its own query, so a version of
                // another app is simply absent.
                Some(v) => {
                    st.store
                        .version(id, v)
                        .await?
                        .ok_or_else(|| Error::NotFound("version".into()))?
                        .summary
                        .id
                }
                None => st
                    .store
                    .versions(id)
                    .await?
                    .first()
                    .map(|v| v.id)
                    .ok_or_else(|| domain(AppsHttpError::NoPublishedVersion))?,
            };
            (AppMode::Run, Some(version))
        }
    };
    // The grant hangs off THIS login session, so logging out (or back-channel logout)
    // removes it without anything having to remember to.
    let login_sid = match read_session(&jar, secure_cookies()) {
        Some(c) => st.sessions.session_id_for_token(c.value()).await?,
        None => None,
    }
    .ok_or(Error::Unauthorized)?;
    let issued = st
        .grants
        .issue_ticket(
            &NewGrant {
                user_id: ctx.principal.user_id,
                workspace_id: ctx.workspace_id,
                app_id: id,
                version_id,
                mode,
                login_sid,
                impersonator: ctx.principal.impersonator.as_ref().map(|i| i.user_id),
            },
            st.ticket_ttl,
        )
        .await?;
    emit(
        &st,
        &ctx,
        "app:open",
        id,
        json!({ "mode": mode.as_str(), "version_id": version_id }),
    );
    Ok(Json(AppTicket {
        url: format!(
            "{}/_ticket?t={issued}",
            st.apps_origin.trim_end_matches('/')
        ),
    }))
}

/// The publish body is optional: an empty body or JSON `null` publishes whatever
/// the draft is now. Parsed by hand rather than with `Option<Json<_>>` because the
/// generated client sends `Content-Type: application/json` even when it has no
/// body (and `null` when given one), which axum's optional `Json` rejects.
fn publish_body(body: &[u8]) -> AppsResult<AppPublish> {
    let trimmed = body.trim_ascii();
    if trimmed.is_empty() || trimmed == b"null" {
        return Ok(AppPublish::default());
    }
    serde_json::from_slice(trimmed)
        .map_err(|e| Error::InvalidInput(format!("invalid publish body: {e}")).into())
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/apps/{id}/publish",
    tag = "apps",
    params(WorkspacePath, ("id" = Uuid, Path, description = "App id")),
    request_body(content = Option<AppPublish>, description = "Optional: a bare POST (no body) publishes the current draft"),
    responses(
        (status = 201, description = "The draft published as a new version", body = AppVersionSummary),
        (status = 400, description = "The body is not an AppPublish", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing app:write on this app", body = ErrorBody),
        (status = 404, description = "No such app in this workspace", body = ErrorBody),
        (status = 409, description = "expected_revision is stale (reload current_revision), or the app is in the trash", body = AppConflict),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn publish(
    State(st): State<AppsApiState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    body: Bytes,
) -> AppsResult<(StatusCode, Json<AppVersionSummary>)> {
    let b = publish_body(&body)?;
    let (obj, row) = app_object_ref(&st, &ctx, id).await?;
    require_object(&*st.pdp, &ctx, APP_WRITE, obj).await?;
    refuse_if_trashed(&row)?;
    match st
        .store
        .publish(id, ctx.principal.user_id, b.expected_revision)
        .await?
    {
        PublishOutcome::Published(v) => {
            emit(
                &st,
                &ctx,
                "app:publish",
                id,
                json!({ "version_no": v.version_no, "from_revision": v.from_revision }),
            );
            Ok((StatusCode::CREATED, Json(v.into())))
        }
        PublishOutcome::Stale { current_revision } => {
            Err(domain(AppsHttpError::StaleRevision { current_revision }))
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/apps/{id}/versions",
    tag = "apps",
    params(WorkspacePath, ("id" = Uuid, Path, description = "App id")),
    responses(
        (status = 200, description = "Published versions, newest first", body = Vec<AppVersionSummary>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing app:write on this app", body = ErrorBody),
        (status = 404, description = "No such app in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn versions(
    State(st): State<AppsApiState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> AppsResult<Json<Vec<AppVersionSummary>>> {
    let (obj, _row) = app_object_ref(&st, &ctx, id).await?;
    require_object(&*st.pdp, &ctx, APP_WRITE, obj).await?;
    let vs = st.store.versions(id).await?;
    Ok(Json(vs.into_iter().map(Into::into).collect()))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/apps/{id}/versions/{version_id}",
    tag = "apps",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "App id"),
        ("version_id" = Uuid, Path, description = "Version id"),
    ),
    responses(
        (status = 200, description = "The version with its source", body = AppVersion),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing app:write on this app", body = ErrorBody),
        (status = 404, description = "No such app in this workspace, or no such version of it", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn version_get(
    State(st): State<AppsApiState>,
    WsContext(ctx): WsContext,
    Path((_ws, id, version_id)): Path<(String, Uuid, Uuid)>,
) -> AppsResult<Json<AppVersion>> {
    let (obj, _row) = app_object_ref(&st, &ctx, id).await?;
    require_object(&*st.pdp, &ctx, APP_WRITE, obj).await?;
    let v = st
        .store
        .version(id, version_id)
        .await?
        .ok_or_else(|| Error::NotFound("version".into()))?;
    Ok(Json(AppVersion {
        summary: v.summary.into(),
        source: v.source,
    }))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/apps/{id}/versions/{version_id}/restore",
    tag = "apps",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "App id"),
        ("version_id" = Uuid, Path, description = "Version to republish"),
    ),
    responses(
        (status = 201, description = "The version's source republished as a new version; the draft is untouched", body = AppVersionSummary),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing app:write on this app", body = ErrorBody),
        (status = 404, description = "No such app in this workspace, or no such version of it", body = ErrorBody),
        (status = 409, description = "The app is in the trash", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn version_restore(
    State(st): State<AppsApiState>,
    WsContext(ctx): WsContext,
    Path((_ws, id, version_id)): Path<(String, Uuid, Uuid)>,
) -> AppsResult<(StatusCode, Json<AppVersionSummary>)> {
    let (obj, row) = app_object_ref(&st, &ctx, id).await?;
    require_object(&*st.pdp, &ctx, APP_WRITE, obj).await?;
    refuse_if_trashed(&row)?;
    let v = st
        .store
        .restore_version(id, version_id, ctx.principal.user_id)
        .await?
        .ok_or_else(|| Error::NotFound("version".into()))?;
    emit(
        &st,
        &ctx,
        "app:version_restore",
        id,
        json!({
            "version_no": v.version_no,
            "from_revision": v.from_revision,
            "restored_from": v.restored_from,
        }),
    );
    Ok((StatusCode::CREATED, Json(v.into())))
}

// ── OpenAPI fragment ────────────────────────────────────────────────────────

/// OpenAPI fragment for `/api/ws/{workspace}/apps…`. Component schemas carry an
/// `App` prefix because component names are global once the fragments merge.
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        list,
        create,
        get_one,
        update,
        trash,
        restore,
        set_folder,
        bookmark_add,
        bookmark_remove,
        draft_get,
        draft_put,
        publish,
        ticket,
        versions,
        version_get,
        version_restore,
    ),
    components(schemas(
        AppSummary,
        AppDetail,
        AppCreate,
        AppUpdate,
        AppSetFolder,
        AppDraft,
        AppSaveDraft,
        AppDraftSaved,
        AppPublish,
        AppTicketRequest,
        AppTicketMode,
        AppTicket,
        AppStaleRevision,
        AppConflict,
        AppVersionSummary,
        AppVersion,
        ErrorBody,
    )),
    tags((name = "apps", description = "Per-workspace data apps: CRUD, folders, trash, bookmarks, revisioned drafts, publish, versions")),
)]
pub struct AppsApi;
