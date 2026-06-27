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
use lakekeeper::api::management::v1::warehouse::{
    Service as WarehouseService, UpdateWarehouseStorageRequest,
};
use lakekeeper::api::management::v1::ApiServer;
use lakekeeper::api::{ApiContext, RequestMetadata};
use lakekeeper::iceberg::{NamespaceIdent, TableIdent};
use lakekeeper::service::WarehouseId;
use lakekeeper_storage_postgres::{PostgresBackend, SecretsState};

use crate::authorizer::EgretAuthorizer;
use crate::bootstrap::system_request_metadata;
use crate::mount::{EngineServer, EngineState};

/// Permission required to read a workspace's catalog.
const PERM: &str = "catalog:read";

/// `ApiServer` specialized to egret's concrete backends — the type whose
/// management `Service` impl carries `get_warehouse`.
type EngineApiServer = ApiServer<PostgresBackend, EgretAuthorizer, SecretsState>;

/// State for the lakekeeper-reading browse API. Carries the embedded engine
/// [`ApiContext`] (lakekeeper reads), the egret-side [`CatalogStore`]
/// (`workspace → warehouse`), the configured `system_user` (to mint the system
/// principal lakekeeper's `Service` calls authorize against), and the
/// session/workspace/authz handles the [`WsContext`] extractor needs.
#[derive(Clone)]
pub struct BrowseState {
    pub engine: ApiContext<EngineState>,
    pub store: CatalogStore,
    /// Vault used to seal/unseal per-workspace S3 secret keys (CE-5). Carried so
    /// the storage-update handler can persist a new secret and read the existing
    /// one when a re-point omits `secret_key`.
    pub secret_store: egret_secrets::SecretStore,
    /// The GLOBAL default S3 storage config (the same one threaded into the
    /// `EngineWarehouseProvisioner`). Used to (a) lazily provision a warehouse for
    /// a workspace that predates catalog-enablement and has no custom storage row,
    /// and (b) report the EFFECTIVE storage config from `GET .../catalog/storage`
    /// when no per-workspace `workspace_storage` row exists. Never serialized
    /// directly — only its non-secret fields surface, and `secret_key` is reported
    /// solely as the `has_secret` boolean.
    pub default_storage: egret_storage::StorageConfig,
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
        .route("/ws/{workspace}/catalog", get(summary))
        .route("/ws/{workspace}/catalog/namespaces", get(list_namespaces))
        .route(
            "/ws/{workspace}/catalog/namespaces/{ns}/tables",
            get(list_tables),
        )
        .route(
            "/ws/{workspace}/catalog/namespaces/{ns}/tables/{table}",
            get(table_detail),
        )
        // Workspace-admin catalog token management (gated `workspace:manage_members`).
        .route(
            "/ws/{workspace}/catalog/tokens",
            get(list_tokens).post(mint_token),
        )
        .route("/ws/{workspace}/catalog/tokens/{id}", delete(revoke_token))
        // Workspace-admin storage read (effective config) + re-point (gated
        // `workspace:manage_members`). The GET lazily provisions a warehouse so the
        // Storage page is usable on a workspace that predates catalog enablement.
        .route(
            "/ws/{workspace}/catalog/storage",
            get(get_storage).put(update_storage),
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
    /// The session's workspace has no provisioned warehouse yet → 404.
    NoWarehouse,
    /// A lakekeeper `Service` call failed (preserves its iceberg HTTP code).
    Engine(lakekeeper::api::IcebergErrorResponse),
    /// Client-supplied input was invalid → 400.
    BadRequest(String),
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
            BrowseApiError::NoWarehouse => {
                (StatusCode::NOT_FOUND, "catalog not provisioned".to_string())
            }
            BrowseApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
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

/// Compact catalog summary for the UI.
#[derive(Serialize)]
struct CatalogSummary {
    id: String,
    name: String,
    warehouse_location: String,
}

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

/// Resolved per-request engine handle: the warehouse `Prefix` for the session's
/// workspace plus a fresh system `RequestMetadata`. Produced only AFTER the
/// `catalog:read` authz check passes, so the egret tenant boundary is enforced
/// before any lakekeeper read.
struct Resolved {
    prefix: Prefix,
    warehouse_id: Uuid,
    request_metadata: RequestMetadata,
}

/// Authorize `catalog:read` on the active workspace, then resolve its lakekeeper
/// warehouse. 404 if the workspace has no warehouse provisioned.
async fn authorized_warehouse(
    st: &BrowseState,
    ctx: &WorkspaceContext,
) -> Result<Resolved, BrowseApiError> {
    // RBAC FIRST: the session must hold `catalog:read` on the path workspace.
    st.authz.require_workspace(ctx, PERM).await?;

    // Then resolve the workspace's lakekeeper warehouse. `WsContext` already
    // bound `ctx.workspace_id` to the path slug + membership, so this is the
    // caller's own workspace — a member of X cannot resolve Y's warehouse.
    let warehouse_id = st
        .store
        .warehouse_for_workspace(ctx.workspace_id.0)
        .await
        .map_err(|_| BrowseApiError::Internal)?
        .ok_or(BrowseApiError::NoWarehouse)?;

    let request_metadata =
        system_request_metadata(&st.system_user).map_err(|_| BrowseApiError::Internal)?;

    Ok(Resolved {
        // lakekeeper's default `prefix_template = "{warehouse_id}"`.
        prefix: Prefix::from(warehouse_id.to_string()),
        warehouse_id,
        request_metadata,
    })
}

/// Lazily ensure the workspace has a provisioned lakekeeper warehouse, returning
/// its UUID.
///
/// Warehouse provisioning used to be create-time only, so a workspace created
/// before the catalog was enabled (e.g. `eng`) has NO warehouse and its Catalog /
/// Storage pages are unusable. This makes provisioning lazy: the first
/// catalog/storage access provisions the warehouse on demand.
///
/// * If `warehouse_for_workspace(ws)` already returns `Some`, that id is returned
///   without touching lakekeeper.
/// * Otherwise the effective [`StorageConfig`] is resolved — the workspace's
///   custom `workspace_storage` row (with its secret unsealed from the vault) if
///   present, else the global [`BrowseState::default_storage`] — and
///   [`provision_workspace_warehouse`](crate::provision_ws::provision_workspace_warehouse)
///   (itself idempotent) creates the warehouse + persists the mapping.
///
/// Callers MUST authorize the workspace first; this resolves the slug from the
/// workspace repo for the warehouse name + key prefix.
async fn ensure_warehouse(
    st: &BrowseState,
    ctx: &WorkspaceContext,
) -> Result<Uuid, BrowseApiError> {
    let ws_id = ctx.workspace_id.0;

    // Fast path: already provisioned.
    if let Some(existing) = st
        .store
        .warehouse_for_workspace(ws_id)
        .await
        .map_err(|_| BrowseApiError::Internal)?
    {
        return Ok(existing);
    }

    // Resolve the workspace slug (warehouse name + `workspaces/<slug>/warehouse`
    // key prefix). `WsContext` already bound `ctx.workspace_id` to the path slug.
    let slug = st
        .workspaces
        .get_by_id(ctx.workspace_id)
        .await
        .map_err(|_| BrowseApiError::Internal)?
        .ok_or(BrowseApiError::NotFound)?
        .slug;

    // Resolve the EFFECTIVE storage config: the workspace's custom row (with its
    // sealed secret unsealed), else the global default.
    let cfg = match st
        .store
        .workspace_storage_for(ws_id)
        .await
        .map_err(|_| BrowseApiError::Internal)?
    {
        Some(row) => {
            let secret_key = match row.secret_ref {
                Some(secret_ref) => st
                    .secret_store
                    .get(secret_ref)
                    .await
                    .map_err(|_| BrowseApiError::Internal)?,
                // A custom row without a sealed secret falls back to the default's
                // secret (the access key still comes from the row).
                None => st.default_storage.secret_key.clone(),
            };
            egret_storage::StorageConfig {
                endpoint: row.endpoint.clone(),
                region: row.region.clone(),
                bucket: row.bucket.clone(),
                access_key: row.access_key.clone(),
                secret_key,
                path_style: row.path_style,
            }
        }
        None => st.default_storage.clone(),
    };

    let warehouse_id = crate::provision_ws::provision_workspace_warehouse(
        st.engine.clone(),
        &st.store,
        ws_id,
        &slug,
        &cfg,
        &st.system_user,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = ?e, workspace_id = %ws_id, "lazy warehouse provisioning failed");
        BrowseApiError::Internal
    })?;

