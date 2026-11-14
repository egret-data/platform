//! SP3c-3: the managed `ws:<slug> members` group that mirrors a workspace's members.

use crate::members;
use egret_perm::CompileError;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

/// The managed group that mirrors a workspace's members. The naming is
/// load-bearing: principal-ref validation, the migration AND the authoring path
/// rely on it, so the ONE definition lives in `egret-perm` (the lower crate the
/// authoring path is in) and is re-exported here for this crate's callers.
pub use egret_perm::ws_members_group_name;

async fn workspace_slug(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<String, CompileError> {
    sqlx::query_scalar("SELECT slug FROM public.workspaces WHERE id = $1")
        .bind(workspace_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(CompileError::from_db)
}

/// Upsert the `ws:<slug> members` managed group and reconcile its `user_groups`
/// membership to exactly the workspace's member user ids. Returns the group id.
/// Idempotent; safe to call on every membership change.
pub async fn sync_ws_members_group_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<Uuid, CompileError> {
    let slug = workspace_slug(tx, workspace_id).await?;
    let name = ws_members_group_name(&slug);
    // Create the managed group, or adopt one that is ALREADY managed. A group of
    // this exact name with a `local`/`oidc` source is NOT adopted: the
    // `WHERE groups.source = 'managed'` guard makes the conflicting UPDATE affect
    // no row, so `RETURNING` yields nothing and we refuse the collision.
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

    let want: std::collections::BTreeSet<Uuid> =
        members::workspace_members_via(tx, &[workspace_id])
            .await?
            .into_iter()
            .map(|m| m.user_id)
            .collect();
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

/// Re-sync the managed `ws:<slug> members` group IFF it already exists, and
/// report whether it did. The existence guard keeps a workspace-member write made
/// BEFORE the migration from minting a managed group (pre-migration there is
/// nothing to sync). Mirrors `role_holder_group::sync_role_holder_group_if_present_in_tx`.
pub async fn sync_ws_members_group_if_present_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<bool, CompileError> {
    let slug = workspace_slug(tx, workspace_id).await?;
    let name = ws_members_group_name(&slug);
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.groups WHERE name = $1 AND source = 'managed')",
    )
    .bind(&name)
    .fetch_one(&mut **tx)
    .await
    .map_err(CompileError::from_db)?;
    if exists {
        sync_ws_members_group_in_tx(tx, workspace_id).await?;
    }
    Ok(exists)
}

/// Re-sync EVERY existing managed `ws:<slug> members` group. For writers that
/// change workspace membership indirectly and do not know which workspaces they
/// touched — a user joining or leaving a group that holds a workspace, a group or
/// user delete. Cheap: only workspaces already migrated have such a group, and
/// each sync is idempotent. Pre-migration this finds nothing.
pub async fn sync_all_existing_ws_members_groups_in_tx(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<(), CompileError> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT w.id FROM public.workspaces w \
           JOIN public.groups g ON g.name = 'ws:' || w.slug || ' members' AND g.source = 'managed' \
          ORDER BY w.id",
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(CompileError::from_db)?;
    for id in ids {
        sync_ws_members_group_in_tx(tx, id).await?;
    }
    Ok(())
}
