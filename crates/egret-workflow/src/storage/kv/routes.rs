//! The KV API (W9c spec §9), mirroring Kestra's `KVController`:
//!
//! ```text
//! GET    /api/ws/{workspace}/workflows/namespaces/{namespace}/kv                  workflow:read
//! GET    /api/ws/{workspace}/workflows/namespaces/{namespace}/kv/inheritance      workflow:read
//! GET    /api/ws/{workspace}/workflows/namespaces/{namespace}/kv/{key}?revision   workflow:read
//! GET    /api/ws/{workspace}/workflows/namespaces/{namespace}/kv/{key}/revisions  workflow:read
//! PUT    /api/ws/{workspace}/workflows/namespaces/{namespace}/kv/{key}            workflow:manage (text/plain)
//! DELETE /api/ws/{workspace}/workflows/namespaces/{namespace}/kv/{key}            workflow:manage
//! DELETE /api/ws/{workspace}/workflows/namespaces/{namespace}/kv                  workflow:manage ({keys})
//! ```
//!
//! # Who may do what
//!
//! W9b's conventions (`nsfiles::routes`): `WsContext` (the session's
//! membership of `{workspace}`), then the route's permission through the PDP
//! (`workflow:read` for a GET, `workflow:manage` for a write), and only then
//! `authorize(workspace, Actor::Api, namespace, Resource::Kv, access)` --
//! both in `nsfiles::routes::grant`. The workspace is `WsContext`'s, so a
//! namespace another workspace holds answers exactly as an empty one (the
//! user's ruling, 2026-10-07): the listings `200 []`, a key read 404.
//!
//! # Errors
//!
//! W9b's `ns_refusal` statuses (spec §9's table): `Author` 400, `NotFound`
//! 404, `Conflict` 409 (the key's lock held past the API's 5 s wait),
//! `TooLarge` 413, `Store` and `Db` 503 (`Db` logged with a fixed message,
//! never sent). The KV routes' own refusals send the message unprefixed, as
//! the fixture compares it (`kv_refusal`); the shared namespace refusal
//! (`grant`) and a malformed `?revision` keep W9b's `invalid input: `
//! prefix (spec §9, 2026-10-08). The `PUT` body's own refusals: 400 (empty,
//! not UTF-8, a blob, clob or sexp, too deep, unreadable, a DATETIME outside
//! years 1-9999 in UTC), 408 (it stalled, or took too long), 413 (over
//! `max_read_bytes`), 415 (not `text/plain`). The bulk body is at most
//! [`MAX_BULK_BODY_BYTES`] (413 naming it).
//!
//! # Holding nothing across the client
//!
//! The `PUT` body is read into memory (at most `max_read_bytes`) before
//! anything is staged, with NO pool connection and NO lock held: the
//! permission check and `WsContext` have returned their connections, and
//! `store::set` takes one only after the body is in hand (its staging PUT,
//! then one short locked transaction with the server-paced copy and `head`,
//! spec §4.3). A `GET` of a value reads its row on a pool connection,
//! releases it, then reads the object (`store::read_value`). The bulk
//! delete holds its key locks across SQL only.
//!
//! # A key named `inheritance`
//!
//! `GET …/kv/inheritance` is the inheritance listing, so a key of that name
//! is not read by that path (Kestra's collision too, `KVController.java:73`,
//! `:103`; spec §9, §15). The router matches the path as sent and `Path`
//! decodes the key afterwards, so `GET …/kv/inheritanc%65` reaches
//! `get_kv_value` and reads the key (review M3; pinned by a test). Its
//! `PUT` and `DELETE` are served on the same path, and its `…/revisions`
//! works.
//!
//! # Audit
//!
//! Each write emits one mutation (plan item 14, W9b's form): action
//! `workflow_namespace_kv:{put|delete|delete_many}`, resource type
//! `workflow_namespace_kv`, resource id `{ns}/{key}` (`{ns}` for a bulk
//! delete, its keys in the detail). No value is ever in the detail.

// Handlers answer `Result<_, Response>`, as W9b's routes do.
#![allow(clippy::result_large_err)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;
use chrono::{DateTime, Utc};
use egret_core::{Error, WorkspaceContext, WorkspacePath};
use egret_workspace::context::WsContext;
use serde::{Deserialize, Serialize};

