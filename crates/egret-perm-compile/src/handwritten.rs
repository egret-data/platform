//! DG #4b (spec `docs/en/specs/2026-10-19-dg-classification-design.md` §4.1,
//! CL7): **the one place that decides how a hand-written Ranger policy reaches
//! a Data object, a principal or a classification.**
//!
//! Three read routes use it — hand-written policies reaching a Data object
//! (route A), naming a principal (route B) and naming a classification's tag
//! (route D) — and explain builds its request with [`object_request`], so
//! "what explain evaluated" and "what this module matched" are the same
//! resource. The match itself is `egret-ranger-eval`'s matcher
//! ([`PreparedPolicyResources::matches`], C2); nothing here re-implements a
//! value comparison.
//!
//! # The match levels (Q2, premise 6)
//!
//! For a RESOURCE request Ranger evaluates a policy's items only on `SELF` or
//! `SELF_AND_ALL_DESCENDANTS` (`egret_ranger_eval::resource` module doc,
//! measured live). So:
//!
//! | `MatchType` (best over every resource map) | [`ObjectMatch`] |
//! |---|---|
//! | `Self_`, `SelfAndAllDescendants` | `Applies` — the policy applies to this object |
//! | `Descendant` | `Part` — the policy names something below this object |
//! | `Ancestor` | `Parent` — it names a parent only; engines do not apply it to this object's data |
//! | `None` | `None` |
//!
//! Task 1 M2 of `docs/en/plans/2026-10-19-dg-classification-live.md` measured
//! each shape against real Trino 483 and StarRocks 4.1.6.
//!
//! # Groups and roles reach nobody on an engine (Q1, premise 4)
//!
//! egret runs no usersync, writes no group provider and serves no roles
//! download, so a hand-written item naming a group (other than `public`) or a
//! role matches nobody on the engine (Task 1 M3). [`item_summary`] marks every
//! such principal `on_engines: false`; `public` and the user `{USER}` are
//! "everyone".

use std::collections::{BTreeMap, BTreeSet};

use egret_perm::EngineType;
use egret_ranger_eval::model::{ItemBucket, PolicyItem, RangerPolicy, ResourceLevel, Unmodelled};
use egret_ranger_eval::resource::{
    MatchType, PreparedPolicyResources, RequestResource, ResourceMapSource,
};
use serde::Serialize;

use crate::grants::resource_level_names;

/// The Data object a request names: a catalog, and optionally a schema, a
/// table and a column, narrowing progressively. The same shape explain's
/// target has, so the two cannot build different requests.
pub use egret_perm::Scope as ObjectTarget;

/// The group every user is in, on every engine.
const PUBLIC: &str = egret_perm::PUBLIC_GROUP;
/// Ranger's "the requesting user" token in an item's `users`.
const ANY_USER: &str = "{USER}";

/// The resource ONE access request names, in the levels the target engine's
/// service-def declares (Trino's middle level is `schema`, StarRocks' is
/// `database`).
///
/// **A level the target does not name is ABSENT, not `*`.** That is what an
/// engine's own request looks like: the plugin names exactly the levels the
/// query touched. It is also not the same thing as the compiler's `*`, which
/// is a POLICY value meaning "every child at this level" — a policy naming
/// `catalog/*/*/*` against a request naming only `catalog` is Ranger's
/// `SELF_AND_ALL_DESCENDANTS`, which matches; a request padded with `*` up to
/// the leaf would be asking about a different resource.
///
/// Stopping at the first absent level is the shape an engine request has, and
/// it is only correct because a gap below a named level cannot reach here:
/// `explain` refuses `{schema: None, table: Some(_)}` with a 400 at the top of
/// its handler, and route A refuses the same shapes with a 400 (spec §4.2).
/// Breaking here under a gap would answer about the nearest named ANCESTOR —
/// fail-open — so both callers must keep their guard.
pub fn object_request(
    engine: EngineType,
    catalog_name: &str,
    target: &ObjectTarget,
) -> RequestResource {
    let names = resource_level_names(engine);
    let mut resource = RequestResource::new();
    resource.insert(names[0], catalog_name);
    for (name, value) in [
        (names[1], target.schema.as_deref()),
        (names[2], target.table.as_deref()),
        (names[3], target.column.as_deref()),
    ] {
        match value {
            Some(v) => resource.insert(name, v),
            None => break,
        }
    }
    resource
}

