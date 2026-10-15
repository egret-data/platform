//! SP4d-3 Task 5: classify every policy in one Ranger service profile — who
//! owns it, whether it can be migrated into the `perm` model, and if not, every
//! reason why, each rendered as a sentence an operator can act on.
//!
//! The decisions are made by the closed grammar ([`crate::grammar`]); this
//! module supplies the three inputs the grammar cannot know from a body alone,
//! and renders its [`Blocker`]s. Getting those inputs wrong silently
//! reintroduces defects the grammar was built to refuse, so each is derived
//! from the database here, never assumed:
//!
//! - **[`ServiceKind`] and [`EngineType`] come from the service row's `type`
//!   and from whether another profile names it as `tagService`**
//!   ([`service_target`]). `trino` / `starrocks` are resource services of
//!   that engine; `tag` — or any profile some other profile names as its
//!   `tagService`, which is what a tag service made through the admin API is —
//!   is a tag service, and its engine is Trino because only
//!   a `trino` profile may carry a `tagService` (`egret_policy::store::
//!   refuse_tag_service_on_non_trino_profile`, and migration 0176's `CHECK`).
//!   A hard-coded `Resource` would let a tag body stored in a resource service
//!   — where it enforces nothing — migrate into the tag service and start
//!   enforcing.
//! - **The catalog lookup is EXACT and scoped to the catalogs this profile's
//!   engines reach** (`catalogs_for_service_via`, the compiler's own
//!   reachability join). `registry.catalogs.name` is globally `UNIQUE`, so an
//!   unscoped lookup could not be ambiguous — but it would resolve a catalog
//!   this profile does not serve. A policy on such a catalog enforces nothing
//!   here today; migrated, the grant would compile into every OTHER profile
//!   whose engine does serve that catalog, widening access there. Scoped, it is
//!   `UnregisteredCatalog`. For a tag service the scope is the union over the
//!   resource profiles that name it as their `tagService` (only path bodies
//!   use the lookup, and those are refused in a tag service anyway; the union
//!   just keeps a second, misleading "unregistered catalog" sentence out of
//!   that refusal).
//! - **The tag lookup is the real `tags.key → tags.name`**
//!   (`tag_names_for_keys_via`), never the key echoed back as the name, which
//!   would hide `TagNameDiffersFromKey`.
//!
//! **`enabled` is the column, stamped into the body before the grammar reads
//! it.** The download serves only `enabled = true` rows and stamps `isEnabled`
//! from that column (`PolicyStore::enabled_policies_for_service`), so the
//! body's own `isEnabled` decides nothing the engine sees. Stamping the column
//! makes the grammar's [`Blocker::Disabled`] fire exactly when the engine
//! enforces nothing — and a switched-off policy is NOT a migration candidate:
//! migrating it would turn an effect nobody has today into live grants. (A
//! policy already migrated is switched off by Task 7's write path, so this is
//! also what keeps a migrated original from being offered a second time.)
//!
//! **A migrated original is reported AS migrated, not as switched off**
//! (SP4d-3 Task 7). After a migration the original's `enabled` is `false`,
//! which is byte-for-byte what an operator switching it off by hand leaves,
//! and [`Blocker::Disabled`]'s advice for that state ("switch it on first")
//! would, for a migrated policy, re-enable a duplicate of what the model now
//! enforces. So the classifier reads `perm.migrated_from` and a row with
//! provenance gets [`ClassifiedPolicy::migrated`] and ONE sentence saying it
//! was migrated and how to undo it, in place of whatever the grammar says.

use std::collections::HashMap;

use egret_perm::{EngineType, Grant, Rule};
use egret_policy::{
    catalogs_for_service_via, get_service_by_id_via, tag_names_for_keys_via, PolicyStoreError,
    ServiceRow, TAG_DEF_NAME,
};
use serde_json::Value;
use uuid::Uuid;

use crate::grammar::{
    grants_from_access_policy, rules_from_mask_or_filter_policy, Blocker, ServiceKind,
};

/// One policy of the profile, as the classifier reports it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
pub struct ClassifiedPolicy {
    pub id: i64,
    pub name: String,
    /// `compiler` | `infra` | `operator` — `policy.authz_policies.managed_by`.
    pub managed_by: String,
    /// The `enabled` COLUMN — what decides whether the engine downloads it.
    pub enabled: bool,
    pub migratable: bool,
    /// Every reason the policy cannot be migrated, as operator-readable
    /// sentences naming the offending value. Empty iff `migratable`.
    pub blockers: Vec<String>,
    /// For a migratable policy, the model objects it would become.
    pub proposed: Option<ProposedObjects>,
    /// Present iff `perm.migrated_from` records this policy as migrated —
    /// which is what tells a migrated original apart from one switched off by
    /// hand (both have `enabled = false`). A migrated policy is never
    /// `migratable`, and its one blocker sentence says how to undo it.
    pub migrated: Option<MigrationRecord>,
    /// SP4d-4 Task 11: what this policy's resource levels mean to Ranger,
    /// independent of migration — a policy that stops above `column` and
    /// allows `select` grants no column read, and one with a gap decides
    /// nothing. Empty for every row that is neither a hand-written
    /// (`operator`) policy of a resource profile nor affected. **Flagged,
    /// never rewritten**: nothing here edits a stored row.
    pub warnings: Vec<PolicyWarning>,
}

