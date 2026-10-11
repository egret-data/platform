//! The equivalence gate (SP4d-3 Task 6), driven with REAL artifacts: every
//! "after" set below is what the grammar (`grammar.rs`) and the compiler
//! (`compile_grant` / `compile_rule`) actually produce from the hand-written
//! "before" body, never a hand-copied stand-in for it. A "wrong" migration is
//! that real output with one thing changed, so each refusal test says which
//! change the gate sees.
//!
//! Pure (no DB).

use std::collections::BTreeSet;

use egret_perm::{EngineType, Grant, PrincipalKind};
use egret_perm_compile::equivalence::{
    compare, compare_profiles, derive_probes, probe_count, Check, Disagreement, GateRefusal, Probe,
    ProfileCase, TagAssignment, Tagging, PROBE_SIBLING_PREFIX, PROBE_UNNAMED_USER,
};
use egret_perm_compile::grammar::{
    grants_from_access_policy, rules_from_mask_or_filter_policy, ServiceKind,
};
use egret_perm_compile::{compile_grant, compile_rule, ResolvedNames};
use egret_policy::admin::access_to_ranger_json;
use egret_ranger_eval::eval::{AccessRequest, EngineConfig, PluginVersion};
use egret_ranger_eval::model::ServicePolicySet;
use egret_ranger_eval::resource::RequestResource;
use egret_ranger_eval::servicedef::ServiceDef;
use serde_json::{json, Value};
use uuid::Uuid;

const CAT: Uuid = Uuid::from_u128(0x1111_1111_1111_1111_1111_1111_1111_1111);
const CAT2: Uuid = Uuid::from_u128(0x2222_2222_2222_2222_2222_2222_2222_2222);
const BOTH: [PluginVersion; 2] = [PluginVersion::Ranger2_5_0, PluginVersion::Ranger2_8_0];

// ── fixtures ─────────────────────────────────────────────────────────────

fn def_json(file: &str) -> Value {
    let path = format!(
        "{}/../egret-policy/src/service-defs/{file}",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("read service-def"))
        .expect("parse service-def")
}

fn trino_def() -> ServiceDef {
    ServiceDef::parse(&def_json("trino.json"))
}

fn catalogs(name: &str) -> Option<Uuid> {
    match name {
        "wh" => Some(CAT),
        "lake" => Some(CAT2),
        _ => None,
    }
}

fn catalog_name(id: Uuid) -> &'static str {
    match id {
        CAT => "wh",
        CAT2 => "lake",
        _ => panic!("no catalog name for {id}"),
    }
}

/// key == name, the one migratable tag shape.
fn tags(key: &str) -> Option<String> {
    (key == "phi").then(|| "phi".to_string())
}

fn level(values: &[&str]) -> Value {
    json!({"values": values, "isExcludes": false, "isRecursive": false})
}

fn item(users: &[&str], groups: &[&str], accesses: &[&str]) -> Value {
    json!({
        "users": users, "groups": groups, "roles": [],
        "accesses": accesses.iter().map(|a| json!({"type": a, "isAllowed": true})).collect::<Vec<_>>()
    })
}

/// A hand-written access policy on `<catalogs>.sales.orders.*`.
fn hand_policy(name: &str, catalogs: &[&str], items: Vec<Value>) -> Value {
    json!({
        "id": 500, "name": name, "service": "trino-p", "policyType": 0, "isEnabled": true,
        "resources": {
            "catalog": level(catalogs),
            "schema":  level(&["sales"]),
            "table":   level(&["orders"]),
            "column":  level(&["*"])
        },
        "policyItems": items,
        "denyPolicyItems": [],
        "allowExceptions": [],
        "denyExceptions": []
    })
}

/// The grants the grammar makes of `body`, each with a distinct id.
fn grants_of(body: &Value) -> Vec<Grant> {
    let mut grants =
        grants_from_access_policy(body, EngineType::Trino, &catalogs, ServiceKind::Resource)
            .unwrap_or_else(|b| panic!("the grammar refused the fixture: {b:?}"))
            .grants;
    for (i, g) in grants.iter_mut().enumerate() {
        g.id = Uuid::from_u128(0xA000 + i as u128);
    }
    grants
}

/// What the compiler emits for `grants`, as Ranger JSON.
fn compiled(grants: &[Grant]) -> Vec<Value> {
    grants
        .iter()
        .map(|g| {
            let input = compile_grant(
                g,
                catalog_name(g.scope.catalog_id),
                "trino-p",
                EngineType::Trino,
            )
            .unwrap_or_else(|e| panic!("compile_grant: {e:?}"));
            access_to_ranger_json(&input)
        })
        .collect()
}

