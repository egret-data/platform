//! SP3c-1 Task 4: the reversible, equivalence-gated rewrite of every
//! `kind='role'` principal into a `kind='group'` ref to that role's managed
//! `role:<name> holders` group (Task 2).
//!
//! # Why the rewrite is byte-identical, and why that IS the equivalence gate
//!
//! A `role` principal and a non-`public` `group` principal are expanded the same
//! way on every recompile, BEFORE the lowering runs:
//! [`crate::members::expand_enumerated_principals`] resolves a role into its
//! holders' stored usernames (`Directory::roles`) and a group into its members'
//! stored usernames (`Directory::groups`), each sorted and deduplicated. Task 2's
//! `sync_role_holder_group_in_tx` makes the managed group's membership EXACTLY
//! the role's holders, so both expansions produce the identical username list.
//! The lowering then names only those usernames in the Ranger body — the `Via`
//! provenance (`Via::Role` vs `Via::Group`) is internal IR and is never rendered
//! into `policy.authz_policies.policy_jsonb` — so the compiled artifact the
//! engine downloads is byte-for-byte the same after the rewrite as before it.
//!
//! The migration proves this, rather than assuming it: for every affected
//! profile it recompiles and fingerprints the compiled bodies BEFORE the rewrite,
//! recompiles and fingerprints them AFTER, and refuses (returns `Err`, so the
//! caller's transaction rolls back) on any difference. This byte-identical
//! fingerprint comparison is the equivalence gate at its strongest granularity:
//! it is the literal artifact the engine enforces, so identical fingerprints mean
//! every probe [`crate::equivalence::derive_probes`] could build evaluates
//! identically on both sides. As an independent, redundant check the migration
//! ALSO builds those probes from the before/after downloads and runs
//! [`crate::equivalence::compare`] over every affected profile, refusing on any
//! verdict divergence (a profile with no bound engine or no probe is already
//! covered by the fingerprint gate, so its `GateRefusal` is not itself a
//! failure).
//!
//! # No `public.access_grants` row is ever written
//!
//! The rewrite only UPDATEs `perm.*` principal tables. It never grants a role to
//! its own holder group (which, through `sync_assignment_grants`, could feed a
//! holder back into the group it mirrors). That latent feedback loop is designed
//! out: this module touches `perm.grant_principals`, `perm.rule_exemptions` and
//! `perm.engine_grant_principals` and nothing else.
//!
//! # Reversible
//!
//! Each rewritten row's prior identity (`table`, `id`, `mode`, `role`,
//! `prev_kind`, `prev_ref`) and a canonical hash of its POST-rewrite identity are
//! recorded in `perm.role_principal_migration`. [`undo_role_principals_in_tx`]
//! takes `FOR UPDATE` on that provenance row, re-hashes each rewritten row as it
//! stands now, refuses `409` if any diverges (a row edited or removed since the
//! migration is no longer the migration's to restore), and otherwise restores
//! each `group` ref to its `role` ref, recompiles, and deletes the provenance
//! row.

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
use crate::role_holder_group::{role_holder_group_name, sync_role_holder_group_in_tx};
use crate::routes::{bound_engines, compiled_artifact_via, PermHttpError};
use crate::trigger::{
    affected_services_for_role_membership, affected_services_for_rule, merge_service_ids,
    recompile_service_in_tx,
};

/// What one migration run did: its provenance id, the roles it rewrote, and how
/// many principal rows changed.
#[derive(Debug, Clone, Serialize)]
pub struct RolePrincipalMigration {
    pub id: i64,
    pub roles: Vec<String>,
    pub rewritten: usize,
}

/// One rewritten principal row, recorded for a hash-guarded undo. `mode` is
/// `None` for `engine_grant_principals`, which has no mode column.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Rewrite {
    /// `"grant_principals"`, `"rule_exemptions"` or `"engine_grant_principals"`.
    table: String,
    /// `grant_id`, `rule_id` or `engine_grant_id` — the owning object.
    id: Uuid,
    /// `"include"`/`"exclude"`; `None` for `engine_grant_principals`.
    mode: Option<String>,
    role: String,
    group_name: String,
    /// Always `"role"` today; recorded so the undo restores the exact kind.
    prev_kind: String,
    prev_ref: String,
}

fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// The canonical hash of a rewritten row's POST-rewrite identity: the row the
/// undo expects to find and restore. A row whose `(table, id, mode)` no longer
/// carries `(group, group_name)` hashes differently (it is gone, in the undo's
/// terms), so the undo refuses.
fn row_hash(r: &Rewrite) -> String {
    sha256_hex(&canonical_json(&serde_json::json!({
        "table": r.table,
        "id": r.id.to_string(),
        "mode": r.mode,
        "kind": "group",
        "ref": r.group_name,
    })))
}

