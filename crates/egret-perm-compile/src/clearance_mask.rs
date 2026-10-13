//! The StarRocks clearance MASK — view governance A2
//! (`docs/en/specs/2026-10-13-view-governance-design.md`).
//!
//! # Why a mask, beside the deny
//!
//! On StarRocks a clearance compiles to a per-column ACCESS deny
//! ([`crate::clearance`]). That withholds the column from a DIRECT read, and
//! nothing else: a plain view is an access boundary, so a reader holding the
//! view grant reads the classified column straight through the base deny
//! (measured on 4.1.4 and 4.1.6). A base-column data MASK, by contrast,
//! applies through every view kind, for every reader, and leaks nothing through
//! predicates, aggregates or joins. So each classified column also gets one
//! `policyType 1` policy whose `{public}` item is `MASK_NULL`.
//!
//! `MASK_HASH` is never the public type: it leaks equality, cardinality and,
//! with a guess, content.
//!
//! # Why it is FOLDED, and through the evaluator
//!
//! Ranger stops at the first matching item of the first applicable data-mask
//! policy, and a data-mask policy has no exceptions. The clearance mask has
//! `policyPriority 1` so it always comes before the rule masks (compiled at 0),
//! which means it decides for EVERYONE who reaches it — cleared users included.
//! It must therefore reproduce, for each cleared user, exactly what the rule
//! masks would have given that user. That answer is not re-derived here: it is
//! asked of `egret-ranger-eval`, egret's model of the plugin's own data-mask
//! evaluation, proved against the real jars, over the profile's OTHER data-mask
//! policies (the desired set without the clearance masks, plus the profile's
//! enabled hand-written masks). Users with the same outcome share one item.
//!
//! **Refuse, never guess.** An `Undecidable` outcome for any cleared user — a
//! same-priority tie between two rule masks, a construct the evaluator does not
//! model — refuses the compile, naming the column and the user
//! ([`CompileError::ClearanceMaskUndecidable`]). So does an outcome the
//! clearance item cannot reproduce: a mask condition, or a value expression on
//! a type other than `CUSTOM` (the renderer writes `valueExpr` only for
//! `CUSTOM`).
//!
//! # Why priority 1 does not trip SP4d-4's overlap refusal
//!
//! Two policies at different `policyPriority` are ORDERED, not ambiguous
//! (`egret_policy::ambiguous_arbitration`): the comparator sorts priority
//! first, so the clearance mask always wins and the rule mask beneath it is
//! never consulted for that column.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use uuid::Uuid;

use egret_perm::{CompileError, EngineType};
use egret_policy::admin::{masking_to_ranger_json, MaskItemInput, MaskingPolicyInput};
use egret_policy::COMPILER_CLEARANCE_PREFIX;
use egret_ranger_eval::eval::{
    evaluate_data_mask_prepared, AccessRequest, EngineConfig, MaskDecision, PluginVersion,
    PreparedPolicySet,
};
use egret_ranger_eval::model::ServicePolicySet;
use egret_ranger_eval::resource::RequestResource;
use egret_ranger_eval::servicedef::ServiceDef;

use crate::grants::{resource_level_names, PolicyTarget};
use crate::rules::{column_resources, stamp_service_type, TaggedColumn, PUBLIC_GROUP};

/// The clearance mask's type for everyone not cleared. Never `MASK_HASH`.
pub(crate) const CLEARANCE_MASK_TYPE: &str = "MASK_NULL";

/// One cleared user's folded outcome: the mask type and, for `CUSTOM`, its
/// value expression.
type Outcome = (String, Option<String>);

/// What a cleared user whose rule masks transform nothing gets.
const NO_TRANSFORM: &str = "MASK_NONE";

