//! SP4d-4 Task 6 and final-review I-1: the measurement behind `MAX_PROBE_WORK`.
//!
//! `#[ignore]`d. For one Trino profile of N single-user access policies on
//! distinct tables, each for its own user `user<i>`, one of which (k = 1) gains
//! a user, it times END TO END everything the migration runs while it holds the
//! profile lock after the recompile: `planned_probe_count` (the cap check),
//! `plan_probes` (which enumerates the factor set again and builds the probes),
//! the `ProfileCase` build, and `compare`. Release only:
//!
//! ```bash
//! cargo test -p egret-perm-compile --release --test gate_timing -- --ignored --nocapture
//! ```
//!
//! Shapes: `base` (single-user allow policies), `deny` (the base plus one deny
//! policy for a group on a wildcard table) and `wild` (every policy names a
//! wildcard schema and a table prefix). Versions: 2.5.0, 2.8.0 and both. Each
//! row prints `shape=… N=… tag=… ver=… full=… pruned=… work=… count=…s plan=…s
//! compare=…s total=…s`; `work` is `pruned x versions x (policies before +
//! after)`, the unit the cap counts. The rows are what the constant's doc
//! quotes. What the migration also holds the lock across and this harness does
//! not time (the recompile's DB writes and the two `artifact_via` reads) is
//! measured by `migrate_route`'s `#[ignore]`d release dry run.

use std::time::Instant;

use egret_perm_compile::equivalence::{
    compare, plan_probes, planned_probe_count, FallbackReason, ProfileCase, PruneMode,
};
use egret_ranger_eval::eval::{EngineConfig, PluginVersion};
use egret_ranger_eval::model::ServicePolicySet;
use egret_ranger_eval::servicedef::ServiceDef;
use serde_json::{json, Value};

fn def_json(file: &str) -> Value {
    let path = format!(
        "{}/../egret-policy/src/service-defs/{file}",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("read service-def"))
        .expect("parse service-def")
}

fn level(values: &[&str]) -> Value {
    json!({"values": values, "isExcludes": false, "isRecursive": false})
}

#[derive(Clone, Copy, PartialEq)]
enum Shape {
    /// Single-user allow policies on distinct tables.
    Base,
    /// The base plus one deny policy for a group on a wildcard table.
    Deny,
    /// Every policy names a wildcard schema and a table prefix.
    Wild,
}

fn table_policy(shape: Shape, i: usize, users: &[&str]) -> Value {
    let (schema, table) = match shape {
        Shape::Wild => ("*".to_string(), format!("t{i}*")),
        _ => (format!("s{i}"), format!("t{i}")),
    };
    json!({
        "id": 500, "name": format!("p{i}"), "service": "trino-p", "policyType": 0,
        "isEnabled": true, "policyPriority": 0,
        "resources": {
            "catalog": level(&["wh"]), "schema": level(&[&schema]),
            "table": level(&[&table]), "column": level(&["*"])
        },
        "policyItems": [{
            "users": users, "groups": [], "roles": [],
            "accesses": [{"type": "select", "isAllowed": true}]
        }],
        "denyPolicyItems": [], "allowExceptions": [], "denyExceptions": []
    })
}

fn deny_policy() -> Value {
    json!({
        "id": 501, "name": "deny-pii", "service": "trino-p", "policyType": 0,
        "isEnabled": true, "policyPriority": 0,
        "resources": {
            "catalog": level(&["wh"]), "schema": level(&["*"]),
            "table": level(&["t*"]), "column": level(&["pii*"])
        },
        "policyItems": [],
        "denyPolicyItems": [{
            "users": [], "groups": ["interns"], "roles": [],
            "accesses": [{"type": "select", "isAllowed": true}]
        }],
        "allowExceptions": [], "denyExceptions": []
    })
}

fn policies(shape: Shape, n: usize) -> Vec<Value> {
    let mut v: Vec<Value> = (0..n)
        .map(|i| table_policy(shape, i, &[&format!("user{i}")]))
        .collect();
    if shape == Shape::Deny {
        v.push(deny_policy());
    }
    v
}

fn tag_mask(mask_type: &str) -> Value {
    json!({
        "id": 700, "name": "mask phi", "service": "trino-tags", "serviceType": "tag",
        "policyType": 1, "isEnabled": true,
        "resources": {"tag": level(&["phi"])},
        "dataMaskPolicyItems": [
            {"users": ["alice"], "groups": [], "roles": [],
             "accesses": [{"type": "select", "isAllowed": true}],
             "dataMaskInfo": {"dataMaskType": "MASK_NONE"}},
            {"users": [], "groups": ["public"], "roles": [],
             "accesses": [{"type": "select", "isAllowed": true}],
             "dataMaskInfo": {"dataMaskType": mask_type}}
        ],
        "policyItems": [], "denyPolicyItems": []
    })
}