/// How a policy's resources reach one object (Q2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectMatch {
    /// `Self_` or `SelfAndAllDescendants`: the policy applies to this object.
    /// `through_pattern` when a level the request names was matched by `*`,
    /// `?`, an exclude or `isRecursive` rather than by its literal value.
    Applies {
        through_pattern: bool,
    },
    /// `Descendant`: the policy names something below this object.
    Part,
    /// `Ancestor`: the policy names a parent of this object only.
    Parent,
    None,
}

fn rank(m: MatchType) -> u8 {
    match m {
        MatchType::Self_ | MatchType::SelfAndAllDescendants => 3,
        MatchType::Descendant => 2,
        MatchType::Ancestor => 1,
        MatchType::None => 0,
    }
}

fn resource_map<'p>(
    policy: &'p RangerPolicy,
    source: ResourceMapSource,
) -> Option<&'p BTreeMap<String, ResourceLevel>> {
    match source {
        ResourceMapSource::Primary => Some(&policy.resources),
        ResourceMapSource::Additional(i) => policy.additional_resources.get(i),
    }
}

/// Whether `map` reached `request` through a pattern at any level the request
/// names: a level whose values hold no literal equal (case-folded) to the
/// request's value, or a level marked `isExcludes`/`isRecursive`.
fn through_pattern(map: &BTreeMap<String, ResourceLevel>, request: &RequestResource) -> bool {
    request.keys().any(|key| {
        let Some(level) = map.get(key) else {
            return false;
        };
        if level.is_excludes || level.is_recursive {
            return true;
        }
        let value = request.get(key).unwrap_or_default();
        !level.values.iter().any(|v| v.eq_ignore_ascii_case(value))
    })
}

/// The best verdict of every resource map of `policy` on `request`, mapped as
/// Q2. `prepared` must have been built from `policy`.
pub fn object_match(
    prepared: &PreparedPolicyResources<'_>,
    policy: &RangerPolicy,
    request: &RequestResource,
) -> ObjectMatch {
    let matches = prepared.matches(request);
    let best = matches
        .iter()
        .map(|m| rank(m.match_type))
        .max()
        .unwrap_or(0);
    match best {
        3 => {
            // Literal when ANY applying map reached it literally.
            let through = matches.iter().filter(|m| rank(m.match_type) == 3).all(|m| {
                resource_map(policy, m.source).is_none_or(|map| through_pattern(map, request))
            });
            ObjectMatch::Applies {
                through_pattern: through,
            }
        }
        2 => ObjectMatch::Part,
        1 => ObjectMatch::Parent,
        _ => ObjectMatch::None,
    }
}

/// Which of a policy's six item lists an item came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Bucket {
    Allow,
    Deny,
    AllowException,
    DenyException,
    Mask,
    RowFilter,
}

impl Bucket {
    fn of(bucket: ItemBucket) -> Bucket {
        match bucket {
            ItemBucket::PolicyItems => Bucket::Allow,
            ItemBucket::DenyPolicyItems => Bucket::Deny,
            ItemBucket::AllowExceptions => Bucket::AllowException,
            ItemBucket::DenyExceptions => Bucket::DenyException,
            ItemBucket::DataMaskPolicyItems => Bucket::Mask,
            ItemBucket::RowFilterPolicyItems => Bucket::RowFilter,
        }
    }
}

/// A group or role an item names. `on_engines` is false for every one: engines
/// resolve no groups (but `public`, which is reported as `everyone` instead)
/// and no roles (Q1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PrincipalMark {
    pub name: String,
    pub on_engines: bool,
}

/// One item of a hand-written policy, as a page shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ItemSummary {
    pub bucket: Bucket,
    /// Usernames, `{USER}` excluded (it is `everyone`).
    pub users: Vec<String>,
    /// Groups, `public` excluded (it is `everyone`).
    pub groups: Vec<PrincipalMark>,
    pub roles: Vec<PrincipalMark>,
    /// The item names the group `public` or the user `{USER}`.
    pub everyone: bool,
    /// Access types, in the item's order.
    pub accesses: Vec<String>,
    /// A mask item's `dataMaskType`.
    pub mask_type: Option<String>,
    /// A row-filter item's `filterExpr`.
    pub row_filter: Option<String>,
}

