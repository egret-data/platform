//! SP3c-3 Task 4: the reversible, equivalence-gated rewrite of every implicit
//! workspace-assignment grant (`perm.grants.origin='workspace_assignment'`,
//! principal `kind='workspace_members'`) into an explicit grant whose principal
//! is the managed `ws:<slug> members` group (Task 1).
//!
//! # Why the rewrite is byte-identical, and why that IS the equivalence gate
//!
//! A `workspace_members` principal and a non-`public` `group` principal are
//! expanded the same way on every recompile, BEFORE the lowering runs:
//! [`crate::members::expand_enumerated_principals`] resolves a workspace into its
//! members' stored usernames and a group into its members' stored usernames,
//! each sorted and deduplicated. Task 1's `sync_ws_members_group_in_tx` makes the
//! managed group's membership EXACTLY the workspace's members, so both expansions
//! produce the identical username list. The lowering then names only those
//! usernames in the Ranger body — the provenance of HOW the list was reached is
//! internal IR and is never rendered into `policy.authz_policies.policy_jsonb` —
//! so the compiled artifact the engine downloads is byte-for-byte the same after
//! the rewrite as before it.
//!
//! The migration proves this, rather than assuming it: for every affected
//! profile it recompiles and fingerprints the compiled bodies BEFORE the rewrite,
//! recompiles and fingerprints them AFTER, and refuses (returns `Err`, so the
//! caller's transaction rolls back) on any difference. This byte-identical
//! fingerprint comparison is the equivalence gate at its strongest granularity:
//! it is the literal artifact the engine enforces. As an independent, redundant
//! check the migration ALSO builds the probes [`crate::equivalence::derive_probes`]
//! from the before/after downloads and runs [`crate::equivalence::compare`] over
//! every affected profile, refusing on any verdict divergence (a profile with no
//! bound engine or no probe is already covered by the fingerprint gate, so its
//! `GateRefusal` is not itself a failure).
//!
//! # What the rewrite touches, and the schema invariant it must keep
//!
//! Per implicit grant it UPDATEs exactly two rows:
//!
//! - its `perm.grant_principals` row: `kind='workspace_members'`,
//!   `ref=<workspace id>` → `kind='group'`, `ref=ws:<slug> members`, `mode`
//!   unchanged (`include`);
//! - its `perm.grants` row: `origin='workspace_assignment'` →
//!   `origin='workspace_assignment_group'`, `origin_workspace_id` UNCHANGED.
//!
//! The rewrite CONVERGES on the exact row the authoring path
//! (`egret_perm::sync_assignment_grants_in_tx`) now writes for a fresh bind
//! (SP3c-3 Task 5): `origin='workspace_assignment_group'` keeping the workspace
//! id, principal `kind='group' ref=ws:<slug> members`. So a migrated grant and a
//! freshly-authored one are byte-identical rows — the authoring path's dedup, its
//! unbind DELETE and its idempotent re-bind all treat them the same. Keeping
//! `origin_workspace_id` is valid under the widened schema CHECK (migration 0227:
//! `(origin IN ('workspace_assignment','workspace_assignment_group')) =
//! (origin_workspace_id IS NOT NULL)`), and it is what keys the grant in the
//! partial unique index `grants_one_per_assignment_group`. The undo does not need
//! `origin_workspace_id` to FIND the grant — provenance records the grant id —
//! and restores `origin='workspace_assignment'` in one UPDATE.
//!
//! **Why this also resolves the undo/re-bind race the Task-4 review flagged.**
//! Before Task 5, a re-bind re-created a fresh `origin='workspace_assignment'`
//! grant, which the undo's restore (also to `workspace_assignment`) could collide
//! with on `grants_one_per_assignment`. Now the authoring path never writes
//! `workspace_assignment` — it writes `workspace_assignment_group` — so the undo's
//! restore target is one that nothing else recreates.
//!
//! # Reversible
//!
//! Each run records, per migrated grant, its pre-migration identity (grant id,
//! origin, origin_workspace_id, principal kind/ref) in `perm.ws_assignment_migration`
//! (migration 0226), alongside a canonical hash of its POST-rewrite identity.
//! [`undo_ws_assignments_in_tx`] takes `FOR UPDATE` on that provenance row,
//! re-hashes each rewritten grant as it stands now, refuses `409` if any diverges
//! (a grant edited since the migration is no longer the migration's to restore),
//! and otherwise restores each grant's `workspace_members` principal and its
//! `workspace_assignment` origin, recompiles, and deletes the provenance row.

