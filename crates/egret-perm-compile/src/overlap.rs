//! **The compiler refuses the overlaps it alone can see, names the RULES, and
//! answers 409 everywhere** (SP4d-4 Task 3; spec Part A, "Which write catches
//! what" and "The refusal").
//!
//! The rule is `egret_policy::unit_overlaps` — spelled once, called by the
//! policy editor with hand-written bodies and here with compiled ones (user
//! ruling: one overlap function for both). It takes every within-download pair
//! from `egret_policy::ambiguous_arbitrations_in` and adds the pairs that
//! cross the tag/resource boundary or meet only through tag assignments: a
//! tag rule and a path rule on one tagged column (Trino), and two tag rules
//! whose tags are assigned on one column. Those need the profile's
//! `catalog_tag_assignments`, which is why only the compiler (and a tag
//! assignment) can see them. This module adds what the rule does not know: a
//! compiled policy's RULE (id and name), the structured 409 body, and the
//! before/after check a tag assignment runs over the whole stored unit.
//!
//! # Every write that recompiles, by code path (re-verified for this task)
//!
//! `rg 'recompile_service_in_tx|recompile_services_in_tx|recompile_service\('
//! crates --glob '!**/tests/**'` — each of these reaches
//! `trigger::compile_desired`, so each refuses an overlapping compiled set:
//!
//! * `egret-perm-compile` `routes.rs`: `write_grant` and `write_rule` (POST and
//!   PATCH of `/perm/grants` and `/perm/rules`, and `POST /perm/preview`, which
//!   runs them in a rolled-back transaction), `delete_grant`, `delete_rule`,
//!   and `compile` (`POST /perm/policies/services/{id}/compile`, through
//!   `recompile_service`);
//! * `migrate.rs`: the migration (through `write_grant`/`write_rule`) and its
//!   undo (`recompile_services_in_tx`);
//! * `role_membership.rs`: `delete_platform_role_and_recompile`,
//!   `with_recompile`;
//! * `egret-bi`: `catalog_tags::recompile_bound_services_in_tx` (tag assign /
//!   unassign), `tags::recompile_for_rename_in_tx`,
//!   `tags::recompile_for_clearance_in_tx`;
//! * `egret-workspace`: `rbac::replace_platform_roles`,
//!   `members::add_with_expiry`, `members::remove`;
//! * `egret-catalog-registry` `store::delete_catalog_with_compiler` and
//!   `egret-catalog-engine` `delete_warehouse_releasing_catalog_with_compiler`
//!   (through the compiler port);
//! * `egret-bin` `main.rs`: the port implementations
//!   (`recompile_catalog_assignment`, `recompile_one_profile`, and the
//!   `recompile_services_in_tx` port method).
//!
//! **The compiler refuses only the pairs a compile CREATES** (user ruling
//! 2026-10-12, fix round 2). It compares the stored unit — compiled AND
//! hand-written policies, as served before the write — with the unit after its
//! compile ([`refuse_created_by_compile_via`]). So a rule write, a migration or
//! a membership-driven recompile that completes a pair, or changes a stored
//! pair's decisive items or coordinates, is refused; a pair already stored is
//! reported (`explain`, `GET /perm/overlaps`) and blocks nothing. (Round 1
//! refused every pair present after the compile; with tag-rule writes and tag
//! renames recompiling every engine-bound profile, one stored pair anywhere
//! refused them installation-wide, and live data could already hold such
//! pairs on deploy.) A migration switches its originals off BEFORE its rule
//! writes (`migrate.rs`, step 1), so the compiled twin is never paired with
//! the policy it replaces.
//!
//! **A limit of the stored-body comparison:** a tag RENAME re-reads the
//! assignments under the new name while the stored compiled tag policy still
//! names the old one, so a stored pair that meets through the renamed tag is
//! absent from BEFORE and present AFTER, and the rename is refused. Fail-closed
//! and narrow; break the pair first.
//!
//! **Inputs the comparison reads as stored** (re-review M-2): the tag
//! assignments on the profile's catalogs and its `tagService`. A caller that
//! changes one of those and then recompiles makes the pair that change
//! completes read as already stored, so it needs its own before-snapshot
//! ([`unit_snapshot_via`] + [`refuse_created_overlaps_via`]). Every such
//! caller takes one: the tag assignment and the tag rename (`egret-bi`), and
//! (SP4d-4 Task 7) the two writes that change which catalogs — and so which
//! assignments — a unit resolves, through `egret-engine-mgmt`'s
//! `ProfileCompilerPort::{overlap_snapshot_in_tx, refuse_created_overlaps_in_tx}`:
//! a profile bind (`PUT /engines/{id}` and external-engine registration,
//! snapshot before `set_engine_authz_service_in_tx`, check after the recompile,
//! one transaction) and an engine ↔ catalog attachment (an external engine's
//! declared attachment the same way; a provisioned engine's, which applies to
//! the cluster before it can recompile, through a dry run on a rolled-back
//! transaction before anything is applied). `tagService` has no UPDATE path at
//! all.
//!
//! **Cost:** with N profiles sharing one tag service, a tag-rule write
//! recompiles all N and each compile loads all N units — about N² unit loads.
//!
//! # Do two recompiles of profiles sharing one tag service serialise?
//!
//! **Yes, read from the code (not run as a concurrency test).**
//! `trigger::lock_services_in_tx` expands the set it is given with
//! `services_and_tag_services_via` (each profile plus the tag service its
//! `tagService` names) and takes `SELECT … FOR UPDATE` on every id in ascending
//! order; `apply_diff_in_tx` re-takes the same row locks over
//! `Compiled::sweep` (the profile and its tag service). Two transactions
//! recompiling profiles A and B that name one tag service T both lock T's
//! `policy.authz_services` row, so the second waits for the first to commit.
//! **The limit:** a caller that does not pre-lock (it calls
//! `recompile_service_in_tx` directly) reads the model and the assignments in
//! `compile_desired` BEFORE `apply_diff_in_tx` takes T's lock, so its overlap
//! check runs on a snapshot another transaction may change before it commits.
//! That limit stands.
//!
//! **The policy editor is serialised with them (fix round 1, review I1).** Its
//! write (`egret_policy::admin`'s `write_policy_checked`) takes `FOR UPDATE` on
//! every service row of every unit it lands in, ascending, through the same
//! `egret_policy::lock_service_rows_via` this crate's `lock_services_in_tx`
//! calls, then checks and writes on that transaction. An editor write and a
//! tag assignment, a recompile or a second editor write on one unit now queue;
//! `two_concurrent_editor_writes_forming_a_pair_store_exactly_one`
//! (`egret-policy/tests/admin_routes.rs`) pins it.