/// One finding about a stored policy's resource levels (SP4d-4 Task 11).
/// `kind` is `select_stops_above_column` (with `level`: the deepest level the
/// policy names) or `level_gap` (with `missing`: the absent level that has a
/// named level beneath it); each carries the `sentence` an operator reads.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
pub struct PolicyWarning {
    pub kind: String,
    pub level: Option<String>,
    pub missing: Option<String>,
    pub sentence: String,
}

/// The `policyId`-keyed answer of `GET .../warnings`: how many warnings the
/// profile carries in all and which policies carry them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
pub struct ProfileWarnings {
    #[serde(rename = "warningCount")]
    pub warning_count: usize,
    pub policies: Vec<WarnedPolicy>,
}

/// One policy that carries at least one [`PolicyWarning`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
pub struct WarnedPolicy {
    pub id: i64,
    pub name: String,
    pub warnings: Vec<PolicyWarning>,
}

/// One `perm.migrated_from` row, as the classifier and the page read it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, utoipa::ToSchema, sqlx::FromRow)]
pub struct MigrationRecord {
    /// The policy's name when it was migrated (it may have been renamed
    /// since; provenance is keyed by policy id).
    pub policy_name: String,
    pub grant_ids: Vec<Uuid>,
    pub rule_ids: Vec<Uuid>,
    /// How many probe evaluations agreed when the migration was proved.
    pub probes_proved: i32,
    pub migrated_by: String,
    /// RFC 3339, UTC.
    pub migrated_at: String,
}

/// The model objects a migratable policy becomes, as the grammar produced
/// them (every id [`Uuid::nil`] until inserted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Candidates {
    Grants(Vec<Grant>),
    Rules(Vec<Rule>),
}

/// How many model objects a migratable policy becomes. No `clearances`:
/// clearance migration is out of scope ([`Blocker::ClearanceNotExpressible`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
pub struct ProposedObjects {
    pub grants: usize,
    pub rules: usize,
}

/// One `policy.authz_policies` row, as the classifier reads it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PolicyRecord {
    pub id: i64,
    pub name: String,
    pub managed_by: String,
    pub enabled: bool,
    pub policy_jsonb: Value,
}

/// Which grammar inputs a service row's `type` implies, or `None` for a type
/// egret's permission model does not compile to. See the module doc.
///
/// `named_as_tag_service` is whether some OTHER profile names this one as its
/// `tagService`. It makes the profile a tag service whatever its type (live
/// finding I5's class): the admin API creates only `trino`/`starrocks`
/// profiles, so a hand-made tag service is a `trino` row, and its policies are
/// served to each linker's engine as `tagPolicies`. `type = 'tag'` stays a tag
/// service on its own.
pub fn service_target(
    service_type: &str,
    named_as_tag_service: bool,
) -> Option<(ServiceKind, EngineType)> {
    if service_type == TAG_DEF_NAME || named_as_tag_service {
        return Some((ServiceKind::Tag, EngineType::Trino));
    }
    crate::trigger::engine_type_for(service_type).map(|e| (ServiceKind::Resource, e))
}

/// Classify one policy. Pure: every database fact arrives as an argument.
/// `target` is [`service_target`] of the profile; `Err` carries the profile's
/// type when it is not one egret compiles to. Never reports `migrated` — that
/// is a database fact [`ServiceInputs::classify`] adds.
pub fn classify_policy(
    rec: &PolicyRecord,
    target: Result<(ServiceKind, EngineType), &str>,
    catalog_ids: &dyn Fn(&str) -> Option<Uuid>,
    tag_names: &dyn Fn(&str) -> Option<String>,
) -> ClassifiedPolicy {
    classify_policy_objects(rec, target, catalog_ids, tag_names).0
}

