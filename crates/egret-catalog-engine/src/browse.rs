//! Session-authenticated internal catalog browse API (for the UI), reading the
//! **embedded lakekeeper** catalog (CE-2 T6).
//!
//! This is the lakekeeper-reading successor to `egret_catalog::browse` (the old
//! browse read egret's now-stale `catalog_*` tables; the lakekeeper engine holds
//! the data now). It is NOT the Iceberg REST surface (`catalog_engine_router`):
//! it is a small, workspace-scoped read API the React UI consumes, gated by
//! egret session RBAC (`catalog:read`) via the session JWT → [`WsContext`],
//! exactly like SQL Lab (`egret-engine`) and BI (`egret-bi`).
//!
//! ## What it reads, and how (the lakekeeper read API used)
//!
//! It calls lakekeeper's **in-process typed `Service` trait methods** — the same
//! methods the Iceberg REST handlers dispatch to — on the embedded engine
//! [`ApiContext`], authorized by the stable system principal
//! ([`crate::bootstrap::system_request_metadata`]):
//!
//! * catalog summary → `WarehouseService::get_warehouse` (management v1) for the
//!   warehouse name + storage profile (→ `warehouse_location`),
//! * namespaces → `NamespaceService::list_namespaces` (iceberg v1), recursed to
//!   flatten the whole namespace tree (the UI consumes a flat list, matching the
//!   old browse's `list_namespaces(cat, None)`),
//! * tables / views → `TablesService::list_tables` + `ViewService::list_views`,
//! * table detail → `TablesService::load_table` (the loaded `TableMetadata`).
//!
//! Each call takes a `Prefix` (= the workspace's warehouse UUID), the engine
//! `ApiContext`, and a `RequestMetadata`. We resolve the warehouse for the
//! session's workspace via [`CatalogStore::warehouse_for_workspace`] AFTER the
//! `catalog:read` authz check, so the egret tenant boundary is enforced first.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{FromRef, Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use egret_auth::session::SessionService;
use egret_catalog::service::storage_conn::{StorageConnInput, WorkspaceStorageConn};
use egret_catalog::service::tokens::CatalogToken;
use egret_catalog::service::CatalogStore;
use egret_catalog::CatalogError;
use egret_core::WorkspaceContext;
use egret_storage::{ObjectStore, S3Store, StorageConfig};
use egret_workspace::context::WsContext;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use lakekeeper::api::iceberg::types::{PageToken, Prefix};
use lakekeeper::api::iceberg::v1::namespace::{
    ListNamespacesQuery, NamespaceParameters, NamespaceService,
};
use lakekeeper::api::iceberg::v1::tables::TablesService;
use lakekeeper::api::iceberg::v1::tables::{ListTablesQuery, LoadTableRequest, TableParameters};
use lakekeeper::api::iceberg::v1::views::ViewService;
use lakekeeper::api::iceberg::v1::ListTablesResponse;
use lakekeeper::api::{ApiContext, RequestMetadata};
use lakekeeper::iceberg::{NamespaceIdent, TableIdent};

use crate::bootstrap::system_request_metadata;
use crate::mount::{EngineServer, EngineState};

/// Permission required to read a workspace's catalog.
const PERM: &str = "catalog:read";

/// State for the lakekeeper-reading browse API. Carries the embedded engine
/// [`ApiContext`] (lakekeeper reads), the egret-side [`CatalogStore`]
/// (`workspace → warehouse`), the configured `system_user` (to mint the system
/// principal lakekeeper's `Service` calls authorize against), and the
/// session/workspace/authz handles the [`WsContext`] extractor needs.
#[derive(Clone)]
pub struct BrowseState {
    pub engine: ApiContext<EngineState>,
    pub store: CatalogStore,
    /// Named per-workspace secret layer (Epic B). Storage connections reference a
    /// `workspace_secrets` id via `secret_ref`; this store creates / resolves /
    /// counts those named secrets. Threaded so the secrets routes, the storage
    /// connection create/update/resolve, AND the warehouse-create handler (which
    /// resolves a registered connection's credentials) all go through one named
    /// layer.
    pub workspace_secrets: egret_secrets::WorkspaceSecretStore,
    pub system_user: Arc<str>,
    pub sessions: SessionService,
    pub workspaces: WorkspaceRepo,
    pub authz: Authorizer,
}

impl FromRef<BrowseState> for SessionService {
    fn from_ref(s: &BrowseState) -> Self {
        s.sessions.clone()
    }
}
impl FromRef<BrowseState> for WorkspaceRepo {
    fn from_ref(s: &BrowseState) -> Self {
        s.workspaces.clone()
    }
}
impl FromRef<BrowseState> for Authorizer {
    fn from_ref(s: &BrowseState) -> Self {
        s.authz.clone()
    }
}

/// Workspace-scoped read-only catalog browse routes, mounted under `/api`.
///
/// Identical route shape to the old `egret_catalog::browse_router` so the
/// frontend is unchanged.
pub fn browse_router() -> Router<BrowseState> {
    Router::new()
        // Workspace-admin warehouse management (gated `workspace:manage_members`):
        // a workspace now owns N user-managed warehouses, each created on a
        // registered storage connection with its own lakekeeper warehouse.
        .route(
            "/ws/{workspace}/catalog/warehouses",
            get(list_warehouses).post(create_warehouse),
        )
        .route(
            "/ws/{workspace}/catalog/warehouses/{wid}",
            get(get_warehouse)
                .put(rename_warehouse)
                .delete(delete_warehouse),
        )
        // Catalog browse, rescoped under a specific warehouse `{wid}` (gated
        // `catalog:read`). The egret warehouse-row id resolves to its lakekeeper
        // UUID (workspace-scoped, 404 cross-tenant) before any lakekeeper read.
        .route(
            "/ws/{workspace}/catalog/warehouses/{wid}/namespaces",
            get(list_namespaces),
        )
        .route(
            "/ws/{workspace}/catalog/warehouses/{wid}/namespaces/{ns}/tables",
            get(list_tables),
        )
        .route(
            "/ws/{workspace}/catalog/warehouses/{wid}/namespaces/{ns}/tables/{table}",
            get(table_detail),
        )
        // Workspace-admin catalog token management, scoped to a warehouse `{wid}`
        // (gated `workspace:manage_members`). A token authenticates against exactly
        // one warehouse.
        .route(
            "/ws/{workspace}/catalog/warehouses/{wid}/tokens",
            get(list_tokens).post(mint_token),
        )
        .route(
            "/ws/{workspace}/catalog/warehouses/{wid}/tokens/{id}",
            delete(revoke_token),
        )
        // Workspace-admin per-workspace storage-connection registry (gated
        // `workspace:manage_members`). CRUD with a live reachability probe on
        // register/edit.
        .route(
            "/ws/{workspace}/storages",
            get(list_storage_conns).post(create_storage_conn),
        )
        .route(
            "/ws/{workspace}/storages/{id}",
            axum::routing::put(update_storage_conn).delete(delete_storage_conn),
        )
        // Workspace-admin named-secret registry (Epic B, gated
        // `workspace:manage_members`). CRUD on `workspace_secrets`; values are
        // write-only and never echoed back. Referenced by storage + database
        // connections via a named secret id.
        .route(
            "/ws/{workspace}/secrets",
            get(list_secrets).post(create_secret),
        )
        .route(
            "/ws/{workspace}/secrets/{id}",
            axum::routing::put(update_secret).delete(delete_secret),
        )
        // Workspace-admin per-connection object browser (gated
        // `workspace:manage_members`): a MinIO-console-like file manager confined
        // STRICTLY to the connection's `key_prefix` root (see `safe_key`). List +
        // upload + delete share the `/objects` path; download is `/objects/content`.
        .route(
            "/ws/{workspace}/storages/{id}/objects",
            get(list_objects).post(upload_object).delete(delete_object),
        )
        .route(
            "/ws/{workspace}/storages/{id}/objects/content",
            get(download_object),
        )
        // Folder management (gated `workspace:manage_members`, path-confined under
        // the connection root via `safe_key`): create a folder marker, and
        // move/rename a file or a whole folder. Recursive folder delete is folded
        // into `delete_object` via `?recursive=true`.
        .route(
            "/ws/{workspace}/storages/{id}/objects/folder",
            axum::routing::post(create_folder),
        )
        .route(
            "/ws/{workspace}/storages/{id}/objects/move",
            axum::routing::post(move_object),
        )
}

// ---------------------------------------------------------------------------
// Error wrapper: authz failures (egret_core::Error) + lakekeeper/engine errors.
// ---------------------------------------------------------------------------

/// HTTP-facing error: an authz/validation failure or an engine read error.
enum BrowseApiError {
    /// egret session-RBAC / validation failure.
    Auth(egret_core::Error),
    /// A lakekeeper `Service` call failed (preserves its iceberg HTTP code).
    Engine(lakekeeper::api::IcebergErrorResponse),
    /// Client-supplied input was invalid → 400.
    BadRequest(String),
    /// A conflicting state (duplicate name, or a still-referenced secret) → 409.
    Conflict(String),
    /// The targeted resource does not exist (or belongs to another tenant) → 404.
    NotFound,
    /// An internal failure building the system principal etc.
    Internal,
}

