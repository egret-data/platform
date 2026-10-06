//! The namespace file API (W9b spec §9), mirroring Kestra's
//! `NamespaceFileController`:
//!
//! ```text
//! GET    /api/ws/{workspace}/workflows/namespaces/{namespace}/files?path&revision   workflow:read
//! GET    /api/ws/{workspace}/workflows/namespaces/{namespace}/files/stats?path      workflow:read
//! GET    /api/ws/{workspace}/workflows/namespaces/{namespace}/files/revisions?path  workflow:read
//! GET    /api/ws/{workspace}/workflows/namespaces/{namespace}/files/search?q        workflow:read
//! GET    /api/ws/{workspace}/workflows/namespaces/{namespace}/files/directory?path  workflow:read
//! POST   /api/ws/{workspace}/workflows/namespaces/{namespace}/files?path            workflow:manage (multipart)
//! POST   /api/ws/{workspace}/workflows/namespaces/{namespace}/files/directory?path  workflow:manage
//! PUT    /api/ws/{workspace}/workflows/namespaces/{namespace}/files?from&to         workflow:manage
//! DELETE /api/ws/{workspace}/workflows/namespaces/{namespace}/files?path            workflow:manage
//! ```
//!
//! # Who may do what
//!
//! Every handler runs, in this order: `WsContext` (the session's membership
//! of `{workspace}`), the route's permission through the PDP (`workflow:read`
//! for a GET, `workflow:manage` for a write), and only then
//! [`authorize`]`(workspace, Actor::Api, namespace, access)` -- the one access
//! function, which checks the namespace (`is_valid_namespace`, no empty
//! segment, no `_files` segment: 400) and hands back the [`Grant`] every
//! store function takes. The workspace is `WsContext`'s, never anything in a
//! query, so another workspace's namespace is unreachable by construction:
//! every statement is scoped by the grant's workspace.
//!
//! # Errors
//!
//! [`NsError`] maps to one status for every route (spec §9's table):
//! `Author` 400, `NotFound` 404, `Conflict` 409, `TooLarge` 413, `Store` and
//! `Db` 503 (`Db` logged, its message never sent). The exceptions are each
//! the fixture's: a `DELETE` of a missing path is 200 and does nothing; a
//! wrong-kind request (a download or `revisions` of a directory, `directory`
//! of a file) is 400 naming the path. No message names a storage key.
//!
//! # Writes answer 200 with no body, as Kestra's
//!
//! (w9b `api.summary.upload_answer`, `api.routes.post_directory`, `move`,
//! `delete_missing`.) Each emits one audit mutation,
//! `workflow_namespace_file:{upload,create_directory,move,delete}`.

// Handlers answer `Result<_, Response>`, as W9a's `storage::routes` and the
// start route do: the `Err` is the response itself, built once per request.
#![allow(clippy::result_large_err)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse as _, Response};
use axum::Json;
use egret_core::{Error, WorkspaceContext, WorkspacePath};
use egret_workspace::context::WsContext;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, ReadBuf};

use super::access::{authorize, Access, Actor, Grant};
use super::path::NsPath;
use super::store::{self, Conflict, NsEntry, Want, Written};
use super::{CreatedBy, NsError};
use crate::routes::{refuse, WorkflowState, WF_MANAGE, WF_READ};
use crate::storage::multipart::{
    multipart_body_limit, multipart_refusal, HeaderBudget, IdleTimeout, MultipartGate,
    UPLOAD_IDLE_TIMEOUT,
};
use crate::storage::routes::{download_headers, NamedErrors};
use crate::storage::{Caps, FileStore};

/// The multipart part that carries the file: Kestra's name (w9b
/// `api.summary.multipart_part_name`; a part named otherwise is refused).
pub const FILE_PART: &str = "fileContent";