fn describe(r: &Rewrite) -> String {
    match &r.mode {
        Some(mode) => format!("{} for object {} ({mode})", r.table, r.id),
        None => format!("{} for object {}", r.table, r.id),
    }
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

/// Every profile whose compiled output a change to one of `roles` can touch:
/// the grant/engine-grant/clearance set ([`affected_services_for_role_membership`])
/// plus, for each rule whose exemption names the role, that rule's affected set.
/// Ascending and deduplicated.
async fn affected_for_roles(
    tx: &mut Transaction<'_, Postgres>,
    roles: &[String],
) -> Result<Vec<i64>, PermHttpError> {
    let mut affected: Vec<i64> = Vec::new();
    for role in roles {
        affected = merge_service_ids(
            &affected,
            &affected_services_for_role_membership(tx, role).await?,
        );
        let rule_ids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT DISTINCT rule_id FROM perm.rule_exemptions \
              WHERE kind = 'role'::perm.principal_kind AND ref = $1",
        )
        .bind(role)
        .fetch_all(&mut **tx)
        .await
        .map_err(PermHttpError::db)?;
        for rid in rule_ids {
            if let Some(rule) = egret_perm::get_rule_via(tx, rid).await? {
                affected =
                    merge_service_ids(&affected, &affected_services_for_rule(tx, &rule).await?);
            }
        }
    }
    Ok(affected)
}

/// The distinct roles any `kind='role'` principal names, across all three
/// principal tables, ascending.
async fn referenced_roles(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<Vec<String>, PermHttpError> {
    sqlx::query_scalar(
        "SELECT ref FROM perm.grant_principals WHERE kind = 'role'::perm.principal_kind \
         UNION \
         SELECT ref FROM perm.rule_exemptions WHERE kind = 'role'::perm.principal_kind \
         UNION \
         SELECT ref FROM perm.engine_grant_principals WHERE kind = 'role'::perm.principal_kind \
         ORDER BY ref",
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(PermHttpError::db)
}

/// Rewrite every `kind='role'` principal of `role` to the managed holder group,
/// in all three tables, returning one [`Rewrite`] per changed row.
async fn rewrite_role(
    tx: &mut Transaction<'_, Postgres>,
    role: &str,
) -> Result<Vec<Rewrite>, PermHttpError> {
    let group_name = role_holder_group_name(role);
    let mut out = Vec::new();

    let grant_rows: Vec<(Uuid, String)> = sqlx::query_as(
        "UPDATE perm.grant_principals \
            SET kind = 'group'::perm.principal_kind, ref = $2 \
          WHERE kind = 'role'::perm.principal_kind AND ref = $1 \
         RETURNING grant_id, mode::text",
    )
    .bind(role)
    .bind(&group_name)
    .fetch_all(&mut **tx)
    .await
    .map_err(PermHttpError::db)?;
    for (id, mode) in grant_rows {
        out.push(Rewrite {
            table: "grant_principals".into(),
            id,
            mode: Some(mode),
            role: role.into(),
            group_name: group_name.clone(),
            prev_kind: "role".into(),
            prev_ref: role.into(),
        });
    }

    let rule_rows: Vec<(Uuid, String)> = sqlx::query_as(
        "UPDATE perm.rule_exemptions \
            SET kind = 'group'::perm.principal_kind, ref = $2 \
          WHERE kind = 'role'::perm.principal_kind AND ref = $1 \
         RETURNING rule_id, mode::text",
    )
    .bind(role)
    .bind(&group_name)
    .fetch_all(&mut **tx)
    .await
    .map_err(PermHttpError::db)?;
    for (id, mode) in rule_rows {
        out.push(Rewrite {
            table: "rule_exemptions".into(),
            id,
            mode: Some(mode),
            role: role.into(),
            group_name: group_name.clone(),
            prev_kind: "role".into(),
            prev_ref: role.into(),
        });
    }

    let engine_rows: Vec<(Uuid,)> = sqlx::query_as(
        "UPDATE perm.engine_grant_principals \
            SET kind = 'group'::perm.principal_kind, ref = $2 \
          WHERE kind = 'role'::perm.principal_kind AND ref = $1 \
         RETURNING engine_grant_id",
    )
    .bind(role)
    .bind(&group_name)
    .fetch_all(&mut **tx)
    .await
    .map_err(PermHttpError::db)?;
    for (id,) in engine_rows {
        out.push(Rewrite {
            table: "engine_grant_principals".into(),
            id,
            mode: None,
            role: role.into(),
            group_name: group_name.clone(),
            prev_kind: "role".into(),
            prev_ref: role.into(),
        });
    }

    Ok(out)
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
            "role-principal migration would change the compiled output of profile #{service_id}; \
             refusing and rolling back. A role and its `role:<name> holders` group must expand to \
             the same usernames, so the compiled artifact must be byte-identical."
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
            "role-principal migration diverges on {} probe(s) in profile #{service_id}; refusing \
             and rolling back",
            ds.len()
        ))),
        Ok(_) => Ok(()),
        Err(GateRefusal::NoProfiles)
        | Err(GateRefusal::NoPluginVersions { .. })
        | Err(GateRefusal::NoProbes { .. }) => Ok(()),
    }
}

