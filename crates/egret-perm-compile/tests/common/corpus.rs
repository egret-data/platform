//! The seeded corpus of (before, after) policy-set pairs (SP4d-4 Task 5), shared by
//! `equivalence.rs` (the Rust evaluator as the oracle) and, through `#[path]`, by
//! `egret-ranger-eval`'s `differential.rs` (the real Ranger jars as the oracle on
//! the probes pruning drops). One generator, so the two oracles judge the SAME
//! pairs.
#![allow(dead_code)]

use egret_perm_compile::equivalence::TagAssignment;
use serde_json::{json, Value};

pub fn def_json(file: &str) -> Value {
    let path = format!(
        "{}/../egret-policy/src/service-defs/{file}",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("read service-def"))
        .expect("parse service-def")
}

pub fn level(values: &[&str]) -> Value {
    json!({"values": values, "isExcludes": false, "isRecursive": false})
}

pub fn item(users: &[&str], groups: &[&str], accesses: &[&str]) -> Value {
    json!({
        "users": users, "groups": groups, "roles": [],
        "accesses": accesses.iter().map(|a| json!({"type": a, "isAllowed": true})).collect::<Vec<_>>()
    })
}

/// A tag ACCESS policy denying alice `select` on the tag `pii`.
pub fn tag_deny_pii() -> Value {
    json!({
        "id": 900, "name": "pii clearance", "service": "trino-tags", "policyType": 0,
        "isEnabled": true, "policyPriority": 0,
        "resources": {"tag": level(&["pii"])},
        "denyPolicyItems": [item(&["alice"], &[], &["select"])]
    })
}

pub fn mask_item(users: &[&str], groups: &[&str], mask_type: &str) -> Value {
    json!({
        "users": users, "groups": groups, "roles": [],
        "accesses": [{"type": "select", "isAllowed": true}],
        "dataMaskInfo": {"dataMaskType": mask_type}
    })
}

pub fn tag_mask(mask_type: &str) -> Value {
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

/// One `catalog_tag_assignments` row on a column.
pub fn assign(catalog: &str, schema: &str, table: &str, column: &str, tag: &str) -> TagAssignment {
    TagAssignment {
        catalog: catalog.into(),
        schema: schema.into(),
        table: table.into(),
        column: column.into(),
        tag: tag.into(),
    }
}

/// A download envelope as the JSON the store serves: `policies` and, when
/// non-empty, a `tagPolicies` block with the embedded tag service-def.
pub fn envelope_json(policies: Vec<Value>, tag_policies: Vec<Value>) -> Value {
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
    env
}

/// SplitMix64: a fixed, dependency-free generator, so the corpus is the same
/// on every machine.
pub struct Rng(u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    pub fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
    pub fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
    pub fn subset<'a>(&mut self, xs: &[&'a str], min: usize) -> Vec<&'a str> {
        loop {
            let s: Vec<&str> = xs.iter().copied().filter(|_| self.chance(50)).collect();
            if s.len() >= min {
                return s;
            }
        }
    }
}

pub const CORPUS_SEED: u64 = 0x5D4_0004_0005;
/// The default corpus; the `#[ignore]`d run uses `LARGE_CORPUS_PAIRS` of the
/// same stream.
pub const CORPUS_PAIRS: usize = 30;
pub const LARGE_CORPUS_PAIRS: usize = 400;

