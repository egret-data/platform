//! Postgres access: a pooled connection wrapper and the embedded migrator.
//! (touched to re-embed migrations: 0037 removes notebook engine column; 0040_user_username adds users.username citext; 0041_kernels adds kernels table; 0042_notebook_kernels adds notebook_kernels table; 0043_kernel_token adds token_hash + token_expires_at; 0044_oidc_refresh_tokens adds oidc_refresh_tokens table; 0045 adds unique index on workspace_warehouses.warehouse_id; 0046 adds notebook_cells.sql_target; 0047 drops notebook_cells.sql_target; 0090 re-adds notebook_cells.sql_target + adds sql_into)
// migrations: through 0067 (E5b: analytics + kernel schemas); 0088 typed_secrets (multi-field secret_data); 0089 engine_tls_cert_ref
// re-embed: 0081 catalog_tag_targets (catalog table/column tag assignments)
// re-embed: 0082 classification_policies (tag-conditioned BI object access; Access P3.3)
// re-embed: 0083 classification_required_groups (clearance by group/department; Access P3.4)
// re-embed: 0092 drop_oidc_refresh_tokens (STS cleanup: dead Keycloak refresh path)

use sqlx::migrate::{MigrateError, Migrator};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

// migration 0023: per-workspace normalized catalog
// migration 0024: catalog perms
// migration 0025: workspace_warehouse mapping
// migration 0026: drop legacy catalog metadata tables (lakekeeper is the engine)
// migration 0027: per-token permissions on catalog_tokens
// migration 0028: per-token namespace_scope on catalog_tokens
// migration 0031: per-workspace DB connections (databases.workspace_id; drop workspace_database_grant)
// migration 0096: analytics.sql_snippets (SQL Lab reusable snippets)
// migration 0097: dataset column/metric semantic-layer config (groupby/filterable/format/certified)
// migration 0098: dataset semantic_type + certification metadata (dataset + metric certified_by/details)
// re-embed: 0104 dataset_semantic_type_allowlist (CHECK on dataset_columns.semantic_type; normalises unknown values to '' first)
// re-embed: 0105 dashboard_width_default_fixed (new dashboards default to Metabase's fixed canvas)
// re-embed: 0106 dashboard_attribution (dashboards.created_at nullable + updated_by)
/// All SQL migrations, embedded at compile time from the workspace `migrations/` dir.
pub static MIGRATOR: Migrator = sqlx::migrate!("../../migrations");

/// A thin handle around the shared Postgres pool.
#[derive(Clone)]
pub struct Db {
    pub pool: PgPool,
}

impl Db {
    /// Connect and build the pool.
    pub async fn connect(url: &str, max_connections: u32) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .connect(url)
            .await?;
        Ok(Self { pool })
    }

    /// Build a `Db` handle without opening a connection: the pool connects
    /// lazily on first use, so this only parses `url` and never touches the
    /// network. For tests that need a `Db` to satisfy a constructor (e.g.
    /// building a full `WorkspaceState` to check that routes are mounted)
    /// without ever issuing a query against a live database.
    pub fn connect_lazy(url: &str) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new().connect_lazy(url)?;
        Ok(Self { pool })
    }

    /// Run all pending migrations.
    pub async fn migrate(&self) -> Result<(), MigrateError> {
        MIGRATOR.run(&self.pool).await
    }

    /// Cheap liveness probe used by `/readyz`.
    pub async fn ping(&self) -> Result<(), sqlx::Error> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrator_embeds_baseline_migration() {
        assert!(
            MIGRATOR.migrations.len() >= 7,
            "expected through the authz migration"
        );
    }
}

// migration registry: 0033_databases_name_per_workspace
// migration registry: 0039_notebook_resource_access
// migration registry: 0040_user_username
// migration registry: 0062_engine_mgmt_schema (moves engines/engine_catalogs to engine_mgmt schema)
// migration registry: 0077_saved_query_objects (saved_queries.folder_id + saved_query:{read,manage} grants)
// migration registry: 0078_access_grant_expires_at (time-bound grants: access_grants.expires_at + partial index)
// migration registry: 0079 access_requests (join-request → approval workflow: access_requests table)
// migration registry: 0080 tags (classification model: tags + tag_assignments + tag:* perms)
// migration registry: 0085 engine_delegator_secret (impersonate-mode delegator secret ref)
// migration registry: 0086 engine_tls (jwt-mode coordinator TLS: tls_secret_id + coordinator_cert)
// migration registry: 0087 engine_root_secret (secure-by-default StarRocks FE root password ref)
