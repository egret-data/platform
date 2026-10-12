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
    changed_policies, compare, compare_profiles, derive_probes, plan_probes, planned_probe_count,
    probe_count, Check, Disagreement, FallbackReason, GateRefusal, Probe, ProfileCase, PruneMode,
    TagAssignment, Tagging, PROBE_SIBLING_PREFIX, PROBE_UNNAMED_USER,
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

/// A disagreement that is no CHANGE: both sides are `Undecidable` for the same
/// reason (`compare` counts any `Undecidable` as a disagreement, so a pair of
/// UNCHANGED policies the evaluator cannot arbitrate shows up once per probe).
/// Pruning drops such a probe wherever no changed policy may be a candidate —
/// the theorem's "decides identically" — and `pruned_agrees` accounts for that
/// class explicitly instead of hiding it in a looser comparison.
fn unchanged_undecidable(d: &Disagreement) -> bool {
    d.before == d.after && d.before.starts_with("cannot be decided")
}

/// SP4d-4 Task 5. The gate on `before -> after` with the FULL probe set and
/// with the PRUNED one: asserts the pruned probes are a subset of the full
/// ones and that the two find exactly the same disagreements (as multisets),
/// up to [`unchanged_undecidable`] ones that pruning is allowed to drop and
/// never to invent. Returns the pruned plan, the full set's other
/// disagreements, and how many unchanged-undecidable ones pruning dropped.
/// Every case in this file is run through it, so a pruning that drops a probe
/// which disagrees fails the case that owns it.
fn pruned_agrees(
    before: &ServicePolicySet,
    after: &ServicePolicySet,
    assignments: &[TagAssignment],
    versions: &[PluginVersion],
) -> (
    egret_perm_compile::equivalence::ProbePlan,
    Vec<Disagreement>,
    usize,
) {
    let def = trino_def();
    let full = plan_probes(before, after, &def, assignments, PruneMode::Full);
    let pruned = plan_probes(before, after, &def, assignments, PruneMode::Pruned);
    assert_eq!(full.fallback, None);
    assert_eq!(full.probes, derive_probes(before, after, &def, assignments));
    assert_eq!(pruned.full_count, full.probes.len());
    // Pruning keeps `derive_probes`' order, so "subset" is "subsequence".
    let mut rest = full.probes.iter();
    assert!(
        pruned.probes.iter().all(|p| rest.any(|f| f == p)),
        "pruned must be a subsequence of full"
    );
    assert_eq!(
        planned_probe_count(before, after, &def, assignments, PruneMode::Pruned),
        pruned.probes.len()
    );
    let key = |d: &Disagreement| format!("{d:?}");
    let run = |probes: Vec<Probe>| {
        let mut ds = match compare(&case(1, before, after, &def, versions, probes)) {
            Ok(ds) => ds,
            Err(GateRefusal::NoProbes { .. }) => vec![],
            Err(e) => panic!("{e:?}"),
        };
        ds.sort_by_key(key);
        ds
    };
    let (full_ds, pruned_ds) = (run(full.probes), run(pruned.probes.clone()));
    let (full_inert, full_ds): (Vec<_>, Vec<_>) =
        full_ds.into_iter().partition(unchanged_undecidable);
    let (pruned_inert, pruned_ds): (Vec<_>, Vec<_>) =
        pruned_ds.into_iter().partition(unchanged_undecidable);
    assert_eq!(
        pruned_ds,
        full_ds,
        "pruning changed the disagreements:\n full:{}\n pruned:{}",
        summary(&full_ds),
        summary(&pruned_ds)
    );
    assert!(
        pruned_inert.iter().all(|d| full_inert.contains(d)),
        "pruning invented a disagreement"
    );
    let dropped = full_inert.len() - pruned_inert.len();
    (pruned, full_ds, dropped)
}

