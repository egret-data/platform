//! SP4d-4 Task 6: the measurement behind `MAX_PROBE_EVALUATIONS`.
//!
//! `#[ignore]`d: it times `compare` over the PRUNED probe set of one Trino
//! profile of N single-user access policies on distinct tables, each for its own user `user<i>`, one of which
//! (k = 1) gains a user. Release only:
//!
//! ```bash
//! cargo test -p egret-perm-compile --release --test gate_timing -- --ignored --nocapture
//! ```
//!
//! Each row prints `N=… tag=… full=… pruned=… compare=…s` (one plugin version,
//! 2.8.0). The rows are what the constant's doc quotes.

use std::time::Instant;

use egret_perm_compile::equivalence::{
    compare, plan_probes, FallbackReason, ProfileCase, PruneMode,
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

fn table_policy(i: usize, users: &[&str]) -> Value {
    json!({
        "id": 500, "name": format!("p{i}"), "service": "trino-p", "policyType": 0,
        "isEnabled": true, "policyPriority": 0,
        "resources": {
            "catalog": level(&["wh"]), "schema": level(&[&format!("s{i}")]),
            "table": level(&[&format!("t{i}")]), "column": level(&["*"])
        },
        "policyItems": [{
            "users": users, "groups": [], "roles": [],
            "accesses": [{"type": "select", "isAllowed": true}]
        }],
        "denyPolicyItems": [], "allowExceptions": [], "denyExceptions": []
    })
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

#[test]
#[ignore = "timing harness; run with --release -- --ignored --nocapture"]
fn gate_time_by_profile_size() {
    let def = ServiceDef::parse(&def_json("trino.json"));
    for tag in [Tag::None, Tag::Unchanged, Tag::Changed] {
        for n in [5usize, 20, 40, 80, 160] {
            // A changed tag mask keeps every probe that carries its tag (~45% of
            // the full set), so the row grows like the unpruned gate did: N=80
            // takes 138 s, and N=160 about ten minutes. Not run.
            if matches!(tag, Tag::Changed) && n == 160 {
                continue;
            }
            let before_p: Vec<Value> = (0..n)
                .map(|i| table_policy(i, &[&format!("user{i}")]))
                .collect();
            let mut after_p = before_p.clone();
            let (before_t, after_t) = match tag {
                Tag::None => {
                    after_p[0] = table_policy(0, &["user0", "bob"]);
                    (vec![], vec![])
                }
                Tag::Unchanged => {
                    after_p[0] = table_policy(0, &["user0", "bob"]);
                    (vec![tag_mask("MASK_NULL")], vec![tag_mask("MASK_NULL")])
                }
                Tag::Changed => (vec![tag_mask("MASK_NULL")], vec![tag_mask("MASK_HASH")]),
            };
            let (before, after) = (envelope(before_p, before_t), envelope(after_p, after_t));
            let plan = plan_probes(&before, &after, &def, &[], PruneMode::Pruned);
            let pruned = plan.probes.len();
            let case = ProfileCase {
                service_id: 1,
                service_name: "timing".into(),
                before: &before,
                after: &after,
                service_def: &def,
                engine: EngineConfig::without_super_users(),
                versions: vec![PluginVersion::Ranger2_8_0],
                probes: plan.probes,
            };
            let started = Instant::now();
            let ds = compare(&case).expect("the gate answered");
            let secs = started.elapsed().as_secs_f64();
            let label = match tag {
                Tag::None => "none",
                Tag::Unchanged => "unchanged",
                Tag::Changed => "changed",
            };
            println!(
                "N={n} tag={label} full={} pruned={pruned} fallback={:?} disagreements={} compare={secs:.2}s",
                plan.full_count,
                plan.fallback,
                ds.len()
            );
            assert!(plan.fallback != Some(FallbackReason::EnvelopeDiffers));
        }
    }
}