/// A download envelope carrying `policies` and, when non-empty, a
/// `tagPolicies` block with the embedded tag service-def.
fn envelope(policies: Vec<Value>, tag_policies: Vec<Value>) -> ServicePolicySet {
    let mut env = json!({
        "serviceName": "trino-p", "serviceId": 1, "policyVersion": 1,
        "policies": policies,
        "serviceDef": def_json("trino.json"),
    });
    if !tag_policies.is_empty() {
        env["tagPolicies"] = json!({
            "serviceName": "trino-tags", "serviceId": 99, "policyVersion": 1,
            "policies": tag_policies,
            "serviceDef": def_json("tag.json"),
        });
    }
    serde_json::from_value(env).expect("parse envelope")
}

fn case<'a>(
    service_id: i64,
    before: &'a ServicePolicySet,
    after: &'a ServicePolicySet,
    def: &'a ServiceDef,
    versions: &[PluginVersion],
    probes: Vec<Probe>,
) -> ProfileCase<'a> {
    ProfileCase {
        service_id,
        service_name: format!("profile-{service_id}"),
        before,
        after,
        service_def: def,
        engine: EngineConfig::without_super_users(),
        versions: versions.to_vec(),
        probes,
    }
}

/// The gate on one profile, probes derived from the two sets, no tags.
fn gate(before: &ServicePolicySet, after: &ServicePolicySet) -> Vec<Disagreement> {
    let def = trino_def();
    let probes = derive_probes(before, after, &def, &[]);
    compare(&case(1, before, after, &def, &BOTH, probes)).expect("the gate answers")
}

fn column(catalog: &str, schema: &str, table: &str, column: &str) -> RequestResource {
    RequestResource::new()
        .with("catalog", catalog)
        .with("schema", schema)
        .with("table", table)
        .with("column", column)
}

/// One `catalog_tag_assignments` row on a column.
fn assign(catalog: &str, schema: &str, table: &str, column: &str, tag: &str) -> TagAssignment {
    TagAssignment {
        catalog: catalog.into(),
        schema: schema.into(),
        table: table.into(),
        column: column.into(),
        tag: tag.into(),
    }
}

fn summary(ds: &[Disagreement]) -> String {
    ds.iter()
        .take(12)
        .map(|d| {
            format!(
                "\n  [{} {:?} {:?} {:?}{:?}] {} {} {}: {} -> {}",
                d.service_id,
                d.plugin_version,
                d.check,
                d.tagging,
                d.tags,
                d.user,
                d.access_type,
                d.resource,
                d.before,
                d.after
            )
        })
        .collect::<String>()
        + &format!("\n  ({} total)", ds.len())
}

// ── 1. equivalent ────────────────────────────────────────────────────────

/// A hand-written policy and its compiled grant decide the same on every
/// probe, on both versions.
#[test]
fn a_policy_and_its_compiled_grant_are_equivalent_on_both_versions() {
    let body = hand_policy(
        "sales read orders",
        &["wh"],
        vec![item(&["alice"], &[], &["select"])],
    );
    let before = envelope(vec![body.clone()], vec![]);
    let after = envelope(compiled(&grants_of(&body)), vec![]);

    let ds = gate(&before, &after);
    assert!(ds.is_empty(), "expected equivalent:{}", summary(&ds));
}