    Ok(warehouse_id)
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

/// GET `/api/ws/{workspace}/catalog` — catalog summary (warehouse id/name/loc).
async fn summary(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
) -> Result<Json<CatalogSummary>, BrowseApiError> {
    // RBAC FIRST: the session must hold `catalog:read` on the path workspace.
    st.authz.require_workspace(&ctx, PERM).await?;

    // Lazily provision the warehouse if this workspace predates catalog enablement
    // (e.g. `eng`), so the Catalog page returns the empty catalog instead of a
    // "not provisioned" state on first view. Provisioning failure → 5xx.
    let warehouse_id = ensure_warehouse(&st, &ctx).await?;
    let request_metadata =
        system_request_metadata(&st.system_user).map_err(|_| BrowseApiError::Internal)?;

    let r = Resolved {
        prefix: Prefix::from(warehouse_id.to_string()),
        warehouse_id,
        request_metadata,
    };

    let wh = <EngineApiServer as WarehouseService<_, _, _>>::get_warehouse(
        WarehouseId::from(r.warehouse_id),
        st.engine.clone(),
        r.request_metadata,
    )
    .await?;

    // `warehouse_location` mirrors the old DTO's storage root. Derive it from the
    // storage profile's base location; fall back to empty if unavailable.
    let warehouse_location = wh
        .storage_profile
        .base_location()
        .map(|loc| loc.to_string())
        .unwrap_or_default();

    Ok(Json(CatalogSummary {
        id: r.warehouse_id.to_string(),
        name: wh.name,
        warehouse_location,
    }))
}

/// GET `/api/ws/{workspace}/catalog/namespaces` — ALL namespaces (flattened).
///
/// lakekeeper's `list_namespaces(parent=None)` returns only top-level
/// namespaces; the UI (like the old browse) wants the whole tree flat, so we
/// recurse, paginating each level.
async fn list_namespaces(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
) -> Result<Json<Vec<NamespaceDto>>, BrowseApiError> {
    let r = authorized_warehouse(&st, &ctx).await?;

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
    Path((_ws, ns)): Path<(String, String)>,
) -> Result<Json<NamespaceContents>, BrowseApiError> {
    let r = authorized_warehouse(&st, &ctx).await?;
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
    Path((_ws, ns, table)): Path<(String, String, String)>,
) -> Result<Json<TableDetail>, BrowseApiError> {
    let r = authorized_warehouse(&st, &ctx).await?;
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

/// GET `/api/ws/{workspace}/catalog/tokens` — list this workspace's tokens.
async fn list_tokens(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
) -> Result<Json<Vec<CatalogTokenDto>>, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;
    let tokens = st.store.list_tokens(ctx.workspace_id.0).await?;
    Ok(Json(
        tokens.into_iter().map(CatalogTokenDto::from).collect(),
    ))
}

/// POST `/api/ws/{workspace}/catalog/tokens` — mint a new token.
async fn mint_token(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Json(req): Json<MintTokenRequest>,
) -> Result<Json<MintTokenResponse>, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;

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
        .create_token(ctx.workspace_id.0, &req.label, &perms, &req.namespace_scope)
        .await?;

