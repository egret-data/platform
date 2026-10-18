//! **SP4c Task 3, input 3 of three: who holds the clearing role.** One
//! transactional write-plus-recompile for a platform role grant, so that a
//! membership change and the engine policies it determines commit together or
//! not at all.
//!
//! # Why a role-membership write needs a recompile at all
//!
//! A role NAME is unresolvable in every engine egret provisions. D0 measured it
//! on Trino — the plugin polls a `/service/roles/download/<svc>` route egret does
//! not serve, so `lastKnownRoleVersion` never leaves `-1`; Trino's plugin passes
//! `userRoles` as a hardcoded `null`; `disableRoleResolution` defaults `true`.
//! D0b re-derived it for StarRocks, where `setUserRoles` is never called
//! anywhere in `fe/` and the on-disk roles cache is an empty document. So egret
//! enumerates a role's HOLDERS at compile time, and that enumeration is stale
//! the instant someone is granted or revoked the role.
//!
//! The staleness is not symmetric, which is why this is not a nicety. A missed
//! GRANT denies someone who should be cleared (annoying). A missed REVOKE leaves
//! them named in a clearance's `denyExceptions` — still able to read the
//! classified column after their clearance was taken away — and, for a `role`
//! grant, still holding the access the model no longer gives them. That is a
//! breach, and it is invisible: egret's own UI reads the `access_grants` row,
//! which was always written correctly.
//!
//! # ONE trigger, TWO consumers
//!
//! The enumeration happens in two places — a clearance's `denyExceptions`
//! ([`crate::clearance`]) and a `role` principal in a grant
//! ([`crate::members`]) — and both read the same `role_usernames`, which mints
//! a holder's missing username (2026-10-18 design R5), so the recompile a
//! grant triggers names a holder who never used an engine. So there is
//! ONE affected-service function,
//! [`crate::affected_services_for_role_membership`], which returns the UNION of
//! both consumers' arms. Two triggers over one enumeration is two chances for
//! one of them to be forgotten, which is the shape of the defect SP4a shipped
//! four separate times.
//!
//! # The write lives in `egret-auth` and the recompile cannot
//!
//! `egret-perm-compile` depends on `egret-auth` (for `users.username`), so
//! `egret_auth::roles::RoleRepo` cannot call back into this crate. The SQL
//! therefore stays there as `assign_in_tx`/`unassign_in_tx` and the transaction
//! is owned here, which also keeps the lock-ordering invariant in one place
//! instead of at every HTTP handler.

use sqlx::PgPool;
use uuid::Uuid;

use egret_auth::roles::RoleRepo;
use egret_perm::CompileError;

use crate::trigger::{
    affected_services_for_role_membership, lock_services_in_tx, recompile_services_in_tx,
};

/// Which direction the membership write goes. An enum rather than a closure so
/// the ORDER in [`with_recompile`] is written once and neither direction can
/// drift from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Assign,
    Unassign,
}

/// Grant platform role `role_id` to principal `(ptype, pid)` and recompile every
/// service the change can reach, in ONE transaction.
///
/// Idempotent, exactly as `RoleRepo::assign` is: a repeated grant writes no row,
/// and the recompile that follows produces a byte-identical policy set which
/// `apply_diff_in_tx` records as `unchanged`.
pub async fn assign_platform_role_and_recompile(
    pool: &PgPool,
    ptype: &str,
    pid: Uuid,
    role_id: Uuid,
) -> Result<(), CompileError> {
    with_recompile(pool, Op::Assign, ptype, pid, role_id).await
}

/// Revoke platform role `role_id` from principal `(ptype, pid)` and recompile —
/// the direction that fails OPEN if it is missed, since the revoked holder stays
/// named in every clearance exception and every role-granted policy.
pub async fn unassign_platform_role_and_recompile(
    pool: &PgPool,
    ptype: &str,
    pid: Uuid,
    role_id: Uuid,
) -> Result<(), CompileError> {
    with_recompile(pool, Op::Unassign, ptype, pid, role_id).await
}

