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

use crate::auth::require_permission;
use crate::error::CpError;
use crate::CpState;

/// Request body for creating/updating a global storage connection. A `new_secret`
/// takes precedence over a provided `secret_ref`: it mints a new global named
/// secret and binds the connection to it.
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Serialize)]
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
async fn download_object(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Path(id): Path<Uuid>,
    axum::extract::Query(dq): axum::extract::Query<DownloadQuery>,
) -> Result<axum::response::Response, CpError> {
    require_permission(&principal, "storage:read")?;
    let (store, bucket) = open_store(&st, id).await?;
    use egret_storage::ObjectStore as _;
    let uri = format!("s3://{bucket}/{}", dq.key);
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

/// Resolve a storage connection + its secret and open an S3 store. Returns the bucket too.
async fn open_store(st: &CpState, id: Uuid) -> Result<(egret_storage::S3Store, String), CpError> {
    let conn = st
        .storage_registry
        .get_storage_conn(id)
        .await?
        .ok_or(CpError::NotFound)?;
    let secret_key = match conn.secret_ref {
        Some(ref_id) => st
            .secrets
            .resolve_plaintext(None, ref_id)
            .await
            .map_err(|e| CpError::BadRequest(format!("resolve secret: {e}")))?,
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
    let store = egret_storage::S3Store::new(&cfg)
        .await
        .map_err(|e| CpError::BadRequest(format!("open storage: {e}")))?;
    Ok((store, conn.bucket))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ListingDto {
    folders: Vec<String>,
    objects: Vec<ObjectDto>,
}
#[derive(serde::Serialize)]
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
async fn browse_objects(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Path(id): Path<Uuid>,
    axum::extract::Query(bq): axum::extract::Query<BrowseQuery>,
) -> Result<Json<ListingDto>, CpError> {
    require_permission(&principal, "storage:read")?;
    let (store, _bucket) = open_store(&st, id).await?;
    use egret_storage::ObjectStore as _;
    let listing = store
        .list(&bq.prefix, Some("/"))
        .await
        .map_err(|e| CpError::BadRequest(format!("list objects: {e}")))?;
    Ok(Json(ListingDto {
        folders: listing.folders,
        objects: listing
            .objects
            .into_iter()
            .map(|o| ObjectDto {
                key: o.key,
                size: o.size,
                last_modified: o.last_modified,
            })
            .collect(),
    }))
}

/// Resolve the connection's `secret_ref`: a `new_secret` mints a new GLOBAL
/// named secret (workspace scope `None`) and wins; otherwise the provided
/// `secret_ref` is used as-is.
async fn resolve_secret_ref(st: &CpState, body: &StorageInput) -> Result<Option<Uuid>, CpError> {
    match &body.new_secret {
        Some(ns) => {
            let id = st.secrets.create(None, &ns.name, None, &ns.value).await?;
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

async fn list(
    State(st): State<CpState>,
    Auth(principal): Auth,
) -> Result<Json<Vec<StorageDto>>, CpError> {
    require_permission(&principal, "storage:read")?;
    let conns = st.storage_registry.list_storage_conns().await?;
    Ok(Json(conns.into_iter().map(Into::into).collect()))
}

async fn create(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Json(body): Json<StorageInput>,
) -> Result<(StatusCode, Json<StorageDto>), CpError> {
    require_permission(&principal, "storage:create")?;
    let secret_ref = resolve_secret_ref(&st, &body).await?;
    let conn = st
        .storage_registry
        .create_storage_conn(&to_input(&body, secret_ref))
        .await?;
    Ok((StatusCode::CREATED, Json(conn.into())))
}

async fn detail(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<StorageDto>, CpError> {
    require_permission(&principal, "storage:read")?;
    let conn = st
        .storage_registry
        .get_storage_conn(id)
        .await?
        .ok_or(CpError::NotFound)?;
    Ok(Json(conn.into()))
}

async fn update(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<StorageInput>,
) -> Result<Json<StorageDto>, CpError> {
    require_permission(&principal, "storage:update")?;
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

async fn remove(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, CpError> {
    require_permission(&principal, "storage:delete")?;
    if !st.storage_registry.delete_storage_conn(id).await? {
        return Err(CpError::NotFound);
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}