use super::key::KvKey;
use super::store::{self, Deleted, KvEntry, KvWritten, NewValue};
use super::value::{parse_api_body, parse_ttl, ApiBodyError, Ttl};
use crate::routes::{refuse, WorkflowState};
use crate::storage::multipart::{IdleTimeout, TotalTimeout};
use crate::storage::nsfiles::access::{authorize, Access, Actor, Grant, Resource};
use crate::storage::nsfiles::routes::{
    core_refusal, grant, no_store, upload_timed_out, upload_took_too_long, NsQuery,
};
use crate::storage::nsfiles::store::Want;
use crate::storage::nsfiles::{CreatedBy, NsError};
use crate::storage::FileStore;

/// The audit resource type of every write here (plan item 14).
const AUDIT_RESOURCE: &str = "workflow_namespace_kv";

/// The key whose `GET` is the inheritance listing (spec §9, §15).
pub const INHERITANCE: &str = "inheritance";

/// The bulk delete's body cap, 1 MiB (W9c Task 6 fix round 1, review M8):
/// 1,000 keys of 350 bytes, quoted and comma-separated, are 353,010 bytes
/// of compact JSON, so a body this large has room for any whitespace. A larger one
/// is 413 naming this cap (not axum's default text), before it is parsed.
pub const MAX_BULK_BODY_BYTES: usize = 1 << 20;

