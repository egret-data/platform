//! Postgres-backed user lookup and creation.

use egret_core::{Result, UserId};
use egret_metastore::Db;
use uuid::Uuid;

use crate::model::User;

#[derive(Clone)]
pub struct UserRepo {
    db: Db,
}

impl UserRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub async fn find_by_email(&self, email: &str) -> Result<Option<User>> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT id, provider, email, name, password_hash, status FROM users WHERE email = $1",
        )
        .bind(email)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(row.map(Into::into))
    }

    pub async fn find_by_id(&self, id: UserId) -> Result<Option<User>> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT id, provider, email, name, password_hash, status FROM users WHERE id = $1",
        )
        .bind(id.0)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(row.map(Into::into))
    }

    /// Insert a local user with a password hash; returns the new id.
    pub async fn insert_local(
        &self,
        email: &str,
        name: &str,
        password_hash: &str,
    ) -> Result<UserId> {
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO users (provider, email, name, password_hash) \
             VALUES ('local', $1, $2, $3) RETURNING id",
        )
        .bind(email)
        .bind(name)
        .bind(password_hash)
        .fetch_one(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(UserId(id))
    }

    pub async fn list(&self, limit: i64, offset: i64) -> Result<Vec<User>> {
        let rows = sqlx::query_as::<_, UserRow>(
            "SELECT id, provider, email, name, password_hash, status FROM users \
             ORDER BY created_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn touch_last_login(&self, id: UserId) -> Result<()> {
        sqlx::query("UPDATE users SET last_login_at = now() WHERE id = $1")
            .bind(id.0)
            .execute(&self.db.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    pub async fn find_by_external_subject(&self, sub: &str) -> Result<Option<User>> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT id, provider, email, name, password_hash, status FROM users \
             WHERE provider = 'oidc' AND external_subject = $1",
        )
        .bind(sub)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(row.map(Into::into))
    }

    pub async fn insert_oidc(&self, sub: &str, email: &str, name: &str) -> Result<UserId> {
        let res = sqlx::query_scalar::<_, uuid::Uuid>(
            "INSERT INTO users (provider, external_subject, email, name) \
             VALUES ('oidc', $1, $2, $3) RETURNING id",
        )
        .bind(sub)
        .bind(email)
        .bind(name)
        .fetch_one(&self.db.pool)
        .await;
        match res {
            Ok(id) => Ok(UserId(id)),
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Err(
                egret_core::Error::Conflict(format!("email or subject already exists: {email}")),
            ),
            Err(e) => Err(db_err(e)),
        }
    }

    pub async fn update_profile(&self, id: UserId, email: &str, name: &str) -> Result<()> {
        sqlx::query("UPDATE users SET email = $1, name = $2 WHERE id = $3")
            .bind(email)
            .bind(name)
            .bind(id.0)
            .execute(&self.db.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct UserRow {
    id: Uuid,
    provider: String,
    email: String,
    name: String,
    password_hash: Option<String>,
    status: String,
}

impl From<UserRow> for User {
    fn from(r: UserRow) -> Self {
        User {
            id: UserId(r.id),
            provider: r.provider,
            email: r.email,
            name: r.name,
            password_hash: r.password_hash,
            status: r.status,
        }
    }
}

fn db_err(e: sqlx::Error) -> egret_core::Error {
    egret_core::Error::Internal(anyhow::anyhow!(e))
}