    Ok(Json(MintTokenResponse {
        token: token.into(),
        raw,
    }))
}

/// DELETE `/api/ws/{workspace}/catalog/tokens/{id}` — revoke a token.
///
/// 404 (not 403) when the id does not belong to this workspace, so the response
/// does not confirm the existence of another tenant's token (IDOR closed).
async fn revoke_token(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> Result<StatusCode, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;
    let revoked = st
        .store
        .revoke_token_for_workspace(id, ctx.workspace_id.0)
        .await?;
    if revoked {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(BrowseApiError::NotFound)
    }
}

// ---------------------------------------------------------------------------
// Workspace-admin storage re-point: `PUT /api/ws/{workspace}/catalog/storage`.
//
// Gated `workspace:manage_members` (same admin gate as token management), and
// IDOR-safe (scoped to `ctx.workspace_id`, bound by `WsContext`). Re-points the
// workspace's lakekeeper warehouse storage AND persists the new
// `workspace_storage` row, sealing any new secret through the vault.
//
// Ordering is load-bearing: lakekeeper is updated FIRST so its `validate_access`
// (a live S3 round-trip) rejects bad credentials before egret mutates its own
// row — a failed update leaves the egret `workspace_storage` row UNCHANGED.
//
// lakekeeper requires the re-pointed profile to keep the SAME location (bucket +
// key prefix); the warehouse was created at `workspaces/<slug>/warehouse`, so the
// handler reuses [`crate::provision_ws::build_storage_profile`] with the path
// slug to derive an identically-located profile.
// ---------------------------------------------------------------------------

