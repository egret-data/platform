//! Postgres access: a pooled connection wrapper and the embedded migrator.
//!

use sqlx::migrate::{MigrateError, Migrator};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

// migration 0023: per-workspace normalized catalog
// migration 0024: catalog perms
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
