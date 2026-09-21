//! Per-engine rendering (spec §4.3). SP3a shipped Trino; SP3b adds StarRocks
//! next to it, and SP5 egret's own connection. The input types and the error
//! are shared: both renderers read the same resolved warehouse and storage.

use uuid::Uuid;

pub mod starrocks;
pub mod trino;

/// The warehouse behind an egret catalog.
#[derive(Clone, Copy, Debug)]
pub struct WarehouseRender<'a> {
    pub name: &'a str,
    pub s3_endpoint: &'a str,
}

/// A resolved storage connection (secret still an id).
#[derive(Clone, Copy, Debug)]
pub struct StorageRender<'a> {
    pub endpoint: Option<&'a str>,
    pub region: &'a str,
    pub path_style: bool,
    pub access_key: &'a str,
    pub secret_key_secret_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RenderError {
    #[error("catalog {0} cannot be read by this engine")]
    NotReadable(String),
    #[error("missing render input: {0}")]
    MissingInput(&'static str),
    #[error("advanced key {0:?} collides with a property the catalog form sets")]
    Collision(String),
    #[error("property {0:?} has a value with a line break")]
    LineBreak(String),
    /// StarRocks only: no pinned driver for this dialect and no override.
    #[error("no JDBC driver is configured for {0:?}")]
    NoJdbcDriver(crate::config::JdbcDialect),
    /// StarRocks only: a name the FE reserves.
    #[error("{0} is a name StarRocks reserves")]
    ReservedName(String),
    /// StarRocks only: the DDL needs the plaintext and the caller did not resolve it.
    #[error("secret {0} was not resolved")]
    MissingSecret(Uuid),
}