/// Request body for re-pointing a workspace's S3/MinIO storage. `secret_key` is
/// optional: when `None`, the existing sealed secret is reused (keep-current).
/// `key_prefix` is accepted for shape-parity with the storage DTO but is NOT
/// applied — the warehouse layout uses the fixed `workspaces/<slug>/warehouse`
/// prefix (changing it would move the location, which lakekeeper rejects).
#[derive(Deserialize)]
struct UpdateStorageRequest {
    provider: String,
    endpoint: Option<String>,
    region: String,
    bucket: String,
    #[allow(dead_code)]
    key_prefix: Option<String>,
    access_key: String,
    secret_key: Option<String>,
    path_style: bool,
}

/// GET `/api/ws/{workspace}/catalog/storage` — the EFFECTIVE storage config.
///
/// Gated `workspace:manage_members` (same admin gate as the re-point), and
/// IDOR-safe (scoped to `ctx.workspace_id`, bound by `WsContext`). Lazily
/// provisions the warehouse first (so a never-visited workspace becomes usable),
/// then returns the secret-free [`WorkspaceStorageInfo`]:
///   * if a custom `workspace_storage` row exists → from that row,
///   * else → derived from the global [`BrowseState::default_storage`] (the
///     effective config the warehouse was provisioned with), with the canonical
///     `workspaces/<slug>/warehouse` key prefix.
///
/// The secret VALUE is never returned — only `has_secret` reflects whether a
/// sealed secret exists (a row's `secret_ref`, or the default's non-empty
/// `secret_key`).
async fn get_storage(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path(slug): Path<String>,
) -> Result<Json<egret_workspace::provision::WorkspaceStorageInfo>, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;

    // Lazily provision the warehouse so a workspace that predates catalog
    // enablement gets one on first Storage view (GET has this side effect by
    // design — the Storage page must be usable without an explicit provision step).
    ensure_warehouse(&st, &ctx).await?;

    let info = match st
        .store
        .workspace_storage_for(ctx.workspace_id.0)
        .await
        .map_err(|_| BrowseApiError::Internal)?
    {
        Some(row) => egret_workspace::provision::WorkspaceStorageInfo {
            provider: row.provider,
            endpoint: row.endpoint,
            region: row.region,
            bucket: row.bucket,
            key_prefix: row.key_prefix,
            access_key: row.access_key,
            has_secret: row.secret_ref.is_some(),
        },
        None => {
            let d = &st.default_storage;
            egret_workspace::provision::WorkspaceStorageInfo {
                provider: "s3".to_string(),
                endpoint: d.endpoint.clone(),
                region: d.region.clone(),
                bucket: d.bucket.clone(),
                key_prefix: Some(format!("workspaces/{slug}/warehouse")),
                access_key: d.access_key.clone(),
                has_secret: !d.secret_key.is_empty(),
            }
        }
    };

    Ok(Json(info))
}

/// PUT `/api/ws/{workspace}/catalog/storage` — re-point the warehouse storage.
async fn update_storage(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path(slug): Path<String>,
    Json(req): Json<UpdateStorageRequest>,
) -> Result<Json<egret_workspace::provision::WorkspaceStorageInfo>, BrowseApiError> {
    update_storage_inner(st, ctx, slug, req).await
}

