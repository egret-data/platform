//! Platform catalog registry (SP3): a catalog is a platform resource that engines
//! attach and workspaces are assigned. This crate owns the `registry` schema, the
//! typed per-kind configuration and its validation, and engine rendering.
//! Engine-side effects (applying to Trino, StarRocks DDL) live
//! in `egret-engine-mgmt`, which depends on this crate.

pub mod compiler_port;
pub mod config;
pub mod error;
pub mod name;
pub mod render;
pub mod sensitive;
pub mod store;
pub mod tag_version;

pub use compiler_port::CatalogAssignmentCompiler;
pub use config::{
    Advanced, CatalogConfig, CatalogDef, CatalogKind, ElasticsearchConfig, EngineType,
    GenericConfig, HiveFamilyConfig, HiveMetastore, IcebergConfig, IcebergMetastore, JdbcConfig,
    JdbcDialect, JdbcSsl, RestAuth, SampleConfig, SecretProperties, ValidationError,
};
pub use error::RegistryError;
pub use name::{
    first_free_name, is_valid_catalog_name, join_truncated, normalize_catalog_name, with_suffix,
    MAX_NAME_LEN,
};
pub use render::starrocks::{
    escape_dq, escape_ident, is_reserved_starrocks_name, render_starrocks, scrub_secrets,
    starrocks_jdbc_uri, JdbcDriver, JdbcDrivers, PropValue, StarRocksCatalogDdl,
    StarRocksRenderInput, StatementForm, DEFAULT_SR_JDBC_DRIVERS, SR_OVERRIDABLE,
};
pub use render::trino::{jdbc_url, render_trino, TrinoCatalogFile, TrinoRenderInput};
pub use render::{RenderError, StorageRender, WarehouseRender};
pub use sensitive::{is_plaintext_secret, is_sensitive_key};
pub use store::{
    AttachedCatalog, AttachmentRow, CatalogEngineRow, CatalogFilter, CatalogRow,
    CatalogWorkspaceRow, NewAttachment, RegistryStore, WarehouseIdentity,
};
pub use tag_version::{
    bump_bound_tag_versions, bump_tag_versions_for_warehouses, services_bound_to_warehouse,
    services_bound_to_warehouses,
};