/// [`classify_policy`], plus the model objects themselves for a migratable
/// policy — the ONE derivation both `classify` (which reports their counts)
/// and the migration (which inserts them) use, so the page's "it would become
/// 1 grant" and what the migration writes cannot be two answers.
pub fn classify_policy_objects(
    rec: &PolicyRecord,
    target: Result<(ServiceKind, EngineType), &str>,
    catalog_ids: &dyn Fn(&str) -> Option<Uuid>,
    tag_names: &dyn Fn(&str) -> Option<String>,
) -> (ClassifiedPolicy, Option<Candidates>) {
    let refuse = |blockers: Vec<String>| {
        (
            ClassifiedPolicy {
                id: rec.id,
                name: rec.name.clone(),
                managed_by: rec.managed_by.clone(),
                enabled: rec.enabled,
                migratable: false,
                blockers,
                proposed: None,
                migrated: None,
                warnings: level_warnings(rec, target),
            },
            None,
        )
    };

    match rec.managed_by.as_str() {
        "operator" => {}
        "compiler" => {
            return refuse(vec![
                "egret's permission compiler wrote this policy from a grant or rule in the \
                 permission model, so it is already managed there and is not a migration \
                 candidate. Change the grant or rule instead; the compiler rewrites this policy."
                    .to_string(),
            ])
        }
        "infra" => {
            return refuse(vec![
                "egret's own plumbing wrote this policy (engine delegation, admin access or \
                 query limits), so it is not a migration candidate and cannot be edited here. \
                 Removing or changing it would break how egret itself reaches the engine."
                    .to_string(),
            ])
        }
        other => {
            return refuse(vec![format!(
                "This policy's owner is recorded as `{other}`, which egret does not recognise \
                 (expected compiler, infra or operator), so it is not offered for migration."
            )])
        }
    }

    let (service, engine) = match target {
        Ok(t) => t,
        Err(service_type) => {
            return refuse(vec![format!(
                "This profile's type is `{service_type}`, which egret's permission model does \
                 not compile to (it compiles to trino and starrocks profiles and their tag \
                 services), so none of its policies can be migrated."
            )])
        }
    };

    // The engine reads `isEnabled` from the column, not from the stored body.
    let mut body = rec.policy_jsonb.clone();
    if let Value::Object(m) = &mut body {
        m.insert("isEnabled".into(), Value::Bool(rec.enabled));
    }

    // Masking (1) and row filter (2) go to the rule grammar; everything else —
    // access (0, the Ranger default when absent), an unknown type, or a type
    // that is not an integer — to the access grammar, which names the problem
    // (`NotAnAccessPolicy`, `Unreadable`) rather than dropping the row.
    let is_rule = matches!(
        body.get("policyType").and_then(Value::as_i64),
        Some(1) | Some(2)
    );
    let outcome = if is_rule {
        rules_from_mask_or_filter_policy(&body, engine, catalog_ids, tag_names, service)
            .map(|c| Candidates::Rules(c.rules))
    } else {
        grants_from_access_policy(&body, engine, catalog_ids, service)
            .map(|c| Candidates::Grants(c.grants))
    };

    match outcome {
        Ok(candidates) => {
            let proposed = match &candidates {
                Candidates::Grants(g) => ProposedObjects {
                    grants: g.len(),
                    rules: 0,
                },
                Candidates::Rules(r) => ProposedObjects {
                    grants: 0,
                    rules: r.len(),
                },
            };
            (
                ClassifiedPolicy {
                    id: rec.id,
                    name: rec.name.clone(),
                    managed_by: rec.managed_by.clone(),
                    enabled: rec.enabled,
                    migratable: true,
                    blockers: Vec::new(),
                    proposed: Some(proposed),
                    migrated: None,
                    warnings: level_warnings(rec, target),
                },
                Some(candidates),
            )
        }
        Err(blockers) => refuse(blockers.iter().map(render_blocker).collect()),
    }
}

/// The measurement every sentence below cites.
const LEVELS_RESULT_DOC: &str = "docs/en/plans/2026-10-11-sp4d-4-task1-levels-result.md";

/// Access types that, held by an item, allow `select`: `select` itself and any
/// access type whose `impliedGrants` include it — read from the embedded
/// service-def with the evaluator's own `implied_access_grants`, never
/// restated here. Computed once per engine.
fn select_granting_access_types(engine: EngineType) -> &'static [String] {
    use std::sync::OnceLock;
    static TRINO: OnceLock<Vec<String>> = OnceLock::new();
    static STARROCKS: OnceLock<Vec<String>> = OnceLock::new();
    let (cell, def) = match engine {
        EngineType::Trino => (&TRINO, egret_policy::trino_service_def()),
        EngineType::StarRocks => (&STARROCKS, egret_policy::starrocks_service_def()),
    };
    cell.get_or_init(|| {
        let mut out = vec!["select".to_string()];
        if let Ok(def) = def {
            let parsed = egret_ranger_eval::servicedef::ServiceDef::parse(&def);
            for (implier, implied) in parsed.implied_access_grants() {
                if implied.contains("select") && !out.iter().any(|o| o == implier) {
                    out.push(implier.to_string());
                }
            }
        }
        out
    })
}

/// Whether any ALLOW item of `body` grants `select` on `engine`.
fn allows_select(body: &Value, engine: EngineType) -> bool {
    let granting = select_granting_access_types(engine);
    body.get("policyItems")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|item| item.get("accesses").and_then(Value::as_array))
        .flatten()
        .filter(|a| a.get("isAllowed").and_then(Value::as_bool).unwrap_or(true))
        .filter_map(|a| a.get("type").and_then(Value::as_str))
        .any(|t| granting.iter().any(|g| g == t))
}

