//! Postgres-backed group upsert + per-user OIDC group synchronization.
//!
//! # Every write to `user_groups` recompiles the engine policies it changes
//!
//! **Since J-F3 Task 1b (design R8), a group's MEMBERSHIP is compiled into the
//! engine policies.** D7 (`docs/en/plans/2026-09-24-permission-model-sp4a-d7-result.md`)
//! measured that no engine egret provisions ever sees a group — no
//! `group-provider.properties`, no usersync — so a Ranger item naming a group
//! matched nobody: an allow on a group granted nobody while `explain` said it
//! did, and a group in a deny include or an exclude failed OPEN. So
//! `egret-perm-compile` now expands a non-`public` group principal into its
//! members' stored usernames at compile time, exactly as SP4c did for roles —
//! and that enumeration is stale the moment a row of `user_groups` changes.
//!
//! The staleness is not symmetric. A missed ADD denies a new member (annoying).
//! A missed REMOVE leaves the removed member named in every policy their group
//! reaches: still granted by an allow, still exempt from a mask, still SUBTRACTED
//! from a deny — a breach that egret's own UI cannot show, because the
//! `user_groups` row it reads was written correctly.
//!
//! # Why the recompile is a port and not a call
//!
//! `egret-perm-compile` depends on this crate (for `users.username`), so these
//! writers cannot call it — the same constraint `egret_auth::roles::RoleRepo`
//! has (`egret_perm_compile::role_membership`'s module doc). Roles solved it by
//! moving the transaction into the compiler crate, which works because every
//! role writer is an HTTP handler in a crate ABOVE the compiler. Groups cannot:
//! one writer is the per-login OIDC sync ([`GroupRepo::sync_user_oidc_groups`],
//! called from `crate::oidc_routes`, in this crate), and another is
//! `UserRepo::hard_delete`, called from `egret-dataapi`, which does not depend
//! on the compiler either. So the recompile is the [`GroupMembershipRecompiler`]
//! port, the same dependency inversion `egret_policy::ProfileRecompiler` uses
//! for the policy editor: the trait lives here, `egret-perm-compile` implements
//! it, `egret-bin` wires it.
//!
//! **Every writer takes the port as an ARGUMENT, not as an optional field.** An
//! `Option` that defaults to `None` is a recompile somebody forgets to wire,
//! and a forgotten recompile is the breach above; a required argument is one
//! the compiler refuses to let a new caller omit. There is deliberately no
//! no-op implementation in this crate for the same reason.
//!
//! The order every writer follows is the one
//! `egret_perm_compile::role_membership::with_recompile` documents: name the
//! affected groups, take every profile lock ASCENDING
//! ([`GroupMembershipRecompiler::lock_affected_in_tx`]), write, recompile on
//! the same transaction ([`GroupMembershipRecompiler::recompile_in_tx`]), and
//! commit only if the recompile succeeded — a committed membership change whose
//! policies were never rewritten is worse than a refused one.

use egret_core::{Result, UserId};
use egret_metastore::Db;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

/// Recompiles the engine policies a change to a group's MEMBERSHIP can reach,
/// on the caller's open transaction — the seam `egret-perm-compile` fills
/// (`PermGroupRecompiler`), because this crate cannot depend on it (module
/// doc).
///
/// Two calls rather than one because the profile locks must be taken BEFORE
/// the membership write and the recompile must run AFTER it. A membership
/// write takes row locks of its own (a user delete also nulls attribution
/// columns such as `catalog_tag_assignments.assigned_by`), and a transaction
/// that took those before reaching for a profile lock could deadlock against
/// a concurrent recompile that holds the profile and wants the rows. Profiles
/// first, ascending, then rows, is the global order
/// `egret_perm_compile::lock_services_in_tx` documents.
#[async_trait::async_trait]
pub trait GroupMembershipRecompiler: Send + Sync {
    /// The services whose compiled policies name a member of any group in
    /// `group_names` (by group NAME — the key `perm.grant_principals` and
    /// `perm.rule_exemptions` store), with every one of their locks taken,
    /// ascending. Called BEFORE the membership write.
    async fn lock_affected_in_tx(
        &self,
        tx: &mut Transaction<'static, Postgres>,
        group_names: &[String],
    ) -> Result<Vec<i64>>;