/// Every item list of `policy` in Ranger's bucket order.
pub(crate) fn buckets(policy: &RangerPolicy) -> [(ItemBucket, &[PolicyItem]); 6] {
    [
        (ItemBucket::PolicyItems, &policy.policy_items),
        (ItemBucket::DenyPolicyItems, &policy.deny_policy_items),
        (ItemBucket::AllowExceptions, &policy.allow_exceptions),
        (ItemBucket::DenyExceptions, &policy.deny_exceptions),
        (
            ItemBucket::DataMaskPolicyItems,
            &policy.data_mask_policy_items,
        ),
        (
            ItemBucket::RowFilterPolicyItems,
            &policy.row_filter_policy_items,
        ),
    ]
}

fn item_names_everyone(item: &PolicyItem) -> bool {
    item.groups.iter().any(|g| g == PUBLIC) || item.users.iter().any(|u| u == ANY_USER)
}

/// One [`ItemSummary`] per item of `policy`, bucket by bucket.
pub fn item_summary(policy: &RangerPolicy) -> Vec<ItemSummary> {
    let mut out = Vec::new();
    for (bucket, items) in buckets(policy) {
        for item in items {
            out.push(ItemSummary {
                bucket: Bucket::of(bucket),
                users: item
                    .users
                    .iter()
                    .filter(|u| *u != ANY_USER)
                    .cloned()
                    .collect(),
                groups: item
                    .groups
                    .iter()
                    .filter(|g| *g != PUBLIC)
                    .map(|g| PrincipalMark {
                        name: g.clone(),
                        on_engines: false,
                    })
                    .collect(),
                roles: item
                    .roles
                    .iter()
                    .map(|r| PrincipalMark {
                        name: r.clone(),
                        on_engines: false,
                    })
                    .collect(),
                everyone: item_names_everyone(item),
                accesses: item
                    .accesses
                    .iter()
                    .map(|a| a.access_type.clone())
                    .collect(),
                mask_type: item
                    .data_mask_info
                    .as_ref()
                    .and_then(|m| m.mask_type.clone()),
                row_filter: item
                    .row_filter_info
                    .as_ref()
                    .and_then(|r| r.filter_expr.clone()),
            });
        }
    }
    out
}

/// `policyType` as the routes name it.
pub fn policy_type_word(policy_type: i32) -> &'static str {
    match policy_type {
        1 => "masking",
        2 => "rowfilter",
        _ => "access",
    }
}

/// The order levels are written in a resource summary.
const LEVEL_ORDER: [&str; 6] = ["catalog", "schema", "database", "table", "column", "tag"];

