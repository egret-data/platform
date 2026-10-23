//! Control-plane Storage router (`/storage`), nested under `/api/control-plane`.
//!
//! Storage connections are GLOBAL after migration 0050; every handler gates on
//! `require_permission(&p, "storage:<action>")` (platform_admin bypasses) and
//! calls the global `StorageConn` CRUD on `StorageRegistry` (egret-storage, E3a).
//! A `new_secret` in the create/update body mints a GLOBAL named secret
//! (`secrets.create(None, ..)`) and uses its id as the connection's `secret_ref`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use egret_auth::principal::Auth;
use egret_secrets::NewSecret;
use egret_storage::{StorageConn, StorageConnInput};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use egret_core::http::ErrorBody;
use egret_core::Ack;

use crate::error::CpError;
use crate::storage_scope::{ConnView, KeyKind, TableDataLocation};
use crate::CpState;

/// Request body for creating/updating a global storage connection. A `new_secret`
/// takes precedence over a provided `secret_ref`: it mints a new global named
/// secret and binds the connection to it.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct StorageInput {
    pub name: String,
    #[serde(default)]
    pub purpose: Option<String>,
    pub provider: String,
    #[serde(default)]
    pub endpoint: Option<String>,
    pub region: String,
    pub bucket: String,
    #[serde(default)]
    pub key_prefix: Option<String>,
    pub access_key: String,
    #[serde(default)]
    pub secret_ref: Option<Uuid>,
    #[serde(default)]
    pub path_style: bool,
    /// Mint a new global named secret inline and use it as `secret_ref`.
    #[serde(default)]
    pub new_secret: Option<NewSecret>,
}

/// Response DTO for a storage connection.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct StorageDto {
    pub id: Uuid,
    pub name: String,
    pub purpose: Option<String>,
    pub provider: String,
    pub endpoint: Option<String>,
    pub region: String,
    pub bucket: String,
    pub key_prefix: Option<String>,
    pub access_key: String,
    pub path_style: bool,
    pub secret_ref: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

impl From<StorageConn> for StorageDto {
    fn from(c: StorageConn) -> Self {
        StorageDto {
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
            secret_ref: c.secret_ref,
            created_at: c.created_at,
        }
    }
}

pub fn router() -> Router<CpState> {
    Router::new()
        .route("/storage", get(list).post(create))
        .route("/storage/{id}", get(detail).put(update).delete(remove))
        .route("/storage/{id}/objects", get(browse_objects))
        .route("/storage/{id}/download", get(download_object))
}

#[derive(serde::Deserialize)]
struct DownloadQuery {
    key: String,
}

