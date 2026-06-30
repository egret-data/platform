//! Per-user OIDC refresh-token reference (the token bytes live in `secrets`).
use egret_core::{Error, Result};
use egret_metastore::Db;
use uuid::Uuid;

#[derive(Clone)]
pub struct OidcRefreshRepo {
    db: Db,
}

impl OidcRefreshRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Upsert the vault secret id holding `user_id`'s refresh token.
    pub async fn set(&self, user_id: Uuid, secret_id: Uuid) -> Result<()> {
        sqlx::query(
            "INSERT INTO oidc_refresh_tokens (user_id, secret_id) VALUES ($1, $2) \
             ON CONFLICT (user_id) DO UPDATE SET secret_id = $2, updated_at = now()",
        )
        .bind(user_id)
        .bind(secret_id)
        .execute(&self.db.pool)
        .await
        .map_err(|e| Error::Internal(anyhow::anyhow!(e)))?;
        Ok(())
    }

    /// The vault secret id for `user_id`, if any.
    pub async fn get(&self, user_id: Uuid) -> Result<Option<Uuid>> {
        let row: Option<Uuid> =
            sqlx::query_scalar("SELECT secret_id FROM oidc_refresh_tokens WHERE user_id = $1")
                .bind(user_id)
                .fetch_optional(&self.db.pool)
                .await
                .map_err(|e| Error::Internal(anyhow::anyhow!(e)))?;
        Ok(row)
    }
}