impl From<CatalogError> for BrowseApiError {
    fn from(e: CatalogError) -> Self {
        match e {
            CatalogError::BadRequest(m) => BrowseApiError::BadRequest(m),
            CatalogError::NotFound(_) => BrowseApiError::NotFound,
            _ => BrowseApiError::Internal,
        }
    }
}

impl From<egret_core::Error> for BrowseApiError {
    fn from(e: egret_core::Error) -> Self {
        BrowseApiError::Auth(e)
    }
}
impl From<lakekeeper::api::IcebergErrorResponse> for BrowseApiError {
    fn from(e: lakekeeper::api::IcebergErrorResponse) -> Self {
        BrowseApiError::Engine(e)
    }
}
impl From<egret_secrets::SecretsError> for BrowseApiError {
    fn from(e: egret_secrets::SecretsError) -> Self {
        use egret_secrets::SecretsError;
        match e {
            SecretsError::NotFound => BrowseApiError::NotFound,
            SecretsError::AlreadyExists(name) => {
                BrowseApiError::Conflict(format!("a secret named {name:?} already exists"))
            }
            SecretsError::InUse(n) => {
                BrowseApiError::Conflict(format!("in use by {n} connections"))
            }
            _ => BrowseApiError::Internal,
        }
    }
}

impl IntoResponse for BrowseApiError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            BrowseApiError::Auth(e) => match e {
                egret_core::Error::NotFound(_) => (StatusCode::NOT_FOUND, "not found".to_string()),
                egret_core::Error::InvalidInput(m) => (StatusCode::BAD_REQUEST, m),
                egret_core::Error::Unauthorized => {
                    (StatusCode::UNAUTHORIZED, "unauthorized".to_string())
                }
                egret_core::Error::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
                egret_core::Error::Conflict(m) => (StatusCode::CONFLICT, m),
                egret_core::Error::TooManyRequests => (
                    StatusCode::TOO_MANY_REQUESTS,
                    "too many requests".to_string(),
                ),
                egret_core::Error::Internal(_) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error".to_string(),
                ),
            },
            BrowseApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            BrowseApiError::Conflict(m) => (StatusCode::CONFLICT, m),
            BrowseApiError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            BrowseApiError::Engine(e) => {
                let code =
                    StatusCode::from_u16(e.error.code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                // Surface lakekeeper's not-found / not-found-equivalents as 404;
                // otherwise hide internal detail behind a generic message.
                let msg = if code == StatusCode::NOT_FOUND {
                    "not found".to_string()
                } else {
                    "internal error".to_string()
                };
                (code, msg)
            }
            BrowseApiError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            ),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

// ---------------------------------------------------------------------------
// DTOs — field names IDENTICAL to the old `egret_catalog::browse` DTOs so the
// frontend is unchanged.
// ---------------------------------------------------------------------------

/// A namespace listed in the browse tree (its dotted levels).
#[derive(Serialize)]
struct NamespaceDto {
    levels: Vec<String>,
}

/// Tables + views contained in a namespace.
#[derive(Serialize)]
struct NamespaceContents {
    tables: Vec<String>,
    views: Vec<String>,
}

/// One column of a table's current schema.
#[derive(Serialize)]
struct ColumnDto {
    name: String,
    #[serde(rename = "type")]
    type_: String,
    required: bool,
}

/// Compact, UI-friendly projection of a table's current state. Deliberately
/// does NOT dump the whole `TableMetadata`.
#[derive(Serialize)]
struct TableDetail {
    name: String,
    location: String,
    current_schema_id: i32,
    columns: Vec<ColumnDto>,
    current_snapshot_id: Option<i64>,
    snapshot_count: usize,
    properties: BTreeMap<String, String>,
}

// ---------------------------------------------------------------------------
// Shared resolution: authz `catalog:read`, then resolve the workspace warehouse.
// ---------------------------------------------------------------------------

/// Resolved per-request engine handle: the warehouse `Prefix` (= its lakekeeper
/// UUID) plus a fresh system `RequestMetadata`. Produced only AFTER the
/// `catalog:read` authz check passes AND the `{wid}` warehouse row is confirmed
/// to belong to the path workspace, so the egret tenant boundary is enforced
/// before any lakekeeper read.
struct Resolved {
    prefix: Prefix,
    request_metadata: RequestMetadata,
}

/// Authorize `catalog:read` on the active workspace, then resolve the egret
/// warehouse-row id `wid` (from the path) to its lakekeeper UUID.
///
/// `store.lakekeeper_id` is workspace-scoped, so a `wid` belonging to another
/// workspace (or a non-existent one) is a 404 — a member of X cannot reach Y's
/// warehouse even by guessing its egret row id.
async fn authorized_warehouse(
    st: &BrowseState,
    ctx: &WorkspaceContext,
    wid: Uuid,
) -> Result<Resolved, BrowseApiError> {
    // RBAC FIRST: the session must hold `catalog:read` on the path workspace.
    st.authz.require_workspace(ctx, PERM).await?;

    // Then resolve the requested warehouse's lakekeeper UUID, scoped to the
    // caller's workspace (cross-workspace `wid` → NotFound).
    let lk_id = st.store.lakekeeper_id(ctx.workspace_id.0, wid).await?;

    let request_metadata =
        system_request_metadata(&st.system_user).map_err(|_| BrowseApiError::Internal)?;

    Ok(Resolved {
        // lakekeeper's default `prefix_template = "{warehouse_id}"`.
        prefix: Prefix::from(lk_id.to_string()),
        request_metadata,
    })
}

/// Empty/default list-namespaces query starting at top level.
fn ns_query(parent: Option<NamespaceIdent>) -> ListNamespacesQuery {
    ListNamespacesQuery {
        page_token: PageToken::Empty,
        page_size: None,
        parent,
        return_uuids: false,
        return_protection_status: false,
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Workspace-admin warehouse management (list / create / get / rename / delete).
//
// Gated `workspace:manage_members` (the workspace-admin gate), and IDOR-safe:
// every operation is scoped to `ctx.workspace_id` (bound by `WsContext`), and the
// warehouse store queries every row by BOTH id AND workspace_id, so a
// cross-workspace `{wid}` resolves to 404. Create drives a live lakekeeper
// `create_warehouse` (a real S3 `validate_access`) on the resolved storage
// connection; delete drives the lakekeeper `delete_warehouse` AFTER the
// ownership-checked egret row delete.
// ---------------------------------------------------------------------------

/// A warehouse as exposed to the workspace-admin UI.
#[derive(Serialize)]
struct WarehouseDto {
    id: Uuid,
    name: String,
    storage_conn_id: Option<Uuid>,
    storage_conn_name: Option<String>,
    created_at: DateTime<Utc>,
}

/// Request body to create a warehouse: a name (unique within the workspace) and
/// the registered storage connection it is backed by (REQUIRED).
#[derive(Deserialize)]
struct CreateWarehouseRequest {
    name: String,
    storage_conn_id: Uuid,
}

/// Request body to rename a warehouse.
#[derive(Deserialize)]
struct RenameWarehouseRequest {
    name: String,
}

/// Resolve a registered storage connection (workspace-scoped) into a usable
/// [`StorageConfig`], unsealing its named secret. 404 if the connection does not
/// exist / belongs to another workspace. Mirrors the `conn_s3` credential path.
async fn conn_storage_config(
    st: &BrowseState,
    ws: Uuid,
    conn_id: Uuid,
) -> Result<StorageConfig, BrowseApiError> {
    let conn = st
        .store
        .get_storage_conn(ws, conn_id)
        .await?
        .ok_or(BrowseApiError::NotFound)?;
    let secret_key = match conn.secret_ref {
        Some(secret_ref) => st
            .workspace_secrets
            .resolve_plaintext(ws, secret_ref)
            .await
            .map_err(|_| BrowseApiError::Internal)?,
        None => String::new(),
    };
    Ok(StorageConfig {
        endpoint: conn.endpoint,
        region: conn.region,
        bucket: conn.bucket,
        access_key: conn.access_key,
        secret_key,
        path_style: conn.path_style,
    })
}

/// GET `/api/ws/{workspace}/catalog/warehouses` — list this workspace's warehouses.
async fn list_warehouses(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
) -> Result<Json<Vec<WarehouseDto>>, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;
    let ws = ctx.workspace_id.0;
    let warehouses = st.store.list_warehouses(ws).await?;

    let mut out = Vec::with_capacity(warehouses.len());
    for wh in warehouses {
        // Resolve the backing connection's name when present (best-effort: a
        // dangling conn id shows no name rather than failing the whole list).
        let storage_conn_name = match wh.storage_conn_id {
            Some(conn_id) => st
                .store
                .get_storage_conn(ws, conn_id)
                .await?
                .map(|c| c.name),
            None => None,
        };
        out.push(WarehouseDto {
            id: wh.id,
            name: wh.name,
            storage_conn_id: wh.storage_conn_id,
            storage_conn_name,
            created_at: wh.created_at,
        });
    }
    Ok(Json(out))
}

/// POST `/api/ws/{workspace}/catalog/warehouses` — create a warehouse on a
/// registered storage connection, provisioning a real lakekeeper warehouse.
async fn create_warehouse(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Json(req): Json<CreateWarehouseRequest>,
) -> Result<Response, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;
    let ws = ctx.workspace_id.0;

    if req.name.trim().is_empty() {
        return Err(BrowseApiError::BadRequest(
            "warehouse name must not be empty".to_string(),
        ));
    }

    // Resolve the backing storage connection's effective credentials (404 if the
    // connection is missing / cross-workspace).
    let cfg = conn_storage_config(&st, ws, req.storage_conn_id).await?;

    // Drive lakekeeper FIRST (live `validate_access`): a bad/unreachable storage
    // connection fails HERE with a 400 and NO egret row is written.
    let project_id = lakekeeper::ProjectId::new(Uuid::nil());
    let rm = system_request_metadata(&st.system_user).map_err(|_| BrowseApiError::Internal)?;
    let lk_id = crate::provision_ws::create_warehouse_on(
        st.engine.clone(),
        rm,
        project_id,
        &req.name,
        &cfg,
    )
    .await
    .map_err(|e| {
        tracing::warn!(error = ?e, "lakekeeper create_warehouse rejected");
        let mut msg = e.to_string();
        msg.truncate(200);
        BrowseApiError::BadRequest(format!("warehouse create rejected: {msg}"))
    })?;

    // Persist the egret row mapping the workspace's warehouse → its lakekeeper id.
    // A duplicate name within the workspace → 409-style conflict. If the insert
    // fails AFTER the lakekeeper warehouse was created, that warehouse is now
    // orphaned — best-effort roll it back (mirrors the seal-then-rollback pattern
    // the storage-conn handlers use for a just-minted named secret).
    let wh = match st
        .store
        .insert_warehouse(ws, lk_id, &req.name, Some(req.storage_conn_id))
        .await
    {
        Ok(wh) => wh,
        Err(e) => {
            // Best-effort roll back the just-created lakekeeper warehouse so a
            // failed egret-row insert never orphans it.
            match system_request_metadata(&st.system_user).ok() {
                Some(rm) => {
                    if let Err(de) =
                        crate::provision_ws::delete_warehouse(st.engine.clone(), rm, lk_id).await
                    {
                        tracing::warn!(
                            error = ?de, warehouse_id = %lk_id,
                            "failed to roll back orphaned lakekeeper warehouse after egret row insert failed"
                        );
                    }
                }
                None => tracing::warn!(
                    warehouse_id = %lk_id,
                    "could not build system principal to roll back orphaned lakekeeper warehouse"
                ),
            }
            return Err(match e {
                CatalogError::AlreadyExists(_) => BrowseApiError::Conflict(
                    "a warehouse with that name already exists".to_string(),
                ),
                other => other.into(),
            });
        }
    };

    Ok((
        StatusCode::CREATED,
        Json(WarehouseDto {
            id: wh.id,
            name: wh.name,
            storage_conn_id: wh.storage_conn_id,
            storage_conn_name: None,
            created_at: wh.created_at,
        }),
    )
        .into_response())
}