/// The refusal of a bulk body over [`MAX_BULK_BODY_BYTES`].
fn bulk_body_too_large() -> Response {
    refuse(
        StatusCode::PAYLOAD_TOO_LARGE,
        format!(
            "the bulk delete body is over {MAX_BULK_BODY_BYTES} bytes: it takes at most \
             {} keys of at most 350 bytes each; nothing was deleted",
            store::MAX_BULK_DELETE_KEYS
        ),
    )
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct KvEntryDto {
    pub key: String,
    /// `STRING`, `NUMBER`, `BOOLEAN`, `DATETIME`, `DATE`, `DURATION` or `JSON`:
    /// the type stored with the latest revision.
    pub r#type: String,
    /// The stored value's size in bytes (its Ion text).
    pub size: u64,
    pub description: Option<String>,
    /// When the key expires; `null` never.
    pub expires_at: Option<DateTime<Utc>>,
    /// The latest revision.
    pub revision: u32,
    /// When the latest revision was written.
    pub updated: DateTime<Utc>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct KvInheritedDto {
    /// The ancestor namespace the key is read from.
    pub namespace: String,
    #[serde(flatten)]
    pub entry: KvEntryDto,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct KvDetailDto {
    /// The stored type (never re-inferred on read).
    pub r#type: String,
    /// The value: a DATETIME as `2025-01-02T03:04:05Z`, a DATE as
    /// `2025-01-02`, a DURATION as Java prints it (`PT24H`), a decimal with
    /// every digit, a JSON value as itself; any JSON type.
    pub value: serde_json::Value,
    pub revision: u32,
    /// When this revision was written.
    pub updated: DateTime<Utc>,
    pub description: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct KvRevisionDto {
    pub revision: u32,
    pub r#type: String,
    pub size: u64,
    pub description: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created: DateTime<Utc>,
    /// The key was soft-deleted at this revision (its latest); the revision
    /// is still served by `GET …/kv/{key}?revision=N`.
    pub deleted: bool,
    /// This revision has expired: it is not served.
    pub expired: bool,
}

/// The bulk delete's request and answer: the keys to delete, and the keys
/// that were live and are now deleted (ascending).
#[derive(Debug, Deserialize, Serialize, utoipa::ToSchema)]
pub struct KvKeysBody {
    pub keys: Vec<String>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct KvRevisionQuery {
    /// A given revision, served even when the key is soft-deleted (not when
    /// it has expired); none = the latest, which must be live.
    pub revision: Option<u32>,
}

// ─── Mapping ────────────────────────────────────────────────────────────────

/// Spec §9's table, W9b's `ns_refusal` statuses: `Author` 400, `NotFound`
/// 404, `Conflict` 409, `TooLarge` 413, `Store` and `Db` 503 -- the KV
/// store's message sent as it is (w9c `api.headers.h_ttl_bad` and
/// `api.routes.get_missing` compare it exactly, with no `invalid input: `
/// or `not found: ` prefix), and a database failure logged with a fixed
/// message that is all the client sees.
fn kv_refusal(e: NsError) -> Response {
    let (status, message) = match e {
        NsError::Author(m) => (StatusCode::BAD_REQUEST, m),
        NsError::NotFound(m) => (StatusCode::NOT_FOUND, m),
        NsError::Conflict(m) => (StatusCode::CONFLICT, m),
        NsError::TooLarge(m) => (StatusCode::PAYLOAD_TOO_LARGE, m),
        NsError::Store(m) => (StatusCode::SERVICE_UNAVAILABLE, m),
        NsError::Db(e) => {
            tracing::error!(error = %e, "a KV route failed in the database");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "the KV store's index is unavailable; retry later".into(),
            )
        }
    };
    refuse(status, message)
}

/// A 400 whose text is sent as it is (see [`kv_refusal`]).
fn bad_request(message: String) -> Response {
    refuse(StatusCode::BAD_REQUEST, message)
}

fn db_unavailable(e: sqlx::Error) -> Response {
    kv_refusal(NsError::Db(Error::Internal(anyhow::Error::new(e))))
}

async fn conn(st: &WorkflowState) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>, Response> {
    st.db.pool.acquire().await.map_err(db_unavailable)
}

fn key(raw: &str) -> Result<KvKey, Response> {
    KvKey::parse(raw).map_err(bad_request)
}

fn file_store() -> Result<Arc<FileStore>, Response> {
    crate::storage::current().ok_or_else(no_store)
}

fn entry_dto(e: KvEntry) -> KvEntryDto {
    KvEntryDto {
        key: e.key.as_str().to_string(),
        r#type: e.value_type.as_str().to_string(),
        size: e.size,
        description: e.description,
        expires_at: e.expires_at,
        revision: e.revision,
        updated: e.created_at,
    }
}

fn audit(
    st: &WorkflowState,
    ctx: &WorkspaceContext,
    action: &str,
    resource_id: String,
    detail: serde_json::Value,
) {
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        action,
        AUDIT_RESOURCE,
        Some(resource_id),
        detail,
    ));
}

// ─── Reads ──────────────────────────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/workflows/namespaces/{namespace}/kv",
    tag = "workflow",
    params(
        WorkspacePath,
        ("namespace" = String, Path, description = "The namespace; one with a `_files` or `_kv` segment, an empty segment or an invalid name is refused (400). A namespace with no flow can hold keys; one that holds none (or that only another workspace holds) answers `[]`."),
    ),
    responses(
        (status = 200, description = "The namespace's live keys (neither soft-deleted nor expired), each its latest revision, in the order their latest revisions were written.", body = Vec<KvEntryDto>),
        (status = 400, description = "A refused namespace (named)", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:read permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 413, description = "The namespace holds more live keys than one answer may hold (`workflow.storage.max_namespace_files`, named).", body = egret_core::http::ErrorBody),
        (status = 503, description = "Internal storage is not configured, or the database is unavailable; the request can be retried.", body = egret_core::http::ErrorBody),
    ),
)]
pub(crate) async fn list_kv(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, namespace)): Path<(String, String)>,
) -> Result<Json<Vec<KvEntryDto>>, Response> {
    let grant = grant(&st, &ctx, &namespace, Resource::Kv, Access::Read).await?;
    let cap = file_store()?.caps.max_namespace_files;
    let mut c = conn(&st).await?;
    let live = store::list_live(&mut c, &grant, None, cap)
        .await
        .map_err(kv_refusal)?;
    Ok(Json(live.into_iter().map(entry_dto).collect()))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/workflows/namespaces/{namespace}/kv/inheritance",
    tag = "workflow",
    params(
        WorkspacePath,
        ("namespace" = String, Path, description = "The namespace whose PARENTS are listed (cut at dots: `a.b.c` lists `a.b`, then `a`)."),
    ),
    responses(
        (status = 200, description = "The live keys of the namespace's ancestors only (not its own), deepest first; a key in two ancestors is listed once, from the deeper (Kestra's `KVController`, `:73-100`). Each entry names its namespace. **This path is also where a key named `inheritance` would be read, so `GET …/kv/inheritance` answers this listing, not that key (Kestra's collision too). The key's value is read by percent-encoding any letter of its name (`GET …/kv/inheritanc%65`): the route is matched on the path as sent and the key decoded afterwards. Its `PUT`, `DELETE` and `…/revisions` work as written, and so do the tasks and `kv()`.**", body = Vec<KvInheritedDto>),
        (status = 400, description = "A refused namespace (named)", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:read permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 413, description = "The ancestors hold more live keys in all than one answer may hold (`workflow.storage.max_namespace_files`, named).", body = egret_core::http::ErrorBody),
        (status = 503, description = "Internal storage is not configured, or the database is unavailable; the request can be retried.", body = egret_core::http::ErrorBody),
    ),
)]
pub(crate) async fn list_inherited_kv(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, namespace)): Path<(String, String)>,
) -> Result<Json<Vec<KvInheritedDto>>, Response> {
    let grant = grant(&st, &ctx, &namespace, Resource::Kv, Access::Read).await?;
    let cap = file_store()?.caps.max_namespace_files;
    // The ancestors of a namespace `authorize` accepted are each accepted:
    // their segments are a prefix of its own.
    let parents = store::ancestors(grant.namespace())
        .into_iter()
        .skip(1)
        .map(|ns| {
            authorize(
                grant.workspace(),
                Actor::Api,
                ns,
                Resource::Kv,
                Access::Read,
            )
            .map_err(bad_request)
        })
        .collect::<Result<Vec<Grant>, Response>>()?;
    let mut c = conn(&st).await?;
    let found = store::list_inherited(&mut c, grant.namespace(), &parents, cap)
        .await
        .map_err(kv_refusal)?;
    Ok(Json(
        found
            .into_iter()
            .map(|e| KvInheritedDto {
                namespace: e.namespace.clone(),
                entry: entry_dto(e),
            })
            .collect(),
    ))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/workflows/namespaces/{namespace}/kv/{key}",
    tag = "workflow",
    params(
        WorkspacePath,
        ("namespace" = String, Path, description = "The namespace"),
        ("key" = String, Path, description = "The key: `[a-zA-Z0-9][a-zA-Z0-9._-]*`, at most 350 bytes. `GET …/kv/inheritance` is the inheritance listing, not the key `inheritance`; that key is read here with any letter of it percent-encoded (`inheritanc%65`), since the route is matched on the path as sent and the key decoded afterwards."),
        KvRevisionQuery,
    ),
    responses(
        (status = 200, description = "The value with its stored type. With no `revision`, the latest, which must be live; with one, that revision, served when the key is soft-deleted.", body = KvDetailDto),
        (status = 400, description = "The key rule, a refused namespace, a `revision` that is not an integer, or a stored object that does not match its record (named)", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:read permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "No such key in this workspace's namespace, a latest read of a soft-deleted or expired key, an expired or unknown revision. Nothing is written by the read.", body = egret_core::http::ErrorBody),
        (status = 413, description = "The stored value is over `workflow.storage.max_read_bytes` (named)", body = egret_core::http::ErrorBody),
        (status = 503, description = "Internal storage or the database is unavailable or not configured; the request can be retried.", body = egret_core::http::ErrorBody),
    ),
)]
pub(crate) async fn get_kv_value(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, namespace, raw_key)): Path<(String, String, String)>,
    NsQuery(q): NsQuery<KvRevisionQuery>,
) -> Result<Json<KvDetailDto>, Response> {
    let grant = grant(&st, &ctx, &namespace, Resource::Kv, Access::Read).await?;
    let key = key(&raw_key)?;
    let fs = file_store()?;
    let want = q.revision.map_or(Want::Latest, Want::Revision);
    let e = {
        let mut c = conn(&st).await?;
        match store::resolve(&mut c, &grant, &key, want).await {
            Ok(e) => e,
            // Kestra's message for a latest read (w9c `api.routes.get_missing`,
            // `get_soft_deleted`, `get_missing_namespace`).
            Err(NsError::NotFound(_)) if want == Want::Latest => {
                return Err(kv_refusal(NsError::NotFound(format!(
                    "No value found for key '{key}' in namespace '{}'",
                    grant.namespace()
                ))))
            }
            Err(e) => return Err(kv_refusal(e)),
        }
        // The connection goes back to the pool here, before the object read.
    };
    let data = store::read_value(&st.db.pool, &fs, &grant, &e)
        .await
        .map_err(kv_refusal)?;
    Ok(Json(KvDetailDto {
        r#type: e.value_type.as_str().to_string(),
        value: data.to_json(),
        revision: e.revision,
        updated: e.created_at,
        description: e.description,
        expires_at: e.expires_at,
    }))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/workflows/namespaces/{namespace}/kv/{key}/revisions",
    tag = "workflow",
    params(
        WorkspacePath,
        ("namespace" = String, Path, description = "The namespace"),
        ("key" = String, Path, description = "The key (`inheritance` included)"),
    ),
    responses(
        (status = 200, description = "Every revision of the key, ascending, each flagged `deleted` (soft-deleted at it) and `expired`.", body = Vec<KvRevisionDto>),
        (status = 400, description = "The key rule, or a refused namespace", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:read permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 404, description = "The key has no revision in this workspace's namespace", body = egret_core::http::ErrorBody),
        (status = 503, description = "Internal storage is not configured, or the database is unavailable; the request can be retried.", body = egret_core::http::ErrorBody),
    ),
)]
pub(crate) async fn kv_revisions(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, namespace, raw_key)): Path<(String, String, String)>,
) -> Result<Json<Vec<KvRevisionDto>>, Response> {
    let grant = grant(&st, &ctx, &namespace, Resource::Kv, Access::Read).await?;
    let key = key(&raw_key)?;
    file_store()?;
    let mut c = conn(&st).await?;
    let rows = store::revisions(&mut c, &grant, &key)
        .await
        .map_err(kv_refusal)?;
    Ok(Json(
        rows.into_iter()
            .map(|e| KvRevisionDto {
                revision: e.revision,
                r#type: e.value_type.as_str().to_string(),
                size: e.size,
                description: e.description,
                expires_at: e.expires_at,
                created: e.created_at,
                deleted: e.deleted,
                expired: e.expired,
            })
            .collect(),
    ))
}

