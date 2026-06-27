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

use axum::extract::{FromRef, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use egret_auth::session::SessionService;
use egret_catalog::service::CatalogStore;
use egret_core::WorkspaceContext;
use egret_workspace::context::WsContext;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use serde::Serialize;
use uuid::Uuid;

use lakekeeper::api::iceberg::types::{PageToken, Prefix};
use lakekeeper::api::iceberg::v1::namespace::{
    ListNamespacesQuery, NamespaceParameters, NamespaceService,
};
use lakekeeper::api::iceberg::v1::tables::TablesService;
use lakekeeper::api::iceberg::v1::tables::{ListTablesQuery, LoadTableRequest, TableParameters};
use lakekeeper::api::iceberg::v1::views::ViewService;
use lakekeeper::api::iceberg::v1::ListTablesResponse;
use lakekeeper::api::management::v1::warehouse::Service as WarehouseService;
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
    /// An internal failure building the system principal etc.
    Internal,
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
    let r = authorized_warehouse(&st, &ctx).await?;

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