/// GET `/api/ws/{workspace}/catalog/warehouses/{wid}` — warehouse detail.
async fn get_warehouse(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path((_ws, wid)): Path<(String, Uuid)>,
) -> Result<Json<WarehouseDto>, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;
    let ws = ctx.workspace_id.0;
    let wh = st.store.get_warehouse(ws, wid).await?;
    let storage_conn_name = match wh.storage_conn_id {
        Some(conn_id) => st
            .store
            .get_storage_conn(ws, conn_id)
            .await?
            .map(|c| c.name),
        None => None,
    };
    Ok(Json(WarehouseDto {
        id: wh.id,
        name: wh.name,
        storage_conn_id: wh.storage_conn_id,
        storage_conn_name,
        created_at: wh.created_at,
    }))
}

/// PUT `/api/ws/{workspace}/catalog/warehouses/{wid}` — rename a warehouse.
async fn rename_warehouse(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path((_ws, wid)): Path<(String, Uuid)>,
    Json(req): Json<RenameWarehouseRequest>,
) -> Result<StatusCode, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;
    if req.name.trim().is_empty() {
        return Err(BrowseApiError::BadRequest(
            "warehouse name must not be empty".to_string(),
        ));
    }
    st.store
        .rename_warehouse(ctx.workspace_id.0, wid, &req.name)
        .await
        .map_err(|e| match e {
            CatalogError::AlreadyExists(_) => {
                BrowseApiError::Conflict("a warehouse with that name already exists".to_string())
            }
            other => other.into(),
        })?;
    Ok(StatusCode::OK)
}

/// DELETE `/api/ws/{workspace}/catalog/warehouses/{wid}` — delete a warehouse.
///
/// Ordering: resolve the lakekeeper id (ownership-checked) → lakekeeper delete →
/// then drop the egret row. So a failed lakekeeper delete leaves the egret row
/// intact (no dangling row pointing at a deleted lakekeeper warehouse, and the
/// admin can retry).
async fn delete_warehouse(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path((_ws, wid)): Path<(String, Uuid)>,
) -> Result<StatusCode, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;
    let ws = ctx.workspace_id.0;

    // Ownership-checked resolution (cross-workspace `wid` → 404).
    let lk_id = st.store.lakekeeper_id(ws, wid).await?;

    let rm = system_request_metadata(&st.system_user).map_err(|_| BrowseApiError::Internal)?;
    crate::provision_ws::delete_warehouse(st.engine.clone(), rm, lk_id)
        .await
        .map_err(|e| {
            tracing::warn!(error = ?e, warehouse_id = %lk_id, "lakekeeper delete_warehouse failed");
            BrowseApiError::Internal
        })?;

    // lakekeeper delete succeeded — now drop the egret row.
    st.store.delete_warehouse_row(ws, wid).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// GET `/api/ws/{workspace}/catalog/warehouses/{wid}/namespaces` — ALL namespaces
/// (flattened).
///
/// lakekeeper's `list_namespaces(parent=None)` returns only top-level
/// namespaces; the UI (like the old browse) wants the whole tree flat, so we
/// recurse, paginating each level.
async fn list_namespaces(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path((_ws, wid)): Path<(String, Uuid)>,
) -> Result<Json<Vec<NamespaceDto>>, BrowseApiError> {
    let r = authorized_warehouse(&st, &ctx, wid).await?;

    let mut out: Vec<NamespaceDto> = Vec::new();
    // BFS over the namespace tree. `None` parent = top level.
    let mut frontier: Vec<Option<NamespaceIdent>> = vec![None];
    while let Some(parent) = frontier.pop() {
        let levels_list = list_namespaces_one_level(
            &st,
            &r.prefix,
            parent,
            // Each level needs its own principal (RequestMetadata is consumed).
            system_request_metadata(&st.system_user).map_err(|_| BrowseApiError::Internal)?,
        )
        .await?;
        for levels in levels_list {
            let ident =
                NamespaceIdent::from_vec(levels.clone()).map_err(|_| BrowseApiError::Internal)?;
            out.push(NamespaceDto { levels });
            frontier.push(Some(ident));
        }
    }

    out.sort_by(|a, b| a.levels.cmp(&b.levels));
    Ok(Json(out))
}

/// List the immediate child namespaces under `parent` (paginated), returning
/// each as its full dotted levels.
async fn list_namespaces_one_level(
    st: &BrowseState,
    prefix: &Prefix,
    parent: Option<NamespaceIdent>,
    mut request_metadata: RequestMetadata,
) -> Result<Vec<Vec<String>>, BrowseApiError> {
    let mut levels: Vec<Vec<String>> = Vec::new();
    let mut page = PageToken::Empty;
    loop {
        let mut query = ns_query(parent.clone());
        query.page_token = page.clone();
        let resp = <EngineServer as NamespaceService<EngineState>>::list_namespaces(
            Some(prefix.clone()),
            query,
            st.engine.clone(),
            request_metadata.clone(),
        )
        .await?;
        for ns in resp.namespaces.iter() {
            levels.push(ns.clone().inner());
        }
        match resp.next_page_token {
            Some(token) if !token.is_empty() => page = PageToken::Present(token),
            _ => break,
        }
        // Refresh the principal for the next page (defensive; cheap).
        request_metadata =
            system_request_metadata(&st.system_user).map_err(|_| BrowseApiError::Internal)?;
    }
    Ok(levels)
}

/// Parse a dotted namespace path segment (`a.b.c`) into its levels.
fn parse_ns(ns: &str) -> Vec<String> {
    ns.split('.').map(|s| s.to_string()).collect()
}