// ─── Writes ─────────────────────────────────────────────────────────────────

/// A `Content-Type` the `PUT` takes: `text/plain` (any parameters), or none.
fn is_text_plain(headers: &axum::http::HeaderMap) -> bool {
    match headers.get(axum::http::header::CONTENT_TYPE) {
        None => true,
        Some(v) => v.to_str().ok().is_some_and(|s| {
            s.split(';')
                .next()
                .is_some_and(|m| m.trim().eq_ignore_ascii_case("text/plain"))
        }),
    }
}

/// A header's text, `None` when absent; a value that is not UTF-8 is 400.
fn header_text(headers: &axum::http::HeaderMap, name: &str) -> Result<Option<String>, Response> {
    headers
        .get(name)
        .map(|v| {
            std::str::from_utf8(v.as_bytes())
                .map(str::to_string)
                .map_err(|_| bad_request(format!("the `{name}` header is not UTF-8 text")))
        })
        .transpose()
}

/// A body over `max_read_bytes`: the request's refusal, naming the cap.
fn body_too_large(cap: u64) -> Response {
    refuse(
        StatusCode::PAYLOAD_TOO_LARGE,
        format!(
            "the request body is over {cap} bytes: a KV value is at most \
             workflow.storage.max_read_bytes ({cap} bytes); nothing was stored"
        ),
    )
}