use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use egret_ranger_eval::eval::{EngineConfig, PluginVersion};
use egret_ranger_eval::servicedef::ServiceDef;

use egret_policy::{
    catalogs_for_service_via, get_service_by_id_via, tag_assignments_for_catalogs_via,
};

use crate::equivalence::{compare, derive_probes, GateRefusal, ProfileCase, TagAssignment};
use crate::migrate::canonical_json;
use crate::routes::{bound_engines, compiled_artifact_via, PermHttpError};
use crate::trigger::{
    affected_services_for_workspace_members, merge_service_ids, recompile_service_in_tx,
};
use crate::ws_members_group::{sync_ws_members_group_in_tx, ws_members_group_name};

/// What one migration run did: its provenance id, the catalogs it touched, and
/// how many implicit grants were rewritten.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct WsAssignmentMigration {
    pub id: i64,
    pub catalogs: Vec<Uuid>,
    pub rewritten: usize,
}

/// One rewritten implicit grant, recorded for a hash-guarded undo: its
/// pre-migration identity plus the managed group it now names.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Original {
    grant_id: Uuid,
    catalog_id: Uuid,
    /// Always `"workspace_assignment"`; recorded so the undo restores the exact
    /// origin.
    origin: String,
    origin_workspace_id: Uuid,
    /// Always `"workspace_members"`; recorded so the undo restores the exact kind.
    principal_kind: String,
    /// The pre-migration principal ref — the workspace id as a string.
    principal_ref: String,
    /// The workspace slug the group name derives from (diagnostic).
    slug: String,
    /// The POST-rewrite principal ref: `ws:<slug> members`.
    group_name: String,
}

fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// The canonical hash of a rewritten grant's POST-rewrite identity: the grant
/// the undo expects to find and restore. Built identically from the known
/// post-rewrite values (in [`migrate_ws_assignments_in_tx`]) and from the row as
/// it currently stands (in the undo), so a grant whose origin or managed-group
/// principal has changed hashes differently and the undo refuses.
fn row_hash(
    grant_id: Uuid,
    origin: &str,
    origin_workspace_id: Option<Uuid>,
    principal_ref: &str,
    principal_mode: &str,
) -> String {
    sha256_hex(&canonical_json(&serde_json::json!({
        "grant_id": grant_id.to_string(),
        "origin": origin,
        "origin_workspace_id": origin_workspace_id.map(|w| w.to_string()),
        "kind": "group",
        "ref": principal_ref,
        "mode": principal_mode,
    })))
}

fn describe(o: &Original) -> String {
    format!(
        "grant {} (catalog {}, workspace {})",
        o.grant_id, o.catalog_id, o.origin_workspace_id
    )
}

/// The compiled bodies of a profile, hashed: `name`+`policy_jsonb` of every
/// `policy.authz_policies` row, ordered by name, keys canonicalised. The
/// envelope's `policy_version` is deliberately excluded — it bumps only when a
/// body changes, and the gate is about the bodies the engine enforces.
async fn compiled_fingerprint(
    tx: &mut Transaction<'_, Postgres>,
    service_id: i64,
) -> Result<String, PermHttpError> {
    let rows: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT name, policy_jsonb FROM policy.authz_policies WHERE service_id = $1 ORDER BY name",
    )
    .bind(service_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(PermHttpError::db)?;
    let v = serde_json::Value::Array(
        rows.into_iter()
            .map(|(name, body)| serde_json::json!({ "name": name, "body": body }))
            .collect(),
    );
    Ok(sha256_hex(&canonical_json(&v)))
}

