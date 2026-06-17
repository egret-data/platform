//! Postgres-backed group upsert + per-user OIDC group synchronization.

use egret_core::{Result, UserId};
use egret_metastore::Db;
use uuid::Uuid;

#[derive(Clone)]
pub struct GroupRepo {
    db: Db,
}

impl GroupRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Insert (source='oidc') or return the existing group id by name.
    pub async fn upsert_oidc_group(&self, name: &str) -> Result<Uuid> {
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO groups (name, source) VALUES ($1, 'oidc') \
             ON CONFLICT (name) DO UPDATE SET name = groups.name RETURNING id",
        )
        .bind(name)
        .fetch_one(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(id)
    }

    /// Replace the user's OIDC-sourced group memberships with `names` (in one tx).
    /// Manual (source='local') memberships are left untouched.
    pub async fn sync_user_oidc_groups(&self, user_id: UserId, names: &[String]) -> Result<()> {
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        sqlx::query(
            "DELETE FROM user_groups WHERE user_id = $1 \
             AND group_id IN (SELECT id FROM groups WHERE source = 'oidc')",
        )
        .bind(user_id.0)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        for name in names {
            sqlx::query(
                "INSERT INTO user_groups (user_id, group_id) \
                 SELECT $1, id FROM groups WHERE name = $2 \
                 ON CONFLICT DO NOTHING",
            )
            .bind(user_id.0)
            .bind(name)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }
}

fn db_err(e: sqlx::Error) -> egret_core::Error {
    egret_core::Error::Internal(anyhow::anyhow!(e))
}