fn is_length_limit(e: &axum::Error) -> bool {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(s) = source {
        if s.is::<http_body_util::LengthLimitError>() {
            return true;
        }
        source = s.source();
    }
    false
}

/// The whole body, at most `cap` bytes, within the upload's idle and total
/// clocks (W9b's 30 s and 12 h; 408). Holds nothing but the bytes.
async fn read_body(
    st: &WorkflowState,
    body: axum::body::Body,
    cap: u64,
) -> Result<Vec<u8>, Response> {
    use futures_util::StreamExt as _;
    let idle = Arc::new(AtomicBool::new(false));
    let over_total = Arc::new(AtomicBool::new(false));
    let limit = usize::try_from(cap).unwrap_or(usize::MAX);
    let limited = axum::body::Body::new(http_body_util::Limited::new(body, limit));
    let mut stream = TotalTimeout::new(
        IdleTimeout::new(
            limited.into_data_stream(),
            st.upload_idle_timeout,
            idle.clone(),
        ),
        st.upload_total_timeout,
        over_total.clone(),
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(chunk) => bytes.extend_from_slice(&chunk),
            Err(e) => {
                return Err(if over_total.load(Ordering::SeqCst) {
                    upload_took_too_long(st.upload_total_timeout)
                } else if idle.load(Ordering::SeqCst) {
                    upload_timed_out(st.upload_idle_timeout)
                } else if is_length_limit(&e) {
                    body_too_large(cap)
                } else {
                    bad_request(format!("the request body could not be read: {e}"))
                })
            }
        }
    }
    if over_total.load(Ordering::SeqCst) {
        return Err(upload_took_too_long(st.upload_total_timeout));
    }
    Ok(bytes)
}