/// The clearance mask's name: `perm-clearance-{tag_id}-mask-{c}.{d}.{t}.{col}`.
/// Under [`COMPILER_CLEARANCE_PREFIX`], so the recompile sweep owns it and the
/// editor's reserved-name guard refuses a human write to it.
pub(crate) fn clearance_mask_name(tag_id: Uuid, col: &TaggedColumn) -> String {
    format!(
        "{COMPILER_CLEARANCE_PREFIX}{tag_id}-mask-{}.{}.{}.{}",
        col.catalog, col.schema, col.table, col.column
    )
}

fn column_label(col: &TaggedColumn) -> String {
    format!(
        "{}.{}.{}.{}",
        col.catalog, col.schema, col.table, col.column
    )
}

/// One clearance mask still to fold: a classified column and the users cleared
/// for EVERY gated classification on it. Built by [`merge_by_column`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingMask {
    pub(crate) tag_id: Uuid,
    pub(crate) column: TaggedColumn,
    pub(crate) cleared: Vec<String>,
}

/// One mask per column. A column carrying several gated classifications gets
/// ONE clearance mask whose cleared set is the INTERSECTION of theirs (a user
/// must be cleared for every classification on the column to see it, which is
/// what the per-tag denies already enforce on a direct read). Two masks on one
/// column at the same priority with different items would be an
/// order-ambiguous pair, and either winner would clear a user for a
/// classification they do not hold. The name carries the lowest tag id.
pub(crate) fn merge_by_column(pending: Vec<PendingMask>) -> Vec<PendingMask> {
    type Key = (String, String, String, String);
    let mut by_col: BTreeMap<Key, PendingMask> = BTreeMap::new();
    for p in pending {
        let key = (
            p.column.catalog.clone(),
            p.column.schema.clone(),
            p.column.table.clone(),
            p.column.column.clone(),
        );
        match by_col.get_mut(&key) {
            None => {
                by_col.insert(key, p);
            }
            Some(have) => {
                let theirs: BTreeSet<&String> = p.cleared.iter().collect();
                have.cleared.retain(|u| theirs.contains(u));
                if p.tag_id < have.tag_id {
                    have.tag_id = p.tag_id;
                }
            }
        }
    }
    by_col.into_values().collect()
}

/// The parsed StarRocks/Trino service-def the evaluator reads.
fn service_def(engine: EngineType) -> Result<ServiceDef, CompileError> {
    let def = match engine {
        EngineType::Trino => egret_policy::trino_service_def(),
        EngineType::StarRocks => egret_policy::starrocks_service_def(),
    }
    .map_err(|e| CompileError::ServiceDefUnreadable {
        engine,
        detail: e.to_string(),
    })?;
    Ok(ServiceDef::parse(&def))
}

