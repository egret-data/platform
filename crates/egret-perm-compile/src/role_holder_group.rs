//! SP3c-1: the managed `role:<name> holders` group that mirrors a role's holders.

use crate::members;
use egret_perm::CompileError;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

/// The managed group that mirrors a role's holders. The naming is load-bearing:
/// principal-ref validation and the migration rely on it.
pub fn role_holder_group_name(role: &str) -> String {
    format!("role:{role} holders")
}

/// Upsert the `role:<name> holders` managed group and reconcile its
/// `user_groups` membership to exactly the role's holder user ids (direct user
/// holders plus members of groups holding the role). Returns the group id.
/// Idempotent; safe to call on every holder change.
pub async fn sync_role_holder_group_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    role: &str,
) -> Result<Uuid, CompileError> {
    let name = role_holder_group_name(role);
    // Create the managed group, or adopt one that is ALREADY managed. A
    // pre-existing group of this exact name with a `local`/`oidc` source is NOT
    // adopted: the `WHERE groups.source = 'managed'` guard makes the conflicting
    // UPDATE affect no row, so `RETURNING` yields nothing and we refuse the
    // collision (SP3c-1 review Fix 2). Silently flipping such a group to managed
    // and reconciling its members to the role's holders would be an access change
    // outside the migration's equivalence gate.
    let group_id: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO public.groups (id, name, source) VALUES (gen_random_uuid(), $1, 'managed') \
         ON CONFLICT (name) DO UPDATE SET source = 'managed' WHERE groups.source = 'managed' \
         RETURNING id",
    )
    .bind(&name)
    .fetch_optional(&mut **tx)
    .await
    .map_err(CompileError::from_db)?;
    let group_id =
        group_id.ok_or_else(|| CompileError::HolderGroupNameCollision { name: name.clone() })?;

    let roles = [role.to_string()];
    let mut want: std::collections::BTreeSet<Uuid> = std::collections::BTreeSet::new();
    for m in members::role_holders_via(tx, &roles).await? {
        want.insert(m.user_id);
    }
    for m in members::role_group_holders_via(tx, &roles).await? {
        want.insert(m.user_id);
    }
    let want: Vec<Uuid> = want.into_iter().collect();

    sqlx::query("DELETE FROM public.user_groups WHERE group_id = $1 AND user_id <> ALL($2)")
        .bind(group_id)
        .bind(&want)
        .execute(&mut **tx)
        .await
        .map_err(CompileError::from_db)?;
    sqlx::query(
        "INSERT INTO public.user_groups (user_id, group_id) \
         SELECT u, $1 FROM UNNEST($2::uuid[]) AS u ON CONFLICT (user_id, group_id) DO NOTHING",
    )
    .bind(group_id)
    .bind(&want)
    .execute(&mut **tx)
    .await
    .map_err(CompileError::from_db)?;
    Ok(group_id)
}

/// Re-sync the managed `role:<name> holders` group IFF it already exists, and
/// report whether it did (SP3c-1 review Fix 1). Every LIVE role-holder-change
/// path calls this in its own transaction BEFORE its recompile, so a group-ref
/// grant re-expands from a fresh `user_groups` instead of a stale one (a revoke
/// would otherwise fail open until the next boot repair).
///
/// The existence guard is what keeps a role write made BEFORE the migration has
/// run from minting a managed group: pre-migration the grants still carry
/// `kind='role'` and expand straight from `access_grants`, so there is nothing to
/// sync and nothing should be created. This mirrors the boot repair
/// (`crate::role_membership::recompile_role_referencing_profiles`), which only
/// touches already-existing managed groups.
pub async fn sync_role_holder_group_if_present_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    role: &str,
) -> Result<bool, CompileError> {
    let name = role_holder_group_name(role);
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.groups WHERE name = $1 AND source = 'managed')",
    )
    .bind(&name)
    .fetch_one(&mut **tx)
    .await
    .map_err(CompileError::from_db)?;
    if exists {
        sync_role_holder_group_in_tx(tx, role).await?;
    }
    Ok(exists)
}