fn body_refusal(e: ApiBodyError) -> Response {
    let message = e.message();
    match e {
        ApiBodyError::TooLarge(_) => refuse(StatusCode::PAYLOAD_TOO_LARGE, message),
        ApiBodyError::Empty
        | ApiBodyError::NotUtf8
        | ApiBodyError::Unmodelled(_)
        | ApiBodyError::TooDeep(_)
        | ApiBodyError::Crashed(_)
        | ApiBodyError::OutOfRange { .. } => bad_request(message),
    }
}

#[utoipa::path(
    put,
    path = "/api/ws/{workspace}/workflows/namespaces/{namespace}/kv/{key}",
    tag = "workflow",
    params(
        WorkspacePath,
        ("namespace" = String, Path, description = "The namespace; it needs no flow"),
        ("key" = String, Path, description = "The key: `[a-zA-Z0-9][a-zA-Z0-9._-]*`, at most 350 bytes (`inheritance` included)"),
        ("description" = Option<String>, Header, description = "The key's description, stored with this revision (`\"\"` is kept as given)."),
        ("ttl" = Option<String>, Header, description = "An ISO-8601 duration (`PT1H`, `P1D`): the key expires that long after the write. `PT0S` is accepted (expired at once); a negative one, a malformed one, or one over 1,000 years is 400."),
    ),
    request_body(
        description = "`text/plain` (or no `Content-Type`), at most `workflow.storage.max_read_bytes`. A body that is exactly ONE Ion value is stored as that value's type: an int, decimal or float a NUMBER (every digit kept), `true`/`false` a BOOLEAN, a list or struct JSON, a string or symbol a STRING -- a DURATION when it reads as one (`PT1H`, `\"P1D\"`) --, a timestamp with a time a DATETIME in UTC (even at midnight), a day-precision timestamp a DATE, `null` a STRING null. Anything else is stored whole as the raw text, typed STRING: two or more values (`hello world`, `1 2`), text that is not Ion (`{a:`), an annotated value at any depth (`x::1`), a year- or month-precision timestamp (`2025T`). Whitespace around the one value is not a second value. A body that stops arriving for 30 seconds, or takes longer than 12 hours in all, is refused (408).",
        content((String = "text/plain")),
    ),
    responses(
        (status = 200, description = "Stored as the key's next revision (it always overwrites). No content and no content type (Kestra's answer)."),
        (status = 400, description = "The key rule, a refused namespace, a malformed or negative `ttl` (`Cannot convert value 'x' to Duration`, `ttl cannot be negative`), an empty or whitespace-only body (`empty body`), a body that is a single Ion value containing a blob, clob or s-expression at any depth (named; even under an annotation; a body of several values is the raw text whatever they hold), a timestamp whose offset carries it outside the years 1-9999 in UTC (named), a raw-text body that is not UTF-8 (`not UTF-8 text`), or one nesting deeper than 100 levels", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace. **workflow:read is not enough.**", body = egret_core::http::ErrorBody),
        (status = 408, description = "No byte of the body arrived for 30 seconds, or the body took longer than 12 hours in all; nothing was stored.", body = egret_core::http::ErrorBody),
        (status = 409, description = "The key is busy: another write holds its lock for longer than the API waits (5 s); nothing was written, try again.", body = egret_core::http::ErrorBody),
        (status = 413, description = "The body, or the value as stored, is over `workflow.storage.max_read_bytes`; or the value holds more values than the dense-value limit (`max_read_bytes`/64), or a number of more than 1,000 digits (each named). Nothing was stored.", body = egret_core::http::ErrorBody),
        (status = 415, description = "A `Content-Type` other than `text/plain`", body = egret_core::http::ErrorBody),
        (status = 503, description = "Internal storage or the database is unavailable or not configured; nothing was stored and the request can be retried.", body = egret_core::http::ErrorBody),
    ),
)]
pub(crate) async fn put_kv_value(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, namespace, raw_key)): Path<(String, String, String)>,
    req: axum::extract::Request,
) -> Result<StatusCode, Response> {
    put_value(&st, &ctx, &namespace, &raw_key, req).await
}

