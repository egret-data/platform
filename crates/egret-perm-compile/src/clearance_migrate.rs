//! SP3c-2 Task 4: the reversible, equivalence-gated migration of every gated
//! clearance (`public.classification_policies`) into a `perm.rules` row of kind
//! `withhold` with its cleared principals recorded as `perm.rule_exemptions`.
//!
//! # What a withhold rule is, and why creating it changes nothing yet
//!
//! A clearance "deny the tagged column to everyone except the cleared" becomes a
//! withhold rule: `kind='withhold'`, `selector_kind='tag'`, `tag_key` the gated
//! tag's key, and NO mask/row-filter payload (migration 0223's CHECK). Each
//! cleared principal becomes an `include` exemption of `kind='group'`:
//!
//! - each `required_roles[i]` → the SP3c-1 managed `role:<name> holders` group
//!   ([`role_holder_group_name`]), whose membership mirrors the role's holders
//!   ([`sync_role_holder_group_in_tx`], ensured here so the ref resolves);
//! - each `required_groups[i]` (a group UUID) → that group's `groups.name`.
//!
//! **This task is ADDITIVE.** Enforcement still reads `classification_policies`
//! ([`crate::ir_load::load_withhold_inputs`]); the read path is switched to the
//! withhold rules in Task 5. The compile loaders that feed a recompile
//! (`egret_perm::store::list_rules_*`) exclude `kind='withhold'` rows, so the
//! rows created here are invisible to the compiler until Task 5. Creating them is
//! therefore a pure addition that leaves every profile's compiled artifact
//! byte-for-byte unchanged.
//!
//! # The equivalence gate
//!
//! Like the role-principal migration ([`crate::role_principal_migrate`]) the
//! migration PROVES, rather than assumes, that it changed no compiled output: for
//! every affected profile it recompiles and fingerprints the compiled bodies
//! BEFORE creating the rules, creates them, recompiles and fingerprints AFTER,
//! and refuses (returns `Err`, so the caller's transaction rolls back) on any
//! difference. Because the read path is unchanged in Task 4, this holds for
//! role-bearing AND groups-only clearances alike — the brief carves groups-only
//! out of the byte-identical assertion because Task 2 already changed how a
//! groups-only clearance lowers, but that change is upstream of Task 4: both the
//! before and the after fingerprints here are taken with Task 2 applied, so the
//! two are equal for every clearance. The per-profile byte-identical check is the
//! strongest granularity (it is the literal artifact the engine downloads), so it
//! subsumes the role-bearing requirement.
//!
//! # Reversible
//!
//! One provenance row per run (`perm.clearance_migration`, migration 0224) holds
//! the created `rule_ids`, a per-rule hash of its exemptions, the migrated tag
//! keys, and the pre-migration `classification_policies` rows as JSON.
//! [`undo_clearances_in_tx`] takes `FOR UPDATE` on that row, re-hashes each
//! rule's exemptions as they stand now, refuses `409` if any diverges (a rule
//! edited or removed since the migration is no longer the migration's to undo),
//! restores `classification_policies` from the snapshot, deletes the withhold
//! rules and their exemptions, recompiles, and deletes the provenance row.

use serde::Serialize;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::migrate::canonical_json;
use crate::role_holder_group::{role_holder_group_name, sync_role_holder_group_in_tx};
use crate::routes::PermHttpError;
use crate::trigger::{affected_services_for_classification, recompile_service_in_tx};

/// What one clearance migration run did: its provenance id, the tag keys it
/// migrated, and how many withhold rules it created.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ClearanceMigration {
    pub id: i64,
    pub tags: Vec<String>,
    pub rules_created: usize,
}

/// One gated `classification_policies` row joined to its tag.
#[derive(sqlx::FromRow)]
struct ClearanceRow {
    tag_id: Uuid,
    tag_key: String,
    required_roles: Vec<String>,
    required_groups: Vec<Uuid>,
    created_by: Option<Uuid>,
}

fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// The canonical hash of one withhold rule's exemptions AS STORED — the row the
/// undo expects to find unchanged. Reading them back from the database (rather
/// than hashing the list we meant to write) makes the migrate-side hash and the
/// undo-side hash identical by construction: same query, same ordering. A rule
/// whose exemptions were edited, or that was deleted (no rows), hashes
/// differently, which is what the undo's divergence check keys on.
async fn rule_exemption_hash(
    tx: &mut Transaction<'_, Postgres>,
    rule_id: Uuid,
) -> Result<String, PermHttpError> {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT kind::text, ref, mode::text FROM perm.rule_exemptions \
          WHERE rule_id = $1 ORDER BY kind, ref, mode",
    )
    .bind(rule_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(PermHttpError::db)?;
    let v = serde_json::Value::Array(
        rows.into_iter()
            .map(|(kind, r, mode)| serde_json::json!({ "kind": kind, "ref": r, "mode": mode }))
            .collect(),
    );
    Ok(sha256_hex(&canonical_json(&v)))
}

/// The compiled bodies of a profile, hashed: `name`+`policy_jsonb` of every
/// `policy.authz_policies` row, ordered by name, keys canonicalised — the same
/// fingerprint [`crate::role_principal_migrate`] gates on. The envelope's
/// `policy_version` is deliberately excluded.
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

/// The unique name of the withhold rule for a tag: stable per tag key, so a
/// second migration run collides on `perm.rules.name`'s UNIQUE and is refused
/// rather than duplicating the rule.
fn withhold_rule_name(tag_key: &str) -> String {
    format!("clearance:{tag_key}")
}

