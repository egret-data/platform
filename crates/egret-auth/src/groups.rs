//! Postgres-backed group upsert + per-user OIDC group synchronization.

use egret_core::{Result, UserId};
use egret_metastore::Db;
use uuid::Uuid;

/// A stored group row (local or synced from OIDC).
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct Group {
    pub id: Uuid,
    pub name: String,
    pub source: String,
}

#[derive(Clone)]
pub struct GroupRepo {
    db: Db,
}

impl GroupRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// List groups (name-ordered) for the principal picker.
    pub async fn list(&self, limit: i64) -> Result<Vec<Group>> {
        sqlx::query_as::<_, Group>("SELECT id, name, source FROM groups ORDER BY name LIMIT $1")
            .bind(limit)
            .fetch_all(&self.db.pool)
            .await
            .map_err(db_err)
    }

    /// Groups whose name contains `q` (case-insensitive; `%`, `_` and `\` in `q`
    /// are literal), name-ordered, at most `limit` — [`list`](Self::list) filtered
    /// in SQL rather than in the caller, which could only filter its own page.
    pub async fn search(&self, q: &str, limit: i64) -> Result<Vec<Group>> {
        sqlx::query_as::<_, Group>(
            "SELECT id, name, source FROM groups WHERE name ILIKE $1 ESCAPE '\\' \
             ORDER BY name LIMIT $2",
        )
        .bind(crate::contains_pattern(q))
        .bind(limit)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)
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

    /// Create a local (admin-managed) group; returns its id.
    /// Errors with `Conflict` if a group with the same name already exists.
    pub async fn create_local(&self, name: &str) -> Result<Uuid> {
        let res = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO groups (name, source) VALUES ($1, 'local') RETURNING id",
        )
        .bind(name)
        .fetch_one(&self.db.pool)
        .await;
        match res {
            Ok(id) => Ok(id),
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Err(
                egret_core::Error::Conflict(format!("group already exists: {name}")),
            ),
            Err(e) => Err(db_err(e)),
        }
    }

    /// Rename a local group. No-op (0 rows) if the group is oidc-sourced or
    /// does not exist; returns the number of rows affected.
    pub async fn rename(&self, id: Uuid, name: &str) -> Result<u64> {
        Ok(
            sqlx::query("UPDATE groups SET name = $2 WHERE id = $1 AND source = 'local'")
                .bind(id)
                .bind(name)
                .execute(&self.db.pool)
                .await
                .map_err(db_err)?
                .rows_affected(),
        )
    }

    /// Delete a local group. No-op (0 rows) if the group is oidc-sourced or
    /// does not exist, so callers can 409 on a synced group.
    pub async fn delete_local(&self, id: Uuid) -> Result<u64> {
        Ok(
            sqlx::query("DELETE FROM groups WHERE id = $1 AND source = 'local'")
                .bind(id)
                .execute(&self.db.pool)
                .await
                .map_err(db_err)?
                .rows_affected(),
        )
    }

    /// Add a user to a group (idempotent).
    pub async fn add_member(&self, group_id: Uuid, user_id: UserId) -> Result<()> {
        sqlx::query(
            "INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2) \
             ON CONFLICT DO NOTHING",
        )
        .bind(user_id.0)
        .bind(group_id)
        .execute(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    /// Remove a user from a group (idempotent).
    pub async fn remove_member(&self, group_id: Uuid, user_id: UserId) -> Result<()> {
        sqlx::query("DELETE FROM user_groups WHERE group_id = $1 AND user_id = $2")
            .bind(group_id)
            .bind(user_id.0)
            .execute(&self.db.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    /// List the member user ids of a group.
    pub async fn members_of(&self, group_id: Uuid) -> Result<Vec<Uuid>> {
        sqlx::query_scalar("SELECT user_id FROM user_groups WHERE group_id = $1")
            .bind(group_id)
            .fetch_all(&self.db.pool)
            .await
            .map_err(db_err)
    }

    /// List the member users (id, email, name) of a group, email-ordered —
    /// used to prefill the admin console's manage-members dialog.
    pub async fn member_users(&self, group_id: Uuid) -> Result<Vec<(Uuid, String, String)>> {
        let rows = sqlx::query_as::<_, MemberUserRow>(
            "SELECT u.id, u.email::text AS email, u.name FROM user_groups ug \
             JOIN users u ON u.id = ug.user_id \
             WHERE ug.group_id = $1 ORDER BY u.email",
        )
        .bind(group_id)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(|r| (r.id, r.email, r.name)).collect())
    }

    /// Names of the groups a user directly belongs to (name-ordered), used to
    /// populate the `groups` field of the `GET /users` DTO.
    pub async fn groups_of_user(&self, user_id: UserId) -> Result<Vec<String>> {
        sqlx::query_scalar(
            "SELECT g.name FROM user_groups ug JOIN groups g ON g.id = ug.group_id \
             WHERE ug.user_id = $1 ORDER BY g.name",
        )
        .bind(user_id.0)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)
    }

    /// Each group with its member count, name-ordered.
    pub async fn list_with_meta(&self) -> Result<Vec<(Group, i64)>> {
        let rows = sqlx::query_as::<_, GroupWithCountRow>(
            "SELECT g.id, g.name, g.source, COUNT(ug.user_id) AS member_count \
             FROM groups g \
             LEFT JOIN user_groups ug ON ug.group_id = g.id \
             GROUP BY g.id, g.name, g.source \
             ORDER BY g.name",
        )
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows
            .into_iter()
            .map(|r| {
                (
                    Group {
                        id: r.id,
                        name: r.name,
                        source: r.source,
                    },
                    r.member_count,
                )
            })
            .collect())
    }
}

#[derive(sqlx::FromRow)]
struct GroupWithCountRow {
    id: Uuid,
    name: String,
    source: String,
    member_count: i64,
}

#[derive(sqlx::FromRow)]
struct MemberUserRow {
    id: Uuid,
    email: String,
    name: String,
}

fn db_err(e: sqlx::Error) -> egret_core::Error {
    egret_core::Error::Internal(anyhow::anyhow!(e))
}