/// SP4d-4 Task 11: the per-policy findings about a stored policy's resource
/// levels. Pure. Only a hand-written (`operator`) policy of a RESOURCE profile
/// is examined — a compiled or infra row always carries every level it needs,
/// and a tag policy has none of these levels. The levels are read from the
/// primary `resources` map only.
///
/// - `level_gap`: a named level beneath an absent one. Ranger decides nothing
///   for such a policy at any depth (Task 1, `gap`).
/// - `select_stops_above_column`: an ACCESS policy whose chain ends above
///   `column` and whose allow items grant `select`. Both plugins check
///   `SELECT col` per column, so it grants no column read.
///
/// Nothing here rewrites a stored row; filling a level is the operator's edit.
fn level_warnings(
    rec: &PolicyRecord,
    target: Result<(ServiceKind, EngineType), &str>,
) -> Vec<PolicyWarning> {
    let Ok((ServiceKind::Resource, engine)) = target else {
        return Vec::new();
    };
    if rec.managed_by != "operator" {
        return Vec::new();
    }
    let policy_type = rec
        .policy_jsonb
        .get("policyType")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let all = crate::grants::resource_level_names(engine);
    let chain: &[&str] = match policy_type {
        0 | 1 => &all,
        2 => &all[..crate::grants::ROW_FILTER_LEVELS],
        _ => return Vec::new(),
    };
    let named = |name: &str| {
        rec.policy_jsonb
            .pointer(&format!("/resources/{name}/values"))
            .and_then(Value::as_array)
            .is_some_and(|vs| vs.iter().any(|v| v.as_str().is_some_and(|s| !s.is_empty())))
    };
    if !chain.iter().any(|n| named(n)) {
        return Vec::new();
    }

    let mut out = Vec::new();
    let mut first_absent: Option<&str> = None;
    let mut reported_gap_at: Option<&str> = None;
    for name in chain {
        match (named(name), first_absent) {
            (false, None) => first_absent = Some(name),
            (true, Some(missing)) if reported_gap_at != Some(missing) => {
                reported_gap_at = Some(missing);
                out.push(PolicyWarning {
                    kind: "level_gap".into(),
                    level: None,
                    missing: Some(missing.to_string()),
                    sentence: format!(
                        "The policy names `{name}` but not `{missing}`, the level above it. \
                         Ranger decides nothing for a policy with a gap in its resource \
                         levels (measured: {LEVELS_RESULT_DOC}), so this policy grants \
                         nothing at any depth. Name `{missing}` (use `*` for every {missing}) \
                         or remove `{name}`; the editor refuses to save such a gap."
                    ),
                });
            }
            _ => {}
        }
    }

    // Stops above `column`: the chain has no gap and ends before the last level.
    let last = chain.len() - 1;
    if policy_type == 0 && reported_gap_at.is_none() && !named(chain[last]) {
        if let Some(stops_at) = chain.iter().rev().find(|n| named(n)) {
            if allows_select(&rec.policy_jsonb, engine) {
                out.push(PolicyWarning {
                    kind: "select_stops_above_column".into(),
                    level: Some((*stops_at).to_string()),
                    missing: None,
                    sentence: format!(
                        "The policy stops at `{stops_at}` and allows `select`. Trino and \
                         StarRocks check `SELECT` on each column, and Ranger does not apply a \
                         policy that stops above `{last_level}` to {last_level}-level requests, \
                         so today it lets nobody read a column; it grants only the checks made \
                         at the depth it stops at (for example INSERT, DELETE, UPDATE, and \
                         Trino's `count(*)`). Measured on the real Ranger jars: \
                         {LEVELS_RESULT_DOC}. To grant the columns, set `{last_level}` to `*` \
                         in the editor — that extends the policy to every {last_level} under \
                         `{stops_at}` and moves it EARLIER in Ranger's evaluation order within \
                         its priority band (the broader policy is evaluated first, not the \
                         more specific one); the save names any policy it now precedes. \
                         egret does not rewrite stored policies.",
                        last_level = chain[last],
                    ),
                });
            }
        }
    }
    out
}

/// The one sentence a migrated original carries, in place of every other.
pub fn migrated_sentence(record: &MigrationRecord) -> String {
    let n_grants = record.grant_ids.len();
    let n_rules = record.rule_ids.len();
    format!(
        "This policy was migrated into egret's permission model on {at} by {by} ({n_grants} \
         grant(s), {n_rules} rule(s), proved on {probes} probe evaluations) and switched off; \
         the model now enforces what it did. Do not switch it back on by hand — that would \
         enforce it twice. To return to the hand-written policy, undo the migration, which \
         deletes those grants and rules and switches this policy back on.",
        at = record.migrated_at,
        by = record.migrated_by,
        probes = record.probes_proved,
    )
}

