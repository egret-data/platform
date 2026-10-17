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
//! A missed recompile fails OPEN in BOTH directions, depending on where the
//! group is named. A missed REMOVE leaves the ex-member in every policy their
//! group reaches: still granted by an allow, still exempt from a mask, still
//! SUBTRACTED from a deny. A missed ADD leaves the new member out: harmless on
//! an allow (they are merely not granted yet), but on a deny include it means
//! the deny does not reach them, and on an exclude or an exemption it means the
//! subtract or the exemption does. Either is a breach egret's own UI cannot
//! show, because the `user_groups` row it reads was written correctly.
//!
//! # Why the recompile is a port and not a call
//!
//! `egret-perm-compile` depends on this crate (for `users.username`), so these
//! writers cannot call it — the same constraint `egret_auth::roles::RoleRepo`
//! has (`egret_perm_compile::role_membership`'s module doc). Roles solved it by
//! moving the transaction into the compiler crate, which works because every
//! role writer is an HTTP handler in a crate ABOVE the compiler. Groups cannot:
//! one writer is the per-login OIDC sync ([`GroupRepo::sync_user_oidc_groups`],
//! called from `crate::oidc_routes`, in this crate; it writes rows of
//! OIDC-sourced groups only, design R1), and another is
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
//! The order every writer follows extends the one
//! `egret_perm_compile::role_membership::with_recompile` documents: advisory
//! locks (below), name the affected groups, take every profile lock ASCENDING
//! ([`GroupMembershipRecompiler::lock_affected_in_tx`]), write, recompile on
//! the same transaction ([`GroupMembershipRecompiler::recompile_in_tx`]), and
//! commit only if the recompile succeeded — a committed membership change whose
//! policies were never rewritten is worse than a refused one.
//!
//! # Two concurrent writers: the advisory locks (fix round 1)
//!
//! Profile locks alone do not serialise a membership write against everything
//! it races, because WHICH profiles to lock is itself decided by reads, and a
//! read under READ COMMITTED is a prediction that a concurrent commit can
//! falsify. Two such falsifications were found, both fail-OPEN:
//!
//! 1. **The moved set was predicted from an unlocked read of the user's
//!    groups.** A login read `U`'s groups while an admin's REMOVE of `U` from
//!    `G` was uncommitted, saw `G`, predicted "nothing moved", locked nothing —
//!    then re-inserted `(U, G)` after the REMOVE had recompiled `G` without
//!    `U`. And `hard_delete` read `U`'s groups, a concurrent ADD to `G`
//!    committed, and the delete's cascade removed `(U, G)` without recompiling
//!    `G`, leaving the deleted username in `G`'s allow.
//! 2. **The affected set was predicted from an unlocked read of the MODEL.** A
//!    REMOVE computed "no grant names `G`", while a concurrent "allow `G`"
//!    compiled with `U` still a member and committed; the REMOVE then committed
//!    with nothing to recompile, and `U` stayed granted.
//!
//! So every writer of `user_groups` takes, before ANY read it predicts from:
//!
//! - a **per-user** lock ([`lock_member_users_via`]) for every user whose rows
//!   it changes — serialising all writers of one user's memberships, so the
//!   groups a writer reads for that user cannot change until it commits; and
//! - a **per-group** lock ([`HeldGroupLocks`]) for every group NAME whose
//!   membership it changes. `egret_perm_compile`'s grant and rule writes (and
//!   the migration) take the same per-group lock for every group they name,
//!   so a model write that newly names `G` and a membership write on `G`
//!   cannot interleave: whichever runs second sees the first committed.
//!
//! **Lock order, everywhere: user locks, then group locks, then profile locks
//! ascending, then rows.** A grant or rule write takes no user lock, so it
//! enters at the second step. Group NAMES are read before they are locked (a
//! writer holding only an id must read the name to lock it), so each writer
//! re-reads after locking and locks again until the set is stable — a rename or
//! delete holding the old name's lock makes the reader wait, and the re-read
//! then sees the new name. Locks taken in that second pass are not globally
//! ordered with the first; Postgres detects the resulting deadlock (advisory
//! locks are in its wait graph) and aborts one transaction, which is the
//! fail-closed outcome and a retry away from success.
//!
//! **Key space.** Both use the TWO-`int4` form of `pg_advisory_xact_lock`,
//! which Postgres keeps disjoint from the single-`bigint` form every other
//! advisory lock in this codebase uses (`roles.rs`, `egret-machine-idp`,
//! `egret-workflow`, `egret-catalog-engine`, `view_tags.rs`, `egret-test-db`:
//! all `pg_advisory*_lock($bigint)` or `hashtextextended(…)`), with the first
//! key a namespace ([`MEMBERSHIP_USER_LOCK_SPACE`],
//! [`MEMBERSHIP_GROUP_LOCK_SPACE`]) and the second `hashtext` of the user id or
//! group name. A hash collision only makes two unrelated writers wait for each
//! other. Transaction-scoped, so a lock cannot outlive the write it guards.

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