/// Collect all table-or-view identifiers (names) under `parent`, paginating.
async fn list_all_names<F, Fut>(
    st: &BrowseState,
    params: &NamespaceParameters,
    mut call: F,
) -> Result<Vec<String>, BrowseApiError>
where
    F: FnMut(ListTablesQuery, RequestMetadata) -> Fut,
    Fut: std::future::Future<
        Output = Result<ListTablesResponse, lakekeeper::api::IcebergErrorResponse>,
    >,
{
    let _ = params; // params captured by the closures
    let mut names: Vec<String> = Vec::new();
    let mut page = PageToken::Empty;
    loop {
        let query = ListTablesQuery {
            page_token: page.clone(),
            page_size: None,
            return_uuids: false,
            return_protection_status: false,
        };
        let rm = system_request_metadata(&st.system_user).map_err(|_| BrowseApiError::Internal)?;
        let resp = call(query, rm).await?;
        for ident in resp.identifiers.iter() {
            names.push(ident.name.clone());
        }
        match resp.next_page_token {
            Some(token) if !token.is_empty() => page = PageToken::Present(token),
            _ => break,
        }
    }
    Ok(names)
}

/// GET `/api/ws/{workspace}/catalog/namespaces/{ns}/tables` — tables + views.
async fn list_tables(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path((_ws, wid, ns)): Path<(String, Uuid, String)>,
) -> Result<Json<NamespaceContents>, BrowseApiError> {
    let r = authorized_warehouse(&st, &ctx, wid).await?;
    let namespace =
        NamespaceIdent::from_vec(parse_ns(&ns)).map_err(|_| BrowseApiError::Internal)?;
    let params = NamespaceParameters {
        prefix: Some(r.prefix.clone()),
        namespace,
    };

    let engine = st.engine.clone();
    let p1 = params.clone();
    let tables = list_all_names(&st, &params, |query, rm| {
        let engine = engine.clone();
        let p1 = p1.clone();
        async move {
            <EngineServer as TablesService<EngineState>>::list_tables(p1, query, engine, rm).await
        }
    })
    .await?;

    let engine = st.engine.clone();
    let p2 = params.clone();
    let views = list_all_names(&st, &params, |query, rm| {
        let engine = engine.clone();
        let p2 = p2.clone();
        async move {
            <EngineServer as ViewService<EngineState>>::list_views(p2, query, engine, rm).await
        }
    })
    .await?;

    Ok(Json(NamespaceContents { tables, views }))
}

/// GET `/api/ws/{workspace}/catalog/namespaces/{ns}/tables/{table}` — detail.
async fn table_detail(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path((_ws, wid, ns, table)): Path<(String, Uuid, String, String)>,
) -> Result<Json<TableDetail>, BrowseApiError> {
    let r = authorized_warehouse(&st, &ctx, wid).await?;
    let namespace =
        NamespaceIdent::from_vec(parse_ns(&ns)).map_err(|_| BrowseApiError::Internal)?;
    let table_ident = TableIdent {
        namespace,
        name: table.clone(),
    };

    let loaded = <EngineServer as TablesService<EngineState>>::load_table(
        TableParameters {
            prefix: Some(r.prefix.clone()),
            table: table_ident,
        },
        // Default load request: no credential vending, all snapshots, no etags.
        LoadTableRequest::default(),
        st.engine.clone(),
        r.request_metadata,
    )
    .await?;

    use lakekeeper::api::iceberg::v1::tables::LoadTableResultOrNotModified;
    let result = match loaded {
        LoadTableResultOrNotModified::LoadTableResult(res) => res,
        // We send no `if-none-match` etags, so the engine never returns 304.
        LoadTableResultOrNotModified::NotModifiedResponse(_) => {
            return Err(BrowseApiError::Internal)
        }
    };

    let md = &result.metadata;
    let schema = md.current_schema();
    let columns = schema
        .as_struct()
        .fields()
        .iter()
        .map(|f| ColumnDto {
            name: f.name.clone(),
            type_: f.field_type.to_string(),
            required: f.required,
        })
        .collect();

    let properties = md
        .properties()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    // Location: the loaded metadata's table location (its storage root).
    let location = md.location().to_string();

    Ok(Json(TableDetail {
        name: table,
        location,
        current_schema_id: schema.schema_id(),
        columns,
        current_snapshot_id: md.current_snapshot_id(),
        snapshot_count: md.snapshots().count(),
        properties,
    }))
}

// ---------------------------------------------------------------------------
// Workspace-admin catalog token management (list / mint / revoke).
//
// Gated `workspace:manage_members` (the workspace-admin permission, same gate as
// `egret-workspace`'s storage/members routes), and IDOR-safe: every operation is
// scoped to `ctx.workspace_id`, which `WsContext` bound to the path slug +
// membership, so a workspace admin can only see/mint/revoke their OWN tokens.
//
// The persisted SHA-256 hash and the raw secret are NEVER serialized into the
// list/mint responses — [`CatalogTokenDto`] has no hash field and the raw token
// is returned exactly once, only from mint.
// ---------------------------------------------------------------------------

/// A catalog token as exposed to the workspace-admin UI. Deliberately omits the
/// stored hash (and never carries the raw secret).
#[derive(Serialize)]
struct CatalogTokenDto {
    id: Uuid,
    label: String,
    permissions: Vec<String>,
    namespace_scope: Vec<String>,
    created_at: DateTime<Utc>,
}

impl From<CatalogToken> for CatalogTokenDto {
    fn from(t: CatalogToken) -> Self {
        CatalogTokenDto {
            id: t.id,
            label: t.label,
            permissions: t.permissions,
            namespace_scope: t.namespace_scope,
            created_at: t.created_at,
        }
    }
}

/// Request body for minting a token. `permissions` are the short UI forms
/// (`"read"` / `"write"`) which we map to the catalog permission strings.
#[derive(Deserialize)]
struct MintTokenRequest {
    label: String,
    permissions: Vec<String>,
    #[serde(default)]
    namespace_scope: Vec<String>,
}

/// Mint response: the persisted token plus the raw secret (shown once).
#[derive(Serialize)]
struct MintTokenResponse {
    token: CatalogTokenDto,
    raw: String,
}

/// Authorize `workspace:manage_members` on the active (path) workspace.
async fn require_workspace_admin(
    st: &BrowseState,
    ctx: &WorkspaceContext,
) -> Result<(), BrowseApiError> {
    st.authz
        .require_workspace(ctx, "workspace:manage_members")
        .await?;
    Ok(())
}

/// Map a short UI permission (`read`/`write`) to its catalog permission string.
fn map_permission(p: &str) -> Result<String, BrowseApiError> {
    match p {
        "read" => Ok("catalog:read".to_string()),
        "write" => Ok("catalog:write".to_string()),
        other => Err(BrowseApiError::BadRequest(format!(
            "unknown permission '{other}'; valid values are: read, write"
        ))),
    }
}

/// GET `/api/ws/{workspace}/catalog/warehouses/{wid}/tokens` — list a warehouse's
/// tokens.
async fn list_tokens(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path((_ws, wid)): Path<(String, Uuid)>,
) -> Result<Json<Vec<CatalogTokenDto>>, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;
    let ws = ctx.workspace_id.0;
    // Verify the warehouse belongs to this workspace first (cross-workspace → 404).
    st.store.get_warehouse(ws, wid).await?;
    let tokens = st.store.list_tokens_for_warehouse(ws, wid).await?;
    Ok(Json(
        tokens.into_iter().map(CatalogTokenDto::from).collect(),
    ))
}

/// POST `/api/ws/{workspace}/catalog/warehouses/{wid}/tokens` — mint a new token
/// scoped to the warehouse.
async fn mint_token(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path((_ws, wid)): Path<(String, Uuid)>,
    Json(req): Json<MintTokenRequest>,
) -> Result<Json<MintTokenResponse>, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;
    let ws = ctx.workspace_id.0;
    // Verify the warehouse belongs to this workspace first (cross-workspace → 404).
    st.store.get_warehouse(ws, wid).await?;

    if req.permissions.is_empty() {
        return Err(BrowseApiError::BadRequest(
            "permissions must not be empty".to_string(),
        ));
    }
    let mut perms = req
        .permissions
        .iter()
        .map(|p| map_permission(p))
        .collect::<Result<Vec<_>, _>>()?;
    // Dedup so e.g. `["read","read"]` persists a single `catalog:read`.
    perms.sort();
    perms.dedup();

    let (token, raw) = st
        .store
        .create_token(ws, wid, &req.label, &perms, &req.namespace_scope)
        .await?;

    Ok(Json(MintTokenResponse {
        token: token.into(),
        raw,
    }))
}

/// DELETE `/api/ws/{workspace}/catalog/warehouses/{wid}/tokens/{id}` — revoke a
/// token.
///
/// 404 (not 403) when the id does not belong to this workspace warehouse, so the
/// response does not confirm the existence of another tenant's token (IDOR
/// closed). The token's `warehouse_id` must match `{wid}` too.
async fn revoke_token(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path((_ws, wid, id)): Path<(String, Uuid, Uuid)>,
) -> Result<StatusCode, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;
    let ws = ctx.workspace_id.0;
    // Verify the warehouse belongs to this workspace (cross-workspace → 404).
    st.store.get_warehouse(ws, wid).await?;

    // Revoke scoped to BOTH the workspace AND the warehouse: a token minted for
    // another warehouse in the same workspace returns `false` here (→ 404), so it
    // can't be revoked via this warehouse's path. The store-layer warehouse_id
    // guard is the defense-in-depth boundary (no separate list pre-check needed).
    let revoked = st.store.revoke_token_for_warehouse(ws, wid, id).await?;
    if revoked {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(BrowseApiError::NotFound)
    }
}