/// Build the clearance mask for one column.
///
/// - `others`: the profile's other data-mask policy bodies (`policyType 1`),
///   never a clearance mask.
/// - `cleared`: the users cleared for the column, as bare usernames.
///
/// Items, in order: one per distinct outcome among the cleared users (sorted by
/// the group's first username), then `{public}` `MASK_NULL`. Priority 1.
#[allow(clippy::too_many_arguments)]
pub(crate) fn fold_clearance_mask(
    name: &str,
    service: &str,
    column: &TaggedColumn,
    cleared: &[String],
    others: &[Value],
    engine: EngineType,
    plugin: PluginVersion,
) -> Result<Value, CompileError> {
    let def = service_def(engine)?;
    let set: ServicePolicySet = serde_json::from_value(serde_json::json!({
        "serviceName": service,
        "policies": others,
    }))
    .map_err(|e| CompileError::Db(format!("clearance mask fold: unreadable policy set: {e}")))?;
    let prepared = PreparedPolicySet::new(&set, &def);
    let config = EngineConfig {
        engine: Some(crate::routes::ranger_engine(engine)),
        plugin_version: Some(plugin),
        ..EngineConfig::without_super_users()
    };

    let resources = column_resources(engine, column, PolicyTarget::Mask);
    let mut resource = RequestResource::new();
    for level in resource_level_names(engine) {
        if let Some(v) = resources.get(level).and_then(|l| l.values.first()) {
            resource.insert(level, v.clone());
        }
    }

    let users: BTreeSet<&String> = cleared.iter().collect();
    // (type, valueExpr) → users, in first-user order.
    let mut groups: Vec<(Outcome, Vec<String>)> = Vec::new();
    for user in users {
        let request = AccessRequest {
            user: user.clone(),
            // No group provider and no role download on the engines egret
            // provisions: `public` is the only group that can match, and it
            // matches regardless (`crate::clearance`'s D0 notes).
            groups: BTreeSet::new(),
            roles: BTreeSet::new(),
            access_type: "select".to_string(),
            resource: resource.clone(),
            owner: None,
            // StarRocks has no tag service, so no tag can reach a mask here.
            resolved_tags: None,
        };
        let refuse = || CompileError::ClearanceMaskUndecidable {
            column: column_label(column),
            user: user.clone(),
        };
        let outcome: Outcome = match evaluate_data_mask_prepared(&prepared, &request, &config) {
            MaskDecision::Undecidable { why } => {
                tracing::warn!(column = %column_label(column), user = %user, ?why,
                        "clearance mask fold refused: the evaluator cannot decide this user's mask");
                return Err(refuse());
            }
            MaskDecision::NoMask => (NO_TRANSFORM.to_string(), None),
            MaskDecision::Masked(m) => {
                if m.mask_condition.is_some() {
                    return Err(refuse());
                }
                match m.mask_type.as_deref() {
                    // A null type determined the result and transforms
                    // nothing: the same observable answer as MASK_NONE.
                    None | Some(NO_TRANSFORM) => (NO_TRANSFORM.to_string(), None),
                    Some("CUSTOM") => ("CUSTOM".to_string(), m.masked_value.clone()),
                    Some(t) if m.masked_value.is_none() => (t.to_string(), None),
                    Some(_) => return Err(refuse()),
                }
            }
        };
        match groups.iter_mut().find(|(o, _)| *o == outcome) {
            Some((_, us)) => us.push(user.clone()),
            None => groups.push((outcome, vec![user.clone()])),
        }
    }

    let mut items: Vec<MaskItemInput> = groups
        .into_iter()
        .map(|((mask_type, value_expr), users)| MaskItemInput {
            users,
            groups: Vec::new(),
            mask_type,
            value_expr,
        })
        .collect();
    items.push(MaskItemInput {
        users: Vec::new(),
        groups: vec![PUBLIC_GROUP.to_string()],
        mask_type: CLEARANCE_MASK_TYPE.to_string(),
        value_expr: None,
    });

    let input = MaskingPolicyInput {
        kind: "masking".to_string(),
        name: name.to_string(),
        service: service.to_string(),
        is_enabled: true,
        resources,
        items,
        description: String::new(),
        labels: Vec::new(),
        // policyPriority 1: ahead of every rule mask (priority 0), and ordered
        // rather than ambiguous beside them.
        is_override: true,
    };
    Ok(stamp_service_type(masking_to_ranger_json(&input), engine))
}

/// [`fold_clearance_mask`] at every plugin version in `plugins`, refusing when
/// they disagree: one download is served to every bound engine.
#[allow(clippy::too_many_arguments)]
pub(crate) fn fold_at_versions(
    profile: &str,
    name: &str,
    column: &TaggedColumn,
    cleared: &[String],
    others: &[Value],
    engine: EngineType,
    plugins: &[PluginVersion],
) -> Result<Value, CompileError> {
    let mut folded: Option<Value> = None;
    for &plugin in plugins {
        let body = fold_clearance_mask(name, profile, column, cleared, others, engine, plugin)?;
        match &folded {
            None => folded = Some(body),
            Some(first) if *first == body => {}
            Some(_) => {
                return Err(CompileError::ClearanceMaskDiffersByPluginVersion {
                    profile: profile.to_string(),
                    column: column_label(column),
                })
            }
        }
    }
    folded.ok_or_else(|| CompileError::ClearanceMaskEngineUnmodelled {
        profile: profile.to_string(),
        reason: "no plugin version to fold at".to_string(),
    })
}

