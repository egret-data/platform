//! Catalog seam wire DTOs — pure serde (no sqlx) mirrors of the concrete store
//! row structs. The concrete store's `FromRow` types live in the
//! `egret-catalog-engine` `store` module (folded in from the former `egret-catalog`
//! crate in E3d-1); these are the transport-neutral shapes the `CatalogPort`
//! exchanges across the seam boundary. Field sets mirror
//! `egret-catalog-engine/src/store/embedded.rs`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A per-workspace/domain warehouse (Iceberg catalog root).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Warehouse {
    pub id: Uuid,
    pub name: String,
    pub workspace_id: Uuid,
    pub storage_conn_id: Uuid,
    pub lakekeeper_warehouse_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// A catalog role (keyed by consumer-type) that Catalog Clients bind to.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CatalogRole {
    pub id: Uuid,
    pub warehouse_id: Uuid,
    pub name: String,
    pub consumer_type: String,
    pub description: Option<String>,
}

/// A single privilege grant on a securable, attached to a catalog role.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CatalogGrant {
    pub id: Uuid,
    pub catalog_role_id: Uuid,
    pub securable_type: String,
    pub namespace: Option<Vec<String>>,
    pub table_name: Option<String>,
    pub privilege: String,
}

/// Input for [`CatalogPort::add_grant`](crate::CatalogPort::add_grant).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GrantInput {
    pub securable_type: String,
    pub namespace: Option<Vec<String>>,
    pub table_name: Option<String>,
    pub privilege: String,
}

/// A persisted Catalog Client (the retrievable secret is never exposed here).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CatalogClient {
    pub id: Uuid,
    pub name: String,
    pub consumer_type: String,
    pub workspace_id: Uuid,
    pub warehouse_id: Uuid,
    pub catalog_role_id: Uuid,
    pub client_id: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

/// Input for [`CatalogPort::create_client`](crate::CatalogPort::create_client).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct NewClient {
    pub name: String,
    pub consumer_type: String,
    pub workspace_id: Uuid,
    pub warehouse_id: Uuid,
    pub catalog_role_id: Uuid,
}

/// The one-time create result: the generated `client_id` + plaintext
/// `client_secret` (shown once; only the argon2 hash is persisted).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CreatedClient {
    pub id: Uuid,
    pub client_id: String,
    pub client_secret: String,
}
