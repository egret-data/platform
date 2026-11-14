//! The implicit workspace grant (SP4a Task 6): assigning a catalog to a
//! workspace grants that workspace `read`.
//!
//! **SP3c-3 Task 5 — the grant is now an explicit group grant.** The original
//! implementation (SP4a D7) stored a `PrincipalKind::WorkspaceMembers` principal
//! on an `origin='workspace_assignment'` grant, because D7 measured that a Ranger
//! GROUP reaches nobody on any engine egret provisions (no `group-provider`, no
//! usersync) and left the compiler to ENUMERATE the workspace's members at
//! compile time. SP3c-3 keeps that enumeration — but routes it through a managed
//! `ws:<slug> members` group (Tasks 1-2) that mirrors the workspace's members and
//! is expanded to those members' usernames exactly as `workspace_members` was.
//! So this path now AUTHORS the migrated shape directly: for each bound
//! workspace it upserts one explicit READ grant whose single principal is
//! `kind='group' ref=ws:<slug> members`. No `workspace_members` principal is
//! written going forward; Task 4's reversible migration backfills the grants
//! authored before SP3c-3.
//!
//! **Why `origin='workspace_assignment_group'` and not plain `explicit`
//! (migration 0227).** The grant must stay deduplicated and reversible: binding
//! the same catalog+workspace twice must not accumulate grants, and unbinding
//! must remove exactly the right one — never a human-authored `explicit` grant
//! on the same catalog. The old shape got this from `origin='workspace_assignment'`
//! plus `origin_workspace_id` plus the partial unique index `grants_one_per_assignment`.
//! A plain `explicit` grant has no such key (and the schema CHECK forbids an
//! `explicit` grant from carrying `origin_workspace_id` at all). So SP3c-3 gives
//! the new grant its OWN origin value, `workspace_assignment_group`, which keeps
//! `origin_workspace_id` set and is keyed by its own partial unique index
//! `grants_one_per_assignment_group`. Task 4's migrate rewrites the old grants
//! onto this SAME shape, so a migrated grant and a freshly-authored one are
//! identical rows.
//!
//! **The managed group itself is ensured on the recompile seam, not here.** The
//! `ws:<slug> members` `public.groups` row and its `user_groups` membership are
//! synced by `egret_perm_compile::ws_members_group::sync_ws_members_group_in_tx`,
//! which this (lower) crate cannot call — `egret-perm-compile` depends on
//! `egret-perm`, so the reverse would be a cycle. Both production callers of this
//! function (`egret-engine-mgmt`'s assignment handler and
//! `egret-catalog-registry::ensure_warehouse_catalog`) recompile the catalog's
//! profiles in the SAME transaction right after this returns, through
//! `CatalogAssignmentCompiler`/`ProfileCompilerPort` — and `egret-bin`'s shared
//! `recompile_catalog_assignment` ensures each bound workspace's managed group
//! before it recompiles, so the group the principal here names exists and is
//! populated by the time the compiler expands it.

use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::error::PermError;
use crate::store::PermStore;

/// The name of the managed group that mirrors a workspace's members:
/// `ws:<slug> members`. The spelling is LOAD-BEARING and shared across crates —
/// the authoring path here writes a `grant_principals.ref` with it, and
/// `egret_perm_compile::ws_members_group` syncs a `public.groups` row under the
/// exact same name and validates principal refs against it. It lives in this
/// (lower) crate so both the authoring path and the compiler derive the name
/// from ONE definition rather than two that could drift
/// (`egret_perm_compile::ws_members_group::ws_members_group_name` re-exports it).
pub fn ws_members_group_name(slug: &str) -> String {
    format!("ws:{slug} members")
}

/// Replace the SET of implicit assignment grants a catalog carries with exactly
/// one per id in `workspace_ids`: an id absent from a previous call's set gets
/// its grant deleted, an id newly present gets one inserted. Each inserted grant
/// is the SP3c-3 group shape (`origin = 'workspace_assignment_group'`, principal
/// `kind='group' ref=ws:<slug> members`). This is set membership only, not a
/// repair pass — an existing grant's `actions`/`effect` or its `grant_principals`
/// row is left untouched by a re-run even if it had drifted from what this
/// function would have written (`ON CONFLICT ... DO NOTHING`, not `DO UPDATE`).
/// Nothing else writes a `workspace_assignment_group` row or its principal today,
/// so that drift cannot happen in practice — but the function does not defend
/// against it, and a future writer of those rows must not assume it does. Opens
/// and commits its own transaction against `store`'s pool — use
/// [`sync_assignment_grants_in_tx`] instead when this write must be atomic
/// with another write already inside a transaction (the catalog-workspace
/// assignment write in `egret-engine-mgmt::catalogs::set_catalog_workspaces`
/// is exactly that case: an assignment that commits while its grant sync
/// fails is the split-brain D3 exists to remove).
pub async fn sync_assignment_grants(
    store: &PermStore,
    catalog_id: Uuid,
    workspace_ids: &[Uuid],
) -> Result<(), PermError> {
    let pool: &PgPool = store.pool();
    let mut tx = pool.begin().await?;
    sync_assignment_grants_in_tx(&mut tx, catalog_id, workspace_ids).await?;
    tx.commit().await?;
    Ok(())
}