/// The Ranger plugin versions a profile's clearance masks are folded at: each
/// distinct version of its bound engines, or — with no engine bound — the
/// plugin of egret's default image for the engine type, which is the version
/// `explain` evaluates an unbound profile at. An engine whose (type, version)
/// pair the evaluator does not model refuses, naming the engine.
pub(crate) async fn fold_plugin_versions(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    profile: &str,
    engine: EngineType,
) -> Result<Vec<PluginVersion>, CompileError> {
    let bound: Vec<crate::routes::BoundEngine> = sqlx::query_as(
        "SELECT name, origin, type, ranger_plugin_version, spec FROM engine_mgmt.engines \
          WHERE authz_service_id = $1 ORDER BY name",
    )
    .bind(service_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| CompileError::Db(e.to_string()))?;
    let unmodelled = |reason: String| CompileError::ClearanceMaskEngineUnmodelled {
        profile: profile.to_string(),
        reason,
    };
    let mut out: Vec<PluginVersion> = Vec::new();
    for e in &bound {
        let v = e.plugin_version().map_err(unmodelled)?;
        if !out.contains(&v) {
            out.push(v);
        }
    }
    if out.is_empty() {
        out.push(
            crate::routes::default_image_plugin_version(crate::routes::ranger_engine(engine))
                .map_err(unmodelled)?,
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SERVICE: &str = "sr_profile";

    fn ssn() -> TaggedColumn {
        TaggedColumn {
            catalog: "sales".into(),
            schema: "s".into(),
            table: "t".into(),
            column: "ssn".into(),
        }
    }

    fn cleared(users: &[&str]) -> Vec<String> {
        users.iter().map(|u| u.to_string()).collect()
    }

    /// A compiled-shape StarRocks mask policy on `sales.s.t.{column}`.
    fn mask_policy(name: &str, column: &str, priority: i64, items: Value) -> Value {
        json!({
            "name": name, "service": SERVICE, "serviceType": "starrocks",
            "policyType": 1, "isEnabled": true, "policyPriority": priority,
            "resources": {
                "catalog":  {"values": ["sales"], "isExcludes": false, "isRecursive": false},
                "database": {"values": ["s"], "isExcludes": false, "isRecursive": false},
                "table":    {"values": ["t"], "isExcludes": false, "isRecursive": false},
                "column":   {"values": [column], "isExcludes": false, "isRecursive": false},
            },
            "dataMaskPolicyItems": items,
            "policyItems": [], "denyPolicyItems": [],
        })
    }

    fn item(users: &[&str], groups: &[&str], mask_type: &str) -> Value {
        json!({
            "users": users, "groups": groups, "roles": [],
            "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": mask_type},
        })
    }

    /// `(users, groups, type)` of each `dataMaskPolicyItems` entry, in order.
    fn items_of(body: &Value) -> Vec<(Vec<String>, Vec<String>, String)> {
        let strs = |v: &Value| -> Vec<String> {
            v.as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap().to_string())
                .collect()
        };
        body["dataMaskPolicyItems"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| {
                (
                    strs(&i["users"]),
                    strs(&i["groups"]),
                    i["dataMaskInfo"]["dataMaskType"]
                        .as_str()
                        .unwrap()
                        .to_string(),
                )
            })
            .collect()
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn fold(cleared_users: &[&str], others: &[Value]) -> Result<Value, CompileError> {
        fold_clearance_mask(
            &clearance_mask_name(Uuid::nil(), &ssn()),
            SERVICE,
            &ssn(),
            &cleared(cleared_users),
            others,
            EngineType::StarRocks,
            PluginVersion::Ranger2_9_0,
        )
    }

    #[test]
    fn cleared_users_get_what_the_rule_masks_give_them_and_everyone_else_gets_null() {
        // Arrange: a WILDCARD mask on every column of the table — MASK_HASH
        // for ann, redact (`MASK`) for {public} — and an exact-column mask on
        // a sibling column that must not reach ssn. (The brief's two-policy
        // version of this — an exact MASK_HASH for ann beside a wildcard MASK
        // for {public}, both at priority 0 — is an order-ambiguous pair for
        // ann; see `an_exact_and_a_wildcard_rule_mask_differing_for_one_user_refuses`.)
        let others = vec![
            mask_policy(
                "perm-rule-a",
                "*",
                0,
                json!([
                    item(&["ann"], &[], "MASK_HASH"),
                    item(&[], &["public"], "MASK")
                ]),
            ),
            mask_policy(
                "perm-rule-b",
                "email",
                0,
                json!([item(&[], &["public"], "MASK_NONE")]),
            ),
        ];

        // Act
        let body = fold(&["ann", "bob"], &others).expect("fold");

        // Assert
        assert_eq!(
            items_of(&body),
            vec![
                (s(&["ann"]), s(&[]), "MASK_HASH".to_string()),
                (s(&["bob"]), s(&[]), "MASK".to_string()),
                (s(&[]), s(&["public"]), "MASK_NULL".to_string()),
            ]
        );
        assert_eq!(body["policyPriority"], json!(1));
        assert_eq!(body["policyType"], json!(1));
        assert_eq!(body["serviceType"], json!("starrocks"));
        assert_eq!(body["service"], json!(SERVICE));
        assert_eq!(
            body["name"],
            json!(format!(
                "{COMPILER_CLEARANCE_PREFIX}{}-mask-sales.s.t.ssn",
                Uuid::nil()
            ))
        );
        let res = &body["resources"];
        assert_eq!(res["catalog"]["values"], json!(["sales"]));
        assert_eq!(res["database"]["values"], json!(["s"]));
        assert_eq!(res["table"]["values"], json!(["t"]));
        assert_eq!(res["column"]["values"], json!(["ssn"]));
        assert_eq!(res.as_object().unwrap().len(), 4);
    }

    #[test]
    fn users_with_the_same_outcome_share_one_item() {
        let others = vec![mask_policy(
            "perm-rule-b",
            "*",
            0,
            json!([item(&[], &["public"], "MASK")]),
        )];
        let body = fold(&["cat", "ann", "bob"], &others).expect("fold");
        assert_eq!(
            items_of(&body),
            vec![
                (s(&["ann", "bob", "cat"]), s(&[]), "MASK".to_string()),
                (s(&[]), s(&["public"]), "MASK_NULL".to_string()),
            ]
        );
    }

    #[test]
    fn a_cleared_user_no_rule_masks_gets_mask_none() {
        // A mask on ANOTHER column does not reach ssn.
        let others = vec![mask_policy(
            "perm-rule-c",
            "email",
            0,
            json!([item(&[], &["public"], "MASK")]),
        )];
        let body = fold(&["bob"], &others).expect("fold");
        assert_eq!(
            items_of(&body),
            vec![
                (s(&["bob"]), s(&[]), "MASK_NONE".to_string()),
                (s(&[]), s(&["public"]), "MASK_NULL".to_string()),
            ]
        );
    }

    #[test]
    fn a_custom_outcome_keeps_its_value_expression() {
        let mut custom = item(&[], &["public"], "CUSTOM");
        custom["dataMaskInfo"]["valueExpr"] = json!("concat('x', {col})");
        let others = vec![mask_policy("perm-rule-d", "ssn", 0, json!([custom]))];
        let body = fold(&["ann"], &others).expect("fold");
        let first = &body["dataMaskPolicyItems"][0];
        assert_eq!(first["users"], json!(["ann"]));
        assert_eq!(first["dataMaskInfo"]["dataMaskType"], json!("CUSTOM"));
        assert_eq!(
            first["dataMaskInfo"]["valueExpr"],
            json!("concat('x', {col})")
        );
    }

    #[test]
    fn an_undecidable_cleared_user_refuses_the_compile() {
        // Two priority-0 rule masks with different types on the same column,
        // both matching ann: a differing tie.
        let others = vec![
            mask_policy(
                "perm-rule-e",
                "ssn",
                0,
                json!([item(&[], &["public"], "MASK")]),
            ),
            mask_policy(
                "perm-rule-f",
                "ssn",
                0,
                json!([item(&[], &["public"], "MASK_NULL")]),
            ),
        ];
        let err = fold(&["ann"], &others).expect_err("undecidable must refuse");
        assert_eq!(
            err,
            CompileError::ClearanceMaskUndecidable {
                column: "sales.s.t.ssn".to_string(),
                user: "ann".to_string(),
            }
        );
    }

    #[test]
    fn an_exact_and_a_wildcard_rule_mask_differing_for_one_user_refuses() {
        // Both priority 0, both match ann, different types: which one Ranger
        // applies to ann is decided by eval order, which the evaluator does not
        // compute — and SP4d-4's overlap guard refuses the same pair. bob
        // matches only the wildcard, but ann alone refuses the whole fold.
        let others = vec![
            mask_policy(
                "perm-rule-a",
                "ssn",
                0,
                json!([item(&["ann"], &[], "MASK_HASH")]),
            ),
            mask_policy(
                "perm-rule-b",
                "*",
                0,
                json!([item(&[], &["public"], "MASK")]),
            ),
        ];
        assert_eq!(
            fold(&["ann", "bob"], &others),
            Err(CompileError::ClearanceMaskUndecidable {
                column: "sales.s.t.ssn".to_string(),
                user: "ann".to_string(),
            })
        );
        // Without ann, bob's outcome is decidable.
        assert_eq!(
            items_of(&fold(&["bob"], &others).expect("fold"))[0],
            (s(&["bob"]), s(&[]), "MASK".to_string())
        );
    }

    #[test]
    fn no_cleared_users_gives_only_the_public_null_item() {
        let others = vec![mask_policy(
            "perm-rule-b",
            "*",
            0,
            json!([item(&[], &["public"], "MASK")]),
        )];
        let body = fold(&[], &others).expect("fold");
        assert_eq!(
            items_of(&body),
            vec![(s(&[]), s(&["public"]), "MASK_NULL".to_string())]
        );
    }

    #[test]
    fn never_emits_mask_hash_for_public() {
        // Even when the rule mask hashes everybody, the uncleared get NULL.
        let others = vec![mask_policy(
            "perm-rule-g",
            "ssn",
            0,
            json!([item(&[], &["public"], "MASK_HASH")]),
        )];
        let body = fold(&["ann"], &others).expect("fold");
        let items = items_of(&body);
        let public: Vec<_> = items
            .iter()
            .filter(|(_, g, _)| g.contains(&"public".to_string()))
            .collect();
        assert_eq!(public.len(), 1);
        assert_eq!(public[0].2, "MASK_NULL");
        assert_eq!(items.last().unwrap().1, s(&["public"]));
    }

    #[test]
    fn a_column_with_two_classifications_folds_once_with_the_intersection() {
        let a = Uuid::from_u128(2);
        let b = Uuid::from_u128(1);
        let merged = merge_by_column(vec![
            PendingMask {
                tag_id: a,
                column: ssn(),
                cleared: cleared(&["ann", "bob"]),
            },
            PendingMask {
                tag_id: b,
                column: ssn(),
                cleared: cleared(&["bob", "cat"]),
            },
        ]);
        assert_eq!(
            merged,
            vec![PendingMask {
                tag_id: b,
                column: ssn(),
                cleared: cleared(&["bob"]),
            }]
        );
    }
}