/// The gate on one profile, probes derived from the two sets, no tags.
fn gate(before: &ServicePolicySet, after: &ServicePolicySet) -> Vec<Disagreement> {
    pruned_agrees(before, after, &[], &BOTH);
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

    pruned_agrees(&before, &after, &assignments, &BOTH);
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

    pruned_agrees(&before, &after, &assignments, &BOTH);
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
    pruned_agrees(&before, &after, &assignments, &BOTH);
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

    pruned_agrees(&before, &after, &assignments, &BOTH);
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
    pruned_agrees(&before, &hashed, &assignments, &BOTH);
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
    pruned_agrees(&before, &unexempt, &assignments, &BOTH);
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

    pruned_agrees(&before, &same, &[], &BOTH);
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

    pruned_agrees(&before, &hashed, &[], &BOTH);
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

    for c in &cases {
        pruned_agrees(c.before, c.after, &[], &[PluginVersion::Ranger2_5_0]);
    }
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

// ── SP4d-4 Task 5: pruning the probes to where a changed policy may matter ──

/// A resource policy on `wh.<schema>.<table>.*` for `users` (allow) with
/// `deny_users` as deny items.
fn orders_like(
    name: &str,
    schema: &str,
    table: &str,
    users: &[&str],
    deny_users: &[&str],
    priority: i32,
) -> Value {
    let mut p = json!({
        "id": 500, "name": name, "service": "trino-p", "policyType": 0, "isEnabled": true,
        "policyPriority": priority,
        "resources": {
            "catalog": level(&["wh"]), "schema": level(&[schema]),
            "table": level(&[table]), "column": level(&["*"])
        },
        "policyItems": if users.is_empty() { vec![] } else { vec![item(users, &[], &["select"])] },
        "denyPolicyItems":
            if deny_users.is_empty() { vec![] } else { vec![item(deny_users, &[], &["select"])] },
        "allowExceptions": [], "denyExceptions": []
    });
    p["id"] = json!(500);
    p
}

/// `wh.sales.*.*` — the spec's `H`.
fn sales_wide(name: &str, users: &[&str], priority: i32) -> Value {
    let mut p = orders_like(name, "sales", "*", users, &[], priority);
    p["resources"]["table"] = level(&["*"]);
    p
}

fn disagreements_at<'a>(
    ds: &'a [Disagreement],
    user: &'a str,
    prefix: &'a str,
) -> impl Iterator<Item = &'a Disagreement> {
    ds.iter()
        .filter(move |d| d.user == user && d.resource.starts_with(prefix))
}

/// The spec's example: an UNCHANGED deny on `wh.sales.orders` for alice, an
/// allow `H` on `wh.sales.*`, and `H` replaced by `G` that differs only by a
/// strictly higher priority. The disagreement sits at `wh.sales.orders.*`,
/// which `G` may match — so it is kept — and it is reported.
#[test]
fn an_unchanged_interacting_deny_is_still_caught() {
    let deny = orders_like("deny alice orders", "sales", "orders", &[], &["alice"], 0);
    let h = sales_wide("alice reads sales", &["alice"], 0);
    let g = sales_wide("alice reads sales", &["alice"], 1);
    let before = envelope(vec![deny.clone(), h], vec![]);
    let after = envelope(vec![deny.clone(), g], vec![]);

    let (plan, ds, _) = pruned_agrees(&before, &after, &[], &BOTH);
    // A non-zero priority in Δ is a fallback condition: the full set.
    assert_eq!(plan.fallback, Some(FallbackReason::NonZeroPriorityInDelta));
    assert_eq!(plan.probes.len(), plan.full_count);
    assert!(
        disagreements_at(&ds, "alice", "wh.sales.orders").any(|d| d.access_type == "select"
            && d.before.starts_with("denied")
            && d.after.starts_with("allowed")),
        "the higher-priority allow reopens the unchanged deny:{}",
        summary(&ds)
    );

    // The same shape with `G` differing from `H` by an added user instead:
    // pruning is ON, and the deny that still holds at `orders` (same priority,
    // deny wins) yields no disagreement there, while the new user's access to
    // the rest of `sales` is reported.
    let deny_both = orders_like(
        "deny alice orders",
        "sales",
        "orders",
        &[],
        &["alice", "bob"],
        0,
    );
    let h = sales_wide("alice reads sales", &["alice"], 0);
    let g = sales_wide("alice reads sales", &["alice", "bob"], 0);
    let before = envelope(vec![deny_both.clone(), h], vec![]);
    let after = envelope(vec![deny_both, g], vec![]);
    let (plan, ds, _) = pruned_agrees(&before, &after, &[], &BOTH);
    assert_eq!(plan.fallback, None);
    assert!(
        plan.probes.len() < plan.full_count,
        "pruning must be active here"
    );
    assert!(
        disagreements_at(&ds, "bob", "wh.sales.egret-probe")
            .any(|d| d.after.starts_with("allowed")),
        "bob gains the rest of sales:{}",
        summary(&ds)
    );
    assert_eq!(
        disagreements_at(&ds, "bob", "wh.sales.orders").count(),
        0,
        "the unchanged deny still holds at orders:{}",
        summary(&ds)
    );
    // ...and the orders coordinate was KEPT (G may match it), not dropped.
    assert!(
        plan.probes.iter().any(|p| p.request.user == "bob"
            && p.request.resource.get("table") == Some("orders")
            && p.request.access_type == "select"),
        "orders is a coordinate G may match"
    );
}

