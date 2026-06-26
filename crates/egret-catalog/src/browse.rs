//! Session-authenticated internal catalog browse API (for the UI).
//!
//! This is NOT the Iceberg REST surface (`iceberg_router`). It reads the
//! normalized catalog rows for the *active workspace's* catalog directly and is
//! gated by RBAC (`catalog:read`) via the session JWT → [`WsContext`], exactly
//! like SQL Lab (`egret-engine`) and BI (`egret-bi`) workspace-scoped APIs.

use axum::extract::{FromRef, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use egret_auth::session::SessionService;
use egret_core::WorkspaceContext;
use egret_workspace::context::WsContext;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use serde::Serialize;

use crate::service::catalogs::Catalog;
use crate::service::CatalogStore;
use crate::CatalogError;

/// Permission required to read a workspace's catalog.
const PERM: &str = "catalog:read";

/// State for the browse API. Carries the [`CatalogStore`] plus the
/// session/workspace/authz handles the [`WsContext`] extractor needs.
#[derive(Clone)]
pub struct BrowseState {
    pub store: CatalogStore,
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
// Error wrapper: authz failures (egret_core::Error) + domain (CatalogError).
// ---------------------------------------------------------------------------

/// HTTP-facing error: an authz/validation failure or a catalog domain error.
enum BrowseApiError {
    Auth(egret_core::Error),
    Domain(CatalogError),
}

impl From<egret_core::Error> for BrowseApiError {
    fn from(e: egret_core::Error) -> Self {
        BrowseApiError::Auth(e)
    }
}
impl From<CatalogError> for BrowseApiError {
    fn from(e: CatalogError) -> Self {
        BrowseApiError::Domain(e)
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
            BrowseApiError::Domain(e) => match &e {
                CatalogError::NotFound(_) => (StatusCode::NOT_FOUND, e.to_string()),
                CatalogError::Forbidden(_) => (StatusCode::FORBIDDEN, "forbidden".to_string()),
                CatalogError::BadRequest(_) => (StatusCode::BAD_REQUEST, e.to_string()),
                CatalogError::AlreadyExists(_)
                | CatalogError::Conflict(_)
                | CatalogError::NamespaceNotEmpty(_) => (StatusCode::CONFLICT, e.to_string()),
                CatalogError::Db(_) | CatalogError::Storage(_) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error".to_string(),
                ),
            },
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

/// Compact catalog summary for the UI.
#[derive(Serialize)]
struct CatalogSummary {
    id: String,
    name: String,
    warehouse_location: String,
}

impl From<Catalog> for CatalogSummary {
    fn from(c: Catalog) -> Self {
        Self {
            id: c.id.to_string(),
            name: c.name,
            warehouse_location: c.warehouse_location,
        }
    }
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
    properties: std::collections::BTreeMap<String, String>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Authorize `catalog:read` on the active workspace, then resolve its catalog.
async fn authorized_catalog(
    st: &BrowseState,
    ctx: &WorkspaceContext,
) -> Result<Catalog, BrowseApiError> {
    st.authz.require_workspace(ctx, PERM).await?;
    let cat = st
        .store
        .get_catalog_by_workspace(ctx.workspace_id.0)
        .await?;
    Ok(cat)
}

/// GET `/api/ws/{workspace}/catalog` — catalog summary.
async fn summary(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
) -> Result<Json<CatalogSummary>, BrowseApiError> {
    let cat = authorized_catalog(&st, &ctx).await?;
    Ok(Json(cat.into()))
}

/// GET `/api/ws/{workspace}/catalog/namespaces` — all namespaces in the catalog.
async fn list_namespaces(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
) -> Result<Json<Vec<NamespaceDto>>, BrowseApiError> {
    let cat = authorized_catalog(&st, &ctx).await?;
    let levels = st.store.list_namespaces(cat.id, None).await?;
    Ok(Json(
        levels
            .into_iter()
            .map(|levels| NamespaceDto { levels })
            .collect(),
    ))
}

/// Parse a dotted namespace path segment (`a.b.c`) into its levels.
fn parse_ns(ns: &str) -> Vec<String> {
    ns.split('.').map(|s| s.to_string()).collect()
}

/// GET `/api/ws/{workspace}/catalog/namespaces/{ns}/tables` — tables + views.
async fn list_tables(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path((_ws, ns)): Path<(String, String)>,
) -> Result<Json<NamespaceContents>, BrowseApiError> {
    let cat = authorized_catalog(&st, &ctx).await?;
    let levels = parse_ns(&ns);
    let ns_row = st.store.get_namespace(cat.id, &levels).await?;
    let tables = st.store.list_tables(ns_row.id).await?;
    let views = st.store.list_views(ns_row.id).await?;
    Ok(Json(NamespaceContents { tables, views }))
}

/// GET `/api/ws/{workspace}/catalog/namespaces/{ns}/tables/{table}` — detail.
async fn table_detail(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path((_ws, ns, table)): Path<(String, String, String)>,
) -> Result<Json<TableDetail>, BrowseApiError> {
    let cat = authorized_catalog(&st, &ctx).await?;
    let levels = parse_ns(&ns);
    let ns_row = st.store.get_namespace(cat.id, &levels).await?;
    let row = st.store.load_table(ns_row.id, &table).await?;
    let md = &row.metadata;

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

    Ok(Json(TableDetail {
        name: row.name,
        location: row.location,
        current_schema_id: schema.schema_id(),
        columns,
        current_snapshot_id: md.current_snapshot_id(),
        snapshot_count: md.snapshots().count(),
        properties,
    }))
}