/// The primary resource map as one line: `catalog=ice; schema=s; table=*` in
/// hierarchy order (other keys after, by name), several values joined with
/// `,`, an excluded level prefixed `not `, a recursive one suffixed
/// ` (recursive)`.
pub fn resource_summary(policy: &RangerPolicy) -> String {
    let mut keys: Vec<&String> = policy.resources.keys().collect();
    keys.sort_by_key(|k| {
        (
            LEVEL_ORDER
                .iter()
                .position(|o| o == k)
                .unwrap_or(LEVEL_ORDER.len()),
            (*k).clone(),
        )
    });
    keys.into_iter()
        .map(|k| {
            let level = &policy.resources[k];
            let mut s = format!(
                "{k}={}{}",
                if level.is_excludes { "not " } else { "" },
                level.values.join(",")
            );
            if level.is_recursive {
                s.push_str(" (recursive)");
            }
            s
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// One sentence per construct of `policy` egret does not evaluate (CL9),
/// deduplicated, in first-seen order. The match is still reported; these say
/// why "applies" is not "decides".
pub fn unmodelled_sentences(policy: &RangerPolicy) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for u in &policy.unmodelled {
        let s = match u {
            Unmodelled::ValiditySchedules => {
                "Has a validity schedule egret does not evaluate.".to_string()
            }
            Unmodelled::PolicyConditions => "Has conditions egret does not evaluate.".to_string(),
            Unmodelled::ItemConditions { .. } => {
                "Has item conditions egret does not evaluate.".to_string()
            }
            Unmodelled::ZoneName(z) => {
                format!("Belongs to security zone {z}, which egret does not evaluate.")
            }
            Unmodelled::MaskExpressionMarker { .. } | Unmodelled::MaskConditionExpr { .. } => {
                "Has a mask expression egret does not evaluate.".to_string()
            }
            Unmodelled::RowFilterExpressionMarker { .. } => {
                "Has a row-filter expression egret does not evaluate.".to_string()
            }
            Unmodelled::ResourceValueExpressionMarker { .. } => {
                "Has a resource value expression egret does not evaluate.".to_string()
            }
            Unmodelled::UserExpressionMarker { .. } => {
                "Names users through an expression egret does not evaluate.".to_string()
            }
            Unmodelled::ExplicitDeny { .. } => {
                "Has a denied access inside an allow item, which egret does not evaluate."
                    .to_string()
            }
            Unmodelled::DelegateAdmin { .. } => {
                "Has delegated admin items, which egret does not evaluate.".to_string()
            }
            // Envelope-level constructs never sit on one policy; named anyway.
            other => format!("Has a construct egret does not evaluate ({other:?})."),
        };
        if seen.insert(s.clone()) {
            out.push(s);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_ranger_eval::servicedef::ServiceDef;
    use serde_json::json;
    use uuid::Uuid;

    fn def(name: &str) -> ServiceDef {
        let v: serde_json::Value = match name {
            "trino" => serde_json::from_str(include_str!(
                "../../egret-policy/src/service-defs/trino.json"
            )),
            "starrocks" => serde_json::from_str(include_str!(
                "../../egret-policy/src/service-defs/starrocks.json"
            )),
            _ => serde_json::from_str(include_str!("../../egret-policy/src/service-defs/tag.json")),
        }
        .unwrap();
        ServiceDef::parse(&v)
    }

    fn policy(v: serde_json::Value) -> RangerPolicy {
        serde_json::from_value(v).expect("policy")
    }

    fn level(values: &[&str]) -> serde_json::Value {
        json!({ "values": values })
    }

    fn access(resources: serde_json::Value) -> RangerPolicy {
        policy(json!({
            "name": "p", "service": "s", "policyType": 0, "isEnabled": true,
            "resources": resources,
            "policyItems": [{ "users": ["ua"], "accesses": [{ "type": "select", "isAllowed": true }] }]
        }))
    }

    fn target(schema: Option<&str>, table: Option<&str>, column: Option<&str>) -> ObjectTarget {
        ObjectTarget {
            catalog_id: Uuid::nil(),
            schema: schema.map(Into::into),
            table: table.map(Into::into),
            column: column.map(Into::into),
        }
    }

    fn matched(p: &RangerPolicy, engine: EngineType, t: &ObjectTarget) -> ObjectMatch {
        let d = def(match engine {
            EngineType::Trino => "trino",
            EngineType::StarRocks => "starrocks",
        });
        let prepared = PreparedPolicyResources::new(p, &d);
        object_match(&prepared, p, &object_request(engine, "ice", t))
    }

    fn table_t() -> ObjectTarget {
        target(Some("s"), Some("t"), None)
    }

    #[test]
    fn object_request_names_each_level_and_stops_at_the_first_absent_one() {
        let r = object_request(EngineType::Trino, "ice", &target(None, None, None));
        assert_eq!(r, RequestResource::new().with("catalog", "ice"));
        let r = object_request(EngineType::Trino, "ice", &target(Some("s"), None, None));
        assert_eq!(
            r,
            RequestResource::new()
                .with("catalog", "ice")
                .with("schema", "s")
        );
        let r = object_request(EngineType::StarRocks, "ice", &table_t());
        assert_eq!(
            r,
            RequestResource::new()
                .with("catalog", "ice")
                .with("database", "s")
                .with("table", "t")
        );
        let r = object_request(
            EngineType::Trino,
            "ice",
            &target(Some("s"), Some("t"), Some("email")),
        );
        assert_eq!(
            r,
            RequestResource::new()
                .with("catalog", "ice")
                .with("schema", "s")
                .with("table", "t")
                .with("column", "email")
        );
        // A gap stops the request: nothing below it is named.
        let r = object_request(EngineType::Trino, "ice", &target(None, Some("t"), None));
        assert_eq!(r, RequestResource::new().with("catalog", "ice"));
    }

    #[test]
    fn an_exact_policy_applies_literally() {
        let p = access(json!({
            "catalog": level(&["ice"]), "schema": level(&["s"]),
            "table": level(&["t"]), "column": level(&["*"])
        }));
        assert_eq!(
            matched(&p, EngineType::Trino, &table_t()),
            ObjectMatch::Applies {
                through_pattern: false
            }
        );
    }

    #[test]
    fn a_wildcard_schema_applies_through_a_pattern() {
        let p = access(json!({
            "catalog": level(&["ice"]), "schema": level(&["*"]),
            "table": level(&["*"]), "column": level(&["*"])
        }));
        assert_eq!(
            matched(&p, EngineType::Trino, &table_t()),
            ObjectMatch::Applies {
                through_pattern: true
            }
        );
    }

    #[test]
    fn a_question_mark_applies_through_a_pattern() {
        let p = access(json!({
            "catalog": level(&["ice"]), "schema": level(&["s"]),
            "table": level(&["t?"]), "column": level(&["*"])
        }));
        let t = target(Some("s"), Some("tx"), None);
        assert_eq!(
            matched(&p, EngineType::Trino, &t),
            ObjectMatch::Applies {
                through_pattern: true
            }
        );
    }

    #[test]
    fn an_exclude_applies_through_a_pattern_and_excludes_its_value() {
        let p = access(json!({
            "catalog": level(&["ice"]), "schema": level(&["s"]),
            "table": { "values": ["u"], "isExcludes": true }, "column": level(&["*"])
        }));
        assert_eq!(
            matched(&p, EngineType::Trino, &table_t()),
            ObjectMatch::Applies {
                through_pattern: true
            }
        );
        assert_eq!(
            matched(&p, EngineType::Trino, &target(Some("s"), Some("u"), None)),
            ObjectMatch::None
        );
    }

    #[test]
    fn a_recursive_level_applies_through_a_pattern() {
        let p = access(json!({
            "catalog": level(&["ice"]), "schema": { "values": ["s"], "isRecursive": true },
            "table": level(&["t"]), "column": level(&["*"])
        }));
        assert_eq!(
            matched(&p, EngineType::Trino, &table_t()),
            ObjectMatch::Applies {
                through_pattern: true
            }
        );
    }

    #[test]
    fn a_literal_differing_only_in_case_is_not_a_pattern() {
        let p = access(json!({
            "catalog": level(&["ICE"]), "schema": level(&["S"]),
            "table": level(&["T"]), "column": level(&["*"])
        }));
        assert_eq!(
            matched(&p, EngineType::Trino, &table_t()),
            ObjectMatch::Applies {
                through_pattern: false
            }
        );
    }

    #[test]
    fn a_column_policy_is_part_of_its_table_and_applies_to_the_column() {
        let p = access(json!({
            "catalog": level(&["ice"]), "schema": level(&["s"]),
            "table": level(&["t"]), "column": level(&["email"])
        }));
        assert_eq!(
            matched(&p, EngineType::Trino, &table_t()),
            ObjectMatch::Part
        );
        assert_eq!(
            matched(
                &p,
                EngineType::Trino,
                &target(Some("s"), Some("t"), Some("email"))
            ),
            ObjectMatch::Applies {
                through_pattern: false
            }
        );
    }

    #[test]
    fn a_schema_only_policy_is_set_on_a_parent_of_a_table() {
        let p = access(json!({ "catalog": level(&["ice"]), "schema": level(&["s"]) }));
        assert_eq!(
            matched(&p, EngineType::Trino, &table_t()),
            ObjectMatch::Parent
        );
        // StarRocks names the level `database`.
        let p = access(json!({ "catalog": level(&["ice"]), "database": level(&["s"]) }));
        assert_eq!(
            matched(&p, EngineType::StarRocks, &table_t()),
            ObjectMatch::Parent
        );
    }

    #[test]
    fn the_best_verdict_over_every_resource_map_wins() {
        let p = policy(json!({
            "name": "p", "service": "s", "policyType": 0, "isEnabled": true,
            "resources": {
                "catalog": level(&["ice"]), "schema": level(&["other"]),
                "table": level(&["*"]), "column": level(&["*"])
            },
            "additionalResources": [{
                "catalog": level(&["ice"]), "schema": level(&["s"]),
                "table": level(&["t"]), "column": level(&["*"])
            }],
            "policyItems": []
        }));
        assert_eq!(
            matched(&p, EngineType::Trino, &table_t()),
            ObjectMatch::Applies {
                through_pattern: false
            }
        );
    }

    #[test]
    fn item_summary_reads_every_bucket_and_marks_principals() {
        let p = policy(json!({
            "name": "p", "service": "s", "policyType": 0, "isEnabled": true,
            "resources": { "catalog": level(&["ice"]) },
            "policyItems": [{ "users": ["ua", "{USER}"], "groups": ["g1", "public"], "roles": ["r1"],
                              "accesses": [{ "type": "select", "isAllowed": true }] }],
            "denyPolicyItems": [{ "users": ["ub"], "accesses": [{ "type": "select", "isAllowed": true }] }],
            "allowExceptions": [{ "users": ["uc"], "accesses": [{ "type": "select", "isAllowed": true }] }],
            "denyExceptions": [{ "groups": ["g2"], "accesses": [{ "type": "select", "isAllowed": true }] }]
        }));
        let s = item_summary(&p);
        assert_eq!(s.len(), 4);
        assert_eq!(
            s[0],
            ItemSummary {
                bucket: Bucket::Allow,
                users: vec!["ua".into()],
                groups: vec![PrincipalMark {
                    name: "g1".into(),
                    on_engines: false
                }],
                roles: vec![PrincipalMark {
                    name: "r1".into(),
                    on_engines: false
                }],
                everyone: true,
                accesses: vec!["select".into()],
                mask_type: None,
                row_filter: None,
            }
        );
        assert_eq!(
            s.iter().map(|i| i.bucket).collect::<Vec<_>>(),
            vec![
                Bucket::Allow,
                Bucket::Deny,
                Bucket::AllowException,
                Bucket::DenyException
            ]
        );
        assert!(!s[1].everyone);
        assert_eq!(s[3].groups[0].name, "g2");
        assert!(!s[3].groups[0].on_engines);
    }

    #[test]
    fn item_summary_reads_mask_and_row_filter() {
        let mask = policy(json!({
            "name": "m", "service": "s", "policyType": 1, "isEnabled": true,
            "resources": { "catalog": level(&["ice"]) },
            "dataMaskPolicyItems": [{ "users": ["{USER}"], "accesses": [{ "type": "select" }],
                                      "dataMaskInfo": { "dataMaskType": "MASK_NULL" } }]
        }));
        let s = item_summary(&mask);
        assert_eq!(s[0].bucket, Bucket::Mask);
        assert_eq!(s[0].mask_type.as_deref(), Some("MASK_NULL"));
        assert!(s[0].everyone);
        assert!(s[0].users.is_empty());
        let rf = policy(json!({
            "name": "r", "service": "s", "policyType": 2, "isEnabled": true,
            "resources": { "catalog": level(&["ice"]) },
            "rowFilterPolicyItems": [{ "groups": ["public"], "accesses": [{ "type": "select" }],
                                       "rowFilterInfo": { "filterExpr": "country = 'VN'" } }]
        }));
        let s = item_summary(&rf);
        assert_eq!(s[0].bucket, Bucket::RowFilter);
        assert_eq!(s[0].row_filter.as_deref(), Some("country = 'VN'"));
        assert!(s[0].everyone);
        assert!(s[0].groups.is_empty());
    }

    #[test]
    fn a_tag_value_matches_by_the_tag_service_def_without_folding_case() {
        let d = def("tag");
        let p = policy(json!({
            "name": "t", "service": "x", "policyType": 0, "isEnabled": true,
            "resources": { "tag": level(&["PII"]) }, "policyItems": []
        }));
        let prepared = PreparedPolicyResources::new(&p, &d);
        let req = |n: &str| RequestResource::new().with("tag", n);
        assert_eq!(
            object_match(&prepared, &p, &req("PII")),
            ObjectMatch::Applies {
                through_pattern: false
            }
        );
        assert_eq!(object_match(&prepared, &p, &req("pii")), ObjectMatch::None);
    }

    #[test]
    fn resource_summary_and_unmodelled_sentences() {
        let p = policy(json!({
            "name": "p", "service": "s", "policyType": 0, "isEnabled": true,
            "resources": {
                "column": level(&["*"]), "table": { "values": ["u"], "isExcludes": true },
                "schema": level(&["s"]), "catalog": level(&["ice"])
            },
            "conditions": [{ "type": "ip", "values": ["1.2.3.4"] }],
            "validitySchedules": [{ "startTime": "2026/01/01 00:00:00" }],
            "policyItems": []
        }));
        assert_eq!(
            resource_summary(&p),
            "catalog=ice; schema=s; table=not u; column=*"
        );
        assert_eq!(
            unmodelled_sentences(&p),
            vec![
                "Has conditions egret does not evaluate.".to_string(),
                "Has a validity schedule egret does not evaluate.".to_string(),
            ]
        );
    }
}
