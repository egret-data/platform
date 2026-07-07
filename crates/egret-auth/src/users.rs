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
    /// Errors with `Conflict` if a user with the same email already exists.
    pub async fn insert_local(
        &self,
        email: &str,
        name: &str,
        password_hash: &str,
    ) -> Result<UserId> {
        let res = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO users (provider, email, name, password_hash) \
             VALUES ('local', $1, $2, $3) RETURNING id",
        )
        .bind(email)
        .bind(name)
        .bind(password_hash)
        .fetch_one(&self.db.pool)
        .await;
        match res {
            Ok(id) => Ok(UserId(id)),
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Err(
                egret_core::Error::Conflict(format!("user already exists: {email}")),
            ),
            Err(e) => Err(db_err(e)),
        }
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

    /// Set a user's status (`'active'` | `'disabled'`).
    pub async fn set_status(&self, id: UserId, status: &str) -> Result<()> {
        sqlx::query("UPDATE users SET status = $2 WHERE id = $1")
            .bind(id.0)
            .bind(status)
            .execute(&self.db.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    /// Return the stable username for `user_id`, deriving and persisting one if
    /// not yet set.  The username is the email local-part slugified to
    /// `[a-z0-9_-]`; collisions are resolved by appending `-2`, `-3`, …
    pub async fn ensure_username(&self, user_id: UserId) -> Result<String> {
        // 1. Fast path: username already set.
        let existing: Option<Option<String>> =
            sqlx::query_scalar("SELECT username::text FROM users WHERE id = $1")
                .bind(user_id.0)
                .fetch_optional(&self.db.pool)
                .await
                .map_err(db_err)?;

        match existing {
            None => {
                return Err(egret_core::Error::NotFound(format!("user {user_id:?}")));
            }
            Some(Some(u)) => return Ok(u),
            Some(None) => {} // need to derive
        }

        // 2. Derive base slug from email local-part.
        let email: String = sqlx::query_scalar("SELECT email::text FROM users WHERE id = $1")
            .bind(user_id.0)
            .fetch_one(&self.db.pool)
            .await
            .map_err(db_err)?;
        let base = slugify_localpart(&email);

        // 3. Try base, then base-2, base-3, … until the unique UPDATE sticks.
        for n in 1..1000u32 {
            let cand = if n == 1 {
                base.clone()
            } else {
                format!("{base}-{n}")
            };
            let res =
                sqlx::query("UPDATE users SET username = $2 WHERE id = $1 AND username IS NULL")
                    .bind(user_id.0)
                    .bind(&cand)
                    .execute(&self.db.pool)
                    .await;
            match res {
                Ok(r) if r.rows_affected() == 1 => return Ok(cand),
                Ok(_) => {
                    // Another concurrent caller set our username first — re-read.
                    if let Some(u) = sqlx::query_scalar::<_, Option<String>>(
                        "SELECT username::text FROM users WHERE id = $1",
                    )
                    .bind(user_id.0)
                    .fetch_one(&self.db.pool)
                    .await
                    .map_err(db_err)?
                    {
                        return Ok(u);
                    }
                }
                Err(sqlx::Error::Database(d)) if d.is_unique_violation() => continue,
                Err(e) => return Err(db_err(e)),
            }
        }

        Err(egret_core::Error::Internal(anyhow::anyhow!(
            "could not allocate a unique username for user {user_id:?} after 999 attempts"
        )))
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

/// Derive a username slug from an email address.
/// Lowercases the local-part, keeps only `[a-z0-9_-]` chars, collapses
/// repeated hyphens/underscores, and falls back to `"user"` if empty.
fn slugify_localpart(email: &str) -> String {
    let local = email.split('@').next().unwrap_or("").to_lowercase();
    // Replace any run of chars outside [a-z0-9_-] with a single '-'
    let mut slug = String::with_capacity(local.len());
    let mut prev_dash = false;
    for ch in local.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            slug.push(ch);
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    // Trim leading/trailing dashes
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "user".to_string()
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_metastore::Db;

    #[test]
    fn slugify_rules() {
        assert_eq!(slugify_localpart("Alice@Corp.com"), "alice");
        assert_eq!(slugify_localpart("alice@other.com"), "alice");
        assert_eq!(slugify_localpart("john.doe@example.com"), "john-doe");
        assert_eq!(slugify_localpart("@empty.com"), "user");
        assert_eq!(slugify_localpart("!!!@x.com"), "user");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn ensure_username_derives_and_is_stable(pool: sqlx::PgPool) {
        let repo = UserRepo::new(Db { pool });
        let a = repo
            .insert_local("Alice@Corp.com", "Alice", "x")
            .await
            .unwrap();
        let u1 = repo.ensure_username(a).await.unwrap();
        assert_eq!(u1, "alice"); // slug of local-part
        let u2 = repo.ensure_username(a).await.unwrap();
        assert_eq!(u2, "alice"); // stable (no re-derive)
        let b = repo
            .insert_local("alice@other.com", "Alice2", "x")
            .await
            .unwrap();
        let ub = repo.ensure_username(b).await.unwrap();
        assert_eq!(ub, "alice-2"); // collision suffix
    }
}