/// The audit resource type of every write here.
const AUDIT_RESOURCE: &str = "workflow_namespace_file";

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct NsFileStatsDto {
    /// The stored path: a leading `/`; a directory's ends with `/`.
    pub path: String,
    /// `File` or `Directory`.
    pub r#type: String,
    /// Bytes; 0 for a directory.
    pub size: u64,
    /// The latest revision; always 1 for a directory.
    pub revision: u32,
    /// When the latest revision was written; `null` only for the namespace's
    /// virtual root (`stats` with no `path`), which has no row.
    pub created: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct NsFileRevisionDto {
    pub revision: u32,
    /// The path was soft-deleted at this revision (its latest); its bytes are
    /// still served by `GET …/files?path&revision`.
    pub deleted: bool,
    pub size: u64,
    pub created: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct NsPathQuery {
    /// The file's path in the namespace (a leading `/` is added when missing).
    pub path: String,
    /// A given revision, served even when the path is soft-deleted; none = the
    /// latest, which must be live.
    pub revision: Option<u32>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct NsTargetQuery {
    /// The path in the namespace (a leading `/` is added when missing).
    pub path: String,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct NsDirQuery {
    /// A path in the namespace; none (or `/`) = the namespace's root.
    pub path: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct NsMoveQuery {
    /// The file or directory to move; it must exist.
    pub from: String,
    /// Where it goes; nothing may be there, not even a deleted path's history.
    pub to: String,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct NsSearchQuery {
    /// A case-insensitive substring of a file's path; `""` finds nothing.
    pub q: String,
}

/// `POST …/files` as `multipart/form-data`: one part, `fileContent`.
pub struct NsFileUploadForm;

impl utoipa::PartialSchema for NsFileUploadForm {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        use utoipa::openapi::schema::{KnownFormat, ObjectBuilder, SchemaFormat, Type};
        ObjectBuilder::new()
            .description(Some(
                "One part, `fileContent`: the file's bytes, streamed to the namespace file at \
                 `path` as its next revision. A `.zip` is stored as one file, never expanded.",
            ))
            .property(
                FILE_PART,
                ObjectBuilder::new()
                    .schema_type(Type::String)
                    .format(Some(SchemaFormat::KnownFormat(KnownFormat::Binary))),
            )
            .required(FILE_PART)
            .into()
    }
}

impl utoipa::ToSchema for NsFileUploadForm {}

// ─── Mapping ────────────────────────────────────────────────────────────────

fn core_refusal(e: Error) -> Response {
    e.into_response()
}

fn bad_request(message: String) -> Response {
    core_refusal(Error::InvalidInput(message))
}

/// Spec §9's table.
fn ns_refusal(e: NsError) -> Response {
    match e {
        NsError::Author(m) => bad_request(m),
        NsError::NotFound(m) => core_refusal(Error::NotFound(m)),
        NsError::Conflict(m) => core_refusal(Error::Conflict(m)),
        NsError::TooLarge(m) => refuse(StatusCode::PAYLOAD_TOO_LARGE, m),
        NsError::Store(m) => refuse(StatusCode::SERVICE_UNAVAILABLE, m),
        NsError::Db(e) => {
            tracing::error!(error = %e, "a namespace file route failed in the database");
            refuse(
                StatusCode::SERVICE_UNAVAILABLE,
                "the namespace file index is unavailable; retry later".into(),
            )
        }
    }
}

fn db_unavailable(e: sqlx::Error) -> Response {
    ns_refusal(NsError::Db(Error::Internal(anyhow::Error::new(e))))
}

/// The route's permission, then [`authorize`] -- in that order, so a caller
/// without the permission learns nothing about the namespace.
async fn grant(
    st: &WorkflowState,
    ctx: &WorkspaceContext,
    namespace: &str,
    access: Access,
) -> Result<Grant, Response> {
    let permission = match access {
        Access::Read => WF_READ,
        Access::Write | Access::Purge => WF_MANAGE,
    };
    egret_core::authz::require(&*st.pdp, ctx, permission)
        .await
        .map_err(core_refusal)?;
    authorize(ctx.workspace_id.0, Actor::Api, namespace, access).map_err(bad_request)
}

fn file_path(raw: &str) -> Result<NsPath, Response> {
    NsPath::parse_file(raw).map_err(bad_request)
}

fn no_store() -> Response {
    refuse(
        StatusCode::SERVICE_UNAVAILABLE,
        "egret has no internal storage configured (the `[storage]` section of its \
         configuration), so no namespace file can be stored or read"
            .into(),
    )
}

fn caps() -> Caps {
    crate::storage::current()
        .map(|fs| fs.caps)
        .unwrap_or_default()
}

async fn conn(st: &WorkflowState) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>, Response> {
    st.db.pool.acquire().await.map_err(db_unavailable)
}

fn stats_dto(e: &NsEntry) -> NsFileStatsDto {
    NsFileStatsDto {
        path: e.path.as_str().to_string(),
        r#type: if e.is_directory { "Directory" } else { "File" }.to_string(),
        size: e.size,
        revision: e.revision,
        created: Some(e.created_at),
    }
}

/// `None`, `""` and `"/"` all name the namespace's root.
fn is_root(raw: Option<&str>) -> bool {
    raw.is_none_or(|p| p.is_empty() || p == "/")
}

/// The latest LIVE row at `raw`, a file or a directory, whichever is there
/// (a path names at most one live entry: a file and a directory never share
/// one, `store::put`'s kind rule). A path that parses as neither is the
/// file rule's 400; one that names nothing is the file's 404.
async fn resolve_any(
    conn: &mut sqlx::PgConnection,
    grant: &Grant,
    raw: &str,
) -> Result<NsEntry, NsError> {
    match (NsPath::parse_file(raw), NsPath::parse_dir(raw)) {
        (Err(e), Err(_)) => Err(NsError::Author(e)),
        (Ok(f), dir) => match store::resolve(conn, grant, &f, Want::Latest).await {
            Err(NsError::NotFound(missing)) => match dir {
                Ok(d) => match store::resolve(conn, grant, &d, Want::Latest).await {
                    Err(NsError::NotFound(_)) => Err(NsError::NotFound(missing)),
                    other => other,
                },
                Err(_) => Err(NsError::NotFound(missing)),
            },
            other => other,
        },
        // Only a directory's form parses: `/a/`, or `/x.v2` (a name a FILE
        // may not have). Missing, the file rule's refusal says why.
        (Err(file_rule), Ok(d)) => match store::resolve(conn, grant, &d, Want::Latest).await {
            Err(NsError::NotFound(m)) if raw.ends_with('/') => Err(NsError::NotFound(m)),
            Err(NsError::NotFound(_)) => Err(NsError::Author(file_rule)),
            other => other,
        },
    }
}

/// Does `raw`, read as a directory, have any row (live or soft-deleted)? The
/// wrong-kind check of a download and of `revisions` (w9b
/// `api.routes.get_file_directory`, `revisions_dir`).
async fn holds_a_directory(
    conn: &mut sqlx::PgConnection,
    grant: &Grant,
    raw: &str,
) -> Result<bool, NsError> {
    let Ok(dir) = NsPath::parse_dir(raw) else {
        return Ok(false);
    };
    match store::revisions(conn, grant, &dir).await {
        Ok(rows) => Ok(!rows.is_empty()),
        Err(NsError::NotFound(_)) => Ok(false),
        Err(e) => Err(e),
    }
}

/// A file route's refusal `otherwise`, unless `raw` is a directory: then the
/// wrong-kind 400 naming it, saying `what` (application of R3, 2026-10-05).
async fn unless_a_directory(
    conn: &mut sqlx::PgConnection,
    grant: &Grant,
    raw: &str,
    what: &str,
    otherwise: NsError,
) -> Response {
    match holds_a_directory(conn, grant, raw).await {
        Ok(true) => bad_request(format!(
            "'{raw}' is a directory in namespace '{}': {what}",
            grant.namespace()
        )),
        Ok(false) => ns_refusal(otherwise),
        Err(e) => ns_refusal(e),
    }
}

fn audit(
    st: &WorkflowState,
    ctx: &WorkspaceContext,
    action: &str,
    grant: &Grant,
    path: &str,
    detail: serde_json::Value,
) {
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        action,
        AUDIT_RESOURCE,
        Some(format!("{}:{path}", grant.namespace())),
        detail,
    ));
}

// ─── Reads ──────────────────────────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/workflows/namespaces/{namespace}/files",
    tag = "workflow",
    params(
        WorkspacePath,
        ("namespace" = String, Path, description = "The namespace; one with a `_files` segment, an empty segment or an invalid name is refused (400). A namespace with no flow can hold files."),
        NsPathQuery,
    ),
    responses(
        (status = 200, description = "The file's bytes, streamed. `Content-Type: application/octet-stream`, `Content-Disposition: attachment` with the file's name (sanitised, and percent-encoded in `filename*`), `X-Content-Type-Options: nosniff`.", content_type = "application/octet-stream", body = Vec<u8>),
        (status = 400, description = "A path rule (named), a refused namespace, `/_flows`, or the path is a directory (named): a directory has no contents. Also a revision whose object is gone from internal storage (named; egret never falls back to another revision).", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:read permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such file in this workspace's namespace, no such revision of it, or (with no `revision`) the path is soft-deleted. A given revision of a soft-deleted path IS served.", body = egret_core::http::ErrorBody),
        (status = 503, description = "Internal storage or the database is unavailable or not configured; the request can be retried.", body = egret_core::http::ErrorBody),
    ),
)]
pub(crate) async fn download_namespace_file(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, namespace)): Path<(String, String)>,
    Query(q): Query<NsPathQuery>,
) -> Result<Response, Response> {
    let grant = grant(&st, &ctx, &namespace, Access::Read).await?;
    let want = q.revision.map_or(Want::Latest, Want::Revision);
    let entry = {
        let mut c = conn(&st).await?;
        const WHAT: &str = "it has no contents to download";
        let path = match NsPath::parse_file(&q.path) {
            Ok(p) => p,
            Err(rule) => {
                let e = NsError::Author(rule);
                return Err(unless_a_directory(&mut c, &grant, &q.path, WHAT, e).await);
            }
        };
        match store::resolve(&mut c, &grant, &path, want).await {
            Ok(e) => e,
            Err(e @ NsError::NotFound(_)) => {
                return Err(unless_a_directory(&mut c, &grant, &q.path, WHAT, e).await)
            }
            Err(e) => return Err(ns_refusal(e)),
        }
        // The connection goes back to the pool here, before the body streams.
    };
    let fs = crate::storage::current().ok_or_else(no_store)?;
    let reader = store::open(&fs, &grant, &entry).await.map_err(ns_refusal)?;
    let named = format!(
        "revision {} of namespace file '{}' in namespace '{}'",
        entry.revision,
        entry.path,
        grant.namespace()
    );
    let body = axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(NamedErrors {
        inner: reader,
        uri: Arc::from(named.as_str()),
    }));
    let mut res = Response::new(body);
    let name = entry.path.as_str().rsplit('/').next().unwrap_or("file");
    download_headers(&mut res, name)?;
    Ok(res)
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/workflows/namespaces/{namespace}/files/stats",
    tag = "workflow",
    params(
        WorkspacePath,
        ("namespace" = String, Path, description = "The namespace"),
        NsDirQuery,
    ),
    responses(
        (status = 200, description = "The live file or directory at `path`. With no `path` (or `/`): the namespace's virtual root, `{path: \"/\", type: \"Directory\", size: 0, revision: 1, created: null}` -- nothing is created, in any namespace, with or without files.", body = NsFileStatsDto),
        (status = 400, description = "A path rule (named), a refused namespace, or `/_flows`", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:read permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "Nothing live at `path` in this workspace's namespace", body = egret_core::http::ErrorBody),
        (status = 503, description = "The database is unavailable; the request can be retried.", body = egret_core::http::ErrorBody),
    ),
)]
pub(crate) async fn namespace_file_stats(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, namespace)): Path<(String, String)>,
    Query(q): Query<NsDirQuery>,
) -> Result<Json<NsFileStatsDto>, Response> {
    let grant = grant(&st, &ctx, &namespace, Access::Read).await?;
    let Some(raw) = q.path.as_deref().filter(|_| !is_root(q.path.as_deref())) else {
        // R3: a virtual root, never a row.
        return Ok(Json(NsFileStatsDto {
            path: "/".into(),
            r#type: "Directory".into(),
            size: 0,
            revision: 1,
            created: None,
        }));
    };
    let mut c = conn(&st).await?;
    let e = resolve_any(&mut c, &grant, raw).await.map_err(ns_refusal)?;
    Ok(Json(stats_dto(&e)))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/workflows/namespaces/{namespace}/files/revisions",
    tag = "workflow",
    params(
        WorkspacePath,
        ("namespace" = String, Path, description = "The namespace"),
        NsTargetQuery,
    ),
    responses(
        (status = 200, description = "Every revision of the file, ascending, a soft-deleted path's included, each flagged `deleted` when the path was deleted at it.", body = Vec<NsFileRevisionDto>),
        (status = 400, description = "A path rule (named), a refused namespace, `/_flows`, or the path is a directory (named): a directory has no revisions.", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:read permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "The path has never held a file in this workspace's namespace (or its history was purged)", body = egret_core::http::ErrorBody),
        (status = 503, description = "The database is unavailable; the request can be retried.", body = egret_core::http::ErrorBody),
    ),
)]
pub(crate) async fn namespace_file_revisions(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, namespace)): Path<(String, String)>,
    Query(q): Query<NsTargetQuery>,
) -> Result<Json<Vec<NsFileRevisionDto>>, Response> {
    let grant = grant(&st, &ctx, &namespace, Access::Read).await?;
    let mut c = conn(&st).await?;
    const WHAT: &str = "a directory has no revisions";
    let path = match NsPath::parse_file(&q.path) {
        Ok(p) => p,
        Err(rule) => {
            let e = NsError::Author(rule);
            return Err(unless_a_directory(&mut c, &grant, &q.path, WHAT, e).await);
        }
    };
    let rows = match store::revisions(&mut c, &grant, &path).await {
        Ok(rows) => rows,
        Err(e @ NsError::NotFound(_)) => {
            return Err(unless_a_directory(&mut c, &grant, &q.path, WHAT, e).await)
        }
        Err(e) => return Err(ns_refusal(e)),
    };
    Ok(Json(
        rows.iter()
            .map(|e| NsFileRevisionDto {
                revision: e.revision,
                deleted: e.deleted,
                size: e.size,
                created: e.created_at,
            })
            .collect(),
    ))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/workflows/namespaces/{namespace}/files/search",
    tag = "workflow",
    params(
        WorkspacePath,
        ("namespace" = String, Path, description = "The namespace"),
        NsSearchQuery,
    ),
    responses(
        (status = 200, description = "The paths of the live FILES whose path contains `q`, case-insensitively (Kestra's rule), in byte order; directories are never listed; `q=\"\"` finds nothing.", body = Vec<String>),
        (status = 400, description = "No `q`, or a refused namespace", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:read permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 413, description = "More files match than one answer may hold (`workflow.storage.max_namespace_files`, named); narrow `q`.", body = egret_core::http::ErrorBody),
        (status = 503, description = "The database is unavailable; the request can be retried.", body = egret_core::http::ErrorBody),
    ),
)]
pub(crate) async fn search_namespace_files(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, namespace)): Path<(String, String)>,
    Query(q): Query<NsSearchQuery>,
) -> Result<Json<Vec<String>>, Response> {
    let grant = grant(&st, &ctx, &namespace, Access::Read).await?;
    let mut c = conn(&st).await?;
    let found = store::search(&mut c, &grant, &q.q, caps().max_namespace_files)
        .await
        .map_err(ns_refusal)?;
    Ok(Json(
        found
            .into_iter()
            .map(|e| e.path.as_str().to_string())
            .collect(),
    ))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/workflows/namespaces/{namespace}/files/directory",
    tag = "workflow",
    params(
        WorkspacePath,
        ("namespace" = String, Path, description = "The namespace"),
        NsDirQuery,
    ),
    responses(
        (status = 200, description = "The live children (files and directories) directly under the directory, in byte order; with no `path`, the namespace root's (an empty list for a namespace with no file).", body = Vec<NsFileStatsDto>),
        (status = 400, description = "A path rule (named), a refused namespace, `/_flows`, or the path is a FILE (named): only a directory has children.", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:read permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "No live directory at `path` in this workspace's namespace", body = egret_core::http::ErrorBody),
        (status = 413, description = "The directory holds more entries than one answer may hold (`workflow.storage.max_namespace_files`, named).", body = egret_core::http::ErrorBody),
        (status = 503, description = "The database is unavailable; the request can be retried.", body = egret_core::http::ErrorBody),
    ),
)]
pub(crate) async fn list_namespace_directory(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, namespace)): Path<(String, String)>,
    Query(q): Query<NsDirQuery>,
) -> Result<Json<Vec<NsFileStatsDto>>, Response> {
    let grant = grant(&st, &ctx, &namespace, Access::Read).await?;
    let cap = caps().max_namespace_files;
    let mut c = conn(&st).await?;
    let dir = match q.path.as_deref() {
        raw if is_root(raw) => None,
        Some(raw) => {
            let entry = resolve_any(&mut c, &grant, raw)
                .await
                .map_err(|e| match e {
                    NsError::NotFound(_) => core_refusal(Error::NotFound(format!(
                        "directory '{raw}' does not exist in namespace '{}'",
                        grant.namespace()
                    ))),
                    other => ns_refusal(other),
                })?;
            if !entry.is_directory {
                // R3: Kestra answers `[]`; a file has no children to list.
                return Err(bad_request(format!(
                    "'{raw}' is a file in namespace '{}', not a directory: it has no children \
                     to list",
                    grant.namespace()
                )));
            }
            Some(entry.path)
        }
        None => None,
    };
    let children = store::list_live(&mut c, &grant, dir.as_ref(), false, cap)
        .await
        .map_err(ns_refusal)?;
    Ok(Json(children.iter().map(stats_dto).collect()))
}

