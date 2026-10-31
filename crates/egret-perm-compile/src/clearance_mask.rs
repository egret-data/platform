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
//! **Skip, never guess, when the cause is not the steward's policies** (user
//! ruling 2026-10-13). When a bound engine runs a plugin the evaluator does not
//! model, when bound plugin versions fold differently, or when a mask that
//! reaches the column names a group other than `public` or any role (the fold
//! evaluates a cleared user with no groups and no roles), no clearance mask is
//! emitted for the column. The per-column deny is compiled as before, so a
//! direct read stays withheld, and the recompile reports a `SkippedItem`
//! (`clearance`, the tag id) saying view protection is not active and why.
//! Binding such an engine is not refused.
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
    /// `column` is spelled as the catalog spells it (read from the column
    /// source), rather than as an assignment was typed.
    pub(crate) catalog_spelled: bool,
}

/// One mask per column. A column carrying several gated classifications gets
/// ONE clearance mask whose cleared set is the INTERSECTION of theirs (a user
/// must be cleared for every classification on the column to see it, which is
/// what the per-tag denies already enforce on a direct read). Two masks on one
/// column at the same priority with different items would be an
/// order-ambiguous pair, and either winner would clear a user for a
/// classification they do not hold. The name carries the lowest tag id.
///
/// The key is FOLDED (spec C5): Ranger matches every StarRocks level
/// case-insensitively, so `Orders.Email` and `orders.email` are one column,
/// and two masks on it would be that same refused tie. The merged mask keeps
/// the catalog's spelling when one of its inputs carries it, else the first
/// spelling in `pending`'s order.
pub(crate) fn merge_by_column(pending: Vec<PendingMask>) -> Vec<PendingMask> {
    use egret_ranger_eval::resource::fold_identifier as fold;
    type Key = (String, String, String, String);
    let mut by_col: BTreeMap<Key, PendingMask> = BTreeMap::new();
    for p in pending {
        let key = (
            fold(&p.column.catalog),
            fold(&p.column.schema),
            fold(&p.column.table),
            fold(&p.column.column),
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
                if p.catalog_spelled && !have.catalog_spelled {
                    have.column = p.column;
                    have.catalog_spelled = true;
                }
            }
        }
    }
    by_col.into_values().collect()
}

/// The request resource for one fully-named column: exactly the levels the
/// clearance mask itself addresses.
fn column_request(engine: EngineType, column: &TaggedColumn) -> RequestResource {
    let resources = column_resources(engine, column, PolicyTarget::Mask);
    let mut resource = RequestResource::new();
    for level in resource_level_names(engine) {
        if let Some(v) = resources.get(level).and_then(|l| l.values.first()) {
            resource.insert(level, v.clone());
        }
    }
    resource
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
    let resource = column_request(engine, column);

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

/// What folding one column's clearance mask came to.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Folded {
    /// The priority-1 mask body to serve.
    Mask(Value),
    /// No mask can be computed for a reason outside the policies the steward
    /// wrote about this user (an engine egret has not modelled, plugin versions
    /// that fold differently, a group or role the fold cannot resolve). The
    /// per-column DENY is still emitted, so a direct read stays withheld; only
    /// the view path is unprotected, and the reason says so (user ruling
    /// 2026-10-13, Task 1 fix round 1).
    Skipped(String),
}

/// The skip reason when a data-mask policy that can reach `column` names a
/// group other than `public`, or any role. The fold evaluates a cleared user
/// with NO groups and NO roles (egret runs no usersync and serves no role
/// download), so it cannot know what an engine that does resolve them would
/// give that user; evaluating anyway would be a guess in one direction or the
/// other. Only policies whose resources match the column (`Self_` or
/// `SelfAndAllDescendants`, the matches a mask evaluation admits) are looked
/// at, and only enabled ones.
pub(crate) fn principal_gap(
    profile: &str,
    column: &TaggedColumn,
    others: &[Value],
    engine: EngineType,
) -> Result<Option<String>, CompileError> {
    use egret_ranger_eval::resource::{match_policy_resources, MatchType};
    let def = service_def(engine)?;
    let set: ServicePolicySet = serde_json::from_value(serde_json::json!({
        "serviceName": profile,
        "policies": others,
    }))
    .map_err(|e| CompileError::Db(format!("clearance mask fold: unreadable policy set: {e}")))?;
    let request = column_request(engine, column);
    for p in set.policies.iter().filter(|p| p.is_enabled) {
        let reaches = match_policy_resources(p, &request, &def).iter().any(|m| {
            matches!(
                m.match_type,
                MatchType::Self_ | MatchType::SelfAndAllDescendants
            )
        });
        if !reaches {
            continue;
        }
        for item in &p.data_mask_policy_items {
            let what = item
                .groups
                .iter()
                .find(|g| g.as_str() != PUBLIC_GROUP)
                .map(|g| format!("group {g:?}"))
                .or_else(|| item.roles.first().map(|r| format!("role {r:?}")));
            if let Some(what) = what {
                return Ok(Some(format!(
                    "view protection (clearance mask) not active for {profile} on column {}: \
                     data-mask policy {:?} names {what}, which egret does not resolve the way \
                     an engine with a group or role provider would, so what a cleared user is \
                     served there cannot be folded — name users in that policy instead",
                    column_label(column),
                    p.name
                )));
            }
        }
    }
    Ok(None)
}

