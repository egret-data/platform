//! Postgres access: a pooled connection wrapper and the embedded migrator.
//! (touched to re-embed migrations: 0037 removes notebook engine column; 0040_user_username adds users.username citext; 0041_kernels adds kernels table; 0042_notebook_kernels adds notebook_kernels table; 0043_kernel_token adds token_hash + token_expires_at; 0044_oidc_refresh_tokens adds oidc_refresh_tokens table; 0045 adds unique index on workspace_warehouses.warehouse_id; 0046 adds notebook_cells.sql_target; 0047 drops notebook_cells.sql_target)
// migrations: through 0067 (E5b: analytics + kernel schemas)

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