/// The plugin versions of a profile's bound engines, ascending and distinct.
/// Empty when none is bound or one is of an unmodelled (type, version) pair —
/// the probe gate then skips this profile (the fingerprint gate already covers
/// it), rather than guessing a version to evaluate at.
async fn probe_versions(
    tx: &mut Transaction<'_, Postgres>,
    service_id: i64,
) -> Result<Vec<PluginVersion>, PermHttpError> {
    let mut versions = Vec::new();
    for e in bound_engines(tx, service_id).await? {
        if let Ok(v) = e.plugin_version() {
            if !versions.contains(&v) {
                versions.push(v);
            }
        }
    }
    Ok(versions)
}

/// The tag assignments on a profile's catalogs, as [`TagAssignment`]s — the real
/// tagging the probe gate resolves each probe's tags from.
async fn profile_tag_assignments(
    tx: &mut Transaction<'_, Postgres>,
    service_id: i64,
) -> Result<Vec<TagAssignment>, PermHttpError> {
    let catalog_ids: Vec<Uuid> = catalogs_for_service_via(tx, service_id)
        .await?
        .into_iter()
        .map(|c| c.id)
        .collect();
    let rows = tag_assignments_for_catalogs_via(tx, &catalog_ids).await?;
    Ok(rows
        .into_iter()
        .map(|r| TagAssignment {
            catalog: r.catalog,
            schema: r.schema_name,
            table: r.table_name,
            column: r.column_name,
            tag: r.tag_name,
        })
        .collect())
}

/// Every profile whose compiled output a change to any of `workspace_ids` can
/// touch. [`affected_services_for_workspace_members`] already unions both the
/// `workspace_members` arm (the pre-rewrite principal) and the `ws:<slug> members`
/// group arm (the post-rewrite principal), so one call per workspace covers both
/// sides of the rewrite. Ascending and deduplicated.
async fn affected_for_workspaces(
    tx: &mut Transaction<'_, Postgres>,
    workspace_ids: &[Uuid],
) -> Result<Vec<i64>, PermHttpError> {
    let mut affected: Vec<i64> = Vec::new();
    for &ws in workspace_ids {
        affected = merge_service_ids(
            &affected,
            &affected_services_for_workspace_members(tx, ws).await?,
        );
    }
    Ok(affected)
}

/// The byte-identical fingerprint gate followed by the probe gate, for one
/// affected profile. `before` is the fingerprint captured before the rewrite.
/// Refuses (returns `Err`) on any divergence.
async fn gate_profile(
    tx: &mut Transaction<'_, Postgres>,
    service_id: i64,
    before: &str,
    before_download: &egret_ranger_eval::model::ServicePolicySet,
) -> Result<(), PermHttpError> {
    let after = compiled_fingerprint(tx, service_id).await?;
    if after != before {
        return Err(PermHttpError::Conflict(format!(
            "workspace-assignment migration would change the compiled output of profile \
             #{service_id}; refusing and rolling back. A workspace's members and its \
             `ws:<slug> members` group must expand to the same usernames, so the compiled \
             artifact must be byte-identical."
        )));
    }

    // The independent probe gate over the same before/after downloads. With
    // byte-identical bodies it can only agree; a disagreement means a bug in the
    // fingerprint gate's premise and is refused. A profile the evaluator cannot
    // probe (no bound engine / no probe) is already covered by the fingerprint
    // gate above, so its refusal is not itself a failure.
    let svc = get_service_by_id_via(tx, service_id).await?;
    let after_download = compiled_artifact_via(tx, &svc).await?;
    let def = ServiceDef::parse(&before_download.service_def);
    let versions = probe_versions(tx, service_id).await?;
    let assignments = profile_tag_assignments(tx, service_id).await?;
    let probes = derive_probes(before_download, &after_download, &def, &assignments);
    let case = ProfileCase {
        service_id,
        service_name: svc.name.clone(),
        before: before_download,
        after: &after_download,
        service_def: &def,
        engine: EngineConfig::without_super_users(),
        versions,
        probes,
    };
    match compare(&case) {
        Ok(ds) if !ds.is_empty() => Err(PermHttpError::Conflict(format!(
            "workspace-assignment migration diverges on {} probe(s) in profile #{service_id}; \
             refusing and rolling back",
            ds.len()
        ))),
        Ok(_) => Ok(()),
        Err(GateRefusal::NoProfiles)
        | Err(GateRefusal::NoPluginVersions { .. })
        | Err(GateRefusal::NoProbes { .. }) => Ok(()),
    }
}

