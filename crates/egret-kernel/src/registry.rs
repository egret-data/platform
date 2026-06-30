//! `KernelRegistry` — parameterized Postgres persistence for the per-(user×workspace) kernel table.

use chrono::{DateTime, Utc};
use egret_metastore::Db;
use uuid::Uuid;

use crate::error::KernelError;

// ── Key ──────────────────────────────────────────────────────────────────────

/// Composite primary key for the `kernels` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KernelKey {
    pub user_id: Uuid,
    pub workspace_id: Uuid,
}

// ── Row ──────────────────────────────────────────────────────────────────────

/// A row returned by `get` / `list_*` queries.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct KernelRow {
    pub gateway_url: String,
    pub pid: Option<i32>,
    pub phase: String,
    pub last_used_at: DateTime<Utc>,
}

// ── Store ────────────────────────────────────────────────────────────────────

/// Thin Postgres store for the `kernels` table.
#[derive(Clone)]
pub struct KernelRegistry {
    db: Db,
}

impl KernelRegistry {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// INSERT or UPDATE the kernel row for `key`.
    ///
    /// On conflict (user_id, workspace_id) updates gateway_url, pid, phase, and last_used_at.
    pub async fn upsert(
        &self,
        key: KernelKey,
        url: &str,
        pid: Option<i32>,
        phase: &str,
    ) -> Result<(), KernelError> {
        sqlx::query(
            "INSERT INTO kernels (user_id, workspace_id, gateway_url, pid, phase) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (user_id, workspace_id) DO UPDATE \
             SET gateway_url = EXCLUDED.gateway_url, \
                 pid         = EXCLUDED.pid, \
                 phase       = EXCLUDED.phase, \
                 last_used_at = now()",
        )
        .bind(key.user_id)
        .bind(key.workspace_id)
        .bind(url)
        .bind(pid)
        .bind(phase)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Fetch the row for `key`, or `None` if no kernel exists.
    pub async fn get(&self, key: KernelKey) -> Result<Option<KernelRow>, KernelError> {
        let row = sqlx::query_as::<_, KernelRow>(
            "SELECT gateway_url, pid, phase, last_used_at \
             FROM kernels WHERE user_id = $1 AND workspace_id = $2",
        )
        .bind(key.user_id)
        .bind(key.workspace_id)
        .fetch_optional(&self.db.pool)
        .await?;
        Ok(row)
    }

    /// Update only the `phase` column for `key`.
    pub async fn set_phase(&self, key: KernelKey, phase: &str) -> Result<(), KernelError> {
        sqlx::query(
            "UPDATE kernels SET phase = $3 \
             WHERE user_id = $1 AND workspace_id = $2",
        )
        .bind(key.user_id)
        .bind(key.workspace_id)
        .bind(phase)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Bump `last_used_at` to now for `key`.
    pub async fn touch(&self, key: KernelKey) -> Result<(), KernelError> {
        sqlx::query(
            "UPDATE kernels SET last_used_at = now() \
             WHERE user_id = $1 AND workspace_id = $2",
        )
        .bind(key.user_id)
        .bind(key.workspace_id)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Delete the kernel row for `key` (idempotent).
    pub async fn delete(&self, key: KernelKey) -> Result<(), KernelError> {
        sqlx::query("DELETE FROM kernels WHERE user_id = $1 AND workspace_id = $2")
            .bind(key.user_id)
            .bind(key.workspace_id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    /// Set the hashed kernel token + its expiry for `key`.
    pub async fn set_token(
        &self,
        key: KernelKey,
        token_hash: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<(), KernelError> {
        sqlx::query(
            "UPDATE kernels SET token_hash = $3, token_expires_at = $4 \
             WHERE user_id = $1 AND workspace_id = $2",
        )
        .bind(key.user_id)
        .bind(key.workspace_id)
        .bind(token_hash)
        .bind(expires_at)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Resolve a hashed kernel token to its `(user_id, workspace_id)`, if a live
    /// (unexpired) kernel row carries it.
    pub async fn resolve_token(&self, token_hash: &str) -> Result<Option<KernelKey>, KernelError> {
        let row: Option<(Uuid, Uuid)> = sqlx::query_as(
            "SELECT user_id, workspace_id FROM kernels \
             WHERE token_hash = $1 AND token_expires_at > now()",
        )
        .bind(token_hash)
        .fetch_optional(&self.db.pool)
        .await?;
        Ok(row.map(|(user_id, workspace_id)| KernelKey {
            user_id,
            workspace_id,
        }))
    }

    /// Return all kernel keys whose `last_used_at` is strictly before `before`.
    ///
    /// Used by the idle reaper to find stale kernels.
    pub async fn list_idle(&self, before: DateTime<Utc>) -> Result<Vec<KernelKey>, KernelError> {
        #[derive(sqlx::FromRow)]
        struct Row {
            user_id: Uuid,
            workspace_id: Uuid,
        }
        let rows = sqlx::query_as::<_, Row>(
            "SELECT user_id, workspace_id FROM kernels WHERE last_used_at < $1",
        )
        .bind(before)
        .fetch_all(&self.db.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| KernelKey {
                user_id: r.user_id,
                workspace_id: r.workspace_id,
            })
            .collect())
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use egret_metastore::Db;

    /// Seed a user + workspace. Returns (workspace_id, user_id).
    /// Mirrors the pattern from egret-notebook store tests.
    async fn seed(db: &Db) -> (Uuid, Uuid) {
        let user_id: Uuid = sqlx::query_scalar(
            "INSERT INTO users (provider, email, name) \
             VALUES ('local','kernel-alice@test.local','Alice') RETURNING id",
        )
        .fetch_one(&db.pool)
        .await
        .expect("insert user");

        let ws_id: Uuid = sqlx::query_scalar(
            "INSERT INTO workspaces (slug, name, created_by) \
             VALUES ('kernel-test-ws','Kernel Test WS',$1) RETURNING id",
        )
        .bind(user_id)
        .fetch_one(&db.pool)
        .await
        .expect("insert workspace");

        (ws_id, user_id)
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn set_and_resolve_token(pool: sqlx::PgPool) {
        let db = Db { pool };
        let reg = KernelRegistry::new(db.clone());
        let (ws, user) = seed(&db).await;
        let key = KernelKey {
            user_id: user,
            workspace_id: ws,
        };
        reg.upsert(key, "http://127.0.0.1:1", Some(1), "ready")
            .await
            .unwrap();

        // No token yet → resolve returns None.
        assert!(reg.resolve_token("deadbeef").await.unwrap().is_none());

        // Set a token → resolves to this key while unexpired.
        reg.set_token(key, "abc123", Utc::now() + chrono::Duration::hours(1))
            .await
            .unwrap();
        assert_eq!(reg.resolve_token("abc123").await.unwrap(), Some(key));

        // Expired token → not resolved.
        reg.set_token(key, "old", Utc::now() - chrono::Duration::minutes(1))
            .await
            .unwrap();
        assert!(reg.resolve_token("old").await.unwrap().is_none());

        // Deleting the row revokes the token.
        reg.set_token(key, "live", Utc::now() + chrono::Duration::hours(1))
            .await
            .unwrap();
        reg.delete(key).await.unwrap();
        assert!(reg.resolve_token("live").await.unwrap().is_none());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn upsert_touch_delete_and_list_idle(pool: sqlx::PgPool) {
        let db = Db { pool };
        let reg = KernelRegistry::new(db.clone());
        let (ws, user) = seed(&db).await;
        let key = KernelKey {
            user_id: user,
            workspace_id: ws,
        };

        reg.upsert(key, "http://127.0.0.1:9999", Some(1234), "ready")
            .await
            .unwrap();

        let row = reg.get(key).await.unwrap().unwrap();
        assert_eq!(row.phase, "ready");
        assert_eq!(row.gateway_url, "http://127.0.0.1:9999");

        // touch bumps last_used_at; list_idle(now - 30min) should NOT include
        // a just-touched kernel (its last_used_at == now > cutoff).
        reg.touch(key).await.unwrap();
        let idle_now = reg
            .list_idle(Utc::now() - chrono::Duration::minutes(30))
            .await
            .unwrap();
        assert!(
            idle_now.iter().all(|k| *k != key),
            "freshly touched kernel should not appear as idle"
        );

        reg.delete(key).await.unwrap();
        assert!(
            reg.get(key).await.unwrap().is_none(),
            "row must be gone after delete"
        );
    }
}