/// Stream a single object from a storage connection's bucket as an attachment.
#[utoipa::path(
    get,
    path = "/api/control-plane/storage/{id}/download",
    tag = "control-plane-storage",
    params(
        ("id" = Uuid, Path, description = "Storage connection id"),
        ("key" = String, Query, description = "Object key to download, relative to the connection's key prefix"),
    ),
    responses(
        (status = 200, description = "Object bytes streamed as an attachment", content_type = "application/octet-stream"),
        (status = 400, description = "Invalid key (a `.`/`..` or empty segment, a backslash or a control character, also once percent-decoded), or the object could not be read", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing storage:read permission, or the caller is not a platform admin and the key is under a prefix egret writes for its users, or under the storage of a warehouse or lakehouse catalog (a table's data or metadata file, whose raw bytes would skip its column masks and row filters)", body = ErrorBody),
        (status = 404, description = "No such storage connection", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn download_object(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Path(id): Path<Uuid>,
    axum::extract::Query(dq): axum::extract::Query<DownloadQuery>,
) -> Result<axum::response::Response, CpError> {
    egret_core::authz::require(&*st.pdp, &principal, "storage:read").await?;
    let conn = get_conn(&st, id).await?;
    let view = conn_view(&st, &principal, &conn)?;
    let key = view.scope_key(&dq.key, KeyKind::Object)?;
    if !view.admin {
        view.refuse_table_data(&dq.key, &table_data_locations(&st.db).await?)?;
    }
    let (store, bucket) = open_store(&st, &conn).await?;
    let uri = format!("s3://{bucket}/{key}");
    let bytes = store
        .get(&uri)
        .await
        .map_err(|e| CpError::BadRequest(format!("get object: {e}")))?;
    let filename = dq
        .key
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or("download");
    use axum::http::header;
    let mut resp = axum::response::Response::new(axum::body::Body::from(bytes));
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        "application/octet-stream".parse().unwrap(),
    );
    if let Ok(v) = format!("attachment; filename=\"{filename}\"").parse() {
        resp.headers_mut().insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(resp)
}

/// Every place catalog tables keep their files (KI-236): the storage
/// connection of each egret warehouse and of each registered catalog bound to
/// one (Iceberg, Hive, Delta Lake, Hudi: the connection is where its tables
/// live), plus each embedded lakekeeper warehouse's own storage profile, which
/// is what the engines write to and is not changed by a later edit of the
/// connection.
async fn table_data_locations(db: &egret_metastore::Db) -> Result<Vec<TableDataLocation>, CpError> {
    let internal = |e: sqlx::Error| {
        tracing::error!(error = %e, "could not read where catalog tables keep their files");
        CpError::Internal
    };
    let mut rows: Vec<(Option<String>, Option<String>, String)> = sqlx::query_as(
        "SELECT s.bucket, s.key_prefix, 'warehouse «' || w.name || '»' \
           FROM catalog.warehouses w \
           JOIN storage.storage_connections s ON s.id = w.storage_conn_id \
         UNION ALL \
         SELECT s.bucket, s.key_prefix, 'catalog «' || c.name || '»' \
           FROM registry.catalogs c \
           JOIN storage.storage_connections s ON s.id = c.storage_connection_id",
    )
    .fetch_all(&db.pool)
    .await
    .map_err(internal)?;
    // The lakekeeper schema exists once the embedded catalog has started.
    let (lakekeeper,): (bool,) =
        sqlx::query_as("SELECT to_regclass('lakekeeper.warehouse') IS NOT NULL")
            .fetch_one(&db.pool)
            .await
            .map_err(internal)?;
    if lakekeeper {
        rows.extend(
            sqlx::query_as::<_, (Option<String>, Option<String>, String)>(
                "SELECT storage_profile->>'bucket', storage_profile->>'key-prefix', \
                        'warehouse «' || warehouse_name || '»' \
                   FROM lakekeeper.warehouse",
            )
            .fetch_all(&db.pool)
            .await
            .map_err(internal)?,
        );
    }
    Ok(rows
        .into_iter()
        .filter_map(|(bucket, prefix, owner)| {
            Some(TableDataLocation::new(&bucket?, prefix.as_deref(), &owner))
        })
        .collect())
}

async fn get_conn(st: &CpState, id: Uuid) -> Result<StorageConn, CpError> {
    st.storage_registry
        .get_storage_conn(id)
        .await?
        .ok_or(CpError::NotFound)
}

/// The connection as `principal` may see it: under its key prefix, with
/// egret's own prefixes closed unless they are a platform admin (KI-206).
fn conn_view<'a>(
    st: &'a CpState,
    principal: &egret_core::context::Principal,
    conn: &'a StorageConn,
) -> Result<ConnView<'a>, CpError> {
    let admin = principal
        .platform_roles
        .iter()
        .any(|r| r == "platform_admin");
    ConnView::new(
        &st.storage_scope.system,
        &conn.bucket,
        conn.key_prefix.as_deref(),
        admin,
    )
}

/// Resolve a storage connection's secret and open its store. Returns the bucket too.
async fn open_store(
    st: &CpState,
    conn: &StorageConn,
) -> Result<(std::sync::Arc<dyn egret_storage::ObjectStore>, String), CpError> {
    let secret_key = match conn.secret_ref {
        Some(ref_id) => {
            let (_user, pw) = st
                .secrets
                .resolve_basic_auth(None, ref_id)
                .await
                .map_err(|e| CpError::BadRequest(format!("resolve secret: {e}")))?;
            pw
        }
        None => return Err(CpError::BadRequest("connection has no secret".into())),
    };
    let cfg = egret_storage::StorageConfig {
        endpoint: conn.endpoint.clone(),
        region: conn.region.clone(),
        bucket: conn.bucket.clone(),
        access_key: conn.access_key.clone(),
        secret_key,
        path_style: conn.path_style,
    };
    let store = st
        .storage_scope
        .opener
        .open(&cfg)
        .await
        .map_err(|e| CpError::BadRequest(format!("open storage: {e}")))?;
    Ok((store, conn.bucket.clone()))
}

#[derive(serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
struct ListingDto {
    /// The connection's key prefix (`""` when it has none). Every key in
    /// this listing, and every `prefix`/`key` the routes take, is relative
    /// to it.
    root: String,
    folders: Vec<String>,
    objects: Vec<ObjectDto>,
    /// Folders at this level left out because egret writes its users' data
    /// there (stored SQL Lab results, Data App sources, workflow files) and
    /// the caller is not a platform admin.
    hidden_system_folders: Vec<String>,
}
#[derive(serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
struct ObjectDto {
    key: String,
    size: i64,
    last_modified: Option<String>,
}

#[derive(serde::Deserialize)]
struct BrowseQuery {
    #[serde(default)]
    prefix: String,
}

/// Browse the objects/folders in a storage connection's bucket (folder-style).
#[utoipa::path(
    get,
    path = "/api/control-plane/storage/{id}/objects",
    tag = "control-plane-storage",
    params(
        ("id" = Uuid, Path, description = "Storage connection id"),
        ("prefix" = Option<String>, Query, description = "Folder prefix to list under, relative to the connection's key prefix"),
    ),
    responses(
        (status = 200, description = "Folders and objects under the prefix", body = ListingDto),
        (status = 400, description = "Invalid prefix, or the bucket could not be listed", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing storage:read permission, or the prefix is one egret writes for its users and the caller is not a platform admin", body = ErrorBody),
        (status = 404, description = "No such storage connection", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn browse_objects(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Path(id): Path<Uuid>,
    axum::extract::Query(bq): axum::extract::Query<BrowseQuery>,
) -> Result<Json<ListingDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &principal, "storage:read").await?;
    let conn = get_conn(&st, id).await?;
    let view = conn_view(&st, &principal, &conn)?;
    let prefix = view.scope_key(&bq.prefix, KeyKind::Folder)?;
    let (store, _bucket) = open_store(&st, &conn).await?;
    let listing = store
        .list(&prefix, Some("/"))
        .await
        .map_err(|e| CpError::BadRequest(format!("list objects: {e}")))?;
    let hidden_system_folders = listing
        .folders
        .iter()
        .filter(|f| view.is_hidden_system(f))
        .filter_map(|f| f.strip_prefix(view.root.as_str()).map(str::to_string))
        .collect();
    Ok(Json(ListingDto {
        root: view.root.clone(),
        folders: listing
            .folders
            .iter()
            .filter_map(|f| view.visible(f))
            .collect(),
        objects: listing
            .objects
            .into_iter()
            .filter_map(|o| {
                Some(ObjectDto {
                    key: view.visible(&o.key)?,
                    size: o.size,
                    last_modified: o.last_modified,
                })
            })
            .collect(),
        hidden_system_folders,
    }))
}

/// Resolve the connection's `secret_ref`: a `new_secret` mints a new GLOBAL
/// named secret (workspace scope `None`) and wins; otherwise the provided
/// `secret_ref` is used as-is.
async fn resolve_secret_ref(st: &CpState, body: &StorageInput) -> Result<Option<Uuid>, CpError> {
    match &body.new_secret {
        Some(ns) => {
            let id = if body.access_key.is_empty() {
                st.secrets.create(None, &ns.name, None, &ns.value).await?
            } else {
                let mut data = std::collections::BTreeMap::new();
                data.insert("username".to_string(), body.access_key.clone());
                data.insert("password".to_string(), ns.value.clone());
                st.secrets
                    .create_typed(
                        None,
                        &ns.name,
                        None,
                        egret_secrets::secret_type::BASIC_AUTH,
                        &data,
                    )
                    .await?
            };
            Ok(Some(id))
        }
        None => Ok(body.secret_ref),
    }
}

fn to_input(body: &StorageInput, secret_ref: Option<Uuid>) -> StorageConnInput {
    StorageConnInput {
        name: body.name.clone(),
        purpose: body.purpose.clone(),
        provider: body.provider.clone(),
        endpoint: body.endpoint.clone(),
        region: body.region.clone(),
        bucket: body.bucket.clone(),
        key_prefix: body.key_prefix.clone(),
        access_key: body.access_key.clone(),
        secret_ref,
        path_style: body.path_style,
    }
}

#[utoipa::path(
    get,
    path = "/api/control-plane/storage",
    tag = "control-plane-storage",
    responses(
        (status = 200, description = "Global storage connections", body = Vec<StorageDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing storage:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list(
    State(st): State<CpState>,
    Auth(principal): Auth,
) -> Result<Json<Vec<StorageDto>>, CpError> {
    egret_core::authz::require(&*st.pdp, &principal, "storage:read").await?;
    let conns = st.storage_registry.list_storage_conns().await?;
    Ok(Json(conns.into_iter().map(Into::into).collect()))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/storage",
    tag = "control-plane-storage",
    request_body = StorageInput,
    responses(
        (status = 201, description = "Storage connection created", body = StorageDto),
        (status = 400, description = "Invalid input", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing storage:create permission", body = ErrorBody),
        (status = 409, description = "A storage connection with this name already exists", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn create(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Json(body): Json<StorageInput>,
) -> Result<(StatusCode, Json<StorageDto>), CpError> {
    egret_core::authz::require(&*st.pdp, &principal, "storage:create").await?;
    let secret_ref = resolve_secret_ref(&st, &body).await?;
    let conn = st
        .storage_registry
        .create_storage_conn(&to_input(&body, secret_ref))
        .await?;
    Ok((StatusCode::CREATED, Json(conn.into())))
}

#[utoipa::path(
    get,
    path = "/api/control-plane/storage/{id}",
    tag = "control-plane-storage",
    params(("id" = Uuid, Path, description = "Storage connection id")),
    responses(
        (status = 200, description = "Storage connection detail", body = StorageDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing storage:read permission", body = ErrorBody),
        (status = 404, description = "No such storage connection", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn detail(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<StorageDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &principal, "storage:read").await?;
    let conn = st
        .storage_registry
        .get_storage_conn(id)
        .await?
        .ok_or(CpError::NotFound)?;
    Ok(Json(conn.into()))
}

#[utoipa::path(
    put,
    path = "/api/control-plane/storage/{id}",
    tag = "control-plane-storage",
    params(("id" = Uuid, Path, description = "Storage connection id")),
    request_body = StorageInput,
    responses(
        (status = 200, description = "Storage connection updated (secret rotation pushed to warehouses)", body = StorageDto),
        (status = 400, description = "Invalid input", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing storage:update permission", body = ErrorBody),
        (status = 404, description = "No such storage connection", body = ErrorBody),
        (status = 409, description = "The new name clashes with an existing connection", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn update(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<StorageInput>,
) -> Result<Json<StorageDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &principal, "storage:update").await?;
    let secret_ref = resolve_secret_ref(&st, &body).await?;
    let updated = st
        .storage_registry
        .update_storage_conn(id, &to_input(&body, secret_ref))
        .await?;
    if !updated {
        return Err(CpError::NotFound);
    }
    let conn = st
        .storage_registry
        .get_storage_conn(id)
        .await?
        .ok_or(CpError::NotFound)?;

    // Warehouses hold a provision-time COPY of the S3 credential in lakekeeper, so
    // editing the connection (e.g. rotating its secret) must be pushed down to every
    // provisioned warehouse using it — otherwise they keep authenticating with the
    // old key. Best-effort: a single warehouse failure doesn't fail the whole update.
    if let (Some(browser), Some(ref_id)) = (st.browser.as_ref(), conn.secret_ref) {
        if let Ok(secret_key) = st.secrets.resolve_plaintext(None, ref_id).await {
            for w in st
                .storage
                .list_warehouses()
                .await?
                .into_iter()
                .filter(|w| w.storage_conn_id == id)
            {
                if let Some(lk) = w.lakekeeper_warehouse_id.as_deref() {
                    let _ = browser
                        .rotate_credential(lk, &conn.access_key, &secret_key)
                        .await;
                }
            }
        }
    }

    Ok(Json(conn.into()))
}

#[utoipa::path(
    delete,
    path = "/api/control-plane/storage/{id}",
    tag = "control-plane-storage",
    params(("id" = Uuid, Path, description = "Storage connection id")),
    responses(
        (status = 200, description = "Storage connection deleted", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing storage:delete permission", body = ErrorBody),
        (status = 404, description = "No such storage connection", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn remove(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, CpError> {
    egret_core::authz::require(&*st.pdp, &principal, "storage:delete").await?;
    if !st.storage_registry.delete_storage_conn(id).await? {
        return Err(CpError::NotFound);
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// OpenAPI fragment for the control-plane Storage router. Merged into
/// `egret_controlplane::openapi()`; the nested `NewSecret` schema is collected
/// automatically from `StorageInput`.
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(list, create, detail, update, remove, browse_objects, download_object),
    components(schemas(StorageInput, StorageDto, ListingDto, ObjectDto, Ack, ErrorBody)),
    tags((name = "control-plane-storage", description = "Global storage connections (platform control plane)")),
)]
pub struct ControlPlaneStorageApi;