/// One random policy named `name`: an access allow (sometimes with deny items,
/// sometimes at priority 1, sometimes with `isExcludes` on a level), a column
/// mask, or a row filter, on a random prefix of catalog/schema/table/column.
pub fn random_policy(rng: &mut Rng, name: &str) -> Value {
    let catalog = *rng.pick(&["wh", "lake"]);
    let schema = *rng.pick(&["sales", "hr", "*", "Sales", "sal*"]);
    let table = *rng.pick(&[
        "orders", "people", "*", "ord*", "*ders", "or?ers", "Orders", "ÓRDERS",
    ]);
    let column = *rng.pick(&["email", "ssn", "*", "EMAIL", "e*"]);
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
                mask_item(&[], &["public"], rng.pick(&["MASK_NULL", "MASK_HASH"]))
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
    // Shapes the first corpus lacked: `isRecursive`, several resource maps,
    // disabled policies, and constructs the reader keeps only as markers.
    if rng.chance(10) && p["resources"].get("table").is_some() {
        p["resources"]["table"]["isRecursive"] = json!(true);
    }
    if rng.chance(12) && p["policyType"] == json!(0) {
        let second = *rng.pick(&["hr", "sales", "*"]);
        p["additionalResources"] = json!([{
            "catalog": level(&[*rng.pick(&["wh", "lake"])]),
            "schema": level(&[second]),
            "table": level(&[*rng.pick(&["people", "orders", "*"])])
        }]);
    }
    if rng.chance(8) {
        p["isEnabled"] = json!(false);
    }
    if rng.chance(6) {
        p["conditions"] =
            json!([{"type": "ip-range", "values": [*rng.pick(&["10.0.0.1", "10.0.0.2"])]}]);
    }
    if rng.chance(4) {
        p["validitySchedules"] =
            json!([{"startTime": *rng.pick(&["2026/01/01 00:00:00", "2026/06/01 00:00:00"])}]);
    }
    p
}

pub fn random_tag_policy(rng: &mut Rng, name: &str) -> Value {
    let tag = *rng.pick(&["pii", "phi", "PII", "p*"]);
    let mut p = if rng.chance(50) {
        let mut d = tag_deny_pii();
        d["policyPriority"] = json!(0);
        d
    } else {
        tag_mask(rng.pick(&["MASK_NULL", "MASK_HASH"]))
    };
    p["resources"] = json!({"tag": level(&[tag])});
    p["name"] = json!(name);
    p
}

/// The corpus: `CORPUS_PAIRS` (before, after) pairs, each 2-4 random policies
/// (and sometimes a tag policy) before, and 1-2 edits after (change a policy,
/// remove one, add one, add/remove a tag policy).
pub fn corpus_json(pairs: usize) -> Vec<(Value, Value, Vec<TagAssignment>)> {
    let mut rng = Rng(CORPUS_SEED);
    (0..pairs)
        .map(|_| {
            let n = 2 + rng.below(3);
            let mut before: Vec<Value> = (0..n)
                .map(|i| random_policy(&mut rng, &format!("p{i}")))
                .collect();
            let mut tag_before: Vec<Value> = (0..rng.below(3))
                .map(|i| random_tag_policy(&mut rng, &format!("t{i}")))
                .collect();
            let mut after = before.clone();
            let mut tag_after = tag_before.clone();
            for _ in 0..1 + rng.below(2) {
                match rng.below(6) {
                    0..=2 if !after.is_empty() => {
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
                        if tag_after.is_empty() || rng.chance(40) {
                            let name = format!("t{}", tag_after.len());
                            tag_after.push(random_tag_policy(&mut rng, &name));
                        } else if rng.chance(50) {
                            tag_after.remove(rng.below(tag_after.len()));
                        } else {
                            let i = rng.below(tag_after.len());
                            let name = format!("t{i}");
                            tag_after[i] = random_tag_policy(&mut rng, &name);
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
            for (i, p) in tag_before.iter_mut().enumerate() {
                p["id"] = json!(700 + i);
            }
            for (i, p) in tag_after.iter_mut().enumerate() {
                p["id"] = json!(800 + i);
            }
            // Several assignments: column- and table-level, two tag names.
            let assignments: Vec<TagAssignment> = (0..rng.below(4))
                .map(|_| {
                    assign(
                        "wh",
                        rng.pick(&["sales", "hr"]),
                        rng.pick(&["orders", "people"]),
                        rng.pick(&["email", "ssn", ""]),
                        rng.pick(&["pii", "phi"]),
                    )
                })
                .collect();
            (
                envelope_json(before, tag_before),
                envelope_json(after, tag_after),
                assignments,
            )
        })
        .collect()
}