// ─── Writes ─────────────────────────────────────────────────────────────────

/// The first read error of the upload body, kept so the route can answer
/// it as the REQUEST's (400, 408, 413) and not as the store's 503: `put`
/// reports a body that fails to read as `NsError::Store`.
struct BodyErrors<R> {
    inner: R,
    first: Arc<Mutex<Option<String>>>,
}

impl<R: AsyncRead + Unpin> AsyncRead for BodyErrors<R> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let r = std::pin::Pin::new(&mut self.inner).poll_read(cx, buf);
        if let std::task::Poll::Ready(Err(e)) = &r {
            let mut first = self.first.lock().expect("body error slot");
            first.get_or_insert_with(|| e.to_string());
        }
        r
    }
}

/// A body over the route's limit -- the REQUEST's refusal, naming the limit.
fn upload_too_large(limit: usize) -> Response {
    refuse(
        StatusCode::PAYLOAD_TOO_LARGE,
        format!(
            "the request body is over {limit} bytes: one upload carries at most \
             workflow.storage.max_file_bytes + 1 MiB in total"
        ),
    )
}

fn upload_timed_out() -> Response {
    refuse(
        StatusCode::REQUEST_TIMEOUT,
        format!(
            "the upload stopped: no byte of the request body arrived for {} seconds; nothing \
             was stored",
            UPLOAD_IDLE_TIMEOUT.as_secs()
        ),
    )
}

