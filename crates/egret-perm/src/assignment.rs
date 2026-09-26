//! The implicit workspace grant (SP4a Task 6): assigning a catalog to a
//! workspace grants that workspace `read`.
//!
//! **D7 branch taken**: `docs/en/plans/2026-09-24-permission-model-sp4a-d7-result.md`,
//! measured 2026-09-24 on the live kind cluster (helm revision 61), found —
//! by mechanism, not by a query experiment — that groups do NOT reach the
//! engine: Trino has no `group-provider.properties`, nothing in
//! `egret-provisioner` writes one, and egret runs no usersync, so
//! `getGroups(user)` is empty for every user on every engine egret
//! provisions today. `workspace-members` therefore CANNOT compile to a
//! Ranger group. Per D7 §3, this module stores `PrincipalKind::WorkspaceMembers`
//! on the implicit grant instead, and leaves EXPANSION into the member list
//! enumerated at compile time to the compiler — which today refuses a
//! `WorkspaceMembers` principal outright (`CompileError::UnexpandedPrincipalKind`,
//! `egret-perm-compile`'s `grants` module) rather than silently dropping it, because Task 4 found
//! that dropping is fail-closed on `include` but fail-OPEN on `exclude`. This
//! task does not weaken that refusal; a later task (recompile-on-membership-write,
//! same staleness shape as D5's StarRocks tag expansion) owns the expansion.
//! One consequence worth being explicit about: until that later task lands,
//! compiling ANY profile that reaches a catalog with an implicit grant will
//! return `Err(CompileError::UnexpandedPrincipalKind)` for that grant — see
//! `actions::CompileError`'s doc comment on that variant (this crate) and
//! the sibling crate `egret-perm-compile`'s `grants` module doc, which owns
//! the compiler itself (split out of this crate in this same fix round —
//! see that crate's own doc comment for why).

use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::error::PermError;
use crate::store::PermStore;

/// Replace the SET of implicit (`origin = 'workspace_assignment'`) grants a
/// catalog carries with exactly one per id in `workspace_ids`: an id absent
/// from a previous call's set gets its grant deleted, an id newly present
/// gets one inserted. This is set membership only, not a repair pass — an
/// existing implicit grant's `actions`/`effect` or its `grant_principals` row
/// is left untouched by a re-run even if it had drifted from what this
/// function would have written (`ON CONFLICT ... DO NOTHING`, not `DO
/// UPDATE`). Nothing else writes an `origin = 'workspace_assignment'` row or
/// its principal today, so that drift cannot happen in practice — but the
/// function does not defend against it, and a future writer of those rows
/// must not assume it does. Opens and commits its own transaction against
/// `store`'s pool — use
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

/// The transaction-composable core of [`sync_assignment_grants`]. Touches
/// only rows with `origin = 'workspace_assignment'` for `catalog_id` — never
/// an explicit grant, which is what
/// `unassigning_removes_the_implicit_grant_but_never_an_explicit_one`
/// (`tests/assignment.rs`) proves. Idempotent THROUGH `grants_one_per_assignment`
/// (the partial unique index on `(catalog_id, origin_workspace_id) WHERE
/// origin = 'workspace_assignment'`, `migrations/0154_perm_model.sql`): the
/// insert below names that index's exact conflict target rather than relying
/// on a plain `ON CONFLICT DO NOTHING`, so re-running with the same
/// workspace set is a no-op instead of racing the index.
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

    // Drop implicit grants for workspaces no longer in the set. An empty
    // `ids` makes `origin_workspace_id = ANY($2)` false for every row, so
    // this correctly removes ALL implicit grants for the catalog when
    // `workspace_ids` is empty.
    sqlx::query(
        "DELETE FROM perm.grants \
           WHERE catalog_id = $1 \
             AND origin = 'workspace_assignment' \
             AND NOT (origin_workspace_id = ANY($2))",
    )
    .bind(catalog_id)
    .bind(&ids)
    .execute(&mut **tx)
    .await?;

    // Upsert one implicit grant per workspace in the set. `ON CONFLICT`
    // names `grants_one_per_assignment`'s exact target (columns + predicate)
    // so a re-run is a no-op through the index, not around it.
    for workspace_id in &ids {
        let new_grant_id: Option<Uuid> = sqlx::query_scalar(
            "INSERT INTO perm.grants (effect, actions, catalog_id, origin, origin_workspace_id) \
             VALUES ('allow'::perm.effect, ARRAY['read'], $1, 'workspace_assignment', $2) \
             ON CONFLICT (catalog_id, origin_workspace_id) WHERE origin = 'workspace_assignment' \
             DO NOTHING \
             RETURNING id",
        )
        .bind(catalog_id)
        .bind(workspace_id)
        .fetch_optional(&mut **tx)
        .await?;

        // Only a newly inserted grant needs its principal row — an existing
        // one (the conflict-DO-NOTHING case) already has it.
        if let Some(grant_id) = new_grant_id {
            sqlx::query(
                "INSERT INTO perm.grant_principals (grant_id, kind, ref, mode) \
                 VALUES ($1, 'workspace_members'::perm.principal_kind, $2, \
                         'include'::perm.principal_mode)",
            )
            .bind(grant_id)
            .bind(workspace_id.to_string())
            .execute(&mut **tx)
            .await?;
        }
    }

    Ok(())
}