/// `PUT …/kv/inheritance`: the key `inheritance` (spec §9; its path is the
/// inheritance listing's, so it is routed here).
pub(crate) async fn put_kv_inheritance_key(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, namespace)): Path<(String, String)>,
    req: axum::extract::Request,
) -> Result<StatusCode, Response> {
    put_value(&st, &ctx, &namespace, INHERITANCE, req).await
}

async fn put_value(
    st: &WorkflowState,
    ctx: &WorkspaceContext,
    namespace: &str,
    raw_key: &str,
    req: axum::extract::Request,
) -> Result<StatusCode, Response> {
    // Everything that can refuse without the body, before a byte of it.
    let grant = grant(st, ctx, namespace, Resource::Kv, Access::Write).await?;
    let key = key(raw_key)?;
    let fs = file_store()?;
    let (parts, body) = req.into_parts();
    if !is_text_plain(&parts.headers) {
        return Err(refuse(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "a KV value is sent as text/plain: one Ion value, or any text".into(),
        ));
    }
    let ttl = header_text(&parts.headers, "ttl")?
        .map(|raw| parse_ttl(&raw, Ttl::Header).map_err(bad_request))
        .transpose()?;
    let description = header_text(&parts.headers, "description")?;
    // The body, holding no connection and no lock.
    let cap = fs.caps.max_read_bytes;
    let bytes = read_body(st, body, cap).await?;
    let value = tokio::task::spawn_blocking(move || parse_api_body(&bytes, cap))
        .await
        .map_err(|join| {
            core_refusal(Error::Internal(anyhow::anyhow!(
                "reading a KV API body stopped: {join}"
            )))
        })?
        .map_err(body_refusal)?;
    let by = CreatedBy::User(ctx.principal.user_id.0);
    let new = NewValue {
        value,
        description,
        ttl,
        overwrite: true,
    };
    let e = match store::set(&st.db.pool, &fs, &grant, &key, new, by).await {
        Ok(KvWritten::Written(e)) => e,
        Ok(KvWritten::Already(_)) => {
            return Err(core_refusal(Error::Internal(anyhow::anyhow!(
                "a user's KV write of '{key}' was taken for a task run's re-claim"
            ))))
        }
        Err(e) => return Err(kv_refusal(e)),
    };
    audit(
        st,
        ctx,
        "workflow_namespace_kv:put",
        format!("{}/{key}", grant.namespace()),
        serde_json::json!({
            "namespace": grant.namespace(),
            "key": key.as_str(),
            "revision": e.revision,
            "type": e.value_type.as_str(),
            "size": e.size,
            "expiresAt": e.expires_at,
        }),
    );
    Ok(StatusCode::OK)
}

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/workflows/namespaces/{namespace}/kv/{key}",
    tag = "workflow",
    params(
        WorkspacePath,
        ("namespace" = String, Path, description = "The namespace"),
        ("key" = String, Path, description = "The key (`inheritance` included)"),
    ),
    responses(
        (status = 200, description = "`true`: the key was live and is now soft-deleted (its revisions are kept, listed by `…/revisions` and served by `?revision=N`); `false`: there was no live key (never written, already deleted, or expired), nothing done.", body = bool),
        (status = 400, description = "The key rule, or a refused namespace", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 409, description = "The key is busy: another write holds its lock for longer than the API waits (5 s); nothing was deleted, try again.", body = egret_core::http::ErrorBody),
        (status = 503, description = "Internal storage is not configured, or the database is unavailable; the request can be retried.", body = egret_core::http::ErrorBody),
    ),
)]
pub(crate) async fn delete_kv_value(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, namespace, raw_key)): Path<(String, String, String)>,
) -> Result<Json<bool>, Response> {
    delete_value(&st, &ctx, &namespace, &raw_key).await
}

/// `DELETE …/kv/inheritance`: the key `inheritance` (see [`put_kv_inheritance_key`]).
pub(crate) async fn delete_kv_inheritance_key(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, namespace)): Path<(String, String)>,
) -> Result<Json<bool>, Response> {
    delete_value(&st, &ctx, &namespace, INHERITANCE).await
}

