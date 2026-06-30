//! `NotebookKernelStore` — maps (notebook_id, user_id) → the gateway's kernel id.

use egret_metastore::Db;
use uuid::Uuid;

use crate::error::KernelError;

// ── Store ────────────────────────────────────────────────────────────────────

/// Thin Postgres store for the `notebook_kernels` table.
#[derive(Clone)]
pub struct NotebookKernelStore {
    db: Db,
}

impl NotebookKernelStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Return the kernel id bound to this (notebook, user) pair, or `None` if none exists.
    pub async fn get(
        &self,
        notebook_id: Uuid,
        user_id: Uuid,
    ) -> Result<Option<String>, KernelError> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT kernel_id FROM notebook_kernels WHERE notebook_id=$1 AND user_id=$2",
        )
        .bind(notebook_id)
        .bind(user_id)
        .fetch_optional(&self.db.pool)
        .await?;
        Ok(row.map(|r| r.0))
    }

    /// Insert or update the kernel id for `(notebook_id, user_id)`.
    pub async fn upsert(
        &self,
        notebook_id: Uuid,
        user_id: Uuid,
        kernel_id: &str,
    ) -> Result<(), KernelError> {
        sqlx::query(
            "INSERT INTO notebook_kernels (notebook_id, user_id, kernel_id) VALUES ($1,$2,$3) \
             ON CONFLICT (notebook_id, user_id) DO UPDATE \
             SET kernel_id = EXCLUDED.kernel_id, last_used_at = now()",
        )
        .bind(notebook_id)
        .bind(user_id)
        .bind(kernel_id)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Bump `last_used_at` to now for `(notebook_id, user_id)`.
    pub async fn touch(&self, notebook_id: Uuid, user_id: Uuid) -> Result<(), KernelError> {
        sqlx::query(
            "UPDATE notebook_kernels SET last_used_at = now() \
             WHERE notebook_id=$1 AND user_id=$2",
        )
        .bind(notebook_id)
        .bind(user_id)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Delete the mapping for `(notebook_id, user_id)` (idempotent).
    pub async fn delete(&self, notebook_id: Uuid, user_id: Uuid) -> Result<(), KernelError> {
        sqlx::query("DELETE FROM notebook_kernels WHERE notebook_id=$1 AND user_id=$2")
            .bind(notebook_id)
            .bind(user_id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use egret_metastore::Db;
    use uuid::Uuid;

    /// Seed a user + workspace + notebook. Returns (notebook_id, user_id).
    async fn seed(db: &Db) -> (Uuid, Uuid) {
        let user_id: Uuid = sqlx::query_scalar(
            "INSERT INTO users (provider, email, name) \
             VALUES ('local','nbk-alice@test.local','Alice') RETURNING id",
        )
        .fetch_one(&db.pool)
        .await
        .expect("insert user");
        let ws_id: Uuid = sqlx::query_scalar(
            "INSERT INTO workspaces (slug, name, created_by) \
             VALUES ('nbk-test-ws','NBK Test WS',$1) RETURNING id",
        )
        .bind(user_id)
        .fetch_one(&db.pool)
        .await
        .expect("insert workspace");
        let nb_id: Uuid = sqlx::query_scalar(
            "INSERT INTO notebooks (workspace_id, owner_user_id, name) \
             VALUES ($1,$2,'nb') RETURNING id",
        )
        .bind(ws_id)
        .bind(user_id)
        .fetch_one(&db.pool)
        .await
        .expect("insert notebook");
        (nb_id, user_id)
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn upsert_get_touch_delete(pool: sqlx::PgPool) {
        let db = Db { pool };
        let store = NotebookKernelStore::new(db.clone());
        let (nb, user) = seed(&db).await;
        assert!(store.get(nb, user).await.unwrap().is_none());
        store.upsert(nb, user, "kid-1").await.unwrap();
        assert_eq!(store.get(nb, user).await.unwrap().as_deref(), Some("kid-1"));
        store.upsert(nb, user, "kid-2").await.unwrap(); // re-bind
        assert_eq!(store.get(nb, user).await.unwrap().as_deref(), Some("kid-2"));
        store.touch(nb, user).await.unwrap();
        store.delete(nb, user).await.unwrap();
        assert!(store.get(nb, user).await.unwrap().is_none());
    }
}