/// SP4d-3's version-split shape: a tag DENY plus a strictly higher-priority
/// no-deny resource ALLOW. Δ is the resource ALLOW; the interacting tag DENY is
/// unchanged. The decided flip is reported on 2.8.0 and the refusal on 2.5.0,
/// exactly as under the full set (`pruned_agrees` pins the equality).
#[test]
fn a_tag_interaction_is_still_caught_under_pruning() {
    let before = envelope(vec![allow_alice_at(0)], vec![tag_deny_pii()]);
    let after = envelope(vec![allow_alice_at(1)], vec![tag_deny_pii()]);
    let assignments = [assign("wh", "sales", "orders", "email", "pii")];
    let (plan, ds, _) = pruned_agrees(&before, &after, &assignments, &BOTH);
    assert_eq!(plan.fallback, Some(FallbackReason::NonZeroPriorityInDelta));
    assert!(
        ds.iter()
            .any(|d| d.plugin_version == PluginVersion::Ranger2_8_0
                && d.tags == ["pii"]
                && d.before.starts_with("denied by a deny item")
                && d.after.starts_with("allowed")),
        "{}",
        summary(&ds)
    );

    // Pruning ON: the same tag DENY, unchanged, and a Δ at priority 0 that
    // covers the tagged coordinate (a user is added to the allow). The tag
    // deny still wins there; the column elsewhere in the table changes.
    let tagged_before = orders_like("orders reader", "sales", "orders", &["alice"], &[], 0);
    let tagged_after = orders_like(
        "orders reader",
        "sales",
        "orders",
        &["alice", "bob"],
        &[],
        0,
    );
    let before = envelope(vec![tagged_before], vec![tag_deny_pii()]);
    let after = envelope(vec![tagged_after], vec![tag_deny_pii()]);
    let (plan, ds, _) = pruned_agrees(&before, &after, &assignments, &BOTH);
    assert_eq!(plan.fallback, None);
    assert!(
        disagreements_at(&ds, "bob", "wh.sales.orders").any(|d| d.tagging == Tagging::Actual
            && d.tags.is_empty()
            && d.after.starts_with("allowed")),
        "bob gains the untagged columns:{}",
        summary(&ds)
    );
}