/// The transaction-composable core of [`sync_assignment_grants`]. Touches only
/// assignment grants for `catalog_id` — never a human-authored `explicit` grant,
/// which is what `unassigning_removes_the_implicit_grant_but_never_an_explicit_one`
/// (`tests/assignment.rs`) proves. Idempotent THROUGH
/// `grants_one_per_assignment_group` (the partial unique index on
/// `(catalog_id, origin_workspace_id) WHERE origin = 'workspace_assignment_group'`,
/// `migrations/0227_ws_assignment_group_origin.sql`): the insert below names that
/// index's exact conflict target rather than relying on a plain `ON CONFLICT DO
/// NOTHING`, so re-running with the same workspace set is a no-op instead of
/// racing the index.
///
/// An assignment grants `read`, never `write` — the brief's own words: this
/// is what makes a catalog assignment visible to an engine, not a blanket
/// write grant nobody asked for.
pub async fn sync_assignment_grants_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    catalog_id: Uuid,
    workspace_ids: &[Uuid],
) -> Result<(), PermError> {
    let ids: Vec<Uuid> = workspace_ids.to_vec();

    // Drop assignment grants for workspaces no longer in the set. Both origins
    // are swept: the SP3c-3 `workspace_assignment_group` shape AND the legacy
    // `workspace_assignment` shape, so unbinding a catalog whose grant has not
    // yet been migrated (Task 4) still removes it rather than leaving it dangling.
    // An empty `ids` makes `origin_workspace_id = ANY($2)` false for every row,
    // so this correctly removes ALL assignment grants for the catalog when
    // `workspace_ids` is empty.
    sqlx::query(
        "DELETE FROM perm.grants \
           WHERE catalog_id = $1 \
             AND origin IN ('workspace_assignment', 'workspace_assignment_group') \
             AND NOT (origin_workspace_id = ANY($2))",
    )
    .bind(catalog_id)
    .bind(&ids)
    .execute(&mut **tx)
    .await?;

    // Upsert one group grant per workspace in the set. `ON CONFLICT` names
    // `grants_one_per_assignment_group`'s exact target (columns + predicate) so a
    // re-run is a no-op through the index, not around it — and so it never
    // conflicts with a human `explicit` grant, which lives under a different
    // origin and is outside this partial index entirely.
    for workspace_id in &ids {
        let new_grant_id: Option<Uuid> = sqlx::query_scalar(
            "INSERT INTO perm.grants (effect, actions, catalog_id, origin, origin_workspace_id) \
             VALUES ('allow'::perm.effect, ARRAY['read'], $1, 'workspace_assignment_group', $2) \
             ON CONFLICT (catalog_id, origin_workspace_id) \
                 WHERE origin = 'workspace_assignment_group' \
             DO NOTHING \
             RETURNING id",
        )
        .bind(catalog_id)
        .bind(workspace_id)
        .fetch_optional(&mut **tx)
        .await?;

        // Only a newly inserted grant needs its principal row — an existing one
        // (the conflict-DO-NOTHING case) already has it. The principal is the
        // managed `ws:<slug> members` group; the slug is read from the same
        // `public.workspaces` row the compiler's group sync resolves it from, and
        // both derive the name through [`ws_members_group_name`].
        if let Some(grant_id) = new_grant_id {
            let slug: String =
                sqlx::query_scalar("SELECT slug FROM public.workspaces WHERE id = $1")
                    .bind(workspace_id)
                    .fetch_one(&mut **tx)
                    .await?;
            sqlx::query(
                "INSERT INTO perm.grant_principals (grant_id, kind, ref, mode) \
                 VALUES ($1, 'group'::perm.principal_kind, $2, 'include'::perm.principal_mode)",
            )
            .bind(grant_id)
            .bind(ws_members_group_name(&slug))
            .execute(&mut **tx)
            .await?;
        }
    }

    Ok(())
}