use std::collections::{BTreeSet, HashMap};

use egret_perm::{CompileError, OverlapKind, OverlapRule};
use egret_policy::{
    get_service_by_id_via, is_compiler_owned_policy_name, load_unit_via, unit_overlaps,
    units_for_services_via, Arbitrated, AssignedTag, OverlapShape, UnitIds, UnitOverlap,
    UnitPolicy,
};
use serde_json::Value;

/// One compiled policy, as the overlap check reads it.
#[derive(Debug, Clone)]
pub struct CompiledForOverlap<'a> {
    /// The service the body is stored under.
    pub service: i64,
    pub is_tag_service: bool,
    pub policy_name: &'a str,
    pub body: &'a Value,
    /// From `DesiredPolicy.rule_id` and the rule's name; `None` for a policy
    /// with no rule (a grant, a clearance — neither is a mask or row filter
    /// today — or a hand-written policy).
    pub rule: Option<OverlapRule>,
}

/// One pair, named for a steward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overlap {
    pub kind: OverlapKind,
    pub coordinate: String,
    /// In the pair's order (policy names ascending).
    pub rules: [OverlapRule; 2],
    pub sentence: String,
}

fn kind_of(a: Arbitrated) -> OverlapKind {
    match a {
        Arbitrated::Mask => OverlapKind::Mask,
        Arbitrated::RowFilter => OverlapKind::RowFilter,
    }
}

/// `rule "<name>"` for a compiled policy, `policy "<name>"` otherwise — so the
/// sentence names what the steward edits, never `perm-rule-<uuid>`.
fn label(rules: &HashMap<String, OverlapRule>, policy_name: &str) -> String {
    match rules.get(policy_name) {
        Some(OverlapRule { id: Some(_), name }) => format!("rule {name:?}"),
        _ => format!("policy {policy_name:?}"),
    }
}

fn member(rules: &HashMap<String, OverlapRule>, policy_name: &str) -> OverlapRule {
    rules.get(policy_name).cloned().unwrap_or(OverlapRule {
        id: None,
        name: policy_name.to_string(),
    })
}