/// First key of the per-USER membership lock (module doc, "Two concurrent
/// writers"). Two-`int4` key space; no other code uses it.
pub const MEMBERSHIP_USER_LOCK_SPACE: i32 = 0x4A46_3301;
/// First key of the per-GROUP membership lock, keyed by group NAME — the key
/// grants and rule exemptions store.
pub const MEMBERSHIP_GROUP_LOCK_SPACE: i32 = 0x4A46_3302;

/// Take the per-user membership lock for every id in `user_ids`, sorted, on
/// `conn`'s transaction. The FIRST lock any `user_groups` writer takes.
pub async fn lock_member_users_via(
    conn: &mut sqlx::PgConnection,
    user_ids: &[Uuid],
) -> std::result::Result<(), sqlx::Error> {
    let mut ids = user_ids.to_vec();
    ids.sort();
    ids.dedup();
    for id in ids {
        sqlx::query("SELECT pg_advisory_xact_lock($1, hashtext($2))")
            .bind(MEMBERSHIP_USER_LOCK_SPACE)
            .bind(id.to_string())
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// Take the per-group membership lock for every name in `names`, sorted, on
/// `conn`'s transaction. Taken by every `user_groups` writer for the groups it
/// changes, and by `egret_perm_compile`'s grant/rule writes for every group
/// they name — always before any profile lock (module doc).
pub async fn lock_group_names_via(
    conn: &mut sqlx::PgConnection,
    names: &[String],
) -> std::result::Result<(), sqlx::Error> {
    let mut names = names.to_vec();
    names.sort();
    names.dedup();
    for name in names {
        sqlx::query("SELECT pg_advisory_xact_lock($1, hashtext($2))")
            .bind(MEMBERSHIP_GROUP_LOCK_SPACE)
            .bind(name)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// The group locks one transaction holds, so a writer can lock what it read,
/// re-read, and lock again until the set is stable (module doc).
#[derive(Default)]
pub struct HeldGroupLocks(std::collections::BTreeSet<String>);

impl HeldGroupLocks {
    /// Lock every name in `names` not already held. `true` when anything new
    /// was locked — the caller's reads predate that lock and must be redone.
    pub async fn cover<'a>(
        &mut self,
        conn: &mut sqlx::PgConnection,
        names: impl IntoIterator<Item = &'a String>,
    ) -> Result<bool> {
        let new: Vec<String> = names
            .into_iter()
            .filter(|n| !self.0.contains(*n))
            .cloned()
            .collect();
        if new.is_empty() {
            return Ok(false);
        }
        lock_group_names_via(conn, &new).await.map_err(db_err)?;
        self.0.extend(new);
        Ok(true)
    }
}

/// How many lock-and-re-read passes a writer makes before giving up. Each pass
/// that locks something new is caused by a concurrent rename or membership
/// change landing between a read and its lock; two in a row is already rare.
pub(crate) const MAX_LOCK_PASSES: usize = 8;

pub(crate) fn unstable() -> egret_core::Error {
    egret_core::Error::Internal(anyhow::anyhow!(
        "the groups this membership write touches kept changing under it; retry"
    ))
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

    /// Which of `names` name a LOCAL (admin-managed) group, each paired with
    /// whether `user_id` is a member of it now — the input to the login's R2
    /// warning ([`crate::oidc::provision_and_sync`]). One read, no lock: it
    /// only feeds a log line, and [`Self::sync_user_oidc_groups`] ignores
    /// these names whatever it returns.
    pub async fn claimed_local_groups(
        &self,
        user_id: UserId,
        names: &[String],
    ) -> Result<Vec<(String, bool)>> {
        sqlx::query_as(
            "SELECT g.name, EXISTS (SELECT 1 FROM user_groups ug \
                 WHERE ug.group_id = g.id AND ug.user_id = $2) \
             FROM groups g WHERE g.name = ANY($1) AND g.source = 'local' \
             ORDER BY g.name",
        )
        .bind(names)
        .bind(user_id.0)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)
    }

    /// Replace the user's OIDC-sourced group memberships with `names` (in one tx).
    /// Manual (source='local') memberships are left untouched.
    ///
    /// **It manages memberships in OIDC-sourced groups ONLY (design R1,
    /// 2026-10-17).** A claimed name that names a LOCAL group is ignored: the
    /// user is not joined, nothing moves, nothing is recompiled. Before R1 the
    /// claimed set matched any group by name, so an IdP group called `admins`
    /// made its members members of the local group `admins` — and of every
    /// role and policy an admin had attached to it — although the API, the UI
    /// and the RBAC spec all say a local group's members are the admin's to
    /// manage. The claimed-set read filters on `source = 'oidc'`, which keeps
    /// a local name out of `moved` (no lock, no recompile) and out of the
    /// INSERT loop. The INSERT repeats the filter as a second guard on the
    /// row itself: while it iterates `claimed` it is redundant (a mutation
    /// dropping it alone survives every test), but if the loop is ever fed
    /// the raw claim again it still cannot write a local membership. A local
    /// membership the user already holds is never touched either way: the
    /// DELETE below is scoped to OIDC groups, and the local rows are carried
    /// into the "after" set unchanged.
    /// [`crate::oidc::provision_and_sync`] logs a warning for each claimed local
    /// name (R2), since a row such a claim added before R1 is still there.
    ///
    /// **Runs on every OIDC login, so it recompiles only what the login
    /// CHANGED.** The groups the user would hold after the write are computed
    /// before it (the local ones they keep, plus every claimed name that names
    /// an OIDC group — the INSERT below joins `groups` by name and source, so a
    /// claimed name with no OIDC row is not a membership), and only the
    /// symmetric difference with the groups they hold now is handed to
    /// `recompiler`. A login whose claim did
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
        // Fix round 1: no other writer of this user's rows may commit between
        // the reads below and this transaction's commit (module doc).
        lock_member_users_via(&mut tx, &[user_id.0])
            .await
            .map_err(db_err)?;

        let mut held = HeldGroupLocks::default();
        let mut passes = 0;
        let (moved, claimed) = loop {
            let before: Vec<(String, String)> = sqlx::query_as(
                "SELECT g.name, g.source FROM user_groups ug \
                 JOIN groups g ON g.id = ug.group_id WHERE ug.user_id = $1",
            )
            .bind(user_id.0)
            .fetch_all(&mut *tx)
            .await
            .map_err(db_err)?;
            let claimed: Vec<String> = sqlx::query_scalar(
                "SELECT name FROM groups WHERE name = ANY($1) AND source = 'oidc'",
            )
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
                .chain(claimed.iter().cloned())
                .collect();
            let moved: Vec<String> = before_set
                .symmetric_difference(&after_set)
                .cloned()
                .collect();
            if !held.cover(&mut tx, &moved).await? {
                break (moved, claimed);
            }
            passes += 1;
            if passes >= MAX_LOCK_PASSES {
                return Err(unstable());
            }
        };

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
        // Only the names the final pass resolved to an OIDC group (J-F3 final review
        // I2), never the raw claim: every one of them the user did not already
        // hold is in `moved`, so it is locked above and recompiled below. A
        // claimed name whose group was created after that read is not joined
        // now — nothing naming it was locked or recompiled, so a deny or an
        // allow's exclude on it would keep compiling to nobody while it had a
        // member. The next login, which sees the group, joins it properly.
        for name in &claimed {
            sqlx::query(
                "INSERT INTO user_groups (user_id, group_id) \
                 SELECT $1, id FROM groups WHERE name = $2 AND source = 'oidc' \
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
        let mut held = HeldGroupLocks::default();
        let mut passes = 0;
        let names = loop {
            let old: Option<String> =
                sqlx::query_scalar("SELECT name FROM groups WHERE id = $1 AND source = 'local'")
                    .bind(id)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(db_err)?;
            let Some(old) = old else {
                // Nothing to rename (oidc-sourced or absent): the UPDATE below
                // would match no row, so there is nothing to recompile.
                return Ok(0);
            };
            let names = vec![old, name.to_string()];
            if !held.cover(&mut tx, &names).await? {
                break names;
            }
            passes += 1;
            if passes >= MAX_LOCK_PASSES {
                return Err(unstable());
            }
        };
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
        let mut held = HeldGroupLocks::default();
        let mut passes = 0;
        let name = loop {
            let name: Option<String> =
                sqlx::query_scalar("SELECT name FROM groups WHERE id = $1 AND source = 'local'")
                    .bind(id)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(db_err)?;
            let Some(name) = name else {
                return Ok(0);
            };
            if !held.cover(&mut tx, std::slice::from_ref(&name)).await? {
                break name;
            }
            passes += 1;
            if passes >= MAX_LOCK_PASSES {
                return Err(unstable());
            }
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
    ///
    /// **Refused with `Conflict` on an OIDC-sourced group (design R4).** That
    /// group's membership is owned by the identity provider: an admin add
    /// would be undone at the next login of an OIDC user (the sync replaces
    /// their OIDC rows) and never undone for a local user, whom no login
    /// reaches — a member the IdP never named, holding every role and policy
    /// the group reaches. With R1 (the sync writes OIDC groups only) this
    /// makes a row's provenance follow from its group's source. The console
    /// already offers no add on such a group; this is the server half.
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
    ///
    /// Allowed on an OIDC-sourced group, unlike [`Self::add_member`] (design
    /// R4): a remove fails closed, so it serves as an emergency revoke until
    /// the user's next login re-applies the IdP's claim.
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
    /// them: user lock → group lock (re-read until the name is stable) →
    /// affected profiles locked → write → recompile (only if a row changed) →
    /// commit.
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
        lock_member_users_via(&mut tx, &[user_id.0])
            .await
            .map_err(db_err)?;
        let mut held = HeldGroupLocks::default();
        let mut passes = 0;
        let name: Option<String> = loop {
            let row: Option<(String, String)> =
                sqlx::query_as("SELECT name, source FROM groups WHERE id = $1")
                    .bind(group_id)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(db_err)?;
            // Design R4 (`add_member`'s doc): refused before any group or
            // profile lock and before the write, so nothing is recompiled. The
            // check sits on this read, not a separate one, so it judges the
            // same row the write would use; a group's source never changes
            // (`upsert_oidc_group` does not flip a local group, and rename and
            // delete refuse an OIDC one).
            if add && matches!(&row, Some((_, source)) if source == "oidc") {
                return Err(egret_core::Error::Conflict(
                    "Cannot add members to an OIDC-synced group; \
                     its membership is owned by the identity provider"
                        .to_string(),
                ));
            }
            let name = row.map(|(n, _)| n);
            match &name {
                Some(n) if held.cover(&mut tx, std::slice::from_ref(n)).await? => {
                    passes += 1;
                    if passes >= MAX_LOCK_PASSES {
                        return Err(unstable());
                    }
                }
                _ => break name,
            }
        };
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

/// A local group with members whose `users.provider = 'oidc'` — report (a)
/// of [`membership_provenance_report`].
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct LocalGroupWithOidcMembers {
    pub group_id: Uuid,
    pub group_name: String,
    /// How many of its members are OIDC-provider users.
    pub oidc_members: i64,
}

/// A non-OIDC user's (`provider` `'local'` or `'service'`) membership in an
/// OIDC-sourced group — report (b) of [`membership_provenance_report`].
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct NonOidcUserInOidcGroup {
    pub user_id: Uuid,
    /// `'local'` or `'service'`.
    pub provider: String,
    pub group_id: Uuid,
    pub group_name: String,
}

/// Both lists of [`membership_provenance_report`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MembershipProvenanceReport {
    pub local_groups_with_oidc_members: Vec<LocalGroupWithOidcMembers>,
    pub non_oidc_users_in_oidc_groups: Vec<NonOidcUserInOidcGroup>,
}

/// **The memberships whose provenance is unknown** (design R5), read in one
/// read-only snapshot. Nothing is changed: the rows are mostly legitimate,
/// `user_groups` records no writer, and an operator who reviews them already
/// sees each group's members in the console.
///
/// - **(a)** Local groups with ≥1 member whose `users.provider = 'oidc'`, with
///   that count. Before R1 the login sync joined a user to ANY group whose name
///   their IdP claim carried, local ones included, so such a row may be the
///   IdP's doing and not an admin's — and it survives R1, which only stops new
///   ones (the sync never touches a local row).
/// - **(b)** Each membership of a non-OIDC user (`provider <> 'oidc'`: a local
///   user or a service account) in an OIDC group: an admin add from before R4.
///   No login ever reaches such a user — a service account cannot log in at
///   all (migration 0119) — so nothing ever removes the row, and the user keeps
///   expanding as a member of every policy and role the group reaches. The
///   predicate is `<> 'oidc'`, not `= 'local'`, so a provider added later is
///   reported rather than silently missed (final review I1: `= 'local'` missed
///   service accounts).
///
/// Returns rows and logs nothing, so the rows are testable;
/// [`warn_unknown_membership_provenance`] is the startup wrapper.
pub async fn membership_provenance_report(
    pool: &sqlx::PgPool,
) -> Result<MembershipProvenanceReport> {
    let mut tx = pool.begin().await.map_err(db_err)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    let local_groups_with_oidc_members = sqlx::query_as(
        "SELECT g.id AS group_id, g.name AS group_name, count(*) AS oidc_members \
         FROM groups g JOIN user_groups ug ON ug.group_id = g.id \
         JOIN users u ON u.id = ug.user_id \
         WHERE g.source = 'local' AND u.provider = 'oidc' \
         GROUP BY g.id, g.name ORDER BY g.name",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(db_err)?;
    let non_oidc_users_in_oidc_groups = sqlx::query_as(
        "SELECT u.id AS user_id, u.provider, g.id AS group_id, g.name AS group_name \
         FROM groups g JOIN user_groups ug ON ug.group_id = g.id \
         JOIN users u ON u.id = ug.user_id \
         WHERE g.source = 'oidc' AND u.provider <> 'oidc' \
         ORDER BY g.name, u.id",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(db_err)?;
    tx.rollback().await.map_err(db_err)?;
    Ok(MembershipProvenanceReport {
        local_groups_with_oidc_members,
        non_oidc_users_in_oidc_groups,
    })
}

/// **The startup pass** (design R5): one `warn!` per row of
/// [`membership_provenance_report`]. Returns the rows it logged. Logs ids,
/// group names and counts — no email. A group name is logged with `?`
/// (Debug), not `%`: an OIDC group's name is IdP-supplied, and Debug escapes a
/// newline that Display would write as a forged second log line
/// (`tests/provenance_log_escapes.rs`).
///
/// Best-effort by design, like
/// `egret_perm_compile::principal_refs::warn_fail_open_principals`: the caller
/// spawns it off the boot path and logs an error instead of refusing to start,
/// because the rows are served either way.
pub async fn warn_unknown_membership_provenance(
    pool: &sqlx::PgPool,
) -> Result<MembershipProvenanceReport> {
    let report = membership_provenance_report(pool).await?;
    for r in &report.local_groups_with_oidc_members {
        tracing::warn!(
            group_id = %r.group_id,
            group = ?r.group_name,
            oidc_members = r.oidc_members,
            "a local group has members who sign in through OIDC; their provenance is unknown: \
             a login before the OIDC-groups fix may have added them from an IdP claim. \
             Review the group's members"
        );
    }
    for r in &report.non_oidc_users_in_oidc_groups {
        tracing::warn!(
            user_id = %r.user_id,
            group_id = %r.group_id,
            group = ?r.group_name,
            provider = %r.provider,
            "a non-OIDC user is a member of an OIDC-synced group; no login ever removes this \
             membership. Review it, and remove it if the identity provider did not intend it"
        );
    }
    Ok(report)
}

fn db_err(e: sqlx::Error) -> egret_core::Error {
    egret_core::Error::Internal(anyhow::anyhow!(e))
}