async fn update_storage_inner(
    st: BrowseState,
    ctx: WorkspaceContext,
    slug: String,
    req: UpdateStorageRequest,
) -> Result<Json<egret_workspace::provision::WorkspaceStorageInfo>, BrowseApiError> {
    require_workspace_admin(&st, &ctx).await?;

    // 0. Only S3/MinIO is supported — reject any other provider BEFORE building a
    //    profile, so the egret row can never record a provider that diverges from
    //    what lakekeeper actually stores (always S3).
    if req.provider != "s3" {
        return Err(BrowseApiError::BadRequest(
            "only s3 storage is supported".to_string(),
        ));
    }

    // 1. Lazily provision the warehouse if missing, so a re-point works even on a
    //    never-visited workspace (one that predates catalog enablement) instead of
    //    409-ing. Idempotent: a no-op when the warehouse already exists.
    let warehouse_id = ensure_warehouse(&st, &ctx).await?;

    // 2. Resolve the effective secret: the request's `secret_key` if present,
    //    else the existing sealed secret (keep-current). Without either we cannot
    //    build S3 credentials.
    let existing = st
        .store
        .workspace_storage_for(ctx.workspace_id.0)
        .await
        .map_err(|_| BrowseApiError::Internal)?;
    let secret_key = match &req.secret_key {
        Some(s) => s.clone(),
        None => {
            let secret_ref = existing
                .as_ref()
                .and_then(|row| row.secret_ref)
                .ok_or_else(|| {
                    BrowseApiError::BadRequest(
                        "secret_key is required (no existing secret to keep)".to_string(),
                    )
                })?;
            st.secret_store
                .get(secret_ref)
                .await
                .map_err(|_| BrowseApiError::Internal)?
        }
    };

    // 3. Build the egret StorageConfig + the lakekeeper profile/credential. The
    //    profile reuses the `workspaces/<slug>/warehouse` prefix so the location
    //    is unchanged (lakekeeper rejects a location change on re-point).
    let cfg = egret_storage::StorageConfig {
        endpoint: req.endpoint.clone(),
        region: req.region.clone(),
        bucket: req.bucket.clone(),
        access_key: req.access_key.clone(),
        secret_key,
        path_style: req.path_style,
    };
    let storage_profile = crate::provision_ws::build_storage_profile(&cfg, &slug)
        .map_err(|e| BrowseApiError::BadRequest(format!("invalid storage profile: {e}")))?;
    let storage_credential = crate::provision_ws::build_storage_credential(&cfg);

    // 4. lakekeeper FIRST — `update_storage` runs `validate_access` (a live S3
    //    round-trip), so bad credentials fail HERE, before egret mutates its row.
    let rm = system_request_metadata(&st.system_user).map_err(|_| BrowseApiError::Internal)?;
    <EngineApiServer as WarehouseService<_, _, _>>::update_storage(
        WarehouseId::from(warehouse_id),
        UpdateWarehouseStorageRequest {
            storage_profile,
            storage_credential: Some(storage_credential),
        },
        st.engine.clone(),
        rm,
    )
    .await
    // A rejected re-point (bad creds / invalid profile / location change) surfaces
    // as 400; the egret row is left UNCHANGED because we return before step 5. The
    // admin configured these credentials, so lakekeeper's message
    // (`InvalidAccessKeyId` / `NoSuchBucket` / …) is actionable — forward a TRIMMED
    // copy (cap length so a large upstream dump can't be echoed verbatim) and log
    // the full error at WARN for operators.
    .map_err(|e| {
        tracing::warn!(error = ?e, "lakekeeper storage update rejected");
        let mut msg = e.error.message.clone();
        msg.truncate(200);
        BrowseApiError::BadRequest(format!("storage update rejected: {msg}"))
    })?;

    // ----- Partial-failure / divergence window -------------------------------
    // lakekeeper's warehouse is now re-pointed (Step 4 committed). Step 5 below
    // (sealing a new secret + upserting the egret `workspace_storage` row) could
    // still fail — if it does, the warehouse points at the NEW storage while
    // egret's row still describes the OLD one (a transient divergence). We do NOT
    // run a cross-system 2-phase commit here; the window is recoverable by simply
    // re-issuing this PUT (idempotent: lakekeeper accepts the same re-point and the
    // egret upsert overwrites the row), which we accept for this pass.
    // -------------------------------------------------------------------------

    // 5. ONLY on lakekeeper success: persist the egret row. Seal a new secret if
    //    one was supplied; otherwise keep the existing `secret_ref`.
    let secret_ref = match &req.secret_key {
        Some(s) => Some(
            st.secret_store
                .put("workspace_s3_secret", s)
                .await
                .map_err(|_| BrowseApiError::Internal)?,
        ),
        None => existing.as_ref().and_then(|row| row.secret_ref),
    };

    let row = egret_catalog::WorkspaceStorageRow {
        workspace_id: ctx.workspace_id.0,
        provider: req.provider.clone(),
        endpoint: req.endpoint.clone(),
        region: req.region.clone(),
        bucket: req.bucket.clone(),
        // Record the CANONICAL warehouse prefix (the same derivation
        // `build_storage_profile` feeds lakekeeper), NOT the request's `key_prefix`
        // (which is not applied) nor the possibly-stale existing-row value — so the
        // persisted row and the returned `WorkspaceStorageInfo` always match the
        // real warehouse layout.
        key_prefix: Some(format!("workspaces/{slug}/warehouse")),
        access_key: req.access_key.clone(),
        secret_ref,
        path_style: req.path_style,
    };
    st.store.upsert_workspace_storage(&row).await?;

    Ok(Json(egret_workspace::provision::WorkspaceStorageInfo {
        provider: row.provider,
        endpoint: row.endpoint,
        region: row.region,
        bucket: row.bucket,
        key_prefix: row.key_prefix,
        access_key: row.access_key,
        has_secret: row.secret_ref.is_some(),
    }))
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

/// Request body to register a storage connection. `secret_key` is REQUIRED on
/// create (we need credentials to seal + probe).
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
    secret_key: String,
    path_style: bool,
}

