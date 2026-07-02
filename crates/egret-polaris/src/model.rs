//! Typed request/response models for the Polaris management API.
//!
//! Request models expose a `to_wire()` producing the exact JSON shape Polaris
//! expects (see `polaris/spec/polaris-management-service.yml`); response models
//! derive [`serde::Deserialize`] and tolerate unknown fields.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{json, Value};

/// Whether a catalog's storage/metadata is managed by Polaris (`INTERNAL`) or
/// an external system (`EXTERNAL`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogType {
    Internal,
    External,
}

impl CatalogType {
    /// The wire string Polaris expects for `Catalog.type`.
    pub fn as_wire(self) -> &'static str {
        match self {
            CatalogType::Internal => "INTERNAL",
            CatalogType::External => "EXTERNAL",
        }
    }
}

/// Storage backend configuration for a catalog (`storageConfigInfo`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageConfig {
    S3 {
        allowed_locations: Vec<String>,
        role_arn: Option<String>,
        region: Option<String>,
    },
    Gcs {
        allowed_locations: Vec<String>,
    },
    Azure {
        allowed_locations: Vec<String>,
    },
    File {
        allowed_locations: Vec<String>,
    },
}

impl StorageConfig {
    /// Produce the `storageConfigInfo` object: `storageType` + `allowedLocations`
    /// plus the S3-only `roleArn`/`region` extras when present.
    pub fn to_wire(&self) -> Value {
        match self {
            StorageConfig::S3 {
                allowed_locations,
                role_arn,
                region,
            } => {
                let mut obj = json!({
                    "storageType": "S3",
                    "allowedLocations": allowed_locations,
                });
                if let Some(arn) = role_arn {
                    obj["roleArn"] = json!(arn);
                }
                if let Some(r) = region {
                    obj["region"] = json!(r);
                }
                obj
            }
            StorageConfig::Gcs { allowed_locations } => json!({
                "storageType": "GCS",
                "allowedLocations": allowed_locations,
            }),
            StorageConfig::Azure { allowed_locations } => json!({
                "storageType": "AZURE",
                "allowedLocations": allowed_locations,
            }),
            StorageConfig::File { allowed_locations } => json!({
                "storageType": "FILE",
                "allowedLocations": allowed_locations,
            }),
        }
    }
}

/// A catalog to create.
#[derive(Debug, Clone)]
pub struct NewCatalog {
    pub name: String,
    pub catalog_type: CatalogType,
    pub storage: StorageConfig,
    pub properties: HashMap<String, String>,
}

impl NewCatalog {
    /// Produce the inner `Catalog` object: `{type, name, properties, storageConfigInfo}`.
    pub fn to_wire(&self) -> Value {
        json!({
            "type": self.catalog_type.as_wire(),
            "name": self.name,
            "properties": self.properties,
            "storageConfigInfo": self.storage.to_wire(),
        })
    }
}

/// A catalog as returned by Polaris. Unknown fields are ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct Catalog {
    pub name: String,
    #[serde(rename = "type", default)]
    pub catalog_type: String,
}

/// The `GET /catalogs` list envelope.
#[derive(Debug, Clone, Deserialize)]
pub struct Catalogs {
    #[serde(default)]
    pub catalogs: Vec<Catalog>,
}

/// A principal (service identity).
#[derive(Debug, Clone, Deserialize)]
pub struct Principal {
    pub name: String,
}

/// Client-credentials for a freshly created principal.
#[derive(Debug, Clone, Deserialize)]
pub struct Credentials {
    #[serde(rename = "clientId")]
    pub client_id: String,
    #[serde(rename = "clientSecret")]
    pub client_secret: String,
}

/// Response of `POST /principals`: the principal plus its one-time credentials.
#[derive(Debug, Clone, Deserialize)]
pub struct PrincipalWithCredentials {
    pub principal: Principal,
    pub credentials: Credentials,
}

/// A grant of a single privilege on a securable to a catalog-role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Grant {
    Catalog {
        privilege: String,
    },
    Namespace {
        namespace: Vec<String>,
        privilege: String,
    },
    Table {
        namespace: Vec<String>,
        table: String,
        privilege: String,
    },
    View {
        namespace: Vec<String>,
        view: String,
        privilege: String,
    },
}

impl Grant {
    /// Produce the discriminated `GrantResource` object.
    pub fn to_wire(&self) -> Value {
        match self {
            Grant::Catalog { privilege } => json!({
                "type": "catalog",
                "privilege": privilege,
            }),
            Grant::Namespace {
                namespace,
                privilege,
            } => json!({
                "type": "namespace",
                "namespace": namespace,
                "privilege": privilege,
            }),
            Grant::Table {
                namespace,
                table,
                privilege,
            } => json!({
                "type": "table",
                "namespace": namespace,
                "tableName": table,
                "privilege": privilege,
            }),
            Grant::View {
                namespace,
                view,
                privilege,
            } => json!({
                "type": "view",
                "namespace": namespace,
                "viewName": view,
                "privilege": privilege,
            }),
        }
    }
}