/// A multipart failure, read through what the gates saw first: the idle
/// timeout (408), the body limit (413, this route's message), then W9a's
/// [`multipart_refusal`] (the header budget, the parser's own status).
fn body_refusal(
    idle: &AtomicBool,
    gate: &MultipartGate,
    limit: usize,
    status: StatusCode,
    text: String,
) -> Response {
    if idle.load(Ordering::SeqCst) {
        return upload_timed_out();
    }
    if gate.body_limit.load(Ordering::SeqCst) {
        return upload_too_large(limit);
    }
    multipart_refusal(gate, limit, status, text)
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/workflows/namespaces/{namespace}/files",
    tag = "workflow",
    params(
        WorkspacePath,
        ("namespace" = String, Path, description = "The namespace; it needs no flow"),
        NsTargetQuery,
    ),
    request_body(
        description = "`multipart/form-data` with one part, `fileContent` (at most `workflow.storage.max_file_bytes`; the body at most that + 1 MiB, each part's headers at most 64 KiB). The bytes become the next revision of `path` (a new path's revision 1; a soft-deleted path's next revision, live again). A `.zip` is stored as one file, never expanded; `/_flows` is refused (flows are published through the flow API). A body that stops arriving for 30 seconds ends the upload (408). **Build the body as a `FormData`** and let the browser set `multipart/form-data; boundary=…`.",
        content((NsFileUploadForm = "multipart/form-data")),
    ),
    responses(
        (status = 200, description = "Stored, as the path's next revision. No body (Kestra's answer).", body = ()),
        (status = 400, description = "A path rule (named), a refused namespace, `/_flows`, no `fileContent` part or another part before it (named), a malformed multipart body, or headers over the budget", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace. **workflow:read is not enough.**", body = egret_core::http::ErrorBody),
        (status = 408, description = "No byte of the body arrived for 30 seconds; nothing was stored and the path's lock was released.", body = egret_core::http::ErrorBody),
        (status = 409, description = "A directory is at `path`, a file is where one of its parent directories goes, or a soft-deleted entry of the other kind still holds history there (named; only a flow's `PurgeFiles` task removes history -- the API has no purge route)", body = egret_core::http::ErrorBody),
        (status = 413, description = "The file is over `workflow.storage.max_file_bytes`, or the body over its limit (named)", body = egret_core::http::ErrorBody),
        (status = 415, description = "Not `multipart/form-data`", body = egret_core::http::ErrorBody),
        (status = 503, description = "Internal storage or the database is unavailable or not configured; nothing was stored and the request can be retried.", body = egret_core::http::ErrorBody),
    ),
)]
pub(crate) async fn upload_namespace_file(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, namespace)): Path<(String, String)>,
    Query(q): Query<NsTargetQuery>,
    req: axum::extract::Request,
) -> Result<StatusCode, Response> {
    use axum::extract::FromRequest as _;
    use futures_util::StreamExt as _;
    // Everything that can refuse without the body, before a byte of it is read.
    let grant = grant(&st, &ctx, &namespace, Access::Write).await?;
    let path = file_path(&q.path)?;
    if !crate::storage::multipart::is_multipart(req.headers()) {
        return Err(refuse(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "a namespace file is uploaded as multipart/form-data, with one part `fileContent`"
                .into(),
        ));
    }
    let fs: Arc<FileStore> = crate::storage::current().ok_or_else(no_store)?;
    let limit = multipart_body_limit(fs.caps);
    let gate = Arc::new(MultipartGate::default());
    let idle = Arc::new(AtomicBool::new(false));
    let req = req.map(|b| {
        let limited = axum::body::Body::new(http_body_util::Limited::new(b, limit));
        axum::body::Body::from_stream(HeaderBudget {
            inner: Box::pin(IdleTimeout::new(
                limited.into_data_stream(),
                UPLOAD_IDLE_TIMEOUT,
                idle.clone(),
            )),
            pending: axum::body::Bytes::new(),
            gate: gate.clone(),
            yielded: false,
        })
    });
    let mut multipart = axum::extract::Multipart::from_request(req, &())
        .await
        .map_err(axum::response::IntoResponse::into_response)?;
    gate.start_headers();
    let next = multipart.next_field().await;
    gate.end_headers();
    let field = next
        .map_err(|e| body_refusal(&idle, &gate, limit, e.status(), e.body_text()))?
        .ok_or_else(|| {
            bad_request(format!(
                "the multipart body has no part `{FILE_PART}`: it carries the file"
            ))
        })?;
    if field.name() != Some(FILE_PART) {
        return Err(bad_request(format!(
            "the multipart part {:?} is not `{FILE_PART}`: the body carries one part, \
             `{FILE_PART}`, with the file",
            field.name().unwrap_or("")
        )));
    }
    let first_error = Arc::new(Mutex::new(None));
    let mut body = BodyErrors {
        inner: tokio_util::io::StreamReader::new(
            field.map(|chunk| chunk.map_err(std::io::Error::other)),
        ),
        first: first_error.clone(),
    };
    let by = CreatedBy::User(ctx.principal.user_id.0);
    let written = store::put(
        &st.db.pool,
        &fs,
        &grant,
        &path,
        &mut body,
        Conflict::Overwrite,
        by,
    )
    .await;
    let entry = match written {
        Ok(Written::Written(e)) => e,
        Ok(Written::Skipped(_)) => {
            return Err(core_refusal(Error::Internal(anyhow::anyhow!(
                "an OVERWRITE upload of '{path}' was skipped"
            ))))
        }
        Err(e) => {
            // The body's own failure is the request's, not the store's.
            if let Some(text) = first_error.lock().expect("body error slot").take() {
                return Err(body_refusal(
                    &idle,
                    &gate,
                    limit,
                    StatusCode::BAD_REQUEST,
                    text,
                ));
            }
            return Err(ns_refusal(e));
        }
    };
    audit(
        &st,
        &ctx,
        "workflow_namespace_file:upload",
        &grant,
        entry.path.as_str(),
        serde_json::json!({
            "namespace": grant.namespace(),
            "path": entry.path.as_str(),
            "revision": entry.revision,
            "size": entry.size,
        }),
    );
    Ok(StatusCode::OK)
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/workflows/namespaces/{namespace}/files/directory",
    tag = "workflow",
    params(
        WorkspacePath,
        ("namespace" = String, Path, description = "The namespace; it needs no flow"),
        NsTargetQuery,
    ),
    responses(
        (status = 200, description = "The directory and each missing parent exist (an existing live directory is not an error). No body (Kestra's answer).", body = ()),
        (status = 400, description = "A path rule (named), the root, a refused namespace, or `/_flows`", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 409, description = "A file is at `path` or where one of its parents goes, live or soft-deleted with its history (named; only a flow's `PurgeFiles` task removes history -- the API has no purge route)", body = egret_core::http::ErrorBody),
        (status = 503, description = "The database is unavailable; the request can be retried.", body = egret_core::http::ErrorBody),
    ),
)]
pub(crate) async fn create_namespace_directory(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, namespace)): Path<(String, String)>,
    Query(q): Query<NsTargetQuery>,
) -> Result<StatusCode, Response> {
    let grant = grant(&st, &ctx, &namespace, Access::Write).await?;
    let dir = NsPath::parse_dir(&q.path).map_err(bad_request)?;
    let by = CreatedBy::User(ctx.principal.user_id.0);
    let e = store::create_directory(&st.db.pool, &grant, &dir, by)
        .await
        .map_err(ns_refusal)?;
    audit(
        &st,
        &ctx,
        "workflow_namespace_file:create_directory",
        &grant,
        e.path.as_str(),
        serde_json::json!({ "namespace": grant.namespace(), "path": e.path.as_str() }),
    );
    Ok(StatusCode::OK)
}