fn quoted(values: &[String]) -> String {
    values
        .iter()
        .map(|v| format!("`{v}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// One [`Blocker`] as a sentence an operator can act on, naming the offending
/// value. EXHAUSTIVE, with no catch-all arm, so a new variant fails to compile
/// here rather than reaching the page as an enum name. Item indices are shown
/// counting from 1, the way the editor lists items.
pub fn render_blocker(b: &Blocker) -> String {
    match b {
        Blocker::UnknownAccessType { access_type } => format!(
            "Access type `{access_type}` has no equivalent in egret's permission model on this \
             engine: egret grants only read and write, each a fixed set of Ranger access types, \
             and spelling is matched exactly (`SELECT` is not `select`). Remove `{access_type}` \
             from the policy, or keep the policy hand-written."
        ),
        Blocker::PartialActionExpansion { action, missing } => {
            let action = action.as_str();
            let missing = quoted(missing);
            format!(
                "The policy grants part of egret's `{action}` permission but not all of it: it \
                 lacks {missing}. egret's `{action}` always grants these together, so migrating \
                 would ADD {missing} — access nobody asked for. Add {missing} to the policy if \
                 that is intended, or keep the policy hand-written."
            )
        }
        Blocker::UnregisteredCatalog { catalog } => format!(
            "Catalog `{catalog}` is not a registered catalog that this profile's engines serve \
             (names are matched exactly, and `*` names no catalog). A migrated grant must point \
             at a catalog egret registers and this profile serves; a policy on any other catalog \
             enforces nothing here, and migrating it would grant on whichever profile does serve \
             it. Name a served catalog exactly, or keep the policy hand-written. (Attaching the \
             catalog to this profile's engine would also work, but it exposes the catalog to \
             that engine's users and starts enforcing every compiled grant on it there.)"
        ),
        Blocker::ResourceFlag { level, flag } => format!(
            "Resource level `{level}` sets `{flag}`; egret's permissions never exclude or recurse, \
             so this level has no equivalent. Clear `{flag}` on `{level}`, or keep the policy \
             hand-written."
        ),
        Blocker::TooManyExceptions { count } => format!(
            "The policy has {count} exception items in one bucket; egret can express at most one \
             exception for the allow items and one for the deny items (a grant's excluded \
             principals). Merge the exceptions into one, or keep the policy hand-written."
        ),
        Blocker::ExceptionAccessMismatch => "An exception item's access types differ from its \
             main item's. egret's excluded principals are excluded from exactly the access the \
             grant gives, so an exception covering a different set cannot be expressed. Make the \
             exception's access types match the main item's."
            .to_string(),
        Blocker::ValiditySchedules => "The policy has validity schedules (it is active only at \
             set times); egret's permissions are always active. Remove the schedule, or keep the \
             policy hand-written."
            .to_string(),
        Blocker::Conditions => "The policy, or one of its items, has conditions; egret's \
             permission model has no conditions. Remove them, or keep the policy hand-written."
            .to_string(),
        Blocker::NonDefaultPriority { priority } => format!(
            "The policy has priority {priority}; every policy egret writes has the normal \
             priority (0), so the override would be lost. Set the priority to normal, or keep \
             the policy hand-written."
        ),
        Blocker::EmptyLevel { level } => format!(
            "Resource level `{level}` has no value, or an empty-string value, which names \
             nothing. Name the resource, or use `*` for every value."
        ),
        Blocker::WildcardOverNamedLevel { level } => format!(
            "Level `{level}` is `*` while a level beneath it is named; egret refuses a grant \
             that names something under a wildcard (it would mean that name in every \
             `{level}`). Name `{level}` explicitly, or make every level beneath it `*` too."
        ),
        Blocker::UnknownTag { key } => format!(
            "The policy names tag `{key}`, and egret has no tag with that key. The migration \
             never creates tags: create a tag whose key and name are both `{key}`, or keep the \
             policy hand-written."
        ),
        Blocker::NothingToMigrate => "The policy has no allow items and no deny items, so it \
             grants and denies nothing; there is nothing to migrate."
            .to_string(),
        Blocker::Unreadable { error } => format!(
            "The policy body cannot be read as a Ranger policy ({error}). Fix the body in the \
             policy editor first."
        ),
        Blocker::NotAnAccessPolicy { policy_type } => format!(
            "The policy's type is {policy_type}; egret recognises access (0), masking (1) and \
             row-filter (2) policies only, so it cannot say what this one does."
        ),
        Blocker::Disabled => "This policy is switched off, so the engine does not download it \
             and it enforces nothing today. Migrating it would create grants or rules that \
             enforce immediately. (This policy has no migration on record — a migrated policy is \
             reported as migrated, not as switched off.) Switch it on first only if it is meant \
             to enforce."
            .to_string(),
        Blocker::DenyAllElse => "The policy denies everything its items do not allow \
             (`isDenyAllElse`); egret has no equivalent. Clear 'deny all else', or keep the \
             policy hand-written."
            .to_string(),
        Blocker::AdditionalResources => "The policy lists additional resources (more than one \
             resource set); an egret grant or rule addresses one. Split it into one policy per \
             resource set."
            .to_string(),
        Blocker::SecurityZone { zone } => format!(
            "The policy belongs to security zone `{zone}`; egret has no security zones. Keep \
             the policy hand-written."
        ),
        // Fix round 1 (Minor 1): the catalog is never omissible, and `*` names
        // no catalog, so the "set it to `*`" remedy below would be wrong here.
        Blocker::MissingLevel { level } if level == "catalog" => "The policy names no \
             `catalog` level. Every egret grant and rule names one catalog this profile serves \
             (and `*` names no catalog), so this policy cannot be migrated as it stands. Name \
             the served catalog it is meant to cover — which changes what it matches, so only \
             if that is the intent — or keep the policy hand-written."
            .to_string(),
        Blocker::MissingLevel { level } => format!(
            "The policy names no `{level}` level, so it stops above `{level}`, and Ranger does \
             not apply it to `{level}`-level requests (measured: {LEVELS_RESULT_DOC}). Today it \
             grants only the checks made at the depth it stops at (for example INSERT, DELETE \
             and UPDATE, and Trino's `count(*)`) — not reading `{level}`-level data. Ranger does \
             not read an absent level as `*`, so it cannot be migrated as it stands; see the \
             policy's warnings. Setting `{level}` to `*` in the editor makes it expressible, but \
             that extends the policy to every `{level}` under what it names and also moves it \
             EARLIER in Ranger's evaluation order within its priority band (the broader policy \
             is evaluated first, not the more specific one), so do it only if that is what the \
             policy is meant to grant."
        ),
        Blocker::UnknownLevel { level } => format!(
            "Resource `{level}` is not a level egret addresses for this engine and policy kind \
             (catalog, schema or database, table and column; a row filter stops at table; a tag \
             policy has only `tag`). Remove `{level}`, or keep the policy hand-written."
        ),
        Blocker::MultipleValues { level, count } => format!(
            "Level `{level}` lists {count} values; below the catalog egret names one value per \
             level (several catalogs are fine). Split it into one policy per `{level}` value."
        ),
        Blocker::ResourceExpression { level, value } => format!(
            "Value `{value}` at level `{level}` is a Ranger runtime expression, resolved per \
             request; egret matches literal names only. Replace it with the names it stands \
             for, or keep the policy hand-written."
        ),
        Blocker::RolePrincipal { role } => format!(
            "The policy names Ranger role `{role}`. egret never writes Ranger roles, and on \
             egret's engines that bucket matches nobody today, so migrating would make `{role}` \
             start matching people. Name the users or groups directly instead."
        ),
        Blocker::UserExpression { user } => format!(
            "User `{user}` is a Ranger runtime expression (such as {{USER}} or {{OWNER}}), \
             resolved per request; egret names users literally. Name the users, or keep the \
             policy hand-written."
        ),
        Blocker::UnusableUserRef { user } => format!(
            "User `{user}` cannot be an egret principal: egret refuses an empty user name and \
             one containing `@`. Use the engine user name, or keep the policy hand-written."
        ),
        Blocker::NoPrincipals => "An item or exception names no user, group or role, so it \
             applies to nobody. Name a principal, or delete the item."
            .to_string(),
        Blocker::EmptyAccesses => "An item or exception has no access types. Add access \
             types, or delete the item."
            .to_string(),
        Blocker::AccessNotAllowed { access_type } => format!(
            "Access type `{access_type}` is listed as not allowed (`isAllowed: false`); egret has \
             no equivalent for a listed-but-not-allowed access. Remove `{access_type}`, or \
             allow it."
        ),
        Blocker::DelegateAdmin => "An item grants delegated admin (the right to administer \
             this policy); egret has no equivalent. Clear delegate admin, or keep the policy \
             hand-written."
            .to_string(),
        Blocker::OrphanException => "The policy has an exception with no main item in the \
             same allow or deny set to make an exception to. Delete the exception, or add the \
             item it belongs to."
            .to_string(),
        Blocker::ForeignItems { bucket } => format!(
            "The policy carries `{bucket}` items, which do not belong to a policy of its type. \
             Remove them, or keep the policy hand-written."
        ),
        Blocker::UnrecognisedField { field } => format!(
            "Field `{field}` is one the migration has not ruled on, so it cannot vouch for what \
             the field does. Remove `{field}`, or keep the policy hand-written."
        ),
        Blocker::NotAMaskOrRowFilter { policy_type } => format!(
            "The policy's type is {policy_type}; egret recognises access (0), masking (1) and \
             row-filter (2) policies only, so it cannot say what this one does."
        ),
        Blocker::ClearanceNotExpressible => "This is a tag access policy. The only tag access \
             policy egret has is a clearance, whose cleared people are the members of egret \
             roles, worked out every time the policy is compiled; a hand-written list of users \
             cannot be turned back into roles, and a tag allow has no equivalent at all. \
             Migrating clearances is out of scope here: keep the policy hand-written, or recreate \
             it as a classification clearance in egret."
            .to_string(),
        Blocker::MaskWithoutType { index } => format!(
            "Mask item {} has no mask type. Ranger reads that as no mask, and in a tag service \
             such an item stops the engine's plugin from loading any policy. Choose a mask type.",
            index + 1
        ),
        Blocker::UnsupportedMaskType { mask_type } => format!(
            "Mask type `{mask_type}` is not one this engine supports; the engine ignores it and \
             serves the column unmasked. Choose a mask type the engine supports."
        ),
        Blocker::ValueExprWithoutCustom { mask_type } => format!(
            "A `{mask_type}` mask carries a custom expression; egret writes an expression only \
             for CUSTOM masks, so it would be dropped. Remove the expression, or change the type \
             to CUSTOM."
        ),
        Blocker::MaskExpression { expr } => format!(
            "Mask expression `{expr}` contains a Ranger request-time expression (`${{{{…}}}}`), \
             resolved per request; egret writes literal mask expressions only."
        ),
        Blocker::FilterExpression { expr } => format!(
            "Row filter `{expr}` contains something Ranger rewrites or resolves per request \
             (`${{{{…}}}}`, a `{{TOKEN}}` or `[[…]]`). egret refuses it: a filter read wrongly \
             leaves rows unfiltered. Write the filter literally, or keep the policy hand-written."
        ),
        Blocker::MissingFilterExpr { index } => format!(
            "Row-filter item {} has no filter expression, which Ranger reads as 'this item \
             decides nothing' — not the same as an empty filter. Set a filter expression, or \
             delete the item.",
            index + 1
        ),
        Blocker::TooManyRuleItems { count } => format!(
            "The policy has {count} items; an egret rule has at most two — an optional exemption \
             first, then one item for everyone. Reduce it to that shape, or keep the policy \
             hand-written."
        ),
        Blocker::NotForEveryone { index } => format!(
            "Item {} is in the 'everyone' position (the last item) but does not name exactly the \
             `public` group and nobody else. An egret rule applies to everyone; a mask or filter \
             for named principals only cannot be expressed.",
            index + 1
        ),
        Blocker::NotAnExemption { index } => format!(
            "Item {} comes before the 'everyone' item but does not lift the control (MASK_NONE \
             with no value, or an empty filter); egret has no per-principal masks or filters.",
            index + 1
        ),
        Blocker::PublicExemption => "The exemption item names the `public` group, which would \
             exempt everyone; egret refuses it. Name the exempt users or groups instead."
            .to_string(),
        Blocker::TagSelectorOnStarRocks => "The policy is addressed by tag on a StarRocks \
             profile. StarRocks has no tag service; egret compiles a tag rule there into one \
             policy per tagged column, a different artifact from this one, so it is not \
             migrated."
            .to_string(),
        Blocker::CompilerRefuses { error } => {
            format!("egret's compiler would refuse the rule this policy becomes: {error}")
        }
        // Fix round 1 (I1): renaming is NOT a safe default remedy — the engine
        // matches every tag policy on `tags.name`, and the name is not unique —
        // and rewriting the policy to name `{name}` is not equivalent (it would
        // cover a different tag's columns, and `{name}` is not a key anyway).
        Blocker::TagNameDiffersFromKey { key, name } => format!(
            "The policy names tag `{key}`. Ranger matches tag policies by tag name, so today it \
             covers whatever tag is named `{key}`; egret's tag with key `{key}` is named \
             `{name}`, so the migrated rule would cover the tag named `{name}` instead — a \
             different set of columns. Keep the policy hand-written. Renaming egret's tag \
             `{key}` from `{name}` to `{key}` would make the two agree, but it changes what \
             every tag policy that names `{name}` covers (those columns would lose their masks \
             or filters), and because tag names are not unique it can leave two tags named \
             `{key}`. Only rename after checking that no tag policy names `{name}` and no other \
             tag is named `{key}`."
        ),
        Blocker::TagBodyInResourceService => "The policy is addressed by tag but stored in a \
             resource profile, where Ranger matches nothing by tag, so it enforces nothing \
             today. Migrated, egret would file it in the tag service and it would start \
             enforcing. Move it to the tag service if it is meant to enforce, or delete it."
            .to_string(),
        Blocker::PathBodyInTagService => "The policy is addressed by catalog, schema and table \
             but stored in a tag service, where it matches nothing, so it enforces nothing \
             today. Migrated, egret would file it in the resource profile and it would start \
             enforcing. Move it to the resource profile if it is meant to enforce, or delete \
             it."
        .to_string(),
        Blocker::Internal { detail } => format!(
            "egret could not classify this policy: an internal check failed ({detail}). This \
             is a defect in egret, not in the policy — please report it. The policy is not \
             migrated."
        ),
    }
}

/// Everything classification reads from the database for one profile — the
/// ONE loader, used by `classify` and by the migration's transaction (which
/// re-classifies inside the transaction rather than trusting the client). See
/// the module doc for how each input is derived and why.
pub struct ServiceInputs {
    pub svc: ServiceRow,
    pub target: Option<(ServiceKind, EngineType)>,
    catalogs: HashMap<String, Uuid>,
    tags: HashMap<String, String>,
    /// Every policy of the profile, ordered by `id`.
    pub rows: Vec<PolicyRecord>,
    /// `perm.migrated_from` rows of this profile, by POLICY ID — a rename of
    /// the switched-off original must not orphan its marker (review I-3).
    pub migrated: HashMap<i64, MigrationRecord>,
}

impl ServiceInputs {
    /// Load the inputs for `service_id`. `NotFound` for an unknown profile.
    pub async fn load(
        conn: &mut sqlx::PgConnection,
        service_id: i64,
    ) -> Result<Self, PolicyStoreError> {
        let svc = get_service_by_id_via(conn, service_id).await?;
        // The OTHER profiles naming this one as `tagService`. Non-empty makes it
        // a tag service whatever its `type` (see [`service_target`]).
        let linked: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM policy.authz_services \
             WHERE config_jsonb ->> 'tagService' = $1 AND id <> $2",
        )
        .bind(&svc.name)
        .bind(service_id)
        .fetch_all(&mut *conn)
        .await?;
        let target = service_target(&svc.r#type, !linked.is_empty());

        // The catalog scope: this profile's engines, or — for a tag service —
        // the engines of every resource profile that names it (module doc).
        let mut scope_profiles = vec![service_id];
        if matches!(target, Some((ServiceKind::Tag, _))) {
            scope_profiles.extend(linked);
        }
        let mut catalogs: HashMap<String, Uuid> = HashMap::new();
        for id in scope_profiles {
            for c in catalogs_for_service_via(conn, id).await? {
                catalogs.insert(c.name, c.id);
            }
        }

        let rows: Vec<PolicyRecord> = sqlx::query_as(
            "SELECT id, name, managed_by, enabled, policy_jsonb FROM policy.authz_policies \
             WHERE service_id = $1 ORDER BY id",
        )
        .bind(service_id)
        .fetch_all(&mut *conn)
        .await?;

        // Every string a tag-addressed body names, resolved in one round trip.
        let mut keys: Vec<String> = rows
            .iter()
            .filter_map(|r| r.policy_jsonb.pointer("/resources/tag/values"))
            .filter_map(Value::as_array)
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        keys.sort();
        keys.dedup();
        let tags: HashMap<String, String> = tag_names_for_keys_via(conn, &keys)
            .await?
            .into_iter()
            .collect();

        let migrated: Vec<(i64, MigrationRecord)> =
            sqlx::query_as::<_, (i64, String, Vec<Uuid>, Vec<Uuid>, i32, String, String)>(
                "SELECT policy_id, policy_name, grant_ids, rule_ids, probes_proved, migrated_by, \
                    to_char(migrated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') \
               FROM perm.migrated_from WHERE service_id = $1",
            )
            .bind(service_id)
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .map(
                |(
                    policy_id,
                    policy_name,
                    grant_ids,
                    rule_ids,
                    probes_proved,
                    migrated_by,
                    migrated_at,
                )| {
                    (
                        policy_id,
                        MigrationRecord {
                            policy_name,
                            grant_ids,
                            rule_ids,
                            probes_proved,
                            migrated_by,
                            migrated_at,
                        },
                    )
                },
            )
            .collect();

        Ok(Self {
            svc,
            target,
            catalogs,
            tags,
            rows,
            migrated: migrated.into_iter().collect(),
        })
    }

    /// Classify one of [`Self::rows`], with its model objects when it is
    /// migratable. A policy with a migration on record is reported as
    /// migrated and never migratable, whatever the grammar says.
    pub fn classify(&self, rec: &PolicyRecord) -> (ClassifiedPolicy, Option<Candidates>) {
        let catalog_ids = |name: &str| self.catalogs.get(name).copied();
        let tag_names = |key: &str| self.tags.get(key).cloned();
        let target = self.target.ok_or(self.svc.r#type.as_str());
        let (mut classified, candidates) =
            classify_policy_objects(rec, target, &catalog_ids, &tag_names);
        if let Some(record) = self.migrated.get(&rec.id) {
            classified.migratable = false;
            classified.proposed = None;
            classified.blockers = vec![migrated_sentence(record)];
            classified.migrated = Some(record.clone());
            return (classified, None);
        }
        (classified, candidates)
    }
}

/// Classify every policy of `service_id`, ordered by `id`. `NotFound` for an
/// unknown profile.
pub async fn classify_service(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<Vec<ClassifiedPolicy>, PolicyStoreError> {
    let inputs = ServiceInputs::load(conn, service_id).await?;
    Ok(inputs.rows.iter().map(|r| inputs.classify(r).0).collect())
}

/// The profile's warnings (SP4d-4 Task 11): the policies of `service_id` that
/// carry at least one [`PolicyWarning`], and the total. Ordered by policy id.
pub async fn profile_warnings(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<ProfileWarnings, PolicyStoreError> {
    let policies: Vec<WarnedPolicy> = classify_service(conn, service_id)
        .await?
        .into_iter()
        .filter(|p| !p.warnings.is_empty())
        .map(|p| WarnedPolicy {
            id: p.id,
            name: p.name,
            warnings: p.warnings,
        })
        .collect();
    Ok(ProfileWarnings {
        warning_count: policies.iter().map(|p| p.warnings.len()).sum(),
        policies,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_service_type_decides_the_kind_and_engine() {
        assert_eq!(
            service_target("trino", false),
            Some((ServiceKind::Resource, EngineType::Trino))
        );
        assert_eq!(
            service_target("starrocks", false),
            Some((ServiceKind::Resource, EngineType::StarRocks))
        );
        assert_eq!(
            service_target("tag", false),
            Some((ServiceKind::Tag, EngineType::Trino))
        );
        assert_eq!(service_target("hive", false), None);
        // Named as some profile's `tagService`: a tag service whatever its type.
        assert_eq!(
            service_target("trino", true),
            Some((ServiceKind::Tag, EngineType::Trino))
        );
    }
}