// ---------------------------------------------------------------------------
// Workspace-admin storage-connection registry (CRUD + reachability validate).
//
// A per-workspace registry of N named storage connections (`workspace_storages`,
// distinct from the single warehouse-backing `workspace_storage` above). Gated
// `workspace:manage_members` (the workspace-admin gate), and IDOR-safe: every
// operation is scoped to `ctx.workspace_id` (bound by `WsContext`), and the
// store queries every row by BOTH id AND workspace_id, so a workspace admin can
// only see/edit/delete their OWN connections.
//
// Register/edit run a LIVE reachability probe (`S3Store::list`) so bad
// credentials / unreachable endpoints are rejected with 400 BEFORE anything is
// persisted. The S3 secret is WRITE-ONLY: it is sealed through the vault and
// referenced by `secret_ref`; it is never serialized into ANY response, and is
// OPTIONAL on edit (omitting it keeps the existing sealed secret).
// ---------------------------------------------------------------------------

/// A registered storage connection as exposed to the workspace-admin UI.
/// Deliberately omits the secret value AND its `secret_ref`; `has_secret`
/// reflects whether a sealed secret exists.
#[derive(Serialize)]
struct WorkspaceStorageConnDto {
    id: Uuid,
    name: String,
    purpose: Option<String>,
    provider: String,
    endpoint: Option<String>,
    region: String,
    bucket: String,
    key_prefix: Option<String>,
    access_key: String,
    path_style: bool,
    has_secret: bool,
    created_at: DateTime<Utc>,
}

impl From<WorkspaceStorageConn> for WorkspaceStorageConnDto {
    fn from(c: WorkspaceStorageConn) -> Self {
        WorkspaceStorageConnDto {
            id: c.id,
            name: c.name,
            purpose: c.purpose,
            provider: c.provider,
            endpoint: c.endpoint,
            region: c.region,
            bucket: c.bucket,
            key_prefix: c.key_prefix,
            access_key: c.access_key,
            path_style: c.path_style,
            has_secret: c.secret_ref.is_some(),
            created_at: c.created_at,
        }
    }
}

/// Request body to register a storage connection. Credentials are supplied as a
/// NAMED secret (Epic B): either `new_secret` (mint a new named secret inline) or
/// `secret_id` (reference an existing named secret in the workspace). Omitting
/// both registers a connection with no secret.
#[derive(Deserialize)]
struct CreateStorageConnRequest {
    name: String,
    #[serde(default)]
    purpose: Option<String>,
    provider: String,
    #[serde(default)]
    endpoint: Option<String>,
    region: String,
    bucket: String,
    #[serde(default)]
    key_prefix: Option<String>,
    access_key: String,
    #[serde(default)]
    secret_id: Option<Uuid>,
    #[serde(default)]
    new_secret: Option<egret_secrets::NewSecret>,
    path_style: bool,
}

/// Request body to edit a storage connection. Credentials follow the same named
/// precedence as create; omitting both `new_secret` and `secret_id` keeps the
/// existing `secret_ref` (keep-current).
#[derive(Deserialize)]
struct UpdateStorageConnRequest {
    name: String,
    #[serde(default)]
    purpose: Option<String>,
    provider: String,
    #[serde(default)]
    endpoint: Option<String>,
    region: String,
    bucket: String,
    #[serde(default)]
    key_prefix: Option<String>,
    access_key: String,
    #[serde(default)]
    secret_id: Option<Uuid>,
    #[serde(default)]
    new_secret: Option<egret_secrets::NewSecret>,
    path_style: bool,
}

/// Phase A (BEFORE the reachability probe): resolve the PLAINTEXT to test with,
/// WITHOUT creating any `workspace_secrets` row, by Epic-B precedence:
///   (a) `new_secret` present → its `value` (nothing is persisted yet),
///   (b) else `secret_id` present → validate ownership via `get_meta`, then
///       `resolve_plaintext`,
///   (c) else → the existing conn's `secret_ref` plaintext (keep-current), or
///       `None` when there is no existing secret.
///
/// Crucially this NEVER mints a named secret: if the probe (or the later conn
/// store write) fails, no row is orphaned. The mint happens in [`persist_conn_secret`]
/// only after the probe succeeds.
async fn resolve_conn_plaintext(
    st: &BrowseState,
    ws: Uuid,
    secret_id: Option<Uuid>,
    new_secret: Option<&egret_secrets::NewSecret>,
    existing: Option<Uuid>,
) -> Result<Option<String>, BrowseApiError> {
    match (new_secret, secret_id) {
        (Some(n), _) => Ok(Some(n.value.clone())),
        (None, Some(id)) => {
            // Validate the named secret belongs to this workspace (404 otherwise).
            st.workspace_secrets.get_meta(ws, id).await?;
            Ok(Some(st.workspace_secrets.resolve_plaintext(ws, id).await?))
        }
        (None, None) => match existing {
            Some(id) => Ok(Some(st.workspace_secrets.resolve_plaintext(ws, id).await?)),
            None => Ok(None),
        },
    }
}

/// Phase B (AFTER the reachability probe succeeds): compute the `secret_ref` to
/// persist, minting the inline `new_secret` NOW (and only now):
///   (a) `new_secret` present → `workspace_secrets.create(...)` → its fresh id,
///   (b) else `secret_id` present → that id (already validated in Phase A),
///   (c) else → `existing` (the keep-current `secret_ref`).
///
/// A freshly-minted id (case a) is returned alongside `true` so the caller can
/// roll it back if the subsequent conn store write fails — otherwise a failed
/// insert would orphan the just-created named secret.
async fn persist_conn_secret(
    st: &BrowseState,
    ws: Uuid,
    secret_id: Option<Uuid>,
    new_secret: Option<&egret_secrets::NewSecret>,
    existing: Option<Uuid>,
) -> Result<(Option<Uuid>, bool), BrowseApiError> {
    match (new_secret, secret_id) {
        (Some(n), _) => {
            let id = st
                .workspace_secrets
                .create(ws, &n.name, None, &n.value)
                .await?;
            Ok((Some(id), true))
        }
        (None, Some(id)) => Ok((Some(id), false)),
        (None, None) => Ok((existing, false)),
    }
}

/// Best-effort roll back a named secret minted in [`persist_conn_secret`] when the
/// conn store write that should have referenced it failed. Logs on failure so a
/// cleanup miss is visible but never masks the original error.
async fn rollback_minted_secret(st: &BrowseState, ws: Uuid, minted: Option<Uuid>) {
    if let Some(id) = minted {
        if let Err(e) = st.workspace_secrets.delete(ws, id).await {
            tracing::warn!(secret_id = %id, error = %e,
                "failed to roll back named secret after storage conn write failure");
        }
    }
}

/// Probe that the storage described by `cfg` is reachable with the given
/// credentials by attempting a folder-style `list` under `key_prefix`. Any
/// failure (auth, missing bucket, unreachable endpoint) → 400 with a trimmed,
/// actionable message; the full error is logged at WARN for operators.
async fn validate_reachable(
    cfg: &StorageConfig,
    key_prefix: Option<&str>,
) -> Result<(), BrowseApiError> {
    let store = S3Store::new(cfg).await.map_err(|e| {
        tracing::warn!(error = ?e, "storage reachability probe: client build failed");
        let mut msg = e.to_string();
        msg.truncate(200);
        BrowseApiError::BadRequest(format!("storage not reachable: {msg}"))
    })?;
    store
        .list(key_prefix.unwrap_or(""), Some("/"))
        .await
        .map_err(|e| {
            tracing::warn!(error = ?e, "storage reachability probe failed");
            let mut msg = e.to_string();
            msg.truncate(200);
            BrowseApiError::BadRequest(format!("storage not reachable: {msg}"))
        })?;
    Ok(())
}

/// Translate a `create_storage_conn` error into the right HTTP code. A unique
/// constraint violation (duplicate connection name in the workspace) becomes a
/// 409-style conflict with an actionable message rather than a generic 500.
fn map_create_conn_error(e: CatalogError) -> BrowseApiError {
    match &e {
        CatalogError::AlreadyExists(m) => BrowseApiError::BadRequest(m.clone()),
        // A unique-name violation surfaces as a transparent `Db(sqlx::Error)`. We
        // can't cheaply pattern-match the DB error across the two sqlx versions in
        // the graph, so detect the unique constraint by its Postgres error text and
        // return an actionable 400 instead of a generic 500.
        CatalogError::Db(db_err) => {
            let msg = db_err.to_string();
            if msg.contains("duplicate key") || msg.contains("unique constraint") {
                BrowseApiError::BadRequest(
                    "a storage connection with that name already exists".to_string(),
                )
            } else {
                BrowseApiError::Internal
            }
        }
        _ => e.into(),
    }
}