/// [`fold_clearance_mask`] at every plugin version in `plugins`. Versions that
/// fold differently are a skip, not a refusal: one download is served to every
/// bound engine, so no single body is right for all of them, and the cause is
/// the engines bound, not anything a steward wrote.
#[allow(clippy::too_many_arguments)]
pub(crate) fn fold_at_versions(
    profile: &str,
    name: &str,
    column: &TaggedColumn,
    cleared: &[String],
    others: &[Value],
    engine: EngineType,
    plugins: &[PluginVersion],
) -> Result<Folded, CompileError> {
    if let Some(reason) = principal_gap(profile, column, others, engine)? {
        return Ok(Folded::Skipped(reason));
    }
    let mut folded: Option<Value> = None;
    for &plugin in plugins {
        let body = fold_clearance_mask(name, profile, column, cleared, others, engine, plugin)?;
        match &folded {
            None => folded = Some(body),
            Some(first) if *first == body => {}
            Some(_) => {
                return Ok(Folded::Skipped(format!(
                    "view protection (clearance mask) not active for {profile} on column {}: \
                     the engines bound to it run Ranger plugin versions at which the mask folds \
                     differently, and one download serves them all — bind engines of one plugin \
                     version (see docs/en/guides/engine-versions.md)",
                    column_label(column)
                )))
            }
        }
    }
    Ok(match folded {
        Some(body) => Folded::Mask(body),
        None => Folded::Skipped(format!(
            "view protection (clearance mask) not active for {profile}: no plugin version to \
             fold at"
        )),
    })
}

/// The plugin versions a profile's clearance masks can be folded at, or why
/// none can.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FoldVersions {
    Modelled(Vec<PluginVersion>),
    /// The skip reason, naming the engine and the remedy.
    Unmodelled(String),
}