/// Delete platform role `role_id` and recompile every service the deletion can
/// reach, in ONE transaction.
///
/// **This is a role-membership write wearing a different name, and missing that
/// re-opened the defect SP4c exists to close.** `access_grants.role_id` is
/// `ON DELETE CASCADE` (`migrations/0068_access_grants.sql`), so deleting a
/// role revokes it from every holder in one statement. An admin deleting a
/// CLEARING role to revoke a clearance therefore got: every grant gone, Cedar
/// closing object access — and the compiled `denyExceptions` still naming the
/// ex-holders, who went on reading the classified column with nothing in the
/// system converging it. The eventual state was fail-closed; the interim was
/// unbounded, and "fails closed" is a claim about the interim or it is not
/// worth making.
///
/// The deletion takes the SAME three calls as a grant or a revoke, in the same
/// order and for the same reasons ([`with_recompile`]) — affected set, locks
/// ascending, write, recompile — with one difference stated here rather than
/// left to be rediscovered: the affected set is computed from the role's NAME
/// **before** the row is gone. Both consumers key on the name
/// (`perm.grant_principals`, `classification_policies.required_roles`) and
/// neither has an FK to `roles`, so the name survives the delete in the model
/// while the row does not — and after the recompile the clearance's exception
/// list is empty, which compiles to a deny withholding the column from
/// everybody. That is the correct reading of "nobody holds the clearing role".
///
/// A role id that does not exist is a no-op, exactly as [`with_recompile`]
/// treats an unresolvable name: the DELETE matches nothing and a full compile
/// for a role nothing references would change nothing.
pub async fn delete_platform_role_and_recompile(
    pool: &PgPool,
    role_id: Uuid,
) -> Result<(), CompileError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| CompileError::Db(e.to_string()))?;

    let name = RoleRepo::name_of_in_tx(&mut tx, role_id)
        .await
        .map_err(|e| CompileError::Db(e.to_string()))?;

    if let Some(name) = name.as_deref() {
        let services = affected_services_for_role_membership(&mut tx, name).await?;
        lock_services_in_tx(&mut tx, &services).await?;
        RoleRepo::delete_in_tx(&mut tx, role_id)
            .await
            .map_err(|e| CompileError::Db(e.to_string()))?;
        for summary in recompile_services_in_tx(&mut tx, &services).await? {
            if !summary.skipped.is_empty() {
                tracing::warn!(
                    role = %name,
                    skipped_count = summary.skipped.len(),
                    "a role deletion's recompile skipped some items"
                );
            }
        }
    } else {
        RoleRepo::delete_in_tx(&mut tx, role_id)
            .await
            .map_err(|e| CompileError::Db(e.to_string()))?;
    }

    tx.commit()
        .await
        .map_err(|e| CompileError::Db(e.to_string()))?;
    Ok(())
}

/// The transaction shape both directions share, so the ORDER cannot drift
/// between them.
///
/// 1. Resolve the role's NAME — that is the key `perm.grant_principals` and
///    `classification_policies.required_roles` both store, never the id.
/// 2. Compute the affected set from the role name.
/// 3. **Take every profile lock ASCENDING, before touching any `access_grants`
///    row.** `lock_services_in_tx` documents why: `perm.compiled`'s cascading FKs
///    mean a recompile takes share locks on model rows a concurrent delete wants
///    exclusively, and acquiring the profile locks first is what makes the global
///    order consistent and turns a deadlock into a queue.
/// 4. Do the membership write.
/// 5. Recompile, propagating any failure so the `?` rolls the membership write
///    back with it — a committed grant whose policies were never rewritten is
///    worse than a rejected one, because egret's UI shows it as live.
async fn with_recompile(
    pool: &PgPool,
    op: Op,
    ptype: &str,
    pid: Uuid,
    role_id: Uuid,
) -> Result<(), CompileError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| CompileError::Db(e.to_string()))?;

    let name = RoleRepo::name_of_in_tx(&mut tx, role_id)
        .await
        .map_err(|e| CompileError::Db(e.to_string()))?;

    // A role name that resolves is looked up BEFORE the write, and the locks are
    // taken before it too; a role that does not exist skips both, because the
    // write itself already answers that case (the INSERT's FK refuses it, the
    // DELETE is a no-op) and a full compile for a nameless role changes nothing.
    if let Some(name) = name.as_deref() {
        let services = affected_services_for_role_membership(&mut tx, name).await?;
        lock_services_in_tx(&mut tx, &services).await?;
        apply(&mut tx, op, ptype, pid, role_id).await?;
        for summary in recompile_services_in_tx(&mut tx, &services).await? {
            if !summary.skipped.is_empty() {
                tracing::warn!(
                    role = %name,
                    skipped_count = summary.skipped.len(),
                    "a role-membership write's recompile skipped some items"
                );
            }
        }
    } else {
        apply(&mut tx, op, ptype, pid, role_id).await?;
    }

    tx.commit()
        .await
        .map_err(|e| CompileError::Db(e.to_string()))?;
    Ok(())
}