/// GET `/api/ws/{workspace}/storages` — list this workspace's storage conns.
async fn list_storage_conns(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
) -> Result<Json<Vec<WorkspaceStorageConnDto>>, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;
    let conns = st.store.list_storage_conns(ctx.workspace_id.0).await?;
    Ok(Json(
        conns
            .into_iter()
            .map(WorkspaceStorageConnDto::from)
            .collect(),
    ))
}

/// POST `/api/ws/{workspace}/storages` — register a storage conn after a live
/// reachability probe; on success the secret is sealed and the row persisted.
async fn create_storage_conn(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Json(req): Json<CreateStorageConnRequest>,
) -> Result<Response, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;

    if req.provider != "s3" {
        return Err(BrowseApiError::BadRequest(
            "only s3 storage is supported".to_string(),
        ));
    }

    let ws = ctx.workspace_id.0;

    // Phase A: resolve the plaintext to PROBE with, WITHOUT minting anything yet —
    // so a failed probe never orphans a named secret.
    let plaintext =
        resolve_conn_plaintext(&st, ws, req.secret_id, req.new_secret.as_ref(), None).await?;

    let cfg = StorageConfig {
        endpoint: req.endpoint.clone(),
        region: req.region.clone(),
        bucket: req.bucket.clone(),
        access_key: req.access_key.clone(),
        secret_key: plaintext.unwrap_or_default(),
        path_style: req.path_style,
    };
    validate_reachable(&cfg, req.key_prefix.as_deref()).await?;

    // Phase B: only AFTER the probe succeeds, mint the inline `new_secret` (if any)
    // and compute the `secret_ref` to persist.
    let (secret_ref, minted) =
        persist_conn_secret(&st, ws, req.secret_id, req.new_secret.as_ref(), None).await?;

    let input = StorageConnInput {
        name: req.name,
        purpose: req.purpose,
        provider: req.provider,
        endpoint: req.endpoint,
        region: req.region,
        bucket: req.bucket,
        key_prefix: req.key_prefix,
        access_key: req.access_key,
        secret_ref,
        path_style: req.path_style,
    };
    let conn = match st.store.create_storage_conn(ws, &input).await {
        Ok(conn) => conn,
        Err(e) => {
            // Roll back a just-minted named secret so a failed insert never orphans it.
            rollback_minted_secret(&st, ws, minted.then_some(secret_ref).flatten()).await;
            return Err(map_create_conn_error(e));
        }
    };

    Ok((
        StatusCode::CREATED,
        Json(WorkspaceStorageConnDto::from(conn)),
    )
        .into_response())
}

/// PUT `/api/ws/{workspace}/storages/{id}` — edit a storage conn. Resolves the
/// effective secret (request's `secret_key`, else the existing sealed one),
/// re-probes reachability, and on success persists the update; a new secret is
/// sealed only when one was supplied.
async fn update_storage_conn(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Json(req): Json<UpdateStorageConnRequest>,
) -> Result<StatusCode, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;

    if req.provider != "s3" {
        return Err(BrowseApiError::BadRequest(
            "only s3 storage is supported".to_string(),
        ));
    }

    let ws = ctx.workspace_id.0;
    let existing = st
        .store
        .get_storage_conn(ws, id)
        .await?
        .ok_or(BrowseApiError::NotFound)?;

    // Phase A: resolve the plaintext to PROBE with (keep-current when neither
    // `new_secret` nor `secret_id` is supplied), WITHOUT minting anything yet.
    let plaintext = resolve_conn_plaintext(
        &st,
        ws,
        req.secret_id,
        req.new_secret.as_ref(),
        existing.secret_ref,
    )
    .await?;

    let cfg = StorageConfig {
        endpoint: req.endpoint.clone(),
        region: req.region.clone(),
        bucket: req.bucket.clone(),
        access_key: req.access_key.clone(),
        secret_key: plaintext.unwrap_or_default(),
        path_style: req.path_style,
    };
    validate_reachable(&cfg, req.key_prefix.as_deref()).await?;

    // Phase B: only AFTER the probe succeeds, mint the inline `new_secret` (if any)
    // and compute the `secret_ref` to persist (else keep the existing one).
    let (secret_ref, minted) = persist_conn_secret(
        &st,
        ws,
        req.secret_id,
        req.new_secret.as_ref(),
        existing.secret_ref,
    )
    .await?;

    let input = StorageConnInput {
        name: req.name,
        purpose: req.purpose,
        provider: req.provider,
        endpoint: req.endpoint,
        region: req.region,
        bucket: req.bucket,
        key_prefix: req.key_prefix,
        access_key: req.access_key,
        secret_ref,
        path_style: req.path_style,
    };
    let updated = match st.store.update_storage_conn(ws, id, &input).await {
        Ok(updated) => updated,
        Err(e) => {
            // Roll back a just-minted named secret so a failed update never orphans it.
            rollback_minted_secret(&st, ws, minted.then_some(secret_ref).flatten()).await;
            return Err(map_create_conn_error(e));
        }
    };
    if updated {
        Ok(StatusCode::OK)
    } else {
        // The conn vanished between the ownership check and the write; if we minted
        // a secret for it, that secret is now orphaned — roll it back.
        rollback_minted_secret(&st, ws, minted.then_some(secret_ref).flatten()).await;
        Err(BrowseApiError::NotFound)
    }
}

/// DELETE `/api/ws/{workspace}/storages/{id}` — delete a storage conn.
///
/// 404 (not 403) when the id does not belong to this workspace, so the response
/// does not confirm the existence of another tenant's connection (IDOR closed).
async fn delete_storage_conn(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> Result<StatusCode, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;
    let deleted = st.store.delete_storage_conn(ctx.workspace_id.0, id).await?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(BrowseApiError::NotFound)
    }
}

// ---------------------------------------------------------------------------
// Workspace-admin named-secret registry (Epic B): CRUD on `workspace_secrets`.
//
// Gated `workspace:manage_members` (the workspace-admin gate), and IDOR-safe:
// every operation is scoped to `ctx.workspace_id` (bound by `WsContext`) and the
// `WorkspaceSecretStore` queries every row by BOTH id AND workspace_id, so a
// cross-workspace id resolves to 404. The secret VALUE is write-only: it is
// sealed through the vault and NEVER serialized into any response.
// ---------------------------------------------------------------------------

/// A named secret as exposed to the workspace-admin UI. Carries the reference
/// count (`used_by`) but never the value.
#[derive(Serialize)]
struct WorkspaceSecretDto {
    id: Uuid,
    name: String,
    description: Option<String>,
    created_at: DateTime<Utc>,
    used_by: i64,
}

impl From<egret_secrets::WorkspaceSecretMeta> for WorkspaceSecretDto {
    fn from(m: egret_secrets::WorkspaceSecretMeta) -> Self {
        WorkspaceSecretDto {
            id: m.id,
            name: m.name,
            description: m.description,
            created_at: m.created_at,
            used_by: m.used_by,
        }
    }
}

/// Request body to create a named secret. `value` is plaintext, sealed and never
/// returned.
#[derive(Deserialize)]
struct CreateSecretRequest {
    name: String,
    #[serde(default)]
    description: Option<String>,
    value: String,
}

/// Request body to edit a named secret. `name`/`description` rename it (when
/// present); `value` rotates the underlying sealed value (when present).
#[derive(Deserialize)]
struct UpdateSecretRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    value: Option<String>,
}

/// GET `/api/ws/{workspace}/secrets` — list this workspace's named secrets.
async fn list_secrets(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
) -> Result<Json<Vec<WorkspaceSecretDto>>, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;
    let metas = st.workspace_secrets.list(ctx.workspace_id.0).await?;
    Ok(Json(
        metas.into_iter().map(WorkspaceSecretDto::from).collect(),
    ))
}

/// POST `/api/ws/{workspace}/secrets` — create a named secret (201 with meta).
async fn create_secret(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Json(req): Json<CreateSecretRequest>,
) -> Result<Response, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;
    let id = st
        .workspace_secrets
        .create(
            ctx.workspace_id.0,
            &req.name,
            req.description.as_deref(),
            &req.value,
        )
        .await?;
    let meta = st
        .workspace_secrets
        .get_meta(ctx.workspace_id.0, id)
        .await?;
    Ok((StatusCode::CREATED, Json(WorkspaceSecretDto::from(meta))).into_response())
}

/// PUT `/api/ws/{workspace}/secrets/{id}` — rename and/or rotate the value.
///
/// 404 (not 403) when the id does not belong to this workspace (IDOR closed).
async fn update_secret(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Json(req): Json<UpdateSecretRequest>,
) -> Result<Json<WorkspaceSecretDto>, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;
    let ws = ctx.workspace_id.0;

    // Rename when either name or description is supplied; the rename takes the
    // (possibly unchanged) current name when only the description is given.
    if req.name.is_some() || req.description.is_some() {
        let current = st.workspace_secrets.get_meta(ws, id).await?;
        let name = req.name.as_deref().unwrap_or(&current.name);
        let description = match &req.name {
            // Description present (Some) overrides; absent keeps the current one.
            _ if req.description.is_some() => req.description.as_deref(),
            _ => current.description.as_deref(),
        };
        st.workspace_secrets
            .rename(ws, id, name, description)
            .await?;
    }
    if let Some(value) = &req.value {
        st.workspace_secrets.update_value(ws, id, value).await?;
    }

    let meta = st.workspace_secrets.get_meta(ws, id).await?;
    Ok(Json(WorkspaceSecretDto::from(meta)))
}