/// A tag policy in Δ keeps the probes whose tags it covers, actual or
/// hypothetical, and the answer is the full set's.
#[test]
fn a_changed_tag_policy_keeps_the_probes_that_carry_its_tag() {
    let reader = hand_policy(
        "everyone reads orders",
        &["wh"],
        vec![item(&[], &["public"], &["select"])],
    );
    let before = envelope(vec![reader.clone()], vec![tag_mask("MASK_NULL")]);
    let after = envelope(
        vec![reader],
        compiled_rules(&tag_mask("MASK_HASH"), ServiceKind::Tag),
    );
    let assignments = [assign("wh", "sales", "orders", "ssn", "phi")];
    let (plan, ds, _) = pruned_agrees(&before, &after, &assignments, &BOTH);
    assert_eq!(plan.fallback, None);
    assert!(!ds.is_empty());
    assert!(plan.probes.iter().all(|p| p
        .request
        .resolved_tags
        .as_ref()
        .is_some_and(|t| !t.as_slice().is_empty())
        || true));
    // Every probe is at a tagging that carries `phi`.
    let carrying = |p: &Probe| {
        p.request
            .resolved_tags
            .as_ref()
            .is_some_and(|t| t.as_slice().iter().any(|t| t.name == "phi"))
    };
    assert!(plan.probes.iter().all(carrying), "only phi-carrying probes");
    assert!(plan.probes.len() < plan.full_count);
}

/// 20 policies on 20 tables, one of which changes: pruning must remove most
/// probes (otherwise "pruned == full" above is satisfied by keeping all).
#[test]
fn pruning_removes_probes() {
    let table = |i: usize, users: &[&str]| {
        orders_like(
            &format!("p{i}"),
            &format!("s{i}"),
            &format!("t{i}"),
            users,
            &[],
            0,
        )
    };
    let before: Vec<Value> = (0..20).map(|i| table(i, &["alice"])).collect();
    let mut after = before.clone();
    after[7] = table(7, &["alice", "bob"]);
    let (before, after) = (envelope(before, vec![]), envelope(after, vec![]));
    let def = trino_def();

    let (plan, ds, _) = pruned_agrees(&before, &after, &[], &[PluginVersion::Ranger2_5_0]);
    assert_eq!(plan.fallback, None);
    assert_eq!(plan.full_count, probe_count(&before, &after, &def, &[]));
    assert!(
        plan.probes.len() < plan.full_count / 4,
        "pruned {} of {}",
        plan.probes.len(),
        plan.full_count
    );
    assert!(disagreements_at(&ds, "bob", "wh.s7.t7").count() > 0);
    assert!(ds
        .iter()
        .all(|d| d.resource.starts_with("wh") && d.user == "bob"));
}

#[test]
fn every_fallback_condition_gives_the_full_set() {
    let def = trino_def();
    let base = orders_like("p", "sales", "orders", &["alice"], &[], 0);
    let widened = orders_like("p", "sales", "orders", &["alice", "bob"], &[], 0);
    let other = orders_like("q", "hr", "people", &["carol"], &[], 0);
    let before = envelope(vec![base.clone(), other.clone()], vec![]);
    let after = envelope(vec![widened.clone(), other.clone()], vec![]);
    let plan_of = |b: &ServicePolicySet, a: &ServicePolicySet| {
        plan_probes(b, a, &def, &[], PruneMode::Pruned)
    };
    let expect_full = |b: &ServicePolicySet, a: &ServicePolicySet, why: FallbackReason| {
        let plan = plan_of(b, a);
        assert_eq!(plan.fallback, Some(why));
        assert_eq!(plan.probes.len(), plan.full_count, "{why:?}");
        assert_eq!(plan.probes, derive_probes(b, a, &def, &[]), "{why:?}");
        assert_eq!(
            planned_probe_count(b, a, &def, &[], PruneMode::Pruned),
            plan.full_count
        );
    };

    // Control: no fallback, and pruning is active.
    let control = plan_of(&before, &after);
    assert_eq!(control.fallback, None);
    assert!(control.probes.len() < control.full_count);

    // 1. E(B) != E(A): the service-def, the tag service-def, the service id,
    //    and an envelope construct on one side.
    let mut a = after.clone();
    a.service_def = json!({"name": "somebody else's def"});
    expect_full(&before, &a, FallbackReason::EnvelopeDiffers);
    let mut a = after.clone();
    a.tag_service_def = json!({"name": "tag"});
    expect_full(&before, &a, FallbackReason::EnvelopeDiffers);
    let mut a = after.clone();
    a.service_id = Some(2);
    expect_full(&before, &a, FallbackReason::EnvelopeDiffers);
    let mut a = after.clone();
    a.unmodelled
        .push(egret_ranger_eval::model::Unmodelled::SecurityZones);
    expect_full(&before, &a, FallbackReason::EnvelopeDiffers);
    //    ...but a bumped `policyVersion` is not an envelope difference.
    let mut a = after.clone();
    a.policy_version = Some(99);
    assert_eq!(plan_of(&before, &a).fallback, None);

    // 1b. The same unmodelled construct on both sides.
    let (mut b, mut a) = (before.clone(), after.clone());
    for s in [&mut b, &mut a] {
        s.unmodelled
            .push(egret_ranger_eval::model::Unmodelled::SecurityZones);
    }
    expect_full(&b, &a, FallbackReason::EnvelopeUnmodelled);

    // 2. A Δ policy with isDenyAllElse, or a non-zero priority.
    let mut deny_all = widened.clone();
    deny_all["isDenyAllElse"] = json!(true);
    expect_full(
        &before,
        &envelope(vec![deny_all, other.clone()], vec![]),
        FallbackReason::DenyAllElseInDelta,
    );
    expect_full(
        &before,
        &envelope(
            vec![
                orders_like("p", "sales", "orders", &["alice"], &[], 3),
                other.clone(),
            ],
            vec![],
        ),
        FallbackReason::NonZeroPriorityInDelta,
    );
    //    ...the priority of the BEFORE side counts too (both versions are in Δ).
    expect_full(
        &envelope(
            vec![
                orders_like("p", "sales", "orders", &["alice"], &[], 3),
                other.clone(),
            ],
            vec![],
        ),
        &after,
        FallbackReason::NonZeroPriorityInDelta,
    );

    // 3. Nothing changed: every question is still asked.
    expect_full(&before, &before.clone(), FallbackReason::EmptyDelta);
}