/// The grammar's SPLITS, each proven by the gate: an allow + deny +
/// deny-exception policy (two grants), two items under one allow exception
/// (two grants sharing the exclusion), and two catalogs (two grants). Each
/// correct migration is equivalent; each has a deliberately wrong twin the
/// gate refuses, naming the principal and coordinate that changed.
#[test]
fn every_split_the_grammar_makes_is_equivalent_and_each_wrong_split_is_caught() {
    // (a) allow alice/dave/erin, deny dave/erin, except erin from the deny.
    let mut deny = hand_policy(
        "allow, deny, deny-exception",
        &["wh"],
        vec![item(&["alice", "dave", "erin"], &[], &["select"])],
    );
    deny["denyPolicyItems"] = json!([item(&["dave", "erin"], &[], &["select"])]);
    deny["denyExceptions"] = json!([item(&["erin"], &[], &["select"])]);

    // (b) two items, one allow exception (carol) applying to both.
    let mut two_items = hand_policy(
        "two items, one exception",
        &["wh"],
        vec![
            item(&["alice", "carol"], &[], &["select"]),
            item(&[], &["analysts"], &["select"]),
        ],
    );
    two_items["allowExceptions"] = json!([item(&["carol"], &[], &["select"])]);

    // (c) two catalogs.
    let two_cats = hand_policy(
        "two catalogs",
        &["wh", "lake"],
        vec![item(&["alice"], &[], &["select"])],
    );

    for (label, body, n) in [
        ("allow+deny+exception", &deny, 2),
        ("two items", &two_items, 2),
        ("two catalogs", &two_cats, 2),
    ] {
        let grants = grants_of(body);
        assert_eq!(grants.len(), n, "{label}: the grammar's split");
        let before = envelope(vec![body.clone()], vec![]);
        let after = envelope(compiled(&grants), vec![]);
        let ds = gate(&before, &after);
        assert!(
            ds.is_empty(),
            "{label}: expected equivalent:{}",
            summary(&ds)
        );
    }

    // Wrong (a1): the deny grant dropped — dave is now allowed.
    let grants = grants_of(&deny);
    let allow_only: Vec<Grant> = grants
        .iter()
        .filter(|g| g.effect == egret_perm::Effect::Allow)
        .cloned()
        .collect();
    let ds = gate(
        &envelope(vec![deny.clone()], vec![]),
        &envelope(compiled(&allow_only), vec![]),
    );
    assert!(!ds.is_empty(), "dropping the deny grant must be caught");
    assert!(
        ds.iter().all(|d| d.user == "dave"),
        "only dave's answers change:{}",
        summary(&ds)
    );
    assert!(ds.iter().any(|d| d.check == Check::Access
        && d.access_type == "select"
        && d.resource == "wh.sales.orders"
        && d.before.starts_with("denied by a deny item")
        && d.after.starts_with("allowed")));

    // Wrong (a2): the deny exception dropped — erin is now denied.
    let mut no_exception = grants.clone();
    for g in &mut no_exception {
        if g.effect == egret_perm::Effect::Deny {
            g.principals.exclude.clear();
        }
    }
    let ds = gate(
        &envelope(vec![deny.clone()], vec![]),
        &envelope(compiled(&no_exception), vec![]),
    );
    assert!(!ds.is_empty(), "dropping the deny exception must be caught");
    assert!(
        ds.iter().all(|d| d.user == "erin"),
        "only erin's answers change:{}",
        summary(&ds)
    );

    // Wrong (b): the exclusion dropped from the grant of the item that NAMES
    // carol and kept on the other — carol is let in.
    let mut half = grants_of(&two_items);
    half[0].principals.exclude.clear();
    let ds = gate(
        &envelope(vec![two_items.clone()], vec![]),
        &envelope(compiled(&half), vec![]),
    );
    assert!(!ds.is_empty(), "a half-applied exception must be caught");
    assert!(
        ds.iter().all(|d| d.user == "carol"),
        "only carol's answers change:{}",
        summary(&ds)
    );

    // Wrong (c): only the first catalog migrated — `lake` loses alice.
    let one_cat = grants_of(&two_cats)[..1].to_vec();
    let ds = gate(
        &envelope(vec![two_cats.clone()], vec![]),
        &envelope(compiled(&one_cat), vec![]),
    );
    assert!(!ds.is_empty(), "a dropped catalog must be caught");
    let dropped = catalog_name(grants_of(&two_cats)[1].scope.catalog_id);
    assert!(
        ds.iter()
            .all(|d| d.user == "alice" && d.resource.starts_with(&format!("{dropped}."))),
        "only alice on {dropped} changes:{}",
        summary(&ds)
    );
}

// ── 2. not equivalent ────────────────────────────────────────────────────