async fn apply(
    conn: &mut sqlx::PgConnection,
    op: Op,
    ptype: &str,
    pid: Uuid,
    role_id: Uuid,
) -> Result<(), CompileError> {
    match op {
        Op::Assign => RoleRepo::assign_in_tx(conn, ptype, pid, role_id).await,
        Op::Unassign => RoleRepo::unassign_in_tx(conn, ptype, pid, role_id).await,
    }
    .map_err(|e| CompileError::Db(e.to_string()))
}

/// Recompile every profile whose model names a role — a `role` principal in
/// `perm.grant_principals`, or any `classification_policies.required_roles` —
/// and return the ids recompiled, ascending (2026-10-18 design R7).
///
/// **Why a boot pass.** Until R5 a role holder with no `users.username` was
/// left out of the compiled policy, and nothing recompiles when they get one:
/// the first query mints it and recompiles nothing. A body stored by the old
/// compiler can therefore still leave out a holder who now has a username,
/// so a deny or exclude on the role misses them and a clearance refuses them.
/// Running the normal recompile over those profiles makes the compiler mint
/// and name them (R5).
///
/// One transaction, through the same calls a role-membership write takes:
/// the union of [`affected_services_for_role_membership`] over every named
/// role, its locks taken ascending ([`lock_services_in_tx`]), then
/// [`recompile_services_in_tx`]. Idempotent: an unchanged body writes nothing
/// and bumps no `policy_version`, which is why it runs on every boot with no
/// record of having run. `egret-bin` spawns it off the boot path, and a
/// failure is logged and never blocks boot.
pub async fn recompile_role_referencing_profiles(pool: &PgPool) -> Result<Vec<i64>, CompileError> {
    let db = |e: sqlx::Error| CompileError::Db(e.to_string());
    let mut tx = pool.begin().await.map_err(db)?;

    let roles: Vec<String> = sqlx::query_scalar(
        "SELECT ref FROM perm.grant_principals WHERE kind = 'role'::perm.principal_kind \
         UNION \
         SELECT unnest(required_roles) FROM classification_policies \
         ORDER BY 1",
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(db)?;

    let mut services: Vec<i64> = Vec::new();
    for role in &roles {
        services.extend(affected_services_for_role_membership(&mut tx, role).await?);
    }
    services.sort_unstable();
    services.dedup();
    if services.is_empty() {
        return Ok(services);
    }

    lock_services_in_tx(&mut tx, &services).await?;
    let summaries = recompile_services_in_tx(&mut tx, &services).await?;
    tx.commit().await.map_err(db)?;

    let rewritten = summaries.iter().filter(|s| s.wrote_anything()).count();
    tracing::info!(
        roles = roles.len(),
        profiles = services.len(),
        rewritten,
        "recompiled the profiles whose model names a role, so a holder an older compile left out \
         is named"
    );
    Ok(services)
}