/// Δ is `(name, body without id)`, compared as values.
#[test]
fn an_id_only_change_is_not_in_delta() {
    let p = orders_like("p", "sales", "orders", &["alice"], &[], 0);
    let mut reid = p.clone();
    reid["id"] = json!(9001);
    // `jsonb` hands the keys back in another order.
    let reordered: Value = serde_json::from_str(
        &serde_json::to_string(&reid)
            .map(|s| {
                let v: serde_json::Map<String, Value> = serde_json::from_str(&s).unwrap();
                let mut keys: Vec<_> = v.into_iter().collect();
                keys.reverse();
                format!(
                    "{{{}}}",
                    keys.iter()
                        .map(|(k, v)| format!("\"{k}\":{v}"))
                        .collect::<Vec<_>>()
                        .join(",")
                )
            })
            .unwrap(),
    )
    .unwrap();
    let before = envelope(vec![p.clone()], vec![]);
    let after = envelope(vec![reordered], vec![]);
    assert!(changed_policies(&before, &after).is_empty());

    // A changed body is in Δ on BOTH sides; a renamed policy too.
    let widened = envelope(
        vec![orders_like(
            "p",
            "sales",
            "orders",
            &["alice", "bob"],
            &[],
            0,
        )],
        vec![],
    );
    assert_eq!(changed_policies(&before, &widened).len(), 2);
    let renamed = envelope(
        vec![orders_like("p2", "sales", "orders", &["alice"], &[], 0)],
        vec![],
    );
    assert_eq!(changed_policies(&before, &renamed).len(), 2);
    // Two same-named policies, one changed: multiset difference.
    let twin = envelope(vec![p.clone(), p.clone()], vec![]);
    let twin_changed = envelope(
        vec![
            p.clone(),
            orders_like("p", "sales", "orders", &["alice", "bob"], &[], 0),
        ],
        vec![],
    );
    assert_eq!(changed_policies(&twin, &twin_changed).len(), 2);
}