/// Resolve a group UUID to its `groups.name`, or refuse: a dangling clearance
/// group reference cannot become a resolvable exemption ref.
async fn group_name_for_id(
    tx: &mut Transaction<'_, Postgres>,
    group_id: Uuid,
) -> Result<String, PermHttpError> {
    sqlx::query_scalar("SELECT name FROM public.groups WHERE id = $1")
        .bind(group_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(PermHttpError::db)?
        .ok_or_else(|| {
            PermHttpError::BadRequest(format!(
                "a clearance names group {group_id}, which no longer exists; its clearance cannot \
                 be migrated to a withhold rule until the reference is fixed"
            ))
        })
}

/// REPLACE a withhold rule's exemptions with exactly the cleared principals:
/// each required role → its managed `role:<name> holders` group (ensured here so
/// the ref resolves), each required group UUID → that group's `groups.name`.
/// Every one is an `include` group exemption. The single source of truth for what
/// a clearance's exemptions are, shared by the bulk migration and by authoring.
async fn replace_rule_exemptions(
    tx: &mut Transaction<'_, Postgres>,
    rule_id: Uuid,
    required_roles: &[String],
    required_groups: &[Uuid],
) -> Result<(), PermHttpError> {
    let mut refs: Vec<String> = Vec::new();
    for role in required_roles {
        // Ensure the managed holder group exists so the ref resolves (SP3c-1).
        sync_role_holder_group_in_tx(tx, role).await?;
        refs.push(role_holder_group_name(role));
    }
    for group_id in required_groups {
        refs.push(group_name_for_id(tx, *group_id).await?);
    }
    sqlx::query("DELETE FROM perm.rule_exemptions WHERE rule_id = $1")
        .bind(rule_id)
        .execute(&mut **tx)
        .await
        .map_err(PermHttpError::db)?;
    // PK (rule_id, kind, ref, mode) dedups a role and a group that resolve to
    // the same name.
    for r in refs {
        sqlx::query(
            "INSERT INTO perm.rule_exemptions (rule_id, kind, ref, mode) \
             VALUES ($1, 'group'::perm.principal_kind, $2, 'include'::perm.principal_mode) \
             ON CONFLICT DO NOTHING",
        )
        .bind(rule_id)
        .bind(&r)
        .execute(&mut **tx)
        .await
        .map_err(PermHttpError::db)?;
    }
    Ok(())
}

/// Create the withhold rule for one clearance and its `include` exemptions,
/// returning the new rule id. Refuses `409` if the rule already exists (the bulk
/// migration must not silently re-migrate).
async fn create_withhold_rule(
    tx: &mut Transaction<'_, Postgres>,
    row: &ClearanceRow,
) -> Result<Uuid, PermHttpError> {
    let name = withhold_rule_name(&row.tag_key);
    let rule_id: Uuid = match sqlx::query_scalar(
        "INSERT INTO perm.rules (name, kind, selector_kind, tag_key) \
         VALUES ($1, 'withhold', 'tag', $2) RETURNING id",
    )
    .bind(&name)
    .bind(&row.tag_key)
    .fetch_one(&mut **tx)
    .await
    {
        Ok(id) => id,
        Err(sqlx::Error::Database(d)) if d.is_unique_violation() => {
            return Err(PermHttpError::Conflict(format!(
                "a withhold rule named {name:?} already exists; this clearance looks already \
                 migrated — undo that migration before migrating again"
            )))
        }
        Err(e) => return Err(PermHttpError::db(e)),
    };
    replace_rule_exemptions(tx, rule_id, &row.required_roles, &row.required_groups).await?;
    Ok(rule_id)
}

/// SP3c-2 Task 6: make a tag's withhold rule match its authored clearance.
/// Called by the clearance authoring path in the same transaction that writes
/// `classification_policies`, so the table (the author store) and the rule (what
/// enforcement reads) never diverge. Creates the rule if absent, then REPLACES
/// its exemptions with exactly the cleared principals. An empty clearance (no
/// roles AND no groups) is no gate, so the rule is removed instead — a withhold
/// rule with no exemption would withhold from everyone.
pub async fn upsert_clearance_withhold_rule_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    tag_id: Uuid,
    required_roles: &[String],
    required_groups: &[Uuid],
) -> Result<(), PermHttpError> {
    if required_roles.is_empty() && required_groups.is_empty() {
        return remove_clearance_withhold_rule_in_tx(tx, tag_id).await;
    }
    let Some(tag_key) = tag_key_for_id(tx, tag_id).await? else {
        return Err(PermHttpError::NotFound);
    };
    let name = withhold_rule_name(&tag_key);
    let rule_id: Uuid = sqlx::query_scalar(
        "INSERT INTO perm.rules (name, kind, selector_kind, tag_key) \
         VALUES ($1, 'withhold', 'tag', $2) \
         ON CONFLICT (name) DO UPDATE SET name = EXCLUDED.name RETURNING id",
    )
    .bind(&name)
    .bind(&tag_key)
    .fetch_one(&mut **tx)
    .await
    .map_err(PermHttpError::db)?;
    replace_rule_exemptions(tx, rule_id, required_roles, required_groups).await
}

/// SP3c-2 Task 6: remove a tag's withhold rule and its exemptions (idempotent).
/// Called when a clearance is cleared, so enforcement drops with it.
pub async fn remove_clearance_withhold_rule_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    tag_id: Uuid,
) -> Result<(), PermHttpError> {
    let Some(tag_key) = tag_key_for_id(tx, tag_id).await? else {
        return Ok(());
    };
    let name = withhold_rule_name(&tag_key);
    sqlx::query(
        "DELETE FROM perm.rule_exemptions WHERE rule_id IN \
           (SELECT id FROM perm.rules WHERE kind = 'withhold' AND name = $1)",
    )
    .bind(&name)
    .execute(&mut **tx)
    .await
    .map_err(PermHttpError::db)?;
    sqlx::query("DELETE FROM perm.rules WHERE kind = 'withhold' AND name = $1")
        .bind(&name)
        .execute(&mut **tx)
        .await
        .map_err(PermHttpError::db)?;
    Ok(())
}

