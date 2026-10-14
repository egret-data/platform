//! View governance Task 7: what the `egret-lineage` policies cost the SP4d-3
//! migration gate's probe budget (`MAX_PROBE_WORK`).
//!
//! The shapes are `gate_timing`'s (one Trino profile of N single-user access
//! policies, one changed, k = 1), at the sizes `MAX_PROBE_WORK`'s doc quotes as
//! at the cap. Each is counted twice: as the compiler emitted it before Task 7,
//! and with what Task 7 adds to a Trino profile — one `egret-lineage` allow on
//! the catalog, the lineage deny exception on the deny policy, and the lineage
//! `MASK_NONE` first item on the tag mask. `work` is the unit the cap counts:
//! `pruned probes x versions x (policies before + after)`.
//!
//! Run with `--nocapture` to print the rows quoted in the Task 7 report.

use egret_perm_compile::equivalence::{planned_probe_count, PruneMode};
use egret_ranger_eval::model::ServicePolicySet;
use egret_ranger_eval::servicedef::ServiceDef;
use serde_json::{json, Value};

const MAX_PROBE_WORK: usize = 1_300_000;

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

#[derive(Clone, Copy, PartialEq, Debug)]
enum Shape {
    Base,
    Deny,
    Wild,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Tag {
    None,
    Unchanged,
    Changed,
}

fn table_policy(shape: Shape, i: usize, users: &[&str]) -> Value {
    let (schema, table) = match shape {
        Shape::Wild => ("*".to_string(), format!("t{i}*")),
        _ => (format!("s{i}"), format!("t{i}")),
    };
    json!({
        "id": 500 + i, "name": format!("p{i}"), "service": "trino-p", "policyType": 0,
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

fn deny_policy(lineage: bool) -> Value {
    let exceptions = if lineage {
        json!([{"users": ["egret-lineage"], "groups": [], "roles": [],
                "accesses": [{"type": "select", "isAllowed": true}]}])
    } else {
        json!([])
    };
    json!({
        "id": 401, "name": "deny-pii", "service": "trino-p", "policyType": 0,
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
        "allowExceptions": [], "denyExceptions": exceptions
    })
}

fn lineage_allow() -> Value {
    json!({
        "id": 400, "name": "perm-clearance-lineage-wh", "service": "trino-p", "policyType": 0,
        "isEnabled": true, "policyPriority": 0,
        "resources": {
            "catalog": level(&["wh"]), "schema": level(&["*"]),
            "table": level(&["*"]), "column": level(&["*"])
        },
        "policyItems": [{
            "users": ["egret-lineage"], "groups": [], "roles": [],
            "accesses": [{"type": "select", "isAllowed": true}, {"type": "use", "isAllowed": true}]
        }],
        "denyPolicyItems": [], "allowExceptions": [], "denyExceptions": []
    })
}

fn tag_mask(mask_type: &str, lineage: bool) -> Value {
    let mut items = vec![
        json!({"users": ["alice"], "groups": [], "roles": [],
               "accesses": [{"type": "select", "isAllowed": true}],
               "dataMaskInfo": {"dataMaskType": "MASK_NONE"}}),
        json!({"users": [], "groups": ["public"], "roles": [],
               "accesses": [{"type": "select", "isAllowed": true}],
               "dataMaskInfo": {"dataMaskType": mask_type}}),
    ];
    if lineage {
        items.insert(
            0,
            json!({"users": ["egret-lineage"], "groups": [], "roles": [],
                   "accesses": [{"type": "select", "isAllowed": true}],
                   "dataMaskInfo": {"dataMaskType": "MASK_NONE"}}),
        );
    }
    json!({
        "id": 700, "name": "mask phi", "service": "trino-tags", "serviceType": "tag",
        "policyType": 1, "isEnabled": true,
        "resources": {"tag": level(&["phi"])},
        "dataMaskPolicyItems": items,
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

fn work(
    shape: Shape,
    tag: Tag,
    n: usize,
    versions: usize,
    lineage: bool,
    def: &ServiceDef,
) -> (usize, usize) {
    let mut before_p: Vec<Value> = (0..n)
        .map(|i| table_policy(shape, i, &[&format!("user{i}")]))
        .collect();
    if shape == Shape::Deny {
        before_p.push(deny_policy(lineage));
    }
    if lineage {
        before_p.push(lineage_allow());
    }
    let mut after_p = before_p.clone();
    let (bt, at) = match tag {
        Tag::None => {
            after_p[0] = table_policy(shape, 0, &["user0", "bob"]);
            (vec![], vec![])
        }
        Tag::Unchanged => {
            after_p[0] = table_policy(shape, 0, &["user0", "bob"]);
            (
                vec![tag_mask("MASK_NULL", lineage)],
                vec![tag_mask("MASK_NULL", lineage)],
            )
        }
        Tag::Changed => (
            vec![tag_mask("MASK_NULL", lineage)],
            vec![tag_mask("MASK_HASH", lineage)],
        ),
    };
    let (before, after) = (envelope(before_p, bt), envelope(after_p, at));
    let probes = planned_probe_count(&before, &after, def, &[], PruneMode::Pruned);
    let count = |s: &ServicePolicySet| s.policies.len() + s.tag_policies.len();
    (probes, probes * versions * (count(&before) + count(&after)))
}

#[test]
fn the_lineage_policies_cost_of_the_probe_budget() {
    let def = ServiceDef::parse(&def_json("trino.json"));
    let mut rows = Vec::new();
    for (shape, tag, n) in [
        (Shape::Base, Tag::None, 91),
        (Shape::Deny, Tag::None, 91),
        (Shape::Wild, Tag::None, 91),
        (Shape::Base, Tag::Unchanged, 71),
        (Shape::Wild, Tag::Unchanged, 71),
        (Shape::Base, Tag::Changed, 17),
        (Shape::Wild, Tag::Changed, 17),
    ] {
        for versions in [1, 2] {
            let (p0, w0) = work(shape, tag, n, versions, false, &def);
            let (p1, w1) = work(shape, tag, n, versions, true, &def);
            println!(
                "shape={shape:?} tag={tag:?} N={n} versions={versions} probes {p0} -> {p1} \
                 work {w0} -> {w1} ({:+.1}%) cap {MAX_PROBE_WORK}",
                (w1 as f64 / w0 as f64 - 1.0) * 100.0
            );
            rows.push((w0, w1));
        }
    }
    // The lineage additions can only add work: one more policy on each side
    // and one more user factor. Asserted so a reading of this test that the
    // additions were free cannot survive a change that made them so by
    // dropping them from the fixture.
    assert!(rows.iter().all(|(w0, w1)| w1 > w0), "{rows:?}");
}
