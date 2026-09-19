//! Platform catalog registry (SP3): a catalog is a platform resource that engines
//! attach and workspaces are assigned. This crate owns the `registry` schema, the
//! typed per-kind configuration and its validation, engine rendering and the
//! boot-time backfill. Engine-side effects (applying to Trino, StarRocks DDL) live
//! in `egret-engine-mgmt`, which depends on this crate.

pub mod config;
pub mod legacy;
pub mod name;
pub mod render;
pub mod sensitive;

pub use config::{
    Advanced, CatalogConfig, CatalogDef, CatalogKind, ElasticsearchConfig, EngineType,
    GenericConfig, HiveFamilyConfig, HiveMetastore, IcebergConfig, IcebergMetastore, JdbcConfig,
    JdbcDialect, JdbcSsl, RestAuth, SampleConfig, SecretProperties, ValidationError,
};
pub use legacy::{
    classify, classify_generic, legacy_props_and_secrets, plaintext_secrets, Classified,
    ClassifyOutcome, LegacyCatalog,
};
pub use name::{
    first_free_name, is_valid_catalog_name, join_truncated, normalize_catalog_name, with_suffix,
    MAX_NAME_LEN,
};
pub use render::trino::{
    jdbc_url, render_trino, RenderError, StorageRender, TrinoCatalogFile, TrinoRenderInput,
    WarehouseRender,
};
pub use sensitive::{is_plaintext_secret, is_sensitive_key};