async fn tag_key_for_id(
    tx: &mut Transaction<'_, Postgres>,
    tag_id: Uuid,
) -> Result<Option<String>, PermHttpError> {
    sqlx::query_scalar("SELECT key FROM public.tags WHERE id = $1")
        .bind(tag_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(PermHttpError::db)
}

/// Migrate every gated clearance into a withhold rule, prove the compiled output
/// is unchanged, and record reversible provenance. On any divergence the
/// function returns `Err` and the caller's transaction rolls back — nothing is
/// committed.
pub async fn migrate_clearances_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    migrated_by: &str,
) -> Result<ClearanceMigration, PermHttpError> {
    let affected = affected_services_for_classification(tx).await?;

    // Before: recompile each affected profile and fingerprint its compiled
    // bodies as the engine would download them NOW.
    let mut before_fp: std::collections::BTreeMap<i64, String> = std::collections::BTreeMap::new();
    for &sid in &affected {
        recompile_service_in_tx(tx, sid).await?;
        before_fp.insert(sid, compiled_fingerprint(tx, sid).await?);
    }

    // Every gated clearance, joined to its tag. A row naming neither a role nor a
    // group is no gate (it compiles to no deny), so it is not migrated — a
    // withhold rule with no exemption would withhold from everyone.
    let rows: Vec<ClearanceRow> = sqlx::query_as(
        "SELECT cp.tag_id, t.key AS tag_key, cp.required_roles, cp.required_groups, cp.created_by \
           FROM classification_policies cp \
           JOIN tags t ON t.id = cp.tag_id \
          WHERE cardinality(cp.required_roles) > 0 OR cardinality(cp.required_groups) > 0 \
          ORDER BY cp.tag_id",
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(PermHttpError::db)?;

    // Snapshot the migrated rows for the undo's restore, before any write.
    let original = serde_json::Value::Array(
        rows.iter()
            .map(|r| {
                serde_json::json!({
                    "tag_id": r.tag_id.to_string(),
                    "required_roles": r.required_roles,
                    "required_groups": r.required_groups.iter().map(|g| g.to_string()).collect::<Vec<_>>(),
                    "created_by": r.created_by.map(|u| u.to_string()),
                })
            })
            .collect(),
    );

    let mut tags: Vec<String> = Vec::with_capacity(rows.len());
    let mut rule_ids: Vec<Uuid> = Vec::with_capacity(rows.len());
    let mut exemption_hashes: Vec<String> = Vec::with_capacity(rows.len());
    let mut rules_json: Vec<serde_json::Value> = Vec::with_capacity(rows.len());
    for row in &rows {
        let rule_id = create_withhold_rule(tx, row).await?;
        let hash = rule_exemption_hash(tx, rule_id).await?;
        rules_json.push(serde_json::json!({
            "tag_key": row.tag_key,
            "rule_id": rule_id.to_string(),
            "name": withhold_rule_name(&row.tag_key),
        }));
        tags.push(row.tag_key.clone());
        rule_ids.push(rule_id);
        exemption_hashes.push(hash);
    }

    // Provenance: one row per run, enabling the hash-guarded undo.
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO perm.clearance_migration \
           (tags, rules, rule_ids, exemption_hashes, original, migrated_by) \
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(&tags)
    .bind(serde_json::Value::Array(rules_json))
    .bind(&rule_ids)
    .bind(&exemption_hashes)
    .bind(&original)
    .bind(migrated_by)
    .fetch_one(&mut **tx)
    .await
    .map_err(PermHttpError::db)?;

    // After: recompile each affected profile and gate its fingerprint against the
    // before fingerprint. Creating withhold rules is invisible to the compiler
    // (the loaders exclude `kind='withhold'`), so the two must be identical.
    for &sid in &affected {
        recompile_service_in_tx(tx, sid).await?;
        let after = compiled_fingerprint(tx, sid).await?;
        let before = before_fp.get(&sid).map(String::as_str).unwrap_or("");
        if after != before {
            return Err(PermHttpError::Conflict(format!(
                "clearance migration would change the compiled output of profile #{sid}; refusing \
                 and rolling back. Creating withhold rules is additive and must leave every \
                 compiled artifact byte-identical until the read path is switched (Task 5)."
            )));
        }
    }

    Ok(ClearanceMigration {
        id,
        tags,
        rules_created: rule_ids.len(),
    })
}

/// Reverse a clearance migration: restore `classification_policies` from the
/// snapshot, delete the withhold rules and their exemptions, recompile, and
/// delete the provenance row. Guarded: `FOR UPDATE` on the provenance row,
/// re-hash every created rule's exemptions as they stand now, and refuse `409`
/// (writing nothing) if any has changed since the migration.
pub async fn undo_clearances_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    migration_id: i64,
) -> Result<(), PermHttpError> {
    let row: Option<(Vec<Uuid>, Vec<String>, serde_json::Value)> = sqlx::query_as(
        "SELECT rule_ids, exemption_hashes, original FROM perm.clearance_migration \
          WHERE id = $1 FOR UPDATE",
    )
    .bind(migration_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(PermHttpError::db)?;
    let Some((rule_ids, exemption_hashes, original)) = row else {
        return Err(PermHttpError::NotFound);
    };

    // Re-hash every created rule's exemptions and refuse on any divergence.
    let mut diverged: Vec<String> = Vec::new();
    for (i, rule_id) in rule_ids.iter().enumerate() {
        let current = rule_exemption_hash(tx, *rule_id).await?;
        if exemption_hashes.get(i).map(String::as_str) != Some(current.as_str()) {
            diverged.push(rule_id.to_string());
        }
    }
    if !diverged.is_empty() {
        return Err(PermHttpError::Conflict(format!(
            "undo refused, nothing was changed: {} withhold rule(s) have changed since the \
             migration ({}). Reconcile these by hand first.",
            diverged.len(),
            diverged.join(", ")
        )));
    }

    // Restore the migrated clearances from the snapshot. Task 4 does not delete
    // them, so this is an idempotent upsert of identical rows; it keeps the undo
    // correct once Task 5 makes the forward migration delete them.
    if let Some(entries) = original.as_array() {
        for entry in entries {
            let tag_id: Uuid = entry
                .get("tag_id")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| PermHttpError::db("clearance provenance has a malformed tag_id"))?;
            let roles: Vec<String> = entry
                .get("required_roles")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let groups: Vec<Uuid> = entry
                .get("required_groups")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().and_then(|s| s.parse().ok()))
                        .collect()
                })
                .unwrap_or_default();
            let created_by: Option<Uuid> = entry
                .get("created_by")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse().ok());
            sqlx::query(
                "INSERT INTO classification_policies (tag_id, required_roles, required_groups, created_by) \
                 VALUES ($1, $2, $3, $4) \
                 ON CONFLICT (tag_id) DO UPDATE SET required_roles = EXCLUDED.required_roles, \
                                                    required_groups = EXCLUDED.required_groups, \
                                                    created_by = EXCLUDED.created_by",
            )
            .bind(tag_id)
            .bind(&roles)
            .bind(&groups)
            .bind(created_by)
            .execute(&mut **tx)
            .await
            .map_err(PermHttpError::db)?;
        }
    }

    // Delete the withhold rules and their exemptions.
    sqlx::query("DELETE FROM perm.rule_exemptions WHERE rule_id = ANY($1)")
        .bind(&rule_ids)
        .execute(&mut **tx)
        .await
        .map_err(PermHttpError::db)?;
    sqlx::query("DELETE FROM perm.rules WHERE id = ANY($1)")
        .bind(&rule_ids)
        .execute(&mut **tx)
        .await
        .map_err(PermHttpError::db)?;

    // Recompile every profile a clearance change reaches, then drop provenance.
    let affected = affected_services_for_classification(tx).await?;
    for &sid in &affected {
        recompile_service_in_tx(tx, sid).await?;
    }

    sqlx::query("DELETE FROM perm.clearance_migration WHERE id = $1")
        .bind(migration_id)
        .execute(&mut **tx)
        .await
        .map_err(PermHttpError::db)?;
    Ok(())
}
