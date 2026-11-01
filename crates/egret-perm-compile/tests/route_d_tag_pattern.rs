//! #803: Route D (`handwritten_for_tag_via`) decides "this hand-written tag
//! policy touches tag T" with `PreparedPolicyResources` + `object_match` — the
//! policy MATCHER. For a tag request Ranger never consults the matcher:
//! `RangerDefaultPolicyEvaluator.evaluate` takes the match type from the
//! `RangerTagAccessRequest`, so the tag repository's resource TRIE is the whole
//! filter (`egret_ranger_eval::resource::trie_reproduces_matcher`,
//! `tests/ranger/src/test/java/EgretTagTrieAgreementTest.java`).
//!
//! Measured 2026-11-01 through `EgretCaseRunner` (real `RangerPolicyEngineImpl`,
//! ranger-plugins-common 2.5.0, 2.8.0 and 2.9.0 — identical on all three; allow,
//! deny and mask variants of each pair agree). The `RANGER` column below is that
//! measurement, pinned; the `route_d` column is computed here from the same
//! code Route D runs. Rows where they differ are the #803 gap: Route D's list
//! omits a policy the engine applies to the tag (a false negative only — no
//! measured row has Route D listing a policy Ranger does not apply).
//!
//! Set `EGRET_803_CORPUS=<corpus.json>` and `EGRET_803_OUT=<out.json>` to dump
//! Route D's verdict for a whole corpus (`{specs:[{key,values,excludes,recursive}], tags:[…]}`).

use egret_perm_compile::handwritten::{object_match, ObjectMatch};
use egret_ranger_eval::model::RangerPolicy;
use egret_ranger_eval::resource::{PreparedPolicyResources, RequestResource};
use egret_ranger_eval::servicedef::ServiceDef;
use serde_json::{json, Value};

fn tag_def() -> ServiceDef {
    let v: Value =
        serde_json::from_str(include_str!("../../egret-policy/src/service-defs/tag.json")).unwrap();
    ServiceDef::parse(&v)
}

fn policy(values: &[String], excludes: bool, recursive: bool) -> RangerPolicy {
    serde_json::from_value(json!({
        "name": "p", "service": "t", "policyType": 0, "isEnabled": true,
        "resources": { "tag": { "values": values, "isExcludes": excludes, "isRecursive": recursive } },
        "denyPolicyItems": [{ "users": ["alice"], "accesses": [{ "type": "select", "isAllowed": true }] }]
    }))
    .unwrap()
}

/// Route D's verdict: does the row appear in `classifications/{id}/handwritten` for `tag`?
fn route_d(def: &ServiceDef, p: &RangerPolicy, tag: &str) -> bool {
    let prepared = PreparedPolicyResources::new(p, def);
    matches!(
        object_match(&prepared, p, &RequestResource::new().with("tag", tag)),
        ObjectMatch::Applies { .. }
    )
}

fn spec(key: &str) -> (Vec<String>, bool, bool) {
    let (rest, ex, rec) = if let Some(r) = key.strip_prefix("EXCLUDES:") {
        (r, true, false)
    } else if let Some(r) = key.strip_prefix("RECURSIVE:") {
        (r, false, true)
    } else {
        (key, false, false)
    };
    (rest.split('|').map(str::to_string).collect(), ex, rec)
}

/// (policy resource, tag name, what Ranger 2.5.0/2.8.0/2.9.0 applies) — measured.
const RANGER: &[(&str, &str, bool)] = &[
    // Agreements: shapes the trie reproduces.
    ("pii", "pii", true),
    ("pii", "PII", false),
    ("p*", "pix", true),
    ("p*", "xii", false),
    ("*", "x", true),
    // Leading `*` is parked at the trie ROOT: returned for every tag.
    ("*i", "pii", true),
    ("*i", "p", true),
    ("*i", "PII", true),
    ("*i", "phone", true),
    // Parked at the literal prefix before the first wildcard.
    ("p*x", "pix", true),
    ("p*x", "pii", true),
    ("p*x", "p", true),
    ("p*x", "xii", false),
    ("pi?", "pi", true),
    ("pi?", "piix", true),
    ("?ii", "phone", true),
    // isExcludes: the excluded tag itself still gets the policy.
    ("EXCLUDES:pii", "pii", true),
    // isRecursive literal: parked as a wildcard at its own node.
    ("RECURSIVE:pii", "piix", true),
    ("RECURSIVE:pii", "pi", false),
];

#[test]
fn route_d_versus_measured_ranger() {
    let def = tag_def();
    let mut gaps = Vec::new();
    for (key, tag, ranger) in RANGER {
        let (values, ex, rec) = spec(key);
        let d = route_d(&def, &policy(&values, ex, rec), tag);
        if d != *ranger {
            gaps.push(format!("{key} vs {tag}: route D {d}, ranger {ranger}"));
        }
        // Pinned direction: Route D never lists what Ranger does not apply.
        assert!(
            !d || *ranger,
            "{key} vs {tag}: route D lists a policy Ranger does not apply"
        );
    }
    if let (Ok(corpus), Ok(out)) = (
        std::env::var("EGRET_803_CORPUS"),
        std::env::var("EGRET_803_OUT"),
    ) {
        let c: Value = serde_json::from_slice(&std::fs::read(corpus).unwrap()).unwrap();
        let mut res = serde_json::Map::new();
        for s in c["specs"].as_array().unwrap() {
            let values: Vec<String> = s["values"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();
            let p = policy(
                &values,
                s["excludes"].as_bool().unwrap(),
                s["recursive"].as_bool().unwrap(),
            );
            for t in c["tags"].as_array().unwrap() {
                let t = t.as_str().unwrap();
                res.insert(
                    format!("{}\t{t}", s["key"].as_str().unwrap()),
                    Value::Bool(route_d(&def, &p, t)),
                );
            }
        }
        std::fs::write(out, serde_json::to_vec_pretty(&Value::Object(res)).unwrap()).unwrap();
    }
    // The #803 gap, pinned. If Route D is fixed to follow the trie this count
    // drops to 0 and this assertion should be inverted to `is_empty()`.
    assert_eq!(gaps.len(), 10, "{gaps:#?}");
}
