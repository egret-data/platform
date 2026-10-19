//! Cross-seam read into the engine-provisioning seam (E1 / `egret-engine-mgmt`).
//!
//! The admin Policies UI shows which provisioned engines bind to each Ranger
//! profile and offers their catalog names for resource autocomplete. That data
//! is owned by the engine seam — read it through this narrow `Port`, never by
//! reaching into engine-mgmt's tables (design §3.2: no cross-seam JOIN). The
//! composition root (egret-bin) supplies the concrete impl over `EngineMgmtStore`.
use async_trait::async_trait;

/// One provisioned engine and the policy profile (if any) it is bound to.
#[derive(Debug, Clone)]
pub struct EngineBinding {
    pub id: uuid::Uuid,
    pub name: String,
    pub status: String,
    /// `provisioned` or `external` (`engine_mgmt.engines.origin`).
    pub origin: String,
    /// The engine type: `trino`, `starrocks`, …
    pub r#type: String,
    /// The `policy.authz_services` id this engine enforces, if bound.
    pub authz_service_id: Option<i64>,
    /// The egret warehouses this engine reads — one per attached egret
    /// warehouse catalog, primary first. The keys for resolving catalog
    /// metadata (schemas/tables/columns) for resource autocomplete.
    ///
    /// A LIST since SP3c-2: an engine has attached several egret catalogs
    /// (each with its own warehouse) since SP3a, and this was the last place
    /// still describing one warehouse per engine — it carried
    /// `engines.warehouse_id`, which SP3c-2 drops. The picker therefore now
    /// offers the schemas of EVERY warehouse the engine can read, not only its
    /// primary's. Empty for an engine that attaches no egret warehouse catalog
    /// (a Trino reading only jdbc catalogs, say).
    pub warehouse_ids: Vec<uuid::Uuid>,
}

/// Transport-neutral read error (no engine-seam error type leaks across).
#[derive(Debug, thiserror::Error)]
#[error("engine binding read error: {0}")]
pub struct EngineReadError(pub String);

#[async_trait]
pub trait EngineBindingReader: Send + Sync {
    /// All provisioned engines with their profile binding.
    async fn list_engine_bindings(&self) -> Result<Vec<EngineBinding>, EngineReadError>;
    /// Catalog names configured on one engine (for resource autocomplete).
    async fn engine_catalog_names(
        &self,
        engine_id: uuid::Uuid,
    ) -> Result<Vec<String>, EngineReadError>;

    // ── Live catalog metadata for resource autocomplete ──────────────────────
    // Introspects a bound engine's warehouse (the embedded Iceberg catalog) so
    // the policy resource picker can suggest real schemas/tables/columns, not
    // just values already used in existing policies. Implementations return an
    // empty vec when the catalog is disabled or the warehouse isn't browsable.

    /// Schema (Iceberg namespace) names in a warehouse's catalog.
    async fn warehouse_schemas(
        &self,
        warehouse_id: uuid::Uuid,
    ) -> Result<Vec<String>, EngineReadError>;
    /// Table names in a warehouse's schema (namespace).
    async fn warehouse_tables(
        &self,
        warehouse_id: uuid::Uuid,
        schema: &str,
    ) -> Result<Vec<String>, EngineReadError>;
    /// Column names of a warehouse table.
    async fn warehouse_columns(
        &self,
        warehouse_id: uuid::Uuid,
        schema: &str,
        table: &str,
    ) -> Result<Vec<String>, EngineReadError>;
}