/// The Ranger plugin versions a profile's clearance masks are folded at: each
/// distinct version of its bound engines, or — with no engine bound — the
/// plugin of egret's default image for the engine type, which is the version
/// `explain` evaluates an unbound profile at. An engine whose (type, version)
/// pair the evaluator does not model makes the masks a SKIP for the profile,
/// naming the engine (user ruling 2026-10-13): binding such an engine is not
/// refused, and the per-column denies are still compiled.
pub(crate) async fn fold_plugin_versions(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    profile: &str,
    engine: EngineType,
) -> Result<FoldVersions, CompileError> {
    let bound: Vec<crate::routes::BoundEngine> = sqlx::query_as(
        "SELECT name, origin, type, ranger_plugin_version, spec FROM engine_mgmt.engines \
          WHERE authz_service_id = $1 ORDER BY name",
    )
    .bind(service_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(CompileError::from_db)?;
    let unmodelled = |name: &str, why: String| {
        FoldVersions::Unmodelled(format!(
            "view protection (clearance mask) not active for {profile}: engine {name} runs an \
             image egret has not modelled — pin it to a known image (see \
             docs/en/guides/engine-versions.md). {why}"
        ))
    };
    let mut out: Vec<PluginVersion> = Vec::new();
    for e in &bound {
        match e.plugin_version() {
            Ok(v) if !out.contains(&v) => out.push(v),
            Ok(_) => {}
            Err(why) => return Ok(unmodelled(&e.name, why)),
        }
    }
    if out.is_empty() {
        match crate::routes::default_image_plugin_version(crate::routes::ranger_engine(engine)) {
            Ok(v) => out.push(v),
            Err(why) => return Ok(unmodelled("(none bound; egret's default image)", why)),
        }
    }
    Ok(FoldVersions::Modelled(out))
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
                catalog_spelled: false,
            },
            PendingMask {
                tag_id: b,
                column: ssn(),
                cleared: cleared(&["bob", "cat"]),
                catalog_spelled: false,
            },
        ]);
        assert_eq!(
            merged,
            vec![PendingMask {
                tag_id: b,
                column: ssn(),
                cleared: cleared(&["bob"]),
                catalog_spelled: false,
            }]
        );
    }

    fn col(table: &str, column: &str) -> TaggedColumn {
        TaggedColumn {
            catalog: "lake".into(),
            schema: "sales".into(),
            table: table.into(),
            column: column.into(),
        }
    }

    /// Ranger matches StarRocks levels case-insensitively, so two spellings
    /// of one column are one resource: one mask, cleared = the intersection,
    /// named after the lower tag id, in the first spelling.
    #[test]
    fn two_spellings_of_one_column_merge_into_one_mask() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let merged = merge_by_column(vec![
            PendingMask {
                tag_id: a,
                column: col("Orders", "Email"),
                cleared: cleared(&["ann", "bob"]),
                catalog_spelled: false,
            },
            PendingMask {
                tag_id: b,
                column: col("orders", "email"),
                cleared: cleared(&["bob", "cat"]),
                catalog_spelled: false,
            },
        ]);
        assert_eq!(
            merged,
            vec![PendingMask {
                tag_id: a,
                column: col("Orders", "Email"),
                cleared: cleared(&["bob"]),
                catalog_spelled: false,
            }]
        );
    }

    /// A mask whose spelling was read from the catalog wins the merge over
    /// an assignment's spelling, whichever comes first.
    #[test]
    fn the_catalog_spelling_wins_the_merge() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let merged = merge_by_column(vec![
            PendingMask {
                tag_id: a,
                column: col("orders", "EMAIL"),
                cleared: cleared(&["ann"]),
                catalog_spelled: false,
            },
            PendingMask {
                tag_id: b,
                column: col("Orders", "Email"),
                cleared: cleared(&["ann"]),
                catalog_spelled: true,
            },
            PendingMask {
                tag_id: b,
                column: col("ORDERS", "email"),
                cleared: cleared(&["ann"]),
                catalog_spelled: false,
            },
        ]);
        assert_eq!(merged.len(), 1, "{merged:?}");
        assert_eq!(merged[0].column, col("Orders", "Email"));
        assert_eq!(merged[0].tag_id, a);
        assert!(merged[0].catalog_spelled);
    }

    fn fold_v(others: &[Value]) -> Folded {
        fold_at_versions(
            SERVICE,
            &clearance_mask_name(Uuid::nil(), &ssn()),
            &ssn(),
            &cleared(&["ann"]),
            others,
            EngineType::StarRocks,
            &[PluginVersion::Ranger2_8_0, PluginVersion::Ranger2_9_0],
        )
        .expect("fold")
    }

    #[test]
    fn a_mask_naming_a_group_other_than_public_is_a_skip_not_a_guess() {
        // A hand-written mask: contractors get NULL, everyone else nothing.
        // The fold evaluates ann with no groups, so it cannot know whether the
        // engine would put ann in `contractors`.
        let others = vec![mask_policy(
            "hand-contractors",
            "ssn",
            0,
            json!([
                item(&[], &["contractors"], "MASK_NULL"),
                item(&[], &["public"], "MASK_NONE")
            ]),
        )];
        match fold_v(&others) {
            Folded::Skipped(reason) => {
                assert!(reason.contains("hand-contractors"), "{reason}");
                assert!(reason.contains("\"contractors\""), "{reason}");
                assert!(reason.contains("sales.s.t.ssn"), "{reason}");
                assert!(
                    reason.starts_with("view protection (clearance mask) not active for"),
                    "{reason}"
                );
            }
            other => panic!("expected a skip, got {other:?}"),
        }
    }

    #[test]
    fn a_mask_naming_a_role_is_a_skip() {
        let mut role_item = item(&[], &[], "MASK_NULL");
        role_item["roles"] = json!(["analyst"]);
        let others = vec![mask_policy(
            "perm-rule-r",
            "*",
            0,
            json!([role_item, item(&[], &["public"], "MASK")]),
        )];
        match fold_v(&others) {
            Folded::Skipped(reason) => assert!(reason.contains("role \"analyst\""), "{reason}"),
            other => panic!("expected a skip, got {other:?}"),
        }
    }

    #[test]
    fn a_group_on_a_mask_that_cannot_reach_the_column_does_not_skip() {
        let others = vec![
            mask_policy(
                "hand-email",
                "email",
                0,
                json!([item(&[], &["contractors"], "MASK_NULL")]),
            ),
            mask_policy(
                "perm-rule-b",
                "*",
                0,
                json!([item(&[], &["public"], "MASK")]),
            ),
        ];
        match fold_v(&others) {
            Folded::Mask(body) => assert_eq!(
                items_of(&body),
                vec![
                    (s(&["ann"]), s(&[]), "MASK".to_string()),
                    (s(&[]), s(&["public"]), "MASK_NULL".to_string()),
                ]
            ),
            other => panic!("expected a mask, got {other:?}"),
        }
    }

    #[test]
    fn an_undecidable_user_still_refuses_through_the_version_fold() {
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
        assert!(matches!(
            fold_at_versions(
                SERVICE,
                "n",
                &ssn(),
                &cleared(&["ann"]),
                &others,
                EngineType::StarRocks,
                &[PluginVersion::Ranger2_9_0],
            ),
            Err(CompileError::ClearanceMaskUndecidable { .. })
        ));
    }
}