/// Request body to edit a storage connection. `secret_key` is OPTIONAL: when
/// `None`, the existing sealed secret is reused (keep-current).
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
    secret_key: Option<String>,
    path_style: bool,
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

    let cfg = StorageConfig {
        endpoint: req.endpoint.clone(),
        region: req.region.clone(),
        bucket: req.bucket.clone(),
        access_key: req.access_key.clone(),
        secret_key: req.secret_key.clone(),
        path_style: req.path_style,
    };
    validate_reachable(&cfg, req.key_prefix.as_deref()).await?;

    let secret_ref = st
        .secret_store
        .put("workspace_storage_conn", &req.secret_key)
        .await
        .map_err(|_| BrowseApiError::Internal)?;

    let input = StorageConnInput {
        name: req.name,
        purpose: req.purpose,
        provider: req.provider,
        endpoint: req.endpoint,
        region: req.region,
        bucket: req.bucket,
        key_prefix: req.key_prefix,
        access_key: req.access_key,
        secret_ref: Some(secret_ref),
        path_style: req.path_style,
    };
    let conn = st
        .store
        .create_storage_conn(ctx.workspace_id.0, &input)
        .await
        .map_err(map_create_conn_error)?;

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

    let existing = st
        .store
        .get_storage_conn(ctx.workspace_id.0, id)
        .await?
        .ok_or(BrowseApiError::NotFound)?;

    // Resolve the effective secret: the request's `secret_key` if present, else
    // the existing sealed secret (keep-current).
    let secret_key = match &req.secret_key {
        Some(s) => s.clone(),
        None => {
            let secret_ref = existing.secret_ref.ok_or_else(|| {
                BrowseApiError::BadRequest(
                    "secret_key is required (no existing secret to keep)".to_string(),
                )
            })?;
            st.secret_store
                .get(secret_ref)
                .await
                .map_err(|_| BrowseApiError::Internal)?
        }
    };

    let cfg = StorageConfig {
        endpoint: req.endpoint.clone(),
        region: req.region.clone(),
        bucket: req.bucket.clone(),
        access_key: req.access_key.clone(),
        secret_key,
        path_style: req.path_style,
    };
    validate_reachable(&cfg, req.key_prefix.as_deref()).await?;

    // Seal a new secret only when one was supplied; otherwise keep the existing
    // `secret_ref`.
    // TODO: new secret_ref orphans in the vault if the subsequent
    // update_storage_conn fails; clean up old/new refs holistically later.
    let secret_ref = match &req.secret_key {
        Some(s) => Some(
            st.secret_store
                .put("workspace_storage_conn", s)
                .await
                .map_err(|_| BrowseApiError::Internal)?,
        ),
        None => existing.secret_ref,
    };

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
    let updated = st
        .store
        .update_storage_conn(ctx.workspace_id.0, id, &input)
        .await
        .map_err(map_create_conn_error)?;
    if updated {
        Ok(StatusCode::OK)
    } else {
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
            .secret_store
            .get(secret_ref)
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