/// A grant to the `public` group is felt by principals no changed policy
/// names — alice (named by an unchanged policy) and the unnamed user. Pruning
/// the principals to those Δ names would lose them.
#[test]
fn a_public_group_grant_is_felt_by_principals_the_changed_policy_does_not_name() {
    let alice = orders_like("alice reads hr", "hr", "people", &["alice"], &[], 0);
    let before = envelope(vec![alice.clone()], vec![]);
    let mut public = orders_like("everyone reads orders", "sales", "orders", &[], &[], 0);
    public["policyItems"] = json!([item(&[], &["public"], &["select"])]);
    let after = envelope(vec![alice, public], vec![]);
    let (plan, ds, _) = pruned_agrees(&before, &after, &[], &BOTH);
    assert_eq!(plan.fallback, None);
    assert!(
        disagreements_at(&ds, PROBE_UNNAMED_USER, "wh.sales.orders").count() > 0,
        "{}",
        summary(&ds)
    );
    assert!(
        disagreements_at(&ds, "alice", "wh.sales.orders").count() > 0,
        "{}",
        summary(&ds)
    );
}

#[test]
fn an_empty_pruned_set_is_a_refusal_not_an_answer() {
    // No coordinate at all (a policy with no resource levels, on one side):
    // the plan is empty and `compare` refuses, never "equivalent".
    let def = trino_def();
    let before = envelope(vec![], vec![]);
    let mut empty_res = orders_like("p", "sales", "orders", &["alice"], &[], 0);
    empty_res["resources"] = json!({});
    let after = envelope(vec![empty_res], vec![]);
    let plan = plan_probes(&before, &after, &def, &[], PruneMode::Pruned);
    assert!(plan.probes.is_empty());
    assert_eq!(plan.fallback, None);
    assert_eq!(
        compare(&case(3, &before, &after, &def, &BOTH, plan.probes)),
        Err(GateRefusal::NoProbes { service_id: 3 })
    );
}

#[test]
fn planned_probe_count_matches_the_build() {
    let def = trino_def();
    let assignments = [assign("wh", "sales", "orders", "email", "pii")];
    let table = |i: usize, users: &[&str]| {
        orders_like(
            &format!("p{i}"),
            &format!("s{i}"),
            &format!("t{i}"),
            users,
            &[],
            0,
        )
    };
    let twenty: Vec<Value> = (0..20).map(|i| table(i, &["alice"])).collect();
    let mut twenty_after = twenty.clone();
    twenty_after[7] = table(7, &["alice", "bob"]);
    let mut changed_def = envelope(twenty.clone(), vec![]);
    changed_def.service_def = json!({"x": 1});
    let cases: Vec<(ServicePolicySet, ServicePolicySet, &[TagAssignment])> = vec![
        (
            envelope(
                vec![
                    orders_like("d", "sales", "orders", &[], &["alice"], 0),
                    sales_wide("h", &["alice"], 0),
                ],
                vec![],
            ),
            envelope(
                vec![
                    orders_like("d", "sales", "orders", &[], &["alice"], 0),
                    sales_wide("h", &["alice"], 1),
                ],
                vec![],
            ),
            &[],
        ),
        (
            envelope(vec![allow_alice_at(0)], vec![tag_deny_pii()]),
            envelope(vec![allow_alice_at(0)], vec![]),
            &assignments,
        ),
        (
            envelope(twenty, vec![]),
            envelope(twenty_after, vec![]),
            &[],
        ),
        (envelope(vec![allow_alice_at(0)], vec![]), changed_def, &[]),
    ];
    for (i, (b, a, asg)) in cases.iter().enumerate() {
        for mode in [PruneMode::Full, PruneMode::Pruned] {
            assert_eq!(
                planned_probe_count(b, a, &def, asg, mode),
                plan_probes(b, a, &def, asg, mode).probes.len(),
                "case {i} {mode:?}"
            );
        }
    }
}

// ── the seeded corpus ────────────────────────────────────────────────────

/// SplitMix64: a fixed, dependency-free generator, so the corpus is the same
/// on every machine.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
    fn subset<'a>(&mut self, xs: &[&'a str], min: usize) -> Vec<&'a str> {
        loop {
            let s: Vec<&str> = xs.iter().copied().filter(|_| self.chance(50)).collect();
            if s.len() >= min {
                return s;
            }
        }
    }
}