/// Rewrite every role principal into its managed holder-group principal, prove
/// the compiled output is unchanged, and record reversible provenance. On any
/// divergence the function returns `Err` and the caller's transaction rolls back
/// — nothing is committed.
pub async fn migrate_role_principals_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    migrated_by: &str,
) -> Result<RolePrincipalMigration, PermHttpError> {
    let roles = referenced_roles(tx).await?;
    let affected = affected_for_roles(tx, &roles).await?;

    // Before: recompile each affected profile from the CURRENT (role-ref) model
    // and capture its compiled fingerprint and download.
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

    // Ensure each role's managed holder group mirrors its holders (Task 2).
    for role in &roles {
        sync_role_holder_group_in_tx(tx, role).await?;
    }

    // Rewrite every role principal, recording provenance.
    let mut rewrites: Vec<Rewrite> = Vec::new();
    for role in &roles {
        rewrites.extend(rewrite_role(tx, role).await?);
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
    let rewrites_json = serde_json::to_value(&rewrites).map_err(PermHttpError::db)?;
    let row_hashes: Vec<String> = rewrites.iter().map(row_hash).collect();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO perm.role_principal_migration (roles, rewrites, row_hashes, migrated_by) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(&roles)
    .bind(&rewrites_json)
    .bind(&row_hashes)
    .bind(migrated_by)
    .fetch_one(&mut **tx)
    .await
    .map_err(PermHttpError::db)?;

    Ok(RolePrincipalMigration {
        id,
        roles,
        rewritten: rewrites.len(),
    })
}

/// Whether a rewritten row still stands exactly as the migration left it, held
/// `FOR UPDATE`.
async fn rewritten_row_present(
    tx: &mut Transaction<'_, Postgres>,
    r: &Rewrite,
) -> Result<bool, PermHttpError> {
    let present: bool = match (r.table.as_str(), &r.mode) {
        ("grant_principals", Some(mode)) => sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM perm.grant_principals \
               WHERE grant_id = $1 AND kind = 'group'::perm.principal_kind AND ref = $2 \
                 AND mode = $3::perm.principal_mode FOR UPDATE)",
        )
        .bind(r.id)
        .bind(&r.group_name)
        .bind(mode)
        .fetch_one(&mut **tx)
        .await
        .map_err(PermHttpError::db)?,
        ("rule_exemptions", Some(mode)) => sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM perm.rule_exemptions \
               WHERE rule_id = $1 AND kind = 'group'::perm.principal_kind AND ref = $2 \
                 AND mode = $3::perm.principal_mode FOR UPDATE)",
        )
        .bind(r.id)
        .bind(&r.group_name)
        .bind(mode)
        .fetch_one(&mut **tx)
        .await
        .map_err(PermHttpError::db)?,
        ("engine_grant_principals", None) => sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM perm.engine_grant_principals \
               WHERE engine_grant_id = $1 AND kind = 'group'::perm.principal_kind AND ref = $2 \
               FOR UPDATE)",
        )
        .bind(r.id)
        .bind(&r.group_name)
        .fetch_one(&mut **tx)
        .await
        .map_err(PermHttpError::db)?,
        _ => {
            return Err(PermHttpError::db(format!(
                "role-principal migration provenance has a malformed rewrite for table {:?}",
                r.table
            )))
        }
    };
    Ok(present)
}

