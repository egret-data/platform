//! Control-plane catalog routes (`/polaris/catalogs`), proxying the external
//! Apache Polaris management API through [`egret_polaris::PolarisClient`].
//!
//! egret owns catalog lifecycle at the control plane: a catalog's allowed
//! storage location is derived from a GLOBAL storage connection
//! (`s3://{bucket}/{name}/`), and the S3 region is carried from that connection.
//! Every handler gates on `require_platform_admin` and builds a fresh
//! `PolarisClient` from the singleton connection.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_catalog::StorageConn;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::require_platform_admin;
use crate::error::CpError;
use crate::polaris_conn::polaris_client;
use crate::CpState;

/// Request body for `POST /polaris/catalogs`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateCatalogBody {
    pub name: String,
    pub storage_connection_id: Uuid,
}

/// Response DTO for a catalog (camelCase for the frontend).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogDto {
    pub name: String,
    pub catalog_type: String,
}

impl From<egret_polaris::Catalog> for CatalogDto {
    fn from(c: egret_polaris::Catalog) -> Self {
        CatalogDto {
            name: c.name,
            catalog_type: c.catalog_type,
        }
    }
}

/// The base storage location for a catalog: `s3://{bucket}/{name}`.
///
/// Pure (no I/O) so the storage-connection → Polaris mapping can be asserted
/// directly in unit tests without a live catalog or mocked transport.
fn catalog_base_location(bucket: &str, name: &str) -> String {
    format!("s3://{bucket}/{name}")
}

/// The allowed storage location for a catalog: `s3://{bucket}/{name}/`.
fn catalog_allowed_location(bucket: &str, name: &str) -> String {
    format!("{}/", catalog_base_location(bucket, name))
}

/// Build the [`egret_polaris::NewCatalog`] for `name` from a storage connection:
/// an INTERNAL catalog whose single allowed S3 location is
/// `s3://{bucket}/{name}/` and whose region is carried from the connection.
///
/// Polaris REQUIRES a `default-base-location` property on catalog creation
/// (a catalog with only `allowedLocations` is rejected with HTTP 400), so it is
/// set to `s3://{bucket}/{name}`.
///
/// Pure (no I/O) so the mapping can be unit-tested directly.
fn new_catalog_from(sc: &StorageConn, name: &str) -> egret_polaris::NewCatalog {
    let mut properties = std::collections::HashMap::new();
    properties.insert(
        "default-base-location".to_string(),
        catalog_base_location(&sc.bucket, name),
    );
    egret_polaris::NewCatalog {
        name: name.to_string(),
        catalog_type: egret_polaris::CatalogType::Internal,
        storage: egret_polaris::StorageConfig::S3 {
            allowed_locations: vec![catalog_allowed_location(&sc.bucket, name)],
            role_arn: None,
            region: Some(sc.region.clone()),
        },
        properties,
    }
}

pub fn router() -> Router<CpState> {
    Router::new()
        .route("/polaris/catalogs", get(list).post(create))
        .route("/polaris/catalogs/{name}", get(detail).delete(remove))
}

async fn create(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Json(body): Json<CreateCatalogBody>,
) -> Result<(StatusCode, Json<CatalogDto>), CpError> {
    require_platform_admin(&principal)?;
    let sc = st
        .storage
        .get_storage_conn(body.storage_connection_id)
        .await?
        .ok_or_else(|| CpError::BadRequest("unknown storage connection".into()))?;
    let client = polaris_client(&st.polaris_conn, &st.secrets).await?;
    let cat = client
        .create_catalog(&new_catalog_from(&sc, &body.name))
        .await?;
    Ok((StatusCode::CREATED, Json(cat.into())))
}

async fn list(
    State(st): State<CpState>,
    Auth(principal): Auth,
) -> Result<Json<Vec<CatalogDto>>, CpError> {
    require_platform_admin(&principal)?;
    let client = polaris_client(&st.polaris_conn, &st.secrets).await?;
    let cats = client.list_catalogs().await?;
    Ok(Json(cats.into_iter().map(Into::into).collect()))
}

async fn detail(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Path(name): Path<String>,
) -> Result<Json<CatalogDto>, CpError> {
    require_platform_admin(&principal)?;
    let client = polaris_client(&st.polaris_conn, &st.secrets).await?;
    let cat = client.get_catalog(&name).await?;
    Ok(Json(cat.into()))
}

async fn remove(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Path(name): Path<String>,
) -> Result<StatusCode, CpError> {
    require_platform_admin(&principal)?;
    let client = polaris_client(&st.polaris_conn, &st.secrets).await?;
    client.delete_catalog(&name).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn storage_conn(bucket: &str, region: &str) -> StorageConn {
        StorageConn {
            id: Uuid::new_v4(),
            name: "test-store".into(),
            purpose: None,
            provider: "s3".into(),
            endpoint: None,
            region: region.into(),
            bucket: bucket.into(),
            key_prefix: None,
            access_key: "AKIATEST".into(),
            secret_ref: None,
            path_style: true,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn allowed_location_is_bucket_and_catalog_name() {
        assert_eq!(
            catalog_allowed_location("egret-lake", "lakehouse"),
            "s3://egret-lake/lakehouse/"
        );
    }

    #[test]
    fn new_catalog_maps_storage_conn_to_internal_s3() {
        let sc = storage_conn("egret-lake", "ap-southeast-1");
        let nc = new_catalog_from(&sc, "lakehouse");

        assert_eq!(nc.name, "lakehouse");
        assert_eq!(nc.catalog_type, egret_polaris::CatalogType::Internal);
        match nc.storage {
            egret_polaris::StorageConfig::S3 {
                allowed_locations,
                role_arn,
                region,
            } => {
                assert_eq!(allowed_locations, vec!["s3://egret-lake/lakehouse/"]);
                assert_eq!(role_arn, None);
                // Region is carried from the storage connection.
                assert_eq!(region.as_deref(), Some("ap-southeast-1"));
            }
            other => panic!("expected S3 storage config, got {other:?}"),
        }
        // Polaris requires default-base-location; without it, create is HTTP 400.
        assert_eq!(
            nc.properties
                .get("default-base-location")
                .map(String::as_str),
            Some("s3://egret-lake/lakehouse")
        );
    }
}