#[utoipa::path(
    put,
    path = "/api/ws/{workspace}/workflows/namespaces/{namespace}/files",
    tag = "workflow",
    params(
        WorkspacePath,
        ("namespace" = String, Path, description = "The namespace"),
        NsMoveQuery,
    ),
    responses(
        (status = 200, description = "Moved: every revision of the file (or of every path under the directory), numbers kept; the source is gone, `to`'s missing parents exist. No body (Kestra's answer). **A move runs to completion once it has started:** it runs detached from the request, so a client that disconnects mid-move loses only this answer -- the move still finishes (or rolls its copies back), never stopping half-way.", body = ()),
        (status = 400, description = "A path rule (named) for either path, a refused namespace, `/_flows`, or a move onto or under itself", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "Nothing live at `from` in this workspace's namespace (named); nothing is written", body = egret_core::http::ErrorBody),
        (status = 409, description = "`to` has any history -- a live entry, or a soft-deleted path's revisions at it, under it, of the other kind, or a file where one of its parents goes (named); nothing is moved and nothing purged (only a flow's `PurgeFiles` task removes history -- the API has no purge route)", body = egret_core::http::ErrorBody),
        (status = 413, description = "The directory holds more paths than one operation may (`workflow.storage.max_namespace_files`)", body = egret_core::http::ErrorBody),
        (status = 503, description = "Internal storage or the database is unavailable; every copy made was rolled back and the source is untouched.", body = egret_core::http::ErrorBody),
    ),
)]
pub(crate) async fn move_namespace_file(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, namespace)): Path<(String, String)>,
    Query(q): Query<NsMoveQuery>,
) -> Result<StatusCode, Response> {
    let grant = grant(&st, &ctx, &namespace, Access::Write).await?;
    // `to` must be a path at all before anything is looked up.
    let (to_file, to_dir) = (NsPath::parse_file(&q.to), NsPath::parse_dir(&q.to));
    if let (Err(rule), Err(_)) = (&to_file, &to_dir) {
        return Err(bad_request(rule.clone()));
    }
    let from = {
        let mut c = conn(&st).await?;
        resolve_any(&mut c, &grant, &q.from)
            .await
            .map_err(ns_refusal)?
    };
    let to = if from.is_directory { to_dir } else { to_file }.map_err(bad_request)?;
    let fs = crate::storage::current().ok_or_else(no_store)?;
    let by = CreatedBy::User(ctx.principal.user_id.0);
    store::move_path(&st.db.pool, &fs, &grant, &from.path, &to, by)
        .await
        .map_err(ns_refusal)?;
    audit(
        &st,
        &ctx,
        "workflow_namespace_file:move",
        &grant,
        from.path.as_str(),
        serde_json::json!({
            "namespace": grant.namespace(),
            "path": from.path.as_str(),
            "to": to.as_str(),
        }),
    );
    Ok(StatusCode::OK)
}

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/workflows/namespaces/{namespace}/files",
    tag = "workflow",
    params(
        WorkspacePath,
        ("namespace" = String, Path, description = "The namespace"),
        NsTargetQuery,
    ),
    responses(
        (status = 200, description = "Soft-deleted: the path and, for a directory, every path under it; parents left empty stay. Revisions are kept (listed by `revisions`, served by `?revision=N`) until a flow's `PurgeFiles` removes them. A path with nothing live is not an error: 200, nothing done. No body.", body = ()),
        (status = 400, description = "A path rule (named), the root, a refused namespace, or `/_flows`", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 503, description = "The database is unavailable; the request can be retried.", body = egret_core::http::ErrorBody),
    ),
)]
pub(crate) async fn delete_namespace_file(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, namespace)): Path<(String, String)>,
    Query(q): Query<NsTargetQuery>,
) -> Result<StatusCode, Response> {
    let grant = grant(&st, &ctx, &namespace, Access::Write).await?;
    let found = {
        let mut c = conn(&st).await?;
        match resolve_any(&mut c, &grant, &q.path).await {
            Ok(e) => Some(e),
            // Idempotent, as Kestra's (w9b `api.routes.delete_missing`).
            Err(NsError::NotFound(_)) => None,
            Err(e) => return Err(ns_refusal(e)),
        }
    };
    let (path, marked) = match found {
        Some(e) => {
            let marked = store::soft_delete(&st.db.pool, &grant, &e.path)
                .await
                .map_err(ns_refusal)?;
            (e.path.as_str().to_string(), marked.len())
        }
        None => (q.path.clone(), 0),
    };
    audit(
        &st,
        &ctx,
        "workflow_namespace_file:delete",
        &grant,
        &path,
        serde_json::json!({
            "namespace": grant.namespace(),
            "path": path,
            "deleted": marked,
        }),
    );
    Ok(StatusCode::OK)
}