fn named(o: UnitOverlap, rules: &HashMap<String, OverlapRule>) -> Overlap {
    Overlap {
        kind: kind_of(o.arbitrated),
        coordinate: o.coordinate.clone(),
        rules: [member(rules, &o.first), member(rules, &o.second)],
        sentence: o.sentence(&|n| label(rules, n), false),
    }
}

/// **Every overlap in one profile's desired set** (its resource service and its
/// tag service) given the profile's tag assignments. Pure; the order is
/// `unit_overlaps`' (kind, then policy names).
///
/// ⚠ The plan's interface also took the profile's `ServiceDef`, for
/// `egret_ranger_eval::resource::match_policy_resources`. The shared rule
/// instead resolves "does this policy apply at this assigned coordinate" with
/// the same may-intersect comparison the within-download rule uses (see
/// `egret_policy::unit_overlap`'s module doc for why), so no def is needed.
pub fn overlaps(desired: &[CompiledForOverlap<'_>], assignments: &[AssignedTag]) -> Vec<Overlap> {
    overlaps_where(desired, assignments, |_| true)
}

/// [`overlaps`], keeping only the pairs `keep` accepts (before naming).
fn overlaps_where(
    desired: &[CompiledForOverlap<'_>],
    assignments: &[AssignedTag],
    keep: impl Fn(&UnitOverlap) -> bool,
) -> Vec<Overlap> {
    let rules: HashMap<String, OverlapRule> = desired
        .iter()
        .filter_map(|d| Some((d.policy_name.to_string(), d.rule.clone()?)))
        .collect();
    let policies: Vec<UnitPolicy<'_>> = desired
        .iter()
        .map(|d| UnitPolicy {
            service: d.service,
            is_tag_service: d.is_tag_service,
            name: d.policy_name,
            body: d.body,
        })
        .collect();
    egret_policy::unit_overlaps(&policies, assignments)
        .into_iter()
        .filter(|o| keep(o))
        .map(|o| named(o, &rules))
        .collect()
}

/// **The compiler's check: refuse only the pairs THIS compile creates** (user
/// ruling 2026-10-12, superseding the wedge; SP4d-4 Task 3 fix round 2).
///
/// For every evaluation unit the compile writes into — the profile and its tag
/// service, plus, when that tag service is shared, every other profile naming
/// it (its resource policies as stored) — compare:
///
/// * BEFORE: the unit as stored ([`egret_policy::load_unit_via`]: compiled AND
///   hand-written, enabled, as the download holds them before this write);
/// * AFTER: the same unit with the compiler-owned policies of the profile and
///   its tag service replaced by `desired`,
///
/// and refuse the first pair in AFTER whose identity is not in BEFORE
/// ([`egret_policy::created_overlaps`], the same comparison the policy editor
/// and the tag assignment make). Identity is both names, both decisive item
/// arrays and the SET of coordinates, so a compile that changes a stored
/// pair's items or where it meets is "creating" it and is refused. A pair that
/// is already stored is NOT refused: it is reported (`explain`, `GET
/// /perm/overlaps`) and blocks nothing. Nothing is skipped either way.
///
/// The shared-tag-service units matter for soundness, not reach: two profiles
/// recompiled in one transaction write one shared tag service, so the SECOND
/// profile's BEFORE already holds the first one's new tag policy. Checking
/// every unit of that tag service at the FIRST write means a pair between the
/// new tag policy and any profile's stored path policy is seen while it is
/// still new.
///
/// Hand-written × hand-written pairs cannot be created here (both bodies and
/// the assignments are the same on both sides); they stay the editor's and
/// the tag assignment's.
///
/// Cost: a compile of a profile whose tag service N profiles share loads N
/// units, and a tag-rule write recompiles all N, so it reads about N² units —
/// fine for the handful of profiles a tag service is shared by today, and the
/// first thing to revisit if that number grows.
pub async fn refuse_created_by_compile_via(
    conn: &mut sqlx::PgConnection,
    profile: &str,
    profile_id: i64,
    tag_service: Option<i64>,
    desired: &[CompiledForOverlap<'_>],
) -> Result<(), CompileError> {
    let db = |e: egret_policy::PolicyStoreError| CompileError::Db(e.to_string());
    let mut units: BTreeSet<UnitIds> = BTreeSet::from([UnitIds {
        resource: Some(profile_id),
        tag_service,
    }]);
    if let Some(t) = tag_service {
        units.extend(
            egret_policy::units_for_service_via(conn, t)
                .await
                .map_err(db)?,
        );
    }
    let written = |sid: i64| sid == profile_id || Some(sid) == tag_service;
    for ids in units {
        let stored = load_unit_via(conn, ids).await.map_err(db)?;
        let before = stored.overlaps();
        let mut after: Vec<UnitPolicy<'_>> = stored
            .unit_policies()
            .into_iter()
            .filter(|p| !(written(p.service) && is_compiler_owned_policy_name(p.name)))
            .collect();
        after.extend(
            desired
                .iter()
                .filter(|d| Some(d.service) == ids.resource || Some(d.service) == ids.tag_service)
                .map(|d| UnitPolicy {
                    service: d.service,
                    is_tag_service: Some(d.service) == ids.tag_service,
                    name: d.policy_name,
                    body: d.body,
                }),
        );
        let Some(o) =
            egret_policy::created_overlaps(&before, unit_overlaps(&after, &stored.assignments))
                .into_iter()
                .next()
        else {
            continue;
        };
        let mut rules: HashMap<String, OverlapRule> = desired
            .iter()
            .filter_map(|d| Some((d.policy_name.to_string(), d.rule.clone()?)))
            .collect();
        let missing: BTreeSet<&str> = [o.first.as_str(), o.second.as_str()]
            .into_iter()
            .filter(|n| !rules.contains_key(*n))
            .collect();
        rules.extend(rules_for_names_via(conn, &missing).await?);
        // Name the profile whose download serves the pair (re-review M-1):
        // for a pair found in ANOTHER profile's unit of a shared tag service,
        // that is that profile, not the one being recompiled.
        let holder = match ids.resource {
            Some(r) if r != profile_id => get_service_by_id_via(conn, r).await.map_err(db)?.name,
            _ => profile.to_string(),
        };
        return Err(into_compile_error(&holder, named(o, &rules)));
    }
    Ok(())
}

/// The refusal for `profile`.
pub(crate) fn into_compile_error(profile: &str, o: Overlap) -> CompileError {
    CompileError::AmbiguousMaskOrFilterArbitration {
        profile: profile.to_string(),
        refusal: o.sentence,
        kind: o.kind,
        coordinate: o.coordinate,
        rules: o.rules.to_vec(),
    }
}

/// **The 409 body** every surface answers an overlap with: `{ "error":
/// <sentence>, "code": "rule_overlap", "rules": [{id, name}, {id, name}],
/// "coordinate": "...", "kind": "mask" | "row_filter" }`. `None` for every
/// other `CompileError`.
pub fn refusal_body(e: &CompileError) -> Option<Value> {
    e.rule_overlap_body()
}

// ── a tag assignment: refuse what it creates, across the whole stored unit ──

/// The pairs stored in every unit of `profile_ids`, read BEFORE a tag write.
#[derive(Debug, Clone, Default)]
pub struct UnitSnapshot {
    units: Vec<UnitIds>,
    before: Vec<UnitOverlap>,
}

/// Read every unit the profiles in `profile_ids` belong to, and its pairs, on
/// the caller's transaction. Pair this with [`refuse_created_overlaps_via`]
/// after the write.
pub async fn unit_snapshot_via(
    conn: &mut sqlx::PgConnection,
    profile_ids: &[i64],
) -> Result<UnitSnapshot, CompileError> {
    let units = units_for_services_via(conn, profile_ids)
        .await
        .map_err(|e| CompileError::Db(e.to_string()))?;
    let mut before = Vec::new();
    for &ids in &units {
        let unit = load_unit_via(conn, ids)
            .await
            .map_err(|e| CompileError::Db(e.to_string()))?;
        before.extend(unit.overlaps());
    }
    Ok(UnitSnapshot { units, before })
}

/// The rule behind every compiled `perm-rule-<uuid>[-suffix]` name, read from
/// `perm.rules` through the one loader the policy editor uses too
/// (`egret_policy::compiled_rule_names_via`).
async fn rules_for_names_via(
    conn: &mut sqlx::PgConnection,
    names: &BTreeSet<&str>,
) -> Result<HashMap<String, OverlapRule>, CompileError> {
    let names: Vec<&str> = names.iter().copied().collect();
    Ok(egret_policy::compiled_rule_names_via(conn, &names)
        .await
        .map_err(|e| CompileError::Db(e.to_string()))?
        .into_iter()
        .map(|(policy, (id, name))| (policy, OverlapRule { id: Some(id), name }))
        .collect())
}

/// **A tag assignment's own check** (user ruling, SP4d-4 Task 3): after the
/// write and its recompile, refuse the first pair in any unit of the snapshot
/// that was not stored before — between hand-written tag policies, compiled
/// rules, or one of each. Pairs that existed before are reported, not refused
/// (`GET /perm/overlaps`; user ruling 2026-10-12); an unassignment can only
/// remove pairs, so it passes. (The compile this write runs reads the
/// assignments as already changed on both of its sides, so the pair an
/// assignment completes is this check's to see, not the compiler's.)
pub async fn refuse_created_overlaps_via(
    conn: &mut sqlx::PgConnection,
    snapshot: &UnitSnapshot,
) -> Result<(), CompileError> {
    for &ids in &snapshot.units {
        let unit = load_unit_via(conn, ids)
            .await
            .map_err(|e| CompileError::Db(e.to_string()))?;
        let Some(o) = egret_policy::created_overlaps(&snapshot.before, unit.overlaps())
            .into_iter()
            .next()
        else {
            continue;
        };
        let names: BTreeSet<&str> = [o.first.as_str(), o.second.as_str()].into();
        let rules = rules_for_names_via(conn, &names).await?;
        let profile_id = ids.resource.or(ids.tag_service).unwrap_or_default();
        let profile = get_service_by_id_via(conn, profile_id)
            .await
            .map_err(|e| CompileError::Db(e.to_string()))?
            .name;
        return Err(into_compile_error(&profile, named(o, &rules)));
    }
    Ok(())
}

// ── the report: every pair that is STORED, in every profile's download ──

/// One member of a stored pair, as a steward finds it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StoredMember {
    /// The rule's id; `null` for a hand-written policy.
    pub id: Option<uuid::Uuid>,
    /// The `policy.authz_policies` id of the stored policy (the one
    /// `PUT /policies/{id}/enabled` takes), for compiled and hand-written alike.
    pub policy_id: Option<i64>,
    /// The rule's name, or the hand-written policy's name.
    pub name: String,
}

/// One pair already stored in a profile's download.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StoredOverlap {
    /// The profile whose download serves the pair (a tag service on its own
    /// when no profile names it).
    pub service_id: i64,
    pub service_name: String,
    /// `"mask"` or `"row_filter"`.
    pub kind: String,
    pub coordinate: String,
    /// In the pair's order (policy names ascending).
    pub rules: Vec<StoredMember>,
    /// Which member Ranger serves today where both match, when that can be
    /// computed; `null` when the order falls to something this report does
    /// not compute (or to the policy name).
    pub served_today: Option<StoredMember>,
    pub sentence: String,
    pub how_to_fix: String,
}

fn how_to_fix(o: &UnitOverlap, hand_written: bool) -> String {
    let tag = match o.shape {
        OverlapShape::SameDownload(_) => "",
        _ => " Or unassign the tag from the column where they meet.",
    };
    let disable = if hand_written {
        " A hand-written member can also be switched off with PUT \
         /api/control-plane/policies/{id}/enabled (use its policyId)."
    } else {
        ""
    };
    format!(
        "Delete one of the two, or narrow one so they no longer select a common resource.{tag}{disable} \
         An edit that narrows a member but still meets the other is refused as a new overlap. \
         A hand-written member cannot be migrated to a rule until the pair is broken."
    )
}

/// **Every pair stored in every profile's download**, read through
/// `egret_policy::unit_overlaps` over the stored units (compiled AND
/// hand-written, enabled, as each download holds them): each profile with its
/// tag service, every tag service on its own when no profile names it. Reads
/// only; the caller should hold a read-only transaction. Ordered by profile
/// name, then the rule's order. A tag service several profiles share appears
/// once per profile, because each profile's download serves the pair.
///
/// This does NOT go through [`refuse_created_by_compile_via`]: that refuses
/// only a pair a write creates and returns nothing for one already stored.
pub async fn stored_overlaps_via(
    conn: &mut sqlx::PgConnection,
) -> Result<Vec<StoredOverlap>, CompileError> {
    let db = |e: egret_policy::PolicyStoreError| CompileError::Db(e.to_string());
    let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM policy.authz_services ORDER BY id")
        .fetch_all(&mut *conn)
        .await
        .map_err(|e| CompileError::Db(e.to_string()))?;
    let units: Vec<UnitIds> = units_for_services_via(conn, &ids).await.map_err(db)?;
    let mut out = Vec::new();
    for ids in units {
        let unit = load_unit_via(conn, ids).await.map_err(db)?;
        let found = unit.overlaps();
        if found.is_empty() {
            continue;
        }
        let names: BTreeSet<&str> = found
            .iter()
            .flat_map(|o| [o.first.as_str(), o.second.as_str()])
            .collect();
        let compiled = rules_for_names_via(conn, &names).await?;
        let policy_ids: HashMap<&str, i64> = unit
            .policies
            .iter()
            .filter_map(|(_, _, b)| Some((b.get("name")?.as_str()?, b.get("id")?.as_i64()?)))
            .collect();
        let member = |name: &str| StoredMember {
            id: compiled.get(name).and_then(|r| r.id),
            policy_id: policy_ids.get(name).copied(),
            name: compiled
                .get(name)
                .map(|r| r.name.clone())
                .unwrap_or_else(|| name.to_string()),
        };
        let service_id = ids.resource.or(ids.tag_service).unwrap_or_default();
        let service_name = get_service_by_id_via(conn, service_id)
            .await
            .map_err(db)?
            .name;
        for o in found {
            let served_today = match &o.shape {
                OverlapShape::SameDownload(pair) => pair.served_today.clone(),
                OverlapShape::TagAndPath {
                    tag_policy,
                    tag_priority,
                    path_priority,
                    ..
                } => (tag_priority >= path_priority).then(|| tag_policy.clone()),
                OverlapShape::TagAndTag { .. } => None,
            };
            let members = [member(&o.first), member(&o.second)];
            let hand_written = members.iter().any(|m| m.id.is_none());
            out.push(StoredOverlap {
                service_id,
                service_name: service_name.clone(),
                kind: o.kind_wire_name().to_string(),
                coordinate: o.coordinate.clone(),
                served_today: served_today.as_deref().map(member),
                sentence: o.sentence(
                    &|n| match compiled.get(n) {
                        Some(r) => format!("rule {:?}", r.name),
                        None => format!("policy {n:?}"),
                    },
                    false,
                ),
                how_to_fix: how_to_fix(&o, hand_written),
                rules: members.to_vec(),
            });
        }
    }
    out.sort_by(|a, b| a.service_name.cmp(&b.service_name));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    fn mask(name: &str, table: &str, mask_type: &str) -> Value {
        json!({
            "name": name, "policyType": 1, "isEnabled": true, "policyPriority": 0,
            "resources": {
                "catalog": {"values": ["wh"]}, "schema": {"values": ["sales"]},
                "table": {"values": [table]}, "column": {"values": ["email"]},
            },
            "dataMaskPolicyItems": [{"groups": ["public"],
                "dataMaskInfo": {"dataMaskType": mask_type}}],
        })
    }

    #[test]
    fn the_sentence_names_rules_by_name_and_the_body_carries_both() {
        let (a, b) = (
            mask("perm-rule-a", "*", "MASK"),
            mask("perm-rule-b", "orders", "MASK_NULL"),
        );
        let ra = OverlapRule {
            id: Some(Uuid::nil()),
            name: "mask emails".into(),
        };
        let desired = [
            CompiledForOverlap {
                service: 1,
                is_tag_service: false,
                policy_name: "perm-rule-a",
                body: &a,
                rule: Some(ra.clone()),
            },
            CompiledForOverlap {
                service: 1,
                is_tag_service: false,
                policy_name: "perm-rule-b",
                body: &b,
                rule: None,
            },
        ];
        let got = overlaps(&desired, &[]);
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(
            got[0].sentence.contains("rule \"mask emails\""),
            "{}",
            got[0].sentence
        );
        assert!(
            got[0].sentence.contains("policy \"perm-rule-b\""),
            "{}",
            got[0].sentence
        );
        assert_eq!(got[0].rules[0], ra);
        let err = into_compile_error("p", got.into_iter().next().unwrap());
        let body = refusal_body(&err).expect("an overlap body");
        assert_eq!(body["code"], "rule_overlap");
        assert_eq!(body["kind"], "mask");
        assert_eq!(body["rules"].as_array().map(Vec::len), Some(2));
        assert!(refusal_body(&CompileError::Db("x".into())).is_none());
    }
}