fn envelope(policies: Vec<Value>, tag_policies: Vec<Value>) -> ServicePolicySet {
    let mut env = json!({
        "serviceName": "trino-p", "serviceId": 1, "policyVersion": 1,
        "policies": policies, "serviceDef": def_json("trino.json"),
    });
    if !tag_policies.is_empty() {
        env["tagPolicies"] = json!({
            "serviceName": "trino-tags", "serviceId": 99, "policyVersion": 1,
            "policies": tag_policies, "serviceDef": def_json("tag.json"),
        });
    }
    serde_json::from_value(env).expect("parse envelope")
}

#[derive(Clone, Copy)]
enum Tag {
    None,
    /// An unchanged tag mask on both sides.
    Unchanged,
    /// The one changed policy IS the tag mask (the worst case for pruning:
    /// every probe carrying `phi` stays).
    Changed,
}

fn one_row(shape: Shape, tag: Tag, n: usize, versions: &[PluginVersion], def: &ServiceDef) {
    let before_p = policies(shape, n);
    let mut after_p = before_p.clone();
    let (before_t, after_t) = match tag {
        Tag::None => {
            after_p[0] = table_policy(shape, 0, &["user0", "bob"]);
            (vec![], vec![])
        }
        Tag::Unchanged => {
            after_p[0] = table_policy(shape, 0, &["user0", "bob"]);
            (vec![tag_mask("MASK_NULL")], vec![tag_mask("MASK_NULL")])
        }
        Tag::Changed => (vec![tag_mask("MASK_NULL")], vec![tag_mask("MASK_HASH")]),
    };
    let (before, after) = (envelope(before_p, before_t), envelope(after_p, after_t));
    let policy_count = |s: &ServicePolicySet| s.policies.len() + s.tag_policies.len();

    let total = Instant::now();
    let started = Instant::now();
    let counted = planned_probe_count(&before, &after, def, &[], PruneMode::Pruned);
    let count_secs = started.elapsed().as_secs_f64();
    let started = Instant::now();
    let plan = plan_probes(&before, &after, def, &[], PruneMode::Pruned);
    let pruned = plan.probes.len();
    let case = ProfileCase {
        service_id: 1,
        service_name: "timing".into(),
        before: &before,
        after: &after,
        service_def: def,
        engine: EngineConfig::without_super_users(),
        versions: versions.to_vec(),
        probes: plan.probes,
    };
    let plan_secs = started.elapsed().as_secs_f64();
    let started = Instant::now();
    let ds = compare(&case).expect("the gate answered");
    let compare_secs = started.elapsed().as_secs_f64();
    let total_secs = total.elapsed().as_secs_f64();

    let work = pruned * versions.len() * (policy_count(&before) + policy_count(&after));
    let name = |s: Shape| match s {
        Shape::Base => "base",
        Shape::Deny => "deny",
        Shape::Wild => "wild",
    };
    let label = match tag {
        Tag::None => "none",
        Tag::Unchanged => "unchanged",
        Tag::Changed => "changed",
    };
    let ver = match versions {
        [PluginVersion::Ranger2_5_0] => "2.5.0",
        [PluginVersion::Ranger2_8_0] => "2.8.0",
        _ => "both",
    };
    println!(
        "shape={} N={n} tag={label} ver={ver} full={} pruned={pruned} fallback={:?} work={work} \
         disagreements={} count={count_secs:.2}s plan={plan_secs:.2}s compare={compare_secs:.2}s \
         total={total_secs:.2}s us/unit={:.2}",
        name(shape),
        plan.full_count,
        plan.fallback,
        ds.len(),
        total_secs * 1e6 / work.max(1) as f64
    );
    assert_eq!(counted, pruned, "the cap check counts what is built");
    assert!(plan.fallback != Some(FallbackReason::EnvelopeDiffers));
}

#[test]
#[ignore = "timing harness; run with --release -- --ignored --nocapture"]
fn gate_time_by_profile_size() {
    let def = ServiceDef::parse(&def_json("trino.json"));
    let version_sets = [
        vec![PluginVersion::Ranger2_5_0],
        vec![PluginVersion::Ranger2_8_0],
        vec![PluginVersion::Ranger2_5_0, PluginVersion::Ranger2_8_0],
    ];
    for shape in [Shape::Base, Shape::Deny, Shape::Wild] {
        for tag in [Tag::None, Tag::Unchanged, Tag::Changed] {
            // A changed tag mask keeps every probe that carries its tag (~45% of
            // the full set), so the row grows like the unpruned gate did: N=80
            // takes about 141 s. Only the sizes under or near the cap are run.
            let sizes: &[usize] = match tag {
                Tag::Changed => &[5, 10, 15, 16, 17, 18],
                _ => &[5, 20, 40, 60, 80],
            };
            for &n in sizes {
                for versions in &version_sets {
                    one_row(shape, tag, n, versions, &def);
                }
            }
        }
    }
}