async fn delete_value(
    st: &WorkflowState,
    ctx: &WorkspaceContext,
    namespace: &str,
    raw_key: &str,
) -> Result<Json<bool>, Response> {
    let grant = grant(st, ctx, namespace, Resource::Kv, Access::Write).await?;
    let key = key(raw_key)?;
    file_store()?;
    let by = CreatedBy::User(ctx.principal.user_id.0);
    let deleted = match store::delete(&st.db.pool, &grant, &key, by).await {
        Ok(Deleted::Deleted) => true,
        Ok(Deleted::Missing | Deleted::Already) => false,
        Err(e) => return Err(kv_refusal(e)),
    };
    audit(
        st,
        ctx,
        "workflow_namespace_kv:delete",
        format!("{}/{key}", grant.namespace()),
        serde_json::json!({
            "namespace": grant.namespace(),
            "key": key.as_str(),
            "deleted": deleted,
        }),
    );
    Ok(Json(deleted))
}

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/workflows/namespaces/{namespace}/kv",
    tag = "workflow",
    params(
        WorkspacePath,
        ("namespace" = String, Path, description = "The namespace"),
    ),
    request_body(
        description = "`{\"keys\": [...]}`: at most 1,000 keys (a key named twice counts twice), in a body of at most 1 MiB (1,048,576 bytes). Keys need no JSON escaping; a body that `\\u`-escapes key characters can exceed the cap. Every key is checked before anything is deleted.",
        content((KvKeysBody = "application/json")),
    ),
    responses(
        (status = 200, description = "`{\"keys\": [...]}`: the keys that were live and are now soft-deleted, ascending; a key that was not live is left out. One transaction: all or nothing.", body = KvKeysBody),
        (status = 400, description = "A malformed or missing body, a key that breaks the key rule (named; nothing is deleted), or a refused namespace", body = egret_core::http::ErrorBody),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing workflow:manage permission, or the workflows module is disabled for this workspace", body = egret_core::http::ErrorBody),
        (status = 409, description = "A key is busy (named), or other bulk deletes are running, for longer than the API waits (5 s); nothing was deleted, try again.", body = egret_core::http::ErrorBody),
        (status = 413, description = "More than 1,000 keys, or a body over 1 MiB (each cap is named); nothing was deleted. Send several requests.", body = egret_core::http::ErrorBody),
        (status = 503, description = "Internal storage is not configured, or the database is unavailable; nothing was deleted and the request can be retried.", body = egret_core::http::ErrorBody),
    ),
)]
pub(crate) async fn delete_kv_values(
    State(st): State<WorkflowState>,
    WsContext(ctx): WsContext,
    Path((_ws, namespace)): Path<(String, String)>,
    body: Result<Json<KvKeysBody>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<KvKeysBody>, Response> {
    let grant = grant(&st, &ctx, &namespace, Resource::Kv, Access::Write).await?;
    let fs = file_store()?;
    let Json(body) = body.map_err(|r| match r.status() {
        // The route's `DefaultBodyLimit` is `MAX_BULK_BODY_BYTES`.
        StatusCode::PAYLOAD_TOO_LARGE => bulk_body_too_large(),
        _ => bad_request(format!(
            "the body is not `{{\"keys\": [...]}}`: {}",
            r.body_text()
        )),
    })?;
    let keys = body
        .keys
        .iter()
        .map(|k| KvKey::parse(k).map_err(|why| bad_request(format!("key '{k}': {why}"))))
        .collect::<Result<Vec<KvKey>, Response>>()?;
    let by = CreatedBy::User(ctx.principal.user_id.0);
    let gone = store::delete_many(&st.db.pool, &fs, &grant, &keys, by)
        .await
        .map_err(kv_refusal)?;
    let gone: Vec<String> = gone.into_iter().map(|k| k.as_str().to_string()).collect();
    audit(
        &st,
        &ctx,
        "workflow_namespace_kv:delete_many",
        grant.namespace().to_string(),
        serde_json::json!({
            "namespace": grant.namespace(),
            "requested": keys.len(),
            "keys": gone,
        }),
    );
    Ok(Json(KvKeysBody { keys: gone }))
}