/// DELETE `/api/ws/{workspace}/secrets/{id}` — delete a named secret (204).
///
/// 404 (not 403) when the id does not belong to this workspace (IDOR closed); a
/// still-referenced secret is 409 with `{ "error": "in use by N connections" }`.
async fn delete_secret(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> Result<StatusCode, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;
    st.workspace_secrets.delete(ctx.workspace_id.0, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Workspace-admin per-connection OBJECT browser (list / download / upload /
// delete) — a MinIO-console-like file manager backend.
//
// Gated `workspace:manage_members` (the workspace-admin gate) and IDOR-safe: the
// connection is resolved via `get_storage_conn(ctx.workspace_id, id)`, which is
// workspace-scoped, so a cross-workspace id → 404 before any S3 call.
//
// PATH SAFETY (the security boundary of this feature): EVERY object op routes its
// user-supplied path through [`safe_key`], which rejects `..` / NUL / a leading
// `/` and joins the path UNDER the connection's `key_prefix` root, asserting the
// result still starts with that root. A user therefore can NEVER read, write,
// list, or delete an object outside their connection's configured root — even
// though the underlying `S3Store` is scoped only to the bucket.
// ---------------------------------------------------------------------------

/// Query string carrying a `key` (single object) for upload/download/delete.
/// `recursive` (delete only) deletes a folder and everything under it.
#[derive(Deserialize)]
struct KeyQuery {
    #[serde(default)]
    key: String,
    #[serde(default)]
    recursive: bool,
}

/// Query string carrying a `path` (folder) for the new-folder route.
#[derive(Deserialize)]
struct PathQuery {
    #[serde(default)]
    path: String,
}

/// Request body for a move/rename: both root-relative paths. A trailing `/` on
/// `from` means "move a whole folder" (and `to` should then also end `/`).
#[derive(Deserialize)]
struct MoveRequest {
    from: String,
    to: String,
}

/// Query string carrying a `prefix` (folder) for listing. Empty = the root.
#[derive(Deserialize)]
struct PrefixQuery {
    #[serde(default)]
    prefix: String,
}

/// One object as exposed to the file-browser UI. `key` is root-relative (the
/// connection's `key_prefix` stripped) so the UI never sees the storage root.
#[derive(Serialize)]
struct ObjectEntry {
    key: String,
    size: i64,
    last_modified: Option<String>,
}

/// Folder-style listing response: root-relative folders + objects.
#[derive(Serialize)]
struct ObjectListing {
    prefix: String,
    folders: Vec<String>,
    objects: Vec<ObjectEntry>,
}

/// Resolve a workspace-scoped connection into its row + a live `S3Store`.
///
/// 404 if the id does not exist or belongs to another workspace (so a
/// cross-tenant id never reaches S3). The connection's sealed secret (if any) is
/// unsealed from the vault; a connection with no `secret_ref` builds an empty
/// secret (matching how the registry persists optional credentials). The returned
/// `WorkspaceStorageConn` is kept so callers can read its `key_prefix` ROOT, which
/// confines every object op.
async fn conn_s3(
    st: &BrowseState,
    ws: Uuid,
    id: Uuid,
) -> Result<(WorkspaceStorageConn, S3Store), BrowseApiError> {
    let conn = st
        .store
        .get_storage_conn(ws, id)
        .await?
        .ok_or(BrowseApiError::NotFound)?;

    let secret_key = match conn.secret_ref {
        Some(secret_ref) => st
            .workspace_secrets
            .resolve_plaintext(ws, secret_ref)
            .await
            .map_err(|_| BrowseApiError::Internal)?,
        None => String::new(),
    };

    let cfg = StorageConfig {
        endpoint: conn.endpoint.clone(),
        region: conn.region.clone(),
        bucket: conn.bucket.clone(),
        access_key: conn.access_key.clone(),
        secret_key,
        path_style: conn.path_style,
    };
    let store = S3Store::new(&cfg)
        .await
        .map_err(|_| BrowseApiError::Internal)?;
    Ok((conn, store))
}

/// Normalize the connection's `key_prefix` root to a canonical form with no
/// leading/trailing `/` (so joins are unambiguous). `None`/empty → `""`.
fn normalize_root(root: Option<&str>) -> String {
    root.unwrap_or("").trim_matches('/').to_string()
}

/// PATH SAFETY — turn a user-supplied path into the EFFECTIVE S3 key under the
/// connection's root, or reject it.
///
/// Rejects (→ 400 `"invalid path"`) any `user_path` that:
///   * contains `..` (a parent-dir traversal),
///   * contains a NUL byte,
///   * starts with `/` (an absolute path).
///
/// On success the effective key is `<root>/<user_path>` (or just `<root>` /
/// `<user_path>` when one side is empty), and we ASSERT the result still starts
/// with `<root>` — a defence-in-depth check so the op can never escape the root.
/// An empty `user_path` is allowed and resolves to the root itself (used by list).
fn safe_key(root: Option<&str>, user_path: &str) -> Result<String, BrowseApiError> {
    let invalid = || BrowseApiError::BadRequest("invalid path".to_string());

    if user_path.contains("..") || user_path.contains('\0') || user_path.starts_with('/') {
        return Err(invalid());
    }

    let root = normalize_root(root);
    // Trim any leading `/` defensively (already rejected above) and collapse a
    // bare user path.
    let user = user_path.trim_start_matches('/');

    let key = match (root.is_empty(), user.is_empty()) {
        (true, _) => user.to_string(),
        (false, true) => root.clone(),
        (false, false) => format!("{root}/{user}"),
    };

    // Defence-in-depth: the effective key must remain confined to the root.
    if !root.is_empty() && key != root && !key.starts_with(&format!("{root}/")) {
        return Err(invalid());
    }
    Ok(key)
}

/// Strip the connection root prefix from an S3-returned key/folder so the UI sees
/// root-relative paths. Keys outside the root (shouldn't happen, since `list` is
/// always called with a root-prefixed prefix) are returned unchanged.
fn strip_root(root: Option<&str>, key: &str) -> String {
    let root = normalize_root(root);
    if root.is_empty() {
        return key.to_string();
    }
    key.strip_prefix(&format!("{root}/"))
        .or_else(|| key.strip_prefix(&root))
        .unwrap_or(key)
        .to_string()
}

/// Build the `s3://bucket/key` URI the `ObjectStore` get/put/delete expect.
fn s3_uri(bucket: &str, key: &str) -> String {
    format!("s3://{bucket}/{key}")
}

/// GET `/api/ws/{workspace}/storages/{id}/objects?prefix=<p>` — folder-style list.
async fn list_objects(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Query(q): Query<PrefixQuery>,
) -> Result<Json<ObjectListing>, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;
    let (conn, store) = conn_s3(&st, ctx.workspace_id.0, id).await?;
    let root = conn.key_prefix.as_deref();

    // An empty prefix lists the root itself. The effective S3 prefix must end in
    // `/` so a folder-style listing returns the folder's CONTENTS (objects +
    // sub-folders) rather than sibling keys sharing the prefix string.
    let effective = safe_key(root, &q.prefix)?;
    let list_prefix = if effective.is_empty() || effective.ends_with('/') {
        effective.clone()
    } else {
        format!("{effective}/")
    };
    let listing = store
        .list(&list_prefix, Some("/"))
        .await
        .map_err(|_| BrowseApiError::Internal)?;

    let folders = listing
        .folders
        .iter()
        .map(|f| strip_root(root, f))
        .collect();
    let objects = listing
        .objects
        .iter()
        // Drop the current folder's own marker object (an empty key equal to the
        // listing prefix, e.g. `<root>/test/`). It backs the folder so it shows
        // up in the PARENT listing, but inside the folder it must not appear as a
        // phantom 0-byte file named after the folder.
        .filter(|o| o.key != list_prefix)
        .map(|o| ObjectEntry {
            key: strip_root(root, &o.key),
            size: o.size,
            last_modified: o.last_modified.clone(),
        })
        .collect();

    Ok(Json(ObjectListing {
        prefix: q.prefix,
        folders,
        objects,
    }))
}

/// Cap on a single downloaded object (100 MB) — refuse larger so a huge object
/// cannot be buffered whole into memory.
const MAX_DOWNLOAD_BYTES: usize = 100 * 1024 * 1024;

/// GET `/api/ws/{workspace}/storages/{id}/objects/content?key=<k>` — download.
async fn download_object(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Query(q): Query<KeyQuery>,
) -> Result<Response, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;
    if q.key.is_empty() {
        return Err(BrowseApiError::BadRequest("invalid path".to_string()));
    }
    let (conn, store) = conn_s3(&st, ctx.workspace_id.0, id).await?;
    let key = safe_key(conn.key_prefix.as_deref(), &q.key)?;

    let bytes = store
        .get(&s3_uri(&conn.bucket, &key))
        .await
        .map_err(|e| match e {
            egret_storage::StorageError::NotFound(_) => BrowseApiError::NotFound,
            _ => BrowseApiError::Internal,
        })?;
    if bytes.len() > MAX_DOWNLOAD_BYTES {
        return Err(BrowseApiError::BadRequest(
            "object too large to download".to_string(),
        ));
    }

    // Basename for the download filename (the part after the last `/`).
    let basename = key.rsplit('/').next().unwrap_or(&key);
    let disposition = format!("attachment; filename=\"{}\"", sanitize_filename(basename));

    let mut resp = (StatusCode::OK, bytes).into_response();
    let headers = resp.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    if let Ok(v) = HeaderValue::from_str(&disposition) {
        headers.insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(resp)
}

/// Strip characters that would break the `filename="..."` header (quotes, control
/// chars, path separators) so a crafted key can't inject header syntax.
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control() && *c != '"' && *c != '\\' && *c != '/')
        .collect()
}