/// Rewrite every implicit workspace-assignment grant into an explicit grant on
/// the managed `ws:<slug> members` group, prove the compiled output is unchanged,
/// and record reversible provenance. On any divergence the function returns `Err`
/// and the caller's transaction rolls back — nothing is committed.
pub async fn migrate_ws_assignments_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    migrated_by: &str,
) -> Result<WsAssignmentMigration, PermHttpError> {
    // Every implicit grant and its single `workspace_members` principal, with
    // the workspace slug the managed group derives from.
    let grants: Vec<(Uuid, Uuid, Uuid, String, String)> = sqlx::query_as(
        "SELECT g.id, g.catalog_id, g.origin_workspace_id, w.slug, p.ref \
           FROM perm.grants g \
           JOIN perm.grant_principals p \
             ON p.grant_id = g.id AND p.kind = 'workspace_members'::perm.principal_kind \
           JOIN public.workspaces w ON w.id = g.origin_workspace_id \
          WHERE g.origin = 'workspace_assignment' \
          ORDER BY g.id",
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(PermHttpError::db)?;

    let workspace_ids: Vec<Uuid> = {
        let mut ws: Vec<Uuid> = grants.iter().map(|g| g.2).collect();
        ws.sort();
        ws.dedup();
        ws
    };
    let affected = affected_for_workspaces(tx, &workspace_ids).await?;

    // Before: recompile each affected profile from the CURRENT (workspace-members)
    // model and capture its compiled fingerprint and download.
    let mut before_fp: std::collections::BTreeMap<i64, String> = std::collections::BTreeMap::new();
    let mut before_dl: std::collections::BTreeMap<i64, egret_ranger_eval::model::ServicePolicySet> =
        std::collections::BTreeMap::new();
    for &sid in &affected {
        recompile_service_in_tx(tx, sid).await?;
        let fp = compiled_fingerprint(tx, sid).await?;
        let svc = get_service_by_id_via(tx, sid).await?;
        let dl = compiled_artifact_via(tx, &svc).await?;
        before_fp.insert(sid, fp);
        before_dl.insert(sid, dl);
    }

    // Rewrite each implicit grant, recording provenance.
    let mut originals: Vec<Original> = Vec::new();
    for (grant_id, catalog_id, origin_workspace_id, slug, principal_ref) in &grants {
        let group_name = ws_members_group_name(slug);

        // Ensure the managed group mirrors the workspace's members (Task 1)
        // BEFORE any ref to it is stored, so principal-ref validation (which the
        // migration bypasses, but the authoring path does not) always finds the
        // `groups` row.
        sync_ws_members_group_in_tx(tx, *origin_workspace_id).await?;

        // Rewrite the principal: workspace_members -> the managed group; mode
        // (include) unchanged.
        let changed = sqlx::query(
            "UPDATE perm.grant_principals \
                SET kind = 'group'::perm.principal_kind, ref = $2 \
              WHERE grant_id = $1 AND kind = 'workspace_members'::perm.principal_kind",
        )
        .bind(grant_id)
        .bind(&group_name)
        .execute(&mut **tx)
        .await
        .map_err(PermHttpError::db)?
        .rows_affected();
        if changed != 1 {
            return Err(PermHttpError::Conflict(format!(
                "workspace-assignment migration expected exactly one workspace_members principal \
                 on grant {grant_id}, found {changed}; refusing and rolling back"
            )));
        }

        // Rewrite the grant's origin to the SP3c-3 group-assignment shape,
        // KEEPING origin_workspace_id. This converges on exactly the row the
        // authoring path (`egret_perm::sync_assignment_grants_in_tx`) now writes
        // for a fresh bind: `origin='workspace_assignment_group'` with its
        // workspace id, deduplicated by the `grants_one_per_assignment_group`
        // partial unique index (migration 0227). A migrated grant and a
        // freshly-authored one are therefore identical rows.
        sqlx::query(
            "UPDATE perm.grants \
                SET origin = 'workspace_assignment_group' \
              WHERE id = $1 AND origin = 'workspace_assignment'",
        )
        .bind(grant_id)
        .execute(&mut **tx)
        .await
        .map_err(PermHttpError::db)?;

        originals.push(Original {
            grant_id: *grant_id,
            catalog_id: *catalog_id,
            origin: "workspace_assignment".into(),
            origin_workspace_id: *origin_workspace_id,
            principal_kind: "workspace_members".into(),
            principal_ref: principal_ref.clone(),
            slug: slug.clone(),
            group_name,
        });
    }

    // After: recompile each affected profile from the rewritten (group-ref)
    // model and gate it against the before fingerprint/download.
    for &sid in &affected {
        recompile_service_in_tx(tx, sid).await?;
        let before = before_fp.get(&sid).map(String::as_str).unwrap_or("");
        let dl = before_dl
            .get(&sid)
            .expect("before download captured for every affected profile");
        gate_profile(tx, sid, before, dl).await?;
    }

    // Provenance: one row per run, enabling the hash-guarded undo.
    let mut catalogs: Vec<Uuid> = originals.iter().map(|o| o.catalog_id).collect();
    catalogs.sort();
    catalogs.dedup();
    let grant_ids: Vec<Uuid> = originals.iter().map(|o| o.grant_id).collect();
    let principal_hashes: Vec<String> = originals
        .iter()
        .map(|o| {
            row_hash(
                o.grant_id,
                "workspace_assignment_group",
                Some(o.origin_workspace_id),
                &o.group_name,
                "include",
            )
        })
        .collect();
    let original_json = serde_json::to_value(&originals).map_err(PermHttpError::db)?;

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO perm.ws_assignment_migration \
           (catalogs, grant_ids, principal_hashes, original, migrated_by) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(&catalogs)
    .bind(&grant_ids)
    .bind(&principal_hashes)
    .bind(&original_json)
    .bind(migrated_by)
    .fetch_one(&mut **tx)
    .await
    .map_err(PermHttpError::db)?;

    Ok(WsAssignmentMigration {
        id,
        catalogs,
        rewritten: originals.len(),
    })
}

/// The current POST-rewrite hash of a migrated grant, held `FOR UPDATE`, or
/// `None` if the grant or its managed-group principal is gone (which the undo
/// treats as a divergence). The `FOR UPDATE` on the join locks both the grant
/// and the principal row for the rest of the transaction.
async fn current_row_hash(
    tx: &mut Transaction<'_, Postgres>,
    o: &Original,
) -> Result<Option<String>, PermHttpError> {
    let row: Option<(String, Option<Uuid>, String, String)> = sqlx::query_as(
        "SELECT g.origin, g.origin_workspace_id, p.ref, p.mode::text \
           FROM perm.grants g \
           JOIN perm.grant_principals p ON p.grant_id = g.id \
          WHERE g.id = $1 AND p.kind = 'group'::perm.principal_kind AND p.ref = $2 \
          FOR UPDATE",
    )
    .bind(o.grant_id)
    .bind(&o.group_name)
    .fetch_optional(&mut **tx)
    .await
    .map_err(PermHttpError::db)?;
    Ok(row.map(|(origin, ws, p_ref, mode)| row_hash(o.grant_id, &origin, ws, &p_ref, &mode)))
}

/// Restore one rewritten grant: its managed-group principal back to its
/// `workspace_members` ref, and its origin back to `workspace_assignment` with
/// its `origin_workspace_id`. Asserts each UPDATE changed EXACTLY one row: the
/// undo already held both rows `FOR UPDATE` through [`current_row_hash`], so a
/// count other than 1 means the row slipped away in the gap — the undo refuses
/// and rolls back rather than reporting a success that restored nothing.
async fn restore_grant(
    tx: &mut Transaction<'_, Postgres>,
    o: &Original,
) -> Result<(), PermHttpError> {
    let principal_changed = sqlx::query(
        "UPDATE perm.grant_principals \
            SET kind = 'workspace_members'::perm.principal_kind, ref = $3 \
          WHERE grant_id = $1 AND kind = 'group'::perm.principal_kind AND ref = $2",
    )
    .bind(o.grant_id)
    .bind(&o.group_name)
    .bind(&o.principal_ref)
    .execute(&mut **tx)
    .await
    .map_err(PermHttpError::db)?
    .rows_affected();

    let grant_changed = sqlx::query(
        "UPDATE perm.grants \
            SET origin = 'workspace_assignment', origin_workspace_id = $2 \
          WHERE id = $1 AND origin = 'workspace_assignment_group'",
    )
    .bind(o.grant_id)
    .bind(o.origin_workspace_id)
    .execute(&mut **tx)
    .await
    .map_err(PermHttpError::db)?
    .rows_affected();

    if principal_changed != 1 || grant_changed != 1 {
        return Err(PermHttpError::Conflict(format!(
            "undo refused, nothing was changed: restoring {} changed {principal_changed} \
             principal row(s) and {grant_changed} grant row(s), expected exactly 1 each — the \
             rewritten grant changed under the undo. Reconcile by hand first.",
            describe(o)
        )));
    }
    Ok(())
}

/// Reverse a workspace-assignment migration: restore every rewritten grant to
/// its implicit `workspace_members` form, recompile, and delete the provenance
/// row. Guarded: `FOR UPDATE` on the provenance row, re-hash every rewritten
/// grant as it stands now, and refuse `409` (writing nothing) if any has changed
/// since the migration.
pub async fn undo_ws_assignments_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    migration_id: i64,
) -> Result<(), PermHttpError> {
    let row: Option<(serde_json::Value, Vec<String>)> = sqlx::query_as(
        "SELECT original, principal_hashes FROM perm.ws_assignment_migration \
          WHERE id = $1 FOR UPDATE",
    )
    .bind(migration_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(PermHttpError::db)?;
    let Some((original_json, principal_hashes)) = row else {
        return Err(PermHttpError::NotFound);
    };
    let originals: Vec<Original> =
        serde_json::from_value(original_json).map_err(PermHttpError::db)?;

    // Re-hash every rewritten grant and refuse on any divergence.
    let mut diverged: Vec<String> = Vec::new();
    for (i, o) in originals.iter().enumerate() {
        let current = current_row_hash(tx, o).await?;
        let recorded = principal_hashes.get(i).map(String::as_str);
        if current.as_deref() != recorded {
            diverged.push(describe(o));
        }
    }
    if !diverged.is_empty() {
        return Err(PermHttpError::Conflict(format!(
            "undo refused, nothing was changed: {} rewritten grant(s) have changed since the \
             migration ({}). Reconcile these by hand first.",
            diverged.len(),
            diverged.join("; ")
        )));
    }

    // Restore each grant, then recompile every profile the restored workspaces
    // reach.
    for o in &originals {
        restore_grant(tx, o).await?;
    }
    let workspace_ids: Vec<Uuid> = {
        let mut ws: Vec<Uuid> = originals.iter().map(|o| o.origin_workspace_id).collect();
        ws.sort();
        ws.dedup();
        ws
    };
    let affected = affected_for_workspaces(tx, &workspace_ids).await?;
    for &sid in &affected {
        recompile_service_in_tx(tx, sid).await?;
    }

    sqlx::query("DELETE FROM perm.ws_assignment_migration WHERE id = $1")
        .bind(migration_id)
        .execute(&mut **tx)
        .await
        .map_err(PermHttpError::db)?;
    Ok(())
}