const CORPUS_SEED: u64 = 0x5D4_0004_0005;
const CORPUS_PAIRS: usize = 200;

/// One random policy named `name`: an access allow (sometimes with deny items,
/// sometimes at priority 1, sometimes with `isExcludes` on a level), a column
/// mask, or a row filter, on a random prefix of catalog/schema/table/column.
fn random_policy(rng: &mut Rng, name: &str) -> Value {
    let catalog = *rng.pick(&["wh", "lake"]);
    let schema = *rng.pick(&["sales", "hr", "*"]);
    let table = *rng.pick(&["orders", "people", "*"]);
    let column = *rng.pick(&["email", "ssn", "*"]);
    let depth = 1 + rng.below(4);
    let mut resources = serde_json::Map::new();
    let levels = [
        ("catalog", catalog),
        ("schema", schema),
        ("table", table),
        ("column", column),
    ];
    for (n, v) in levels.iter().take(depth) {
        resources.insert((*n).to_string(), level(&[v]));
    }
    let users = rng.subset(&["alice", "bob"], 0);
    let groups = if rng.chance(25) {
        vec!["public"]
    } else {
        vec![]
    };
    let kind = rng.below(10);
    let mut p = json!({
        "id": 1000, "name": name, "service": "trino-p", "policyType": 0, "isEnabled": true,
        "resources": resources,
        "policyItems": [], "denyPolicyItems": [], "allowExceptions": [], "denyExceptions": []
    });
    match kind {
        0..=5 => {
            let accesses: &[&str] = if rng.chance(50) {
                &["select"]
            } else {
                &["select", "insert"]
            };
            p["policyItems"] = json!([item(&users, &groups, accesses)]);
            if rng.chance(30) {
                p["denyPolicyItems"] =
                    json!([item(&rng.subset(&["alice", "bob"], 1), &[], &["select"])]);
            }
            if rng.chance(20) {
                p["policyPriority"] = json!(1);
            }
            if depth >= 2 && rng.chance(15) {
                p["resources"]["schema"]["isExcludes"] = json!(true);
            }
        }
        6..=7 => {
            p["policyType"] = json!(1);
            p["resources"] = json!({
                "catalog": level(&[catalog]), "schema": level(&[schema]),
                "table": level(&[table]), "column": level(&[column])
            });
            p["dataMaskPolicyItems"] = json!([
                mask_item(&["alice"], &[], "MASK_NONE"),
                mask_item(&[], &["public"], *rng.pick(&["MASK_NULL", "MASK_HASH"]))
            ]);
        }
        _ => {
            p["policyType"] = json!(2);
            p["resources"] = json!({
                "catalog": level(&[catalog]), "schema": level(&[schema]), "table": level(&[table])
            });
            p["rowFilterPolicyItems"] = json!([
                {"users": ["bob"], "groups": [], "roles": [],
                 "accesses": [{"type": "select", "isAllowed": true}],
                 "rowFilterInfo": {"filterExpr": ""}},
                {"users": [], "groups": ["public"], "roles": [],
                 "accesses": [{"type": "select", "isAllowed": true}],
                 "rowFilterInfo": {"filterExpr": *rng.pick(&["r = 1", "r = 2"])}}
            ]);
        }
    }
    p
}

fn random_tag_policy(rng: &mut Rng) -> Value {
    if rng.chance(50) {
        tag_deny_pii()
    } else {
        tag_mask(*rng.pick(&["MASK_NULL", "MASK_HASH"]))
    }
}

