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

    /// Insert a service account: a user row that can never log in.
    ///
    /// Generic over the executor so a caller inside a transaction (the Data API
    /// service-account create path, which must write this row and its
    /// `dataapi.service_accounts` binding atomically) and a caller on the pool
    /// use the SAME write.
    ///
    /// P2 could not do that and inlined an equivalent INSERT instead, which left
    /// [`UserRepo::insert_service`] with zero production callers: two statements
    /// free to drift, and a test suite pinning the one production had stopped
    /// taking. Taking `E: PgExecutor` costs nothing and removes the second copy —
    /// `&pool`, `&mut PgConnection` and `&mut *tx` all satisfy it.
    ///
    /// An associated function rather than a method: there is no `&self` to be had
    /// inside someone else's transaction, and the repo holds nothing this write
    /// needs.
    ///
    /// `password_hash` is deliberately not a parameter — a service account
    /// authenticates via an OAuth2 client, and `users_service_has_no_password`
    /// (migration 0119) rejects a row that has one.
    pub async fn insert_service_with<'e, E>(exec: E, email: &str, name: &str) -> Result<UserId>
    where
        E: sqlx::PgExecutor<'e>,
    {
        let res = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO users (provider, email, name) \
             VALUES ('service', $1, $2) RETURNING id",
        )
        .bind(email)
        .bind(name)
        .fetch_one(exec)
        .await;
        match res {
            Ok(id) => Ok(UserId(id)),
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Err(
                egret_core::Error::Conflict(format!("user already exists: {email}")),
            ),
            Err(e) => Err(db_err(e)),
        }
    }

    /// [`insert_service_with`](Self::insert_service_with) on this repo's pool.
    ///
    /// **Its only callers are tests, and that is now correct rather than a
    /// smell.** Production writes this row inside a transaction (the Data API
    /// service-account create path) and so calls `insert_service_with` directly;
    /// the previous round's finding — "zero production callers" — was about a
    /// second, INLINED copy of the statement, which is gone. This wrapper shares
    /// that one statement, so a test calling it exercises the write production
    /// takes. It stays because `egret-auth`'s and `egret-controlplane`'s suites
    /// have no transaction to hand and should not open one to seed a fixture.
    pub async fn insert_service(&self, email: &str, name: &str) -> Result<UserId> {
        Self::insert_service_with(&self.db.pool, email, name).await
    }

    /// Human users, newest first. Service accounts (`provider = 'service'`) are
    /// EXCLUDED: this feeds the admin Users page and the Access console's
    /// principal picker, where a machine identity is noise at best and gets
    /// mistaken for a person at worst. Resolution by id (`find_by_id`) is
    /// deliberately NOT filtered — a service account's name must still render
    /// next to its own grants and audit rows.
    pub async fn list(&self, limit: i64, offset: i64) -> Result<Vec<User>> {
        let rows = sqlx::query_as::<_, UserRow>(
            "SELECT id, provider, email, name, password_hash, status FROM users \
             WHERE provider <> 'service' \
             ORDER BY created_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// Human users whose email or name contains `q` (case-insensitive; `%`, `_`
    /// and `\` in `q` are literal), newest first, at most `limit`. The same rows
    /// [`list`](Self::list) returns, filtered in SQL — `list` pages by recency, so
    /// filtering its page in the caller can never find an older user.
    pub async fn search(&self, q: &str, limit: i64) -> Result<Vec<User>> {
        let rows = sqlx::query_as::<_, UserRow>(
            "SELECT id, provider, email, name, password_hash, status FROM users \
             WHERE provider <> 'service' \
               AND (email ILIKE $1 ESCAPE '\\' OR name ILIKE $1 ESCAPE '\\') \
             ORDER BY created_at DESC, id DESC LIMIT $2",
        )
        .bind(crate::contains_pattern(q))
        .bind(limit)
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

    /// Set a local user's password hash (admin reset). Caller hashes the plaintext.
    pub async fn set_password(&self, id: UserId, password_hash: &str) -> Result<()> {
        sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
            .bind(id.0)
            .bind(password_hash)
            .execute(&self.db.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    /// Return the stable username for `user_id`, deriving and persisting one if
    /// not yet set.  The username is the email local-part slugified to
    /// `[a-z0-9_-]`; collisions are resolved by appending `-2`, `-3`, …
    ///
    /// A thin wrapper over [`ensure_username_via`] so there is ONE
    /// implementation of what a username is — see that function's doc for why
    /// a connection-generic form exists at all.
    pub async fn ensure_username(&self, user_id: UserId) -> Result<String> {
        let mut conn = self.db.pool.acquire().await.map_err(db_err)?;
        ensure_username_via(&mut conn, user_id).await
    }

    /// Read-only, batched lookup of already-allocated usernames for `ids`.
    ///
    /// Unlike [`ensure_username`](Self::ensure_username) this NEVER mints one:
    /// a user missing from the returned map has not yet triggered allocation
    /// (no query run, no notebook opened, no engine token issued). That is a
    /// real, distinct state from "has a username" — the policy admin picker
    /// (`egret-policy::admin::list_principals`) must be able to tell them
    /// apart so it can refuse to offer a user with no `ref` to write, rather
    /// than inventing one.
    /// A thin wrapper over [`usernames_via`] so there is ONE implementation of
    /// this read — see that function's doc for why a connection-generic form
    /// exists.
    pub async fn usernames(&self, ids: &[Uuid]) -> Result<std::collections::HashMap<Uuid, String>> {
        let mut conn = self.db.pool.acquire().await.map_err(db_err)?;
        usernames_via(&mut conn, ids).await
    }

    /// Hard-delete `target` and dependents in one transaction. Attribution columns
    /// (created_by/assigned_by/owner_user_id/…) are auto-nulled by their
    /// `ON DELETE SET NULL` FKs (mig 0093); cascade tables (sessions/user_groups/…)
    /// auto-clean. `access_grants` is swept explicitly (polymorphic principal_id, no
    /// FK). The 23503 catch is a safety net: it fires only if a future table adds a
    /// NOT-NULL FK to users(id) without an ON DELETE rule.
    ///
    /// Returns `Some((name, email))` straight off the row `DELETE`'s own
    /// `RETURNING` — the row is gone once this returns, so a caller that needs
    /// to say WHO it deleted (Data API's `service_account:delete` audit event
    /// is the case that needs this) has those columns without a second,
    /// racing query.
    ///
    /// Returns `None`, rather than an error, if `target` was already gone by
    /// the time this ran. That is deliberate and preserves the function's
    /// original behaviour from before it grew a `RETURNING`: this is a SHARED
    /// method, and `egret-controlplane`'s `delete_user` — which does not read
    /// the return value at all — has always treated "already deleted" as an
    /// idempotent success rather than a 500. `fetch_optional`, not
    /// `fetch_one`, is what keeps that true; a caller that flipped this to
    /// `fetch_one` made a same-row-deleted-twice race in the control plane's
    /// user admin start failing loudly for a condition it used to shrug off.
    ///
    /// **It is also a `user_groups` write, and recompiles like one (J-F3 Task
    /// 1b).** A group principal compiles to its members' usernames
    /// (`crate::groups`' module doc), so the deleted user is named in every
    /// policy one of their groups reaches until those policies are recompiled —
    /// and a username freed by this delete can be minted again for the NEXT
    /// person whose email slugs to it, who would inherit every allow the stale
    /// policy still grants. The groups are read before the delete, their
    /// profiles locked first, and recompiled on this transaction after the user
    /// row is gone, through `recompiler`.
    pub async fn hard_delete(
        &self,
        target: UserId,
        recompiler: &dyn crate::groups::GroupMembershipRecompiler,
    ) -> Result<Option<(String, String)>> {
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        let groups = crate::groups::group_names_of_user_in_tx(&mut tx, target.0).await?;
        let services = if groups.is_empty() {
            Vec::new()
        } else {
            recompiler.lock_affected_in_tx(&mut tx, &groups).await?
        };
        sqlx::query("DELETE FROM sessions WHERE user_id = $1")
            .bind(target.0)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        sqlx::query("DELETE FROM user_groups WHERE user_id = $1")
            .bind(target.0)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        sqlx::query(
            "DELETE FROM access_grants WHERE principal_type = 'user' AND principal_id = $1",
        )
        .bind(target.0)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        // The row delete is the one that can trip a content-ownership FK
        // (23503). Map that specific case to a Conflict (→ 409); on return the
        // dropped `tx` rolls back, so no dependent rows above are lost.
        //
        // `fetch_optional`, deliberately not `fetch_one`: zero rows means
        // `target` was already gone, which this function has always treated
        // as an idempotent success rather than an error — see the doc comment
        // above. `fetch_one` would turn that into a `RowNotFound` and, via
        // `db_err`, a 500 for a caller that never asked for one.
        let res: std::result::Result<Option<(String, String)>, sqlx::Error> =
            sqlx::query_as("DELETE FROM users WHERE id = $1 RETURNING name, email")
                .bind(target.0)
                .fetch_optional(&mut *tx)
                .await;
        let deleted = match res {
            Ok(row) => row,
            Err(e) => {
                if e.as_database_error().and_then(|d| d.code()).as_deref() == Some("23503") {
                    return Err(egret_core::Error::Conflict(
                        "cannot delete: this user owns content in a table that isn't auto-handled \
                         on delete — a new FK to users(id) needs an ON DELETE rule."
                            .into(),
                    ));
                }
                return Err(db_err(e));
            }
        };
        if !groups.is_empty() {
            recompiler.recompile_in_tx(&mut tx, &services).await?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(deleted)
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

/// Connection-generic core of [`UserRepo::ensure_username`] — SP4a D3.
///
/// **Why this exists.** `egret-perm-compile`'s Ranger compiler expands a
/// `workspace_members` principal into the member list enumerated at compile
/// time (D7 §3: groups never reach the engine), and the name it must emit is
/// the bare username the engine sees — `users.username`. That column is
/// NULLable and **no user-insert path in egret sets it**: `insert_local`,
/// the OIDC upsert and the kernel's user rows all leave it NULL, and it is
/// minted lazily the first time someone runs a query, opens a notebook or is
/// issued an engine token. A compiler that only READ the column would
/// therefore omit every member who has not yet used an engine — including,
/// on a fresh install, the bootstrap admin — and the omission is invisible:
/// the policy compiles, deploys, is listed live, and denies that person.
/// Minting here closes it, and is safe to do from a compile because the
/// allocation is idempotent and stable: once a row has a username, every
/// later call returns the same one.
///
/// Runs every statement on `conn`, so a caller inside its own open
/// transaction both sees and keeps its own writes. **The collision retry is
/// wrapped in a SAVEPOINT**, which the pool-based caller never needed: a
/// unique violation aborts the *enclosing* transaction in Postgres, so the
/// `continue` that tries `base-2` after `base` was taken would poison a
/// caller's whole recompile instead of retrying inside it.
pub async fn ensure_username_via(conn: &mut sqlx::PgConnection, user_id: UserId) -> Result<String> {
    use sqlx::Connection as _;

    // 1. Fast path: username already set.
    let existing: Option<Option<String>> =
        sqlx::query_scalar("SELECT username::text FROM users WHERE id = $1")
            .bind(user_id.0)
            .fetch_optional(&mut *conn)
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
        .fetch_one(&mut *conn)
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
        // A principal egret itself runs as on an engine (`egret-lineage`) is
        // never a human's name: the engine JWT would carry its grants. Treated
        // as taken; migration 0187 also makes it unrepresentable.
        if egret_core::engine_principals::is_reserved_engine_username(&cand) {
            continue;
        }
        let mut sp = conn.begin().await.map_err(db_err)?;
        let res = sqlx::query("UPDATE users SET username = $2 WHERE id = $1 AND username IS NULL")
            .bind(user_id.0)
            .bind(&cand)
            .execute(&mut *sp)
            .await;
        match res {
            Ok(r) if r.rows_affected() == 1 => {
                sp.commit().await.map_err(db_err)?;
                return Ok(cand);
            }
            Ok(_) => {
                // Another concurrent caller set our username first — re-read.
                sp.commit().await.map_err(db_err)?;
                if let Some(u) = sqlx::query_scalar::<_, Option<String>>(
                    "SELECT username::text FROM users WHERE id = $1",
                )
                .bind(user_id.0)
                .fetch_one(&mut *conn)
                .await
                .map_err(db_err)?
                {
                    return Ok(u);
                }
            }
            Err(sqlx::Error::Database(d)) if d.is_unique_violation() => {
                sp.rollback().await.map_err(db_err)?;
                continue;
            }
            Err(e) => {
                let _ = sp.rollback().await;
                return Err(db_err(e));
            }
        }
    }

    Err(egret_core::Error::Internal(anyhow::anyhow!(
        "could not allocate a unique username for user {user_id:?} after 999 attempts"
    )))
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

/// Connection-generic core of [`UserRepo::usernames`] — SP4c Task 2.
///
/// **Read-only, and that is the point: this NEVER mints.** A user missing from
/// the returned map has no `users.username` allocated yet (no query run, no
/// notebook opened, no engine token issued), which is a real, distinct state
/// from "has a username" — and therefore also a real, distinct state from
/// "exists on the engine at all", since the bare username is the only identity
/// Trino/StarRocks ever see for a person (`X-Trino-User`).
///
/// **Why `egret-perm-compile` reads rather than re-derives.** `ensure_username`
/// lowercases the email local-part, slugifies it and resolves collisions with a
/// `-2`, `-3`, … suffix, so two humans whose local parts slug to the same base
/// differ only by a suffix nothing local can predict. A re-derived slug names
/// the WRONG person while compiling, deploying and reporting success. SP4c's
/// clearance compiler (`egret_perm_compile::clearance`) and its role-grant
/// expansion (`egret_perm_compile::members::role_usernames`) both go through
/// here, on the caller's own connection, so a compile inside an open
/// transaction sees that transaction's own writes.
pub async fn usernames_via(
    conn: &mut sqlx::PgConnection,
    ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, String>> {
    let rows: Vec<(Uuid, Option<String>)> =
        sqlx::query_as("SELECT id, username::text FROM users WHERE id = ANY($1)")
            .bind(ids)
            .fetch_all(&mut *conn)
            .await
            .map_err(db_err)?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, u)| u.map(|u| (id, u)))
        .collect())
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

    /// A recompile port that reaches no profile — this test is about the rows
    /// `hard_delete` removes; what it recompiles is asserted in
    /// `crates/egret-perm-compile/tests/group_grants.rs` and
    /// `tests/groups_db.rs`.
    struct NoProfiles;

    #[async_trait::async_trait]
    impl crate::groups::GroupMembershipRecompiler for NoProfiles {
        async fn lock_affected_in_tx(
            &self,
            _tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
            _group_names: &[String],
        ) -> Result<Vec<i64>> {
            Ok(Vec::new())
        }

        async fn recompile_in_tx(
            &self,
            _tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
            _service_ids: &[i64],
        ) -> Result<()> {
            Ok(())
        }
    }

    /// `hard_delete` removes the user row plus every dependent it's supposed to
    /// clean up in its transaction: live sessions, group memberships, and
    /// platform role/permission grants (`access_grants`) — leaving no orphaned
    /// rows and no error from the final `DELETE FROM users`.
    #[sqlx::test(migrations = "../../migrations")]
    async fn hard_delete_removes_user_and_dependents(pool: sqlx::PgPool) {
        let db = Db { pool };
        let repo = UserRepo::new(db.clone());
        let groups = crate::groups::GroupRepo::new(db.clone());
        let sessions = crate::session::SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        );

        let uid = repo
            .insert_local("hard_delete_target@egret.local", "Target", "x")
            .await
            .unwrap();

        // Group membership.
        let group_id = groups.create_local("hard-delete-test-group").await.unwrap();
        groups.add_member(group_id, uid, &NoProfiles).await.unwrap();
        assert_eq!(
            groups.groups_of_user(uid).await.unwrap(),
            vec!["hard-delete-test-group".to_string()]
        );

        // A platform role/permission grant via `access_grants`.
        let (role_id,): (Uuid,) = sqlx::query_as(
            "INSERT INTO roles (scope, name, description) VALUES ('platform', 'hard_delete_test_role', 't') \
             RETURNING id",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
             VALUES ('user', $1, $2, 'platform', NULL)",
        )
        .bind(uid.0)
        .bind(role_id)
        .execute(&db.pool)
        .await
        .unwrap();

        // A live session.
        let issued = sessions
            .create(uid, "test-agent", "127.0.0.1")
            .await
            .unwrap();
        assert!(sessions.validate(&issued.token).await.unwrap().is_some());

        repo.hard_delete(uid, &NoProfiles).await.unwrap();

        assert!(
            repo.find_by_id(uid).await.unwrap().is_none(),
            "user row gone"
        );
        assert!(
            groups.groups_of_user(uid).await.unwrap().is_empty(),
            "group membership cleaned"
        );
        assert!(
            sessions.validate(&issued.token).await.unwrap().is_none(),
            "session cleaned/invalidated"
        );
        let grant_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM access_grants WHERE principal_type = 'user' AND principal_id = $1",
        )
        .bind(uid.0)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(grant_count, 0, "access_grants cleaned");
    }
}