/// The compiled grant omits one user: the gate names that user, that access
/// type and that resource, and nobody else.
#[test]
fn a_grant_that_omits_a_user_is_named_by_user_access_type_and_resource() {
    let body = hand_policy(
        "sales read orders",
        &["wh"],
        vec![item(&["alice", "bob"], &[], &["select"])],
    );
    let mut grants = grants_of(&body);
    grants[0]
        .principals
        .include
        .retain(|p| !(p.kind == PrincipalKind::User && p.r#ref == "bob"));

    let ds = gate(
        &envelope(vec![body], vec![]),
        &envelope(compiled(&grants), vec![]),
    );
    assert!(!ds.is_empty(), "omitting bob must be caught");
    assert!(
        ds.iter()
            .all(|d| d.user == "bob" && d.access_type == "select"),
        "only bob's select changes:{}",
        summary(&ds)
    );
    let on_table = ds
        .iter()
        .find(|d| d.resource == "wh.sales.orders")
        .unwrap_or_else(|| panic!("no disagreement on the table:{}", summary(&ds)));
    assert_eq!(on_table.check, Check::Access);
    assert_eq!(on_table.before, "allowed (policy \"sales read orders\")");
    assert_eq!(on_table.after, "denied: no policy matched");
    // Which versions report it is case 3's job alone (the per-version loop's
    // mutation must kill exactly one test).
}

// ── 3. the version split ─────────────────────────────────────────────────

/// A tag ACCESS policy denying alice `select` on the tag `pii`.
fn tag_deny_pii() -> Value {
    json!({
        "id": 900, "name": "pii clearance", "service": "trino-tags", "policyType": 0,
        "isEnabled": true, "policyPriority": 0,
        "resources": {"tag": level(&["pii"])},
        "denyPolicyItems": [item(&["alice"], &[], &["select"])]
    })
}

/// A resource ALLOW for alice on `wh.sales.orders.*`, at `priority`, with no
/// deny items.
fn allow_alice_at(priority: i32) -> Value {
    let mut p = hand_policy(
        "alice reads orders",
        &["wh"],
        vec![item(&["alice"], &[], &["select"])],
    );
    p["policyPriority"] = json!(priority);
    p
}

/// **The known divergence, built from the evaluator's own behaviour** (SP4d-2b
/// Task 5, `UndecidableCause::TagDenyReopenedDiffersByPluginVersion`, measured
/// on both real engines): a tag DENY plus a STRICTLY higher-priority resource
/// ALLOW with no deny items is Allowed on 2.8.0 and Denied on 2.5.0. At
/// priority 0 the same allow cannot reopen the deny on either version.
///
/// So raising the resource allow from 0 to 1 — the after-state here — changes
/// 2.8.0's answer from Denied to Allowed on the tagged column.
///
/// ⚠ **"for 2.8.0 only" (the brief) is not reachable, and why.** The
/// evaluator reproduces 2.8.0's answer and REFUSES on 2.5.0 for exactly that
/// shape (2.5.0's arm is unmodelled), and that refusal is the only
/// version-dependent branch in `egret-ranger-eval` — so wherever the two
/// versions' decided answers would differ, 2.5.0's is `Undecidable`, which the
/// gate counts as a disagreement. 2.5.0 therefore reports this probe too, as
/// "cannot be decided"; what only 2.8.0 reports is the decided FLIP. A gate
/// that evaluated only 2.5.0 would still refuse — but it would show the
/// operator a refusal where the truth is "StarRocks would start allowing
/// this", and on a 2.8.0-only profile it would be the whole answer.
#[test]
fn a_tag_deny_reopened_on_2_8_0_is_a_decided_flip_on_2_8_0_and_a_refusal_on_2_5_0() {
    let before = envelope(vec![allow_alice_at(0)], vec![tag_deny_pii()]);
    let after = envelope(vec![allow_alice_at(1)], vec![tag_deny_pii()]);
    let def = trino_def();
    let assignments = [assign("wh", "sales", "orders", "email", "pii")];
    let probes = derive_probes(&before, &after, &def, &assignments);

    let ds = compare(&case(1, &before, &after, &def, &BOTH, probes)).expect("answers");

    // Only alice, only where `pii` is (actually or hypothetically) carried.
    // Which tagging is not asserted here: that is the tag tests' job, so that
    // a mutation of the tag input kills those and not this one.
    assert!(
        ds.iter().all(|d| d.user == "alice"
            && d.resource.starts_with("wh.sales.orders")
            && d.tags == ["pii"]),
        "only alice on a pii-tagged resource changes:{}",
        summary(&ds)
    );
    let on_2_8: Vec<_> = ds
        .iter()
        .filter(|d| d.plugin_version == PluginVersion::Ranger2_8_0)
        .collect();
    assert!(
        on_2_8.iter().any(|d| d.check == Check::Access
            && d.access_type == "select"
            && d.before == "denied by a deny item (policy \"pii clearance\")"
            && d.after == "allowed (policy \"alice reads orders\")"),
        "2.8.0 must report the decided Denied -> Allowed flip:{}",
        summary(&ds)
    );
    let on_2_5: Vec<_> = ds
        .iter()
        .filter(|d| d.plugin_version == PluginVersion::Ranger2_5_0)
        .collect();
    assert!(!on_2_5.is_empty(), "2.5.0 must not pass silently");
    assert!(
        on_2_5.iter().all(|d| d.before.starts_with("denied")
            && d.after == "cannot be decided: TagDenyReopenedDiffersByPluginVersion"),
        "2.5.0's disagreement is the evaluator's refusal, never an allow:{}",
        summary(&ds)
    );
}

/// **Case 3b, the MIRROR — the one that justifies the per-version loop**
/// (Task 6 review M1). Before: the tag DENY plus a strictly higher-priority
/// no-deny ALLOW, which 2.8.0 already reopens (Allowed) and 2.5.0 does not
/// (Trino's true answer is Denied; the evaluator refuses). After: the same
/// ALLOW with the tag deny gone — Allowed on both.
///
/// So 2.8.0 sees NO change and 2.5.0 does: Trino would start allowing alice on
/// the tagged column. A gate that evaluated only 2.8.0 would pass this and
/// ship it.
#[test]
fn removing_a_tag_deny_that_only_2_8_0_already_reopens_is_caught_on_2_5_0_alone() {
    // The allow covers every coordinate, so wherever the tag deny reaches,
    // 2.8.0 reopens it — removing the deny then changes no 2.8.0 answer.
    let mut everywhere = allow_alice_at(1);
    for name in ["catalog", "schema", "table", "column"] {
        everywhere["resources"][name] = level(&["*"]);
    }
    let before = envelope(vec![everywhere.clone()], vec![tag_deny_pii()]);
    let after = envelope(vec![everywhere], vec![]);
    let def = trino_def();
    let assignments = [assign("wh", "sales", "orders", "email", "pii")];
    let probes = derive_probes(&before, &after, &def, &assignments);

    let ds = compare(&case(1, &before, &after, &def, &BOTH, probes.clone())).expect("answers");
    assert!(
        ds.iter()
            .all(|d| d.plugin_version == PluginVersion::Ranger2_5_0),
        "2.8.0 sees no change:{}",
        summary(&ds)
    );
    assert!(
        ds.iter().any(|d| d.user == "alice"
            && d.access_type == "select"
            && d.resource.starts_with("wh.sales.orders")
            && d.before == "cannot be decided: TagDenyReopenedDiffersByPluginVersion"
            && d.after == "allowed (policy \"alice reads orders\")"),
        "2.5.0 must report the change:{}",
        summary(&ds)
    );

    // What a 2.8.0-only gate would have said.
    let only_2_8 = compare(&case(
        1,
        &before,
        &after,
        &def,
        &[PluginVersion::Ranger2_8_0],
        probes,
    ))
    .expect("answers");
    assert_eq!(only_2_8, vec![], "a 2.8.0-only gate passes this");
}

// ── 4. probes reach past both sets' vocabulary ───────────────────────────

/// A pair that only ever mentions `wh.sales.orders` and `alice` still yields
/// probes at a sibling of every level, for a principal neither names, and for
/// an access type neither grants.
#[test]
fn the_probe_set_reaches_past_both_policies_vocabulary() {
    let body = hand_policy(
        "sales read orders",
        &["wh"],
        vec![item(&["alice"], &[], &["select"])],
    );
    let before = envelope(vec![body.clone()], vec![]);
    let after = envelope(compiled(&grants_of(&body)), vec![]);
    let probes: Vec<AccessRequest> = derive_probes(&before, &after, &trino_def(), &[])
        .into_iter()
        .map(|p| p.request)
        .collect();

    let is_sibling = |v: Option<&str>| v.is_some_and(|v| v.starts_with(PROBE_SIBLING_PREFIX));
    let at = |p: &AccessRequest, k: &str| p.resource.get(k).map(str::to_string);
    let named = |v: Option<String>, want: &str| v.as_deref() == Some(want);

    assert!(probes.iter().any(|p| is_sibling(p.resource.get("catalog"))));
    assert!(probes
        .iter()
        .any(|p| named(at(p, "catalog"), "wh") && is_sibling(p.resource.get("schema"))));
    assert!(probes.iter().any(|p| named(at(p, "catalog"), "wh")
        && named(at(p, "schema"), "sales")
        && is_sibling(p.resource.get("table"))));
    assert!(probes.iter().any(|p| named(at(p, "table"), "orders")
        && named(at(p, "schema"), "sales")
        && is_sibling(p.resource.get("column"))));
    // A sibling continued to full depth: another schema holding a table of
    // the same name — what a grant scoped too wide would leak into.
    assert!(probes.iter().any(|p| is_sibling(p.resource.get("schema"))
        && named(at(p, "table"), "orders")
        && p.resource.get("column").is_some()));

    let users: BTreeSet<&str> = probes.iter().map(|p| p.user.as_str()).collect();
    assert!(users.contains(PROBE_UNNAMED_USER), "{users:?}");
    assert!(users.iter().any(|u| *u != "alice"));

    let access: BTreeSet<&str> = probes.iter().map(|p| p.access_type.as_str()).collect();
    for unmentioned in ["insert", "delete", "drop"] {
        assert!(access.contains(unmentioned), "{access:?}");
    }
    // Every depth of the named path is asked, as the engines ask it.
    for depth in 1..=4 {
        assert!(
            probes.iter().any(|p| p.resource.len() == depth
                && p.resource.get("catalog") == Some("wh")
                && p.resource.keys().all(|k| !is_sibling(p.resource.get(k)))),
            "depth {depth}"
        );
    }
}

// ── masks and row filters: the whole result, not "masked or not" ─────────

fn mask_item(users: &[&str], groups: &[&str], mask_type: &str) -> Value {
    json!({
        "users": users, "groups": groups, "roles": [],
        "accesses": [{"type": "select", "isAllowed": true}],
        "dataMaskInfo": {"dataMaskType": mask_type}
    })
}

fn tag_mask(mask_type: &str) -> Value {
    json!({
        "id": 700, "name": "mask phi", "service": "trino-tags", "serviceType": "tag",
        "policyType": 1, "isEnabled": true,
        "resources": {"tag": level(&["phi"])},
        "dataMaskPolicyItems": [
            mask_item(&["alice"], &[], "MASK_NONE"),
            mask_item(&[], &["public"], mask_type)
        ],
        "policyItems": [], "denyPolicyItems": []
    })
}

/// What the compiler emits for a mask / filter body.
fn compiled_rules(body: &Value, service: ServiceKind) -> Vec<Value> {
    let rules =
        rules_from_mask_or_filter_policy(body, EngineType::Trino, &catalogs, &tags, service)
            .unwrap_or_else(|b| panic!("the grammar refused the fixture: {b:?}"))
            .rules;
    rules
        .iter()
        .enumerate()
        .flat_map(|(i, r)| {
            let mut r = r.clone();
            r.id = Uuid::from_u128(0xB000 + i as u128);
            let (catalog, tag_name) = match &r.selector {
                egret_perm::Selector::Path(s) => (catalog_name(s.catalog_id), None),
                egret_perm::Selector::Tag { key } => ("unused", tags(key)),
            };
            compile_rule(
                &r,
                EngineType::Trino,
                "trino-p",
                Some("trino-tags"),
                &ResolvedNames {
                    catalog_name: catalog,
                    tag_name: tag_name.as_deref(),
                },
                &[],
                100,
            )
            .unwrap_or_else(|e| panic!("compile_rule: {e:?}"))
        })
        .collect()
}

/// A tag-addressed mask (alice exempt, everyone else nulled) and its compiled
/// rule, probed the way Task 7 calls the gate: the profile's tag-assignment
/// rows (`phi` on `ssn`) and nothing else — no resource policy names `ssn`.
///
/// **Review I1.** The first version took the tagged coordinates and the tag
/// resolver as two inputs; with no coordinates and a real resolver every probe
/// sat on an untagged column and a changed mask compared EQUIVALENT. The rows
/// are now the one input, so the tagged column is probed WITH its tag.
///
/// Equivalent as migrated. A wrong mask TYPE and a dropped exemption are each
/// caught on `ssn` under its ACTUAL tagging, with the whole mask rendered, and
/// an untagged sibling column changes under no actual tagging.
#[test]
fn a_tag_mask_change_is_caught_on_the_actually_tagged_column_from_the_assignments_alone() {
    let reader = hand_policy(
        "everyone reads orders",
        &["wh"],
        vec![item(&[], &["public"], &["select"])],
    );
    let before = envelope(vec![reader.clone()], vec![tag_mask("MASK_NULL")]);
    let after = envelope(
        vec![reader.clone()],
        compiled_rules(&tag_mask("MASK_NULL"), ServiceKind::Tag),
    );

    let def = trino_def();
    let ssn = column("wh", "sales", "orders", "ssn");
    let assignments = [assign("wh", "sales", "orders", "ssn", "phi")];
    let probes = || derive_probes(&before, &after, &def, &assignments);

    // Under ACTUAL tagging the probe set holds the tagged column carrying
    // `phi`, and a column of the same table carrying nothing.
    let ps = probes();
    let actual_tags_on = |p: &Probe| {
        p.request.resolved_tags.as_ref().map(|t| {
            t.as_slice()
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>()
        })
    };
    assert!(ps.iter().any(|p| p.tagging == Tagging::Actual
        && p.request.resource == ssn
        && actual_tags_on(p) == Some(vec!["phi".to_string()])));
    assert!(ps.iter().any(|p| p.tagging == Tagging::Actual
        && p.request.resource.get("table") == Some("orders")
        && p.request.resource.get("column").is_some_and(|c| c != "ssn")
        && actual_tags_on(p) == Some(vec![])));

    let ds = compare(&case(1, &before, &after, &def, &BOTH, probes())).expect("answers");
    assert!(ds.is_empty(), "expected equivalent:{}", summary(&ds));

    let actual = |ds: Vec<Disagreement>| -> Vec<Disagreement> {
        ds.into_iter()
            .filter(|d| d.tagging == Tagging::Actual)
            .collect()
    };

    // Wrong mask type.
    let hashed = envelope(
        vec![reader.clone()],
        compiled_rules(&tag_mask("MASK_HASH"), ServiceKind::Tag),
    );
    let ds = actual(compare(&case(1, &before, &hashed, &def, &BOTH, probes())).expect("answers"));
    assert!(
        !ds.is_empty(),
        "a changed mask type must be caught under actual tagging"
    );
    assert!(
        ds.iter().all(|d| d.check == Check::DataMask
            && d.resource == "wh.sales.orders.ssn"
            && d.tags == ["phi"]
            && d.user != "alice"),
        "only non-exempt users' masks on ssn change:{}",
        summary(&ds)
    );
    assert!(ds.iter().any(
        |d| d.before.starts_with("masked MASK_NULL") && d.after.starts_with("masked MASK_HASH")
    ));

    // Dropped exemption: alice is now nulled.
    let mut no_exempt = tag_mask("MASK_NULL");
    no_exempt["dataMaskPolicyItems"] = json!([mask_item(&[], &["public"], "MASK_NULL")]);
    let unexempt = envelope(
        vec![reader.clone()],
        compiled_rules(&no_exempt, ServiceKind::Tag),
    );
    let ds = actual(compare(&case(1, &before, &unexempt, &def, &BOTH, probes())).expect("answers"));
    assert!(
        !ds.is_empty()
            && ds
                .iter()
                .all(|d| d.user == "alice" && d.resource == "wh.sales.orders.ssn"),
        "only alice on ssn changes:{}",
        summary(&ds)
    );
}

/// A tag rule that NO column carries today is still proven: every
/// table/column coordinate is probed again carrying each tag name the policies
/// address. Without that, a migrated tag rule with a wrong mask would pass
/// the gate and wait, enforcing nothing wrong, until somebody tags a column.
#[test]
fn a_tag_rule_change_is_caught_even_when_no_column_carries_the_tag() {
    let reader = hand_policy(
        "everyone reads orders",
        &["wh"],
        vec![item(&[], &["public"], &["select"])],
    );
    let before = envelope(vec![reader.clone()], vec![tag_mask("MASK_NULL")]);
    let same = envelope(
        vec![reader.clone()],
        compiled_rules(&tag_mask("MASK_NULL"), ServiceKind::Tag),
    );
    let hashed = envelope(
        vec![reader.clone()],
        compiled_rules(&tag_mask("MASK_HASH"), ServiceKind::Tag),
    );
    let def = trino_def();

    let ds = compare(&case(
        1,
        &before,
        &same,
        &def,
        &BOTH,
        derive_probes(&before, &same, &def, &[]),
    ))
    .expect("answers");
    assert!(ds.is_empty(), "expected equivalent:{}", summary(&ds));

    let ds = compare(&case(
        1,
        &before,
        &hashed,
        &def,
        &BOTH,
        derive_probes(&before, &hashed, &def, &[]),
    ))
    .expect("answers");
    assert!(!ds.is_empty(), "no assignments must not mean vacuous");
    assert!(
        ds.iter().all(|d| d.tagging == Tagging::Hypothetical
            && d.tags == ["phi"]
            && d.check == Check::DataMask
            && d.user != "alice"),
        "{}",
        summary(&ds)
    );
}

fn filter_policy(expr: &str) -> Value {
    json!({
        "id": 800, "name": "only EU rows", "service": "trino-p", "policyType": 2,
        "isEnabled": true,
        "resources": {
            "catalog": level(&["wh"]), "schema": level(&["sales"]), "table": level(&["orders"])
        },
        "rowFilterPolicyItems": [
            {"users": ["bob"], "groups": [], "roles": [],
             "accesses": [{"type": "select", "isAllowed": true}],
             "rowFilterInfo": {"filterExpr": ""}},
            {"users": [], "groups": ["public"], "roles": [],
             "accesses": [{"type": "select", "isAllowed": true}],
             "rowFilterInfo": {"filterExpr": expr}}
        ],
        "policyItems": [], "denyPolicyItems": []
    })
}

/// A row filter and its compiled rule are equivalent; a changed expression is
/// caught with both expressions rendered.
#[test]
fn a_row_filter_is_compared_by_its_expression() {
    let body = filter_policy("region = 'EU'");
    let before = envelope(vec![body.clone()], vec![]);
    let after = envelope(compiled_rules(&body, ServiceKind::Resource), vec![]);
    let ds = gate(&before, &after);
    assert!(ds.is_empty(), "expected equivalent:{}", summary(&ds));

    let wrong = envelope(
        compiled_rules(&filter_policy("region = 'US'"), ServiceKind::Resource),
        vec![],
    );
    let ds = gate(&before, &wrong);
    assert!(!ds.is_empty(), "a changed filter must be caught");
    assert!(
        ds.iter()
            .all(|d| d.check == Check::RowFilter && d.user != "bob"),
        "only non-exempt users' filters change:{}",
        summary(&ds)
    );
    let on_table = ds
        .iter()
        .find(|d| d.resource == "wh.sales.orders")
        .unwrap_or_else(|| panic!("no disagreement on the table:{}", summary(&ds)));
    assert_eq!(
        on_table.before,
        "filtered by \"region = 'EU'\" (policy \"only EU rows\")"
    );
    assert!(on_table.after.starts_with("filtered by \"region = 'US'\""));
}

// ── every affected profile ───────────────────────────────────────────────

/// The migrated grant compiles into EVERY profile serving `wh`. On the source
/// profile P it replaces the hand-written policy (equivalent); on profile Q,
/// which never granted alice anything on `wh`, it is new access. The gate
/// over both reports exactly Q's disagreements, each naming Q.
#[test]
fn a_migration_equivalent_on_its_own_profile_but_widening_another_is_caught_on_that_profile() {
    let body = hand_policy(
        "sales read orders",
        &["wh"],
        vec![item(&["alice"], &[], &["select"])],
    );
    let migrated = compiled(&grants_of(&body));

    let p_before = envelope(vec![body.clone()], vec![]);
    let p_after = envelope(migrated.clone(), vec![]);

    let q_own = hand_policy(
        "carol reads lake",
        &["lake"],
        vec![item(&["carol"], &[], &["select"])],
    );
    let q_before = envelope(vec![q_own.clone()], vec![]);
    let mut q_after_policies = vec![q_own];
    q_after_policies.extend(migrated);
    let q_after = envelope(q_after_policies, vec![]);

    let def = trino_def();
    let p_probes = derive_probes(&p_before, &p_after, &def, &[]);
    let q_probes = derive_probes(&q_before, &q_after, &def, &[]);
    let cases = [
        case(
            1,
            &p_before,
            &p_after,
            &def,
            &[PluginVersion::Ranger2_5_0],
            p_probes,
        ),
        case(
            2,
            &q_before,
            &q_after,
            &def,
            &[PluginVersion::Ranger2_5_0],
            q_probes,
        ),
    ];

    assert_eq!(
        compare(&cases[0]).expect("answers"),
        vec![],
        "P is equivalent"
    );
    let q_alone = compare(&cases[1]).expect("answers");
    assert!(!q_alone.is_empty(), "Q changes");

    let ds = compare_profiles(&cases).expect("answers");
    assert_eq!(ds, q_alone, "exactly Q's disagreements, nothing from P");
    assert!(ds
        .iter()
        .all(|d| d.service_id == 2 && d.service_name == "profile-2" && d.user == "alice"));
    assert!(ds.iter().any(|d| d.resource == "wh.sales.orders"
        && d.before == "denied: no policy matched"
        && d.after.starts_with("allowed")));
    // …and a profile the gate cannot evaluate refuses the whole gate, even
    // when it comes AFTER one that answered (the loop must reach it).
    let mut unversioned = cases[1].clone();
    unversioned.versions.clear();
    assert_eq!(
        compare_profiles(&[cases[0].clone(), unversioned]),
        Err(GateRefusal::NoPluginVersions { service_id: 2 })
    );
}

// ── vacuous answers are refused ──────────────────────────────────────────

#[test]
fn a_gate_with_nothing_to_evaluate_refuses_instead_of_answering_equivalent() {
    let set = envelope(
        vec![hand_policy(
            "sales read orders",
            &["wh"],
            vec![item(&["alice"], &[], &["select"])],
        )],
        vec![],
    );
    let def = trino_def();
    let probes = derive_probes(&set, &set, &def, &[]);
    assert!(!probes.is_empty());

    // Two empty downloads mention no coordinate, so they derive no probe — and
    // that is a refusal, never a vacuous "equivalent".
    let empty = envelope(vec![], vec![]);
    let none = derive_probes(&empty, &empty, &def, &[]);
    assert!(none.is_empty());
    assert_eq!(
        compare(&case(8, &empty, &empty, &def, &BOTH, none)),
        Err(GateRefusal::NoProbes { service_id: 8 })
    );

    assert_eq!(compare_profiles(&[]), Err(GateRefusal::NoProfiles));
    assert_eq!(
        compare(&case(7, &set, &set, &def, &[], probes.clone())),
        Err(GateRefusal::NoPluginVersions { service_id: 7 })
    );
    assert_eq!(
        compare(&case(7, &set, &set, &def, &BOTH, vec![])),
        Err(GateRefusal::NoProbes { service_id: 7 })
    );
}

/// Task 7 review I-1: the migration refuses over its cap on `probe_count`,
/// before building anything — so the count must be exactly what
/// `derive_probes` would build, on every shape: plain grants, several
/// catalogs, tag policies (hypothetical tagging), and real assignments.
#[test]
fn the_probe_count_is_exactly_the_number_of_probes_derived() {
    let def = trino_def();
    let plain = hand_policy(
        "p",
        &["wh"],
        vec![item(&["alice"], &["analysts"], &["select"])],
    );
    let two = hand_policy("q", &["wh", "lake"], vec![item(&["bob"], &[], &["select"])]);
    let cases: Vec<(ServicePolicySet, ServicePolicySet, Vec<TagAssignment>)> = vec![
        (envelope(vec![], vec![]), envelope(vec![], vec![]), vec![]),
        (
            envelope(vec![plain.clone()], vec![]),
            envelope(compiled(&grants_of(&plain)), vec![]),
            vec![],
        ),
        (
            envelope(vec![two.clone()], vec![]),
            envelope(compiled(&grants_of(&two)), vec![]),
            vec![],
        ),
        (
            envelope(vec![allow_alice_at(1)], vec![tag_deny_pii()]),
            envelope(vec![allow_alice_at(1)], vec![]),
            vec![],
        ),
        (
            envelope(
                vec![plain.clone()],
                compiled_rules(&tag_mask("MASK_NULL"), ServiceKind::Tag),
            ),
            envelope(
                vec![plain],
                compiled_rules(&tag_mask("MASK_HASH"), ServiceKind::Tag),
            ),
            vec![
                assign("wh", "sales", "orders", "ssn", "phi"),
                assign("wh", "sales", "orders", "", "pii"),
            ],
        ),
    ];
    for (i, (b, a, assignments)) in cases.iter().enumerate() {
        assert_eq!(
            probe_count(b, a, &def, assignments),
            derive_probes(b, a, &def, assignments).len(),
            "case {i}"
        );
    }
}
