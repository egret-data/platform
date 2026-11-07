//! #803: Route D (`handwritten_for_tag_via`) lists the hand-written tag
//! policies the engines apply to a classification's tag. For a tag request
//! Ranger never consults the policy matcher: `RangerDefaultPolicyEvaluator.evaluate`
//! takes the match type from the `RangerTagAccessRequest`, so the tag
//! repository's resource TRIE is the whole filter — wider than the matcher for
//! `*i`, `?ii`, `{USER}`, `p*x`, `isRecursive` and `isExcludes`.
//!
//! `fixtures/route_d_tag_trie.json` is the measurement: 37 tag-resource shapes
//! × 24 tag names, run through `EgretCaseRunner` (real `RangerPolicyEngineImpl`,
//! ranger-plugins-common 2.5.0, 2.8.0 and 2.9.0 — identical on all three; the
//! allow, deny and mask variants of each pair agree). Route D's per-policy
//! decision ([`tag_reach`]) must equal it on every pair.

use egret_perm_compile::handwritten::{tag_reach, TagReach};
use egret_ranger_eval::model::RangerPolicy;
use egret_ranger_eval::servicedef::ServiceDef;
use serde::Deserialize;
use serde_json::{json, Value};

fn tag_def() -> ServiceDef {
    let v: Value =
        serde_json::from_str(include_str!("../../egret-policy/src/service-defs/tag.json")).unwrap();
    ServiceDef::parse(&v)
}

fn policy(values: &[&str], excludes: bool, recursive: bool, policy_type: i32) -> RangerPolicy {
    serde_json::from_value(json!({
        "name": "p", "service": "t", "policyType": policy_type, "isEnabled": true,
        "resources": { "tag": { "values": values, "isExcludes": excludes, "isRecursive": recursive } },
        "denyPolicyItems": [{ "users": ["alice"], "accesses": [{ "type": "select", "isAllowed": true }] }]
    }))
    .unwrap()
}

#[derive(Deserialize)]
struct Corpus {
    tags: Vec<String>,
    specs: Vec<Spec>,
}

#[derive(Deserialize)]
struct Spec {
    values: Vec<String>,
    excludes: bool,
    recursive: bool,
    ranger: String,
}

fn corpus() -> Corpus {
    serde_json::from_str(include_str!("fixtures/route_d_tag_trie.json")).unwrap()
}

/// Every one of the 888 measured pairs, for an access and a masking policy.
#[test]
fn route_d_lists_exactly_what_ranger_applies_on_the_whole_corpus() {
    let def = tag_def();
    let c = corpus();
    assert_eq!(c.specs.len() * c.tags.len(), 888);
    for policy_type in [0, 1] {
        let mut wrong = Vec::new();
        for s in &c.specs {
            let values: Vec<&str> = s.values.iter().map(String::as_str).collect();
            let p = policy(&values, s.excludes, s.recursive, policy_type);
            assert_eq!(s.ranger.len(), c.tags.len());
            for (tag, bit) in c.tags.iter().zip(s.ranger.chars()) {
                let ranger = bit == '1';
                let listed = tag_reach(&p, &def, tag).is_some();
                if listed != ranger {
                    wrong.push(format!(
                        "{:?} ex={} rec={} vs {tag}: route D {listed}, ranger {ranger}",
                        s.values, s.excludes, s.recursive
                    ));
                }
            }
        }
        assert!(
            wrong.is_empty(),
            "type {policy_type}: {} wrong: {wrong:#?}",
            wrong.len()
        );
    }
}

fn reach(values: &[&str], excludes: bool, recursive: bool, tag: &str) -> Option<TagReach> {
    tag_reach(&policy(values, excludes, recursive, 0), &tag_def(), tag)
}

fn widened(prefix: &str) -> Option<TagReach> {
    Some(TagReach {
        through_pattern: false,
        engine_tag_prefix: Some(prefix.to_string()),
    })
}

/// The flag the UI shows: set exactly when the engine reaches a tag the
/// pattern does not match, carrying the prefix the engine actually used.
#[test]
fn a_row_the_pattern_does_not_match_is_flagged_with_the_engines_prefix() {
    let plain = |through_pattern| {
        Some(TagReach {
            through_pattern,
            engine_tag_prefix: None,
        })
    };
    // The pattern matches: not widened.
    assert_eq!(reach(&["pii"], false, false, "pii"), plain(false));
    assert_eq!(reach(&["p*"], false, false, "pix"), plain(true));
    assert_eq!(reach(&["*i"], false, false, "pii"), plain(true));
    assert_eq!(reach(&["p*x"], false, false, "pix"), plain(true));
    // Leading wildcard or token: every tag.
    assert_eq!(reach(&["*i"], false, false, "phone"), widened(""));
    assert_eq!(reach(&["?ii"], false, false, "phone"), widened(""));
    assert_eq!(reach(&["{USER}"], false, false, "pii"), widened(""));
    // Interior wildcard: every tag starting with the literal prefix.
    assert_eq!(reach(&["p*x"], false, false, "phone"), widened("p"));
    assert_eq!(reach(&["pi?"], false, false, "piix"), widened("pi"));
    // Several values: the longest prefix that reaches the tag.
    assert_eq!(reach(&["*x", "pi*x"], false, false, "pii"), widened("pi"));
    // isRecursive: the whole literal is the prefix.
    assert_eq!(reach(&["pii"], false, true, "piix"), widened("pii"));
    // isExcludes: every tag, the excluded one included.
    assert_eq!(reach(&["pii"], true, false, "pii"), widened(""));
    // Not reached.
    assert_eq!(reach(&["p*x"], false, false, "xii"), None);
    assert_eq!(reach(&["pii"], false, false, "PII"), None);
}

/// Shapes the measurement did not cover keep the matcher's answer.
#[test]
fn an_unmeasured_shape_falls_back_to_the_matcher() {
    let def = tag_def();
    let extra: RangerPolicy = serde_json::from_value(json!({
        "name": "p", "service": "t", "policyType": 0, "isEnabled": true,
        "resources": { "tag": { "values": ["*i"] }, "other": { "values": ["x"] } },
        "denyPolicyItems": [{ "users": ["alice"], "accesses": [{ "type": "select", "isAllowed": true }] }]
    }))
    .unwrap();
    // The matcher finds no hierarchy for `{tag, other}`; the trie rule is
    // not applied to a map it was not measured on.
    assert_eq!(tag_reach(&extra, &def, "phone"), None);
    // An empty value: not measured, so `*i` keeps its matcher answer.
    assert_eq!(reach(&["*i", ""], false, false, "phone"), None);
    assert_eq!(
        reach(&["*i", ""], false, false, "pii"),
        Some(TagReach {
            through_pattern: true,
            engine_tag_prefix: None
        })
    );
}