/// POST `/api/ws/{workspace}/storages/{id}/objects?key=<k>` — upload raw bytes.
async fn upload_object(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Query(q): Query<KeyQuery>,
    body: Bytes,
) -> Result<StatusCode, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;
    if q.key.is_empty() {
        return Err(BrowseApiError::BadRequest("invalid path".to_string()));
    }
    let (conn, store) = conn_s3(&st, ctx.workspace_id.0, id).await?;
    let key = safe_key(conn.key_prefix.as_deref(), &q.key)?;

    store
        .put(&s3_uri(&conn.bucket, &key), body.to_vec())
        .await
        .map_err(|_| BrowseApiError::Internal)?;
    Ok(StatusCode::CREATED)
}

/// DELETE `/api/ws/{workspace}/storages/{id}/objects?key=<k>[&recursive=<bool>]`
/// — delete one object, or (folder + `recursive=true`) a folder and everything
/// under it.
///
/// A `key` ending in `/` OR `recursive=true` is treated as a folder: we `list`
/// the folder flat (NO delimiter → `Listing.objects` are FULL, root-prefixed
/// keys, since `list` does not strip the root) and `delete` every returned
/// object, then the folder marker itself. Best-effort: if any delete errors we
/// stop and return 500 reporting how many were removed.
async fn delete_object(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Query(q): Query<KeyQuery>,
) -> Result<StatusCode, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;
    if q.key.is_empty() {
        return Err(BrowseApiError::BadRequest("invalid path".to_string()));
    }
    let (conn, store) = conn_s3(&st, ctx.workspace_id.0, id).await?;
    let root = conn.key_prefix.as_deref();
    let key = safe_key(root, &q.key)?;

    let is_folder = q.recursive || q.key.ends_with('/');
    if is_folder {
        // The effective folder prefix MUST end in `/` so the flat list returns
        // only objects under this folder (not sibling keys sharing the string).
        let folder_prefix = if key.ends_with('/') {
            key.clone()
        } else {
            format!("{key}/")
        };
        // Flat list (delimiter=None): `objects` are full root-prefixed keys.
        let listing = store
            .list(&folder_prefix, None)
            .await
            .map_err(|_| BrowseApiError::Internal)?;

        // Best-effort: on the first delete error we stop and report 500. Some
        // objects may already be removed (a partial delete the caller can retry —
        // re-issuing the recursive delete is idempotent).
        let mut removed = 0usize;
        for obj in &listing.objects {
            store
                .delete(&s3_uri(&conn.bucket, &obj.key))
                .await
                .map_err(|e| {
                    tracing::warn!(error = ?e, removed, "recursive delete failed mid-way");
                    BrowseApiError::Internal
                })?;
            removed += 1;
        }
        let _ = removed;
        // Delete the folder marker too (idempotent if it doesn't exist).
        store
            .delete(&s3_uri(&conn.bucket, &folder_prefix))
            .await
            .map_err(|_| BrowseApiError::Internal)?;
        return Ok(StatusCode::NO_CONTENT);
    }

    store
        .delete(&s3_uri(&conn.bucket, &key))
        .await
        .map_err(|_| BrowseApiError::Internal)?;
    Ok(StatusCode::NO_CONTENT)
}

/// POST `/api/ws/{workspace}/storages/{id}/objects/folder?path=<p>` — create an
/// empty folder by writing a zero-byte `<root>/<p>/` marker object.
///
/// Path-confined under the connection root via [`safe_key`]; an empty `path` is
/// rejected (400). The effective key is normalized to exactly one trailing `/`.
async fn create_folder(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Query(q): Query<PathQuery>,
) -> Result<StatusCode, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;
    if q.path.trim().is_empty() {
        return Err(BrowseApiError::BadRequest("invalid path".to_string()));
    }
    let (conn, store) = conn_s3(&st, ctx.workspace_id.0, id).await?;
    let key = safe_key(conn.key_prefix.as_deref(), q.path.trim_end_matches('/'))?;
    let marker = format!("{key}/");

    store
        .put(&s3_uri(&conn.bucket, &marker), Vec::new())
        .await
        .map_err(|_| BrowseApiError::Internal)?;
    Ok(StatusCode::CREATED)
}

/// POST `/api/ws/{workspace}/storages/{id}/objects/move` — move/rename a file or
/// folder via copy-then-delete.
///
/// Both `from` and `to` are path-confined under the connection root via
/// [`safe_key`]. Rejects (400): `from == to`, and a folder-into-itself move
/// (`from` ends `/` and `to` starts with `from`). A file (`from` not ending `/`)
/// is a single copy+delete; a folder (`from` ends `/`) flat-lists under the
/// prefix (full root-prefixed keys) and copies+deletes each, re-rooting each key
/// under `to`, then moves the folder marker. Best-effort on partial failure.
async fn move_object(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    Json(req): Json<MoveRequest>,
) -> Result<StatusCode, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;
    if req.from.trim().is_empty() || req.to.trim().is_empty() {
        return Err(BrowseApiError::BadRequest("invalid path".to_string()));
    }
    // `from == to` is a no-op move — reject (compare the RAW user paths).
    if req.from == req.to {
        return Err(BrowseApiError::BadRequest(
            "source and destination are the same".to_string(),
        ));
    }
    // Folder-into-itself: moving `f/` into `f/h/` would recurse — reject.
    if req.from.ends_with('/') && req.to.starts_with(&req.from) {
        return Err(BrowseApiError::BadRequest(
            "cannot move a folder into itself".to_string(),
        ));
    }

    let (conn, store) = conn_s3(&st, ctx.workspace_id.0, id).await?;
    let root = conn.key_prefix.as_deref();
    let from_key = safe_key(root, &req.from)?;
    let to_key = safe_key(root, &req.to)?;
    let bucket = &conn.bucket;

    let is_folder = req.from.ends_with('/');
    if !is_folder {
        // Single file: copy then delete.
        store
            .copy(&s3_uri(bucket, &from_key), &s3_uri(bucket, &to_key))
            .await
            .map_err(|e| match e {
                egret_storage::StorageError::NotFound(_) => BrowseApiError::NotFound,
                _ => BrowseApiError::Internal,
            })?;
        store
            .delete(&s3_uri(bucket, &from_key))
            .await
            .map_err(|_| BrowseApiError::Internal)?;
        return Ok(StatusCode::OK);
    }

    // Folder move. Normalize both effective keys to a single trailing `/`.
    let from_prefix = if from_key.ends_with('/') {
        from_key.clone()
    } else {
        format!("{from_key}/")
    };
    let to_prefix = if to_key.ends_with('/') {
        to_key.clone()
    } else {
        format!("{to_key}/")
    };

    // Flat list (delimiter=None): full root-prefixed keys under the folder.
    let listing = store
        .list(&from_prefix, None)
        .await
        .map_err(|_| BrowseApiError::Internal)?;

    for obj in &listing.objects {
        // Re-root each key from `from_prefix` to `to_prefix`. `list` returns keys
        // under `from_prefix`, so the strip always succeeds; fall back defensively.
        let rel = obj.key.strip_prefix(&from_prefix).unwrap_or(&obj.key);
        let dst = format!("{to_prefix}{rel}");
        store
            .copy(&s3_uri(bucket, &obj.key), &s3_uri(bucket, &dst))
            .await
            .map_err(|_| BrowseApiError::Internal)?;
        store
            .delete(&s3_uri(bucket, &obj.key))
            .await
            .map_err(|_| BrowseApiError::Internal)?;
    }
    // Move the folder marker (best-effort: the source marker may not exist).
    let _ = store
        .copy(&s3_uri(bucket, &from_prefix), &s3_uri(bucket, &to_prefix))
        .await;
    let _ = store.delete(&s3_uri(bucket, &from_prefix)).await;

    Ok(StatusCode::OK)
}