    /// Recompile `service_ids` (as returned by
    /// [`lock_affected_in_tx`](Self::lock_affected_in_tx)) on `tx`. An error
    /// refuses the membership write, which rolls back with it.
    async fn recompile_in_tx(
        &self,
        tx: &mut Transaction<'static, Postgres>,
        service_ids: &[i64],
    ) -> Result<()>;
}

/// The names of every group `user_id` directly belongs to, on `conn`.
pub(crate) async fn group_names_of_user_in_tx(
    conn: &mut sqlx::PgConnection,
    user_id: Uuid,
) -> Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT g.name FROM user_groups ug JOIN groups g ON g.id = ug.group_id \
         WHERE ug.user_id = $1 ORDER BY g.name",
    )
    .bind(user_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)
}

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
    ///
    /// **Runs on every OIDC login, so it recompiles only what the login
    /// CHANGED.** The groups the user would hold after the write are computed
    /// before it (the local ones they keep, plus every claimed name that names a
    /// group — the INSERT below joins `groups` by name, so a claimed name with
    /// no row is not a membership), and only the symmetric difference with the
    /// groups they hold now is handed to `recompiler`. A login whose claim did
    /// not move — the common case — takes no profile lock and compiles nothing;
    /// one that did recompiles only the profiles whose policies name one of
    /// the moved groups. Recompiling on every login would make every login an
    /// installation-wide write, which is the cost `egret_perm_compile::members`
    /// once cited for not expanding groups at all.
    ///
    /// A recompile failure refuses the sync and the login with it (the caller
    /// answers 500): the membership stays as it was and so does every policy
    /// computed from it, which is the consistent state. Committing the sync
    /// without the recompile would be the inconsistent one — and on a REMOVE it
    /// is the breach this module's doc describes.
    pub async fn sync_user_oidc_groups(
        &self,
        user_id: UserId,
        names: &[String],
        recompiler: &dyn GroupMembershipRecompiler,
    ) -> Result<()> {
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;

        let before: Vec<(String, String)> = sqlx::query_as(
            "SELECT g.name, g.source FROM user_groups ug JOIN groups g ON g.id = ug.group_id \
             WHERE ug.user_id = $1",
        )
        .bind(user_id.0)
        .fetch_all(&mut *tx)
        .await
        .map_err(db_err)?;
        let claimed: Vec<String> =
            sqlx::query_scalar("SELECT name FROM groups WHERE name = ANY($1)")
                .bind(names)
                .fetch_all(&mut *tx)
                .await
                .map_err(db_err)?;
        let before_set: std::collections::BTreeSet<String> =
            before.iter().map(|(n, _)| n.clone()).collect();
        let after_set: std::collections::BTreeSet<String> = before
            .iter()
            .filter(|(_, source)| source != "oidc")
            .map(|(n, _)| n.clone())
            .chain(claimed)
            .collect();
        let moved: Vec<String> = before_set
            .symmetric_difference(&after_set)
            .cloned()
            .collect();

        let services = if moved.is_empty() {
            Vec::new()
        } else {
            recompiler.lock_affected_in_tx(&mut tx, &moved).await?
        };

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

        if !moved.is_empty() {
            recompiler.recompile_in_tx(&mut tx, &services).await?;
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
    ///
    /// **A rename is a membership change for TWO names.** Grants and rule
    /// exemptions name a group by NAME (`perm.grant_principals.ref`), with no FK
    /// to `groups`, so after the rename every principal naming the OLD name
    /// expands to nobody — a deny or exclude on it evaporates — and any naming
    /// the NEW name starts expanding to these members. Both are recompiled.
    pub async fn rename(
        &self,
        id: Uuid,
        name: &str,
        recompiler: &dyn GroupMembershipRecompiler,
    ) -> Result<u64> {
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        let old: Option<String> =
            sqlx::query_scalar("SELECT name FROM groups WHERE id = $1 AND source = 'local'")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(db_err)?;
        let Some(old) = old else {
            // Nothing to rename (oidc-sourced or absent): the UPDATE below would
            // match no row, so there is nothing to lock or recompile.
            return Ok(0);
        };
        let names = vec![old, name.to_string()];
        let services = recompiler.lock_affected_in_tx(&mut tx, &names).await?;
        let rows = sqlx::query("UPDATE groups SET name = $2 WHERE id = $1 AND source = 'local'")
            .bind(id)
            .bind(name)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?
            .rows_affected();
        if rows > 0 {
            recompiler.recompile_in_tx(&mut tx, &services).await?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(rows)
    }

    /// Delete a local group. No-op (0 rows) if the group is oidc-sourced or
    /// does not exist, so callers can 409 on a synced group.
    ///
    /// `user_groups.group_id` is `ON DELETE CASCADE`
    /// (`migrations/0002_identity.sql`), so this removes EVERY member at once —
    /// the largest REMOVE there is — and is recompiled like one. The affected
    /// set is read from the group's name before the row is gone; afterwards a
    /// principal still naming it expands to nobody.
    pub async fn delete_local(
        &self,
        id: Uuid,
        recompiler: &dyn GroupMembershipRecompiler,
    ) -> Result<u64> {
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        let name: Option<String> =
            sqlx::query_scalar("SELECT name FROM groups WHERE id = $1 AND source = 'local'")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(db_err)?;
        let Some(name) = name else {
            return Ok(0);
        };
        let services = recompiler
            .lock_affected_in_tx(&mut tx, std::slice::from_ref(&name))
            .await?;
        let rows = sqlx::query("DELETE FROM groups WHERE id = $1 AND source = 'local'")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?
            .rows_affected();
        if rows > 0 {
            recompiler.recompile_in_tx(&mut tx, &services).await?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(rows)
    }

    /// Add a user to a group (idempotent), recompiling every policy the group
    /// reaches in the same transaction (module doc).
    pub async fn add_member(
        &self,
        group_id: Uuid,
        user_id: UserId,
        recompiler: &dyn GroupMembershipRecompiler,
    ) -> Result<()> {
        self.change_member(group_id, user_id, true, recompiler)
            .await
    }

    /// Remove a user from a group (idempotent), recompiling in the same
    /// transaction — the direction that fails OPEN when it is missed (module
    /// doc).
    pub async fn remove_member(
        &self,
        group_id: Uuid,
        user_id: UserId,
        recompiler: &dyn GroupMembershipRecompiler,
    ) -> Result<()> {
        self.change_member(group_id, user_id, false, recompiler)
            .await
    }

    /// The one shape both directions share, so the order cannot drift between
    /// them: name → lock → write → recompile (only if a row changed) → commit.
    /// A group id that does not exist skips the lock and lets the write answer
    /// (the INSERT's FK refuses it, the DELETE matches nothing), as
    /// `egret_perm_compile::role_membership` treats an unresolvable role.
    async fn change_member(
        &self,
        group_id: Uuid,
        user_id: UserId,
        add: bool,
        recompiler: &dyn GroupMembershipRecompiler,
    ) -> Result<()> {
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        let name: Option<String> = sqlx::query_scalar("SELECT name FROM groups WHERE id = $1")
            .bind(group_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_err)?;
        let services = match &name {
            Some(n) => {
                recompiler
                    .lock_affected_in_tx(&mut tx, std::slice::from_ref(n))
                    .await?
            }
            None => Vec::new(),
        };
        let sql = if add {
            "INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2) \
             ON CONFLICT DO NOTHING"
        } else {
            "DELETE FROM user_groups WHERE user_id = $1 AND group_id = $2"
        };
        let rows = sqlx::query(sql)
            .bind(user_id.0)
            .bind(group_id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?
            .rows_affected();
        if name.is_some() && rows > 0 {
            recompiler.recompile_in_tx(&mut tx, &services).await?;
        }
        tx.commit().await.map_err(db_err)?;
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