/// Restore one rewritten row from its holder-group ref back to its role ref.
/// Asserts it changed EXACTLY one row: the undo already held the row `FOR UPDATE`
/// through [`rewritten_row_present`], so an affected count other than 1 means it
/// slipped away in the gap — the undo refuses and rolls back rather than deleting
/// the provenance and reporting a success that restored nothing.
async fn restore_row(tx: &mut Transaction<'_, Postgres>, r: &Rewrite) -> Result<(), PermHttpError> {
    let affected = match (r.table.as_str(), &r.mode) {
        ("grant_principals", Some(mode)) => sqlx::query(
            "UPDATE perm.grant_principals \
                SET kind = $4::perm.principal_kind, ref = $5 \
              WHERE grant_id = $1 AND kind = 'group'::perm.principal_kind AND ref = $2 \
                AND mode = $3::perm.principal_mode",
        )
        .bind(r.id)
        .bind(&r.group_name)
        .bind(mode)
        .bind(&r.prev_kind)
        .bind(&r.prev_ref)
        .execute(&mut **tx)
        .await
        .map_err(PermHttpError::db)?
        .rows_affected(),
        ("rule_exemptions", Some(mode)) => sqlx::query(
            "UPDATE perm.rule_exemptions \
                SET kind = $4::perm.principal_kind, ref = $5 \
              WHERE rule_id = $1 AND kind = 'group'::perm.principal_kind AND ref = $2 \
                AND mode = $3::perm.principal_mode",
        )
        .bind(r.id)
        .bind(&r.group_name)
        .bind(mode)
        .bind(&r.prev_kind)
        .bind(&r.prev_ref)
        .execute(&mut **tx)
        .await
        .map_err(PermHttpError::db)?
        .rows_affected(),
        ("engine_grant_principals", None) => sqlx::query(
            "UPDATE perm.engine_grant_principals \
                SET kind = $3::perm.principal_kind, ref = $4 \
              WHERE engine_grant_id = $1 AND kind = 'group'::perm.principal_kind AND ref = $2",
        )
        .bind(r.id)
        .bind(&r.group_name)
        .bind(&r.prev_kind)
        .bind(&r.prev_ref)
        .execute(&mut **tx)
        .await
        .map_err(PermHttpError::db)?
        .rows_affected(),
        _ => {
            return Err(PermHttpError::db(format!(
                "role-principal migration provenance has a malformed rewrite for table {:?}",
                r.table
            )))
        }
    };
    if affected != 1 {
        return Err(PermHttpError::Conflict(format!(
            "undo refused, nothing was changed: restoring {} affected {affected} row(s), expected \
             exactly 1 — the rewritten row changed under the undo. Reconcile by hand first.",
            describe(r)
        )));
    }
    Ok(())
}

/// Reverse a role-principal migration: restore every rewritten `group` ref to
/// its `role` ref, recompile, and delete the provenance row. Guarded: `FOR
/// UPDATE` on the provenance row, re-hash every rewritten row as it stands now,
/// and refuse `409` (writing nothing) if any has changed since the migration.
pub async fn undo_role_principals_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    migration_id: i64,
) -> Result<(), PermHttpError> {
    let row: Option<(Vec<String>, serde_json::Value, Vec<String>)> = sqlx::query_as(
        "SELECT roles, rewrites, row_hashes FROM perm.role_principal_migration \
          WHERE id = $1 FOR UPDATE",
    )
    .bind(migration_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(PermHttpError::db)?;
    let Some((roles, rewrites_json, row_hashes)) = row else {
        return Err(PermHttpError::NotFound);
    };
    let rewrites: Vec<Rewrite> =
        serde_json::from_value(rewrites_json).map_err(PermHttpError::db)?;

    // Re-hash every rewritten row and refuse on any divergence.
    let mut diverged: Vec<String> = Vec::new();
    for (i, r) in rewrites.iter().enumerate() {
        let present = rewritten_row_present(tx, r).await?;
        let current = if present { Some(row_hash(r)) } else { None };
        let recorded = row_hashes.get(i).map(String::as_str);
        if current.as_deref() != recorded {
            diverged.push(describe(r));
        }
    }
    if !diverged.is_empty() {
        return Err(PermHttpError::Conflict(format!(
            "undo refused, nothing was changed: {} rewritten principal row(s) have changed since \
             the migration ({}). Reconcile these by hand first.",
            diverged.len(),
            diverged.join("; ")
        )));
    }

    // Restore each row, then recompile every profile the restored roles reach.
    for r in &rewrites {
        restore_row(tx, r).await?;
    }
    let affected = affected_for_roles(tx, &roles).await?;
    for &sid in &affected {
        recompile_service_in_tx(tx, sid).await?;
    }

    sqlx::query("DELETE FROM perm.role_principal_migration WHERE id = $1")
        .bind(migration_id)
        .execute(&mut **tx)
        .await
        .map_err(PermHttpError::db)?;
    Ok(())
}