/// The corpus: `CORPUS_PAIRS` (before, after) pairs, each 2-4 random policies
/// (and sometimes a tag policy) before, and 1-2 edits after (change a policy,
/// remove one, add one, add/remove a tag policy).
fn corpus() -> Vec<(ServicePolicySet, ServicePolicySet, Vec<TagAssignment>)> {
    let mut rng = Rng(CORPUS_SEED);
    (0..CORPUS_PAIRS)
        .map(|_| {
            let n = 2 + rng.below(3);
            let mut before: Vec<Value> = (0..n)
                .map(|i| random_policy(&mut rng, &format!("p{i}")))
                .collect();
            let mut tag_before = if rng.chance(30) {
                vec![random_tag_policy(&mut rng)]
            } else {
                vec![]
            };
            let mut after = before.clone();
            let mut tag_after = tag_before.clone();
            for _ in 0..1 + rng.below(2) {
                match rng.below(6) {
                    0 | 1 | 2 if !after.is_empty() => {
                        let i = rng.below(after.len());
                        let name = format!("p{i}");
                        after[i] = random_policy(&mut rng, &name);
                    }
                    3 if after.len() > 1 => {
                        let i = rng.below(after.len());
                        after.remove(i);
                    }
                    4 => {
                        let name = format!("p{}", after.len() + 10);
                        after.push(random_policy(&mut rng, &name));
                    }
                    _ => {
                        if tag_after.is_empty() {
                            tag_after.push(random_tag_policy(&mut rng));
                        } else if rng.chance(50) {
                            tag_after.clear();
                        } else {
                            tag_after = vec![random_tag_policy(&mut rng)];
                        }
                    }
                }
            }
            // The ids are not part of Δ: give the two sides different ones.
            for (i, p) in after.iter_mut().enumerate() {
                p["id"] = json!(2000 + i);
            }
            for (i, p) in before.iter_mut().enumerate() {
                p["id"] = json!(1000 + i);
            }
            for p in tag_before.iter_mut() {
                p["id"] = json!(700);
            }
            let assignments = if rng.chance(50) {
                vec![assign(
                    "wh",
                    "sales",
                    "orders",
                    *rng.pick(&["email", ""]),
                    *rng.pick(&["pii", "phi"]),
                )]
            } else {
                vec![]
            };
            (
                envelope(before, tag_before),
                envelope(after, tag_after),
                assignments,
            )
        })
        .collect()
}

/// `compare` over the pruned probes returns EXACTLY the disagreements it
/// returns over the full set, on a fixed-seed corpus; and the corpus really
/// exercises pruning (most pairs prune, some fall back, some disagree).
#[test]
fn pruned_and_full_find_the_same_disagreements_on_a_seeded_corpus() {
    let mut pruned_pairs = 0;
    let mut fell_back = 0;
    let mut with_disagreements = 0;
    let (mut full_total, mut pruned_total, mut dropped_inert) = (0usize, 0usize, 0usize);
    for (i, (before, after, assignments)) in corpus().iter().enumerate() {
        // Same answer as the full set, version by version.
        // The two versions answer differently only where a tag policy is in
        // play, so only those pairs pay for both.
        let versions: &[PluginVersion] =
            if before.tag_policies.is_empty() && after.tag_policies.is_empty() {
                &[PluginVersion::Ranger2_8_0]
            } else {
                &BOTH
            };
        let (plan, ds, dropped) = pruned_agrees(before, after, assignments, versions);
        dropped_inert += dropped;
        full_total += plan.full_count;
        pruned_total += plan.probes.len();
        match plan.fallback {
            Some(_) => fell_back += 1,
            None if plan.probes.len() < plan.full_count => pruned_pairs += 1,
            None => {}
        }
        if !ds.is_empty() {
            with_disagreements += 1;
        }
        let _ = i;
    }
    eprintln!(
        "corpus: seed={CORPUS_SEED:#x} pairs={CORPUS_PAIRS} pruned={pruned_pairs} \
         fell_back={fell_back} with_disagreements={with_disagreements} \
         probes full={full_total} pruned={pruned_total} \
         dropped_unchanged_undecidable={dropped_inert}"
    );
    assert!(
        pruned_pairs >= CORPUS_PAIRS / 4,
        "pruned only {pruned_pairs}"
    );
    assert!(fell_back > 0, "no fallback exercised");
    assert!(
        with_disagreements >= CORPUS_PAIRS / 4,
        "only {with_disagreements} pairs disagree"
    );
    assert!(pruned_total < full_total);
}
