//! Resource matching (SP4d Task 4): the behaviours Ranger's own matcher was
//! measured to have, not the shape of our types.
//!
//! Every expectation below was read off Apache Ranger's source at tag
//! `release-ranger-2.8.0` and confirmed byte-identical at
//! `release-ranger-2.5.0` for the three files that decide a match
//! (`RangerDefaultPolicyResourceMatcher`, `RangerAbstractResourceMatcher`,
//! `RangerDefaultResourceMatcher` — the only diffs between the two tags are
//! a `commons-lang` → `commons-lang3` import swap and plugin-context
//! plumbing in `createResourceMatcher`), and `applyExcludes` was
//! additionally disassembled out of the real
//! `ranger-plugins-common-2.5.0.jar` with `javap -c`. 2.5.0 is what Trino
//! 470 enforces and 2.8.0 what StarRocks fe-ubuntu:4.1 enforces.

use std::collections::BTreeMap;

use egret_ranger_eval::model::{RangerPolicy, ResourceLevel};
use egret_ranger_eval::resource::{
    match_policy_resources, match_resource, MatchType, RequestResource, ResourceMapSource,
};
use egret_ranger_eval::servicedef::{PolicyType, ServiceDef};
use serde_json::{json, Value};

/// The embedded Trino service-def the seeder uses, read as a fixture.
/// Library code in this crate never touches the filesystem; test code may.
fn trino_def() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../egret-policy/src/service-defs/trino.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("read trino service-def"))
        .expect("parse trino service-def")
}

/// The captured `/policies/download` envelope, whose `serviceDef` carries
/// all 13 Trino resources with their real `matcherOptions`.
fn captured_envelope() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/ranger/fixtures/service-policies.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("read captured envelope"))
        .expect("parse captured envelope")
}

fn resources(levels: &[(&str, &[&str])]) -> BTreeMap<String, ResourceLevel> {
    levels
        .iter()
        .map(|(name, values)| {
            (
                (*name).to_string(),
                ResourceLevel {
                    values: values.iter().map(|v| (*v).to_string()).collect(),
                    is_excludes: false,
                    is_recursive: false,
                },
            )
        })
        .collect()
}

fn excluding(levels: &[(&str, &[&str])]) -> BTreeMap<String, ResourceLevel> {
    let mut map = resources(levels);
    for level in map.values_mut() {
        level.is_excludes = true;
    }
    map
}

fn request(values: &[(&str, &str)]) -> RequestResource {
    values
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

// ── 1. The live-measured T1 case ─────────────────────────────────────────

/// A policy naming FEWER hierarchy levels than the request is an ANCESTOR
/// match, never a SELF one. Source: `getMatchType` sets
/// `MatchType.ANCESTOR` on the first hierarchy level that has a request
/// value but no policy matcher ("More resource-values than matchers").
/// Live measurement: a grant compiled to `{catalog, schema}` produced
/// `Access Denied: Cannot select from columns [v, id] in table or view
/// sibling` on real Trino — see
/// `docs/en/plans/2026-09-24-permission-model-sp4a-live-result.md` §3 T1
/// and `crates/egret-perm-compile/src/grants.rs:227-254`.
///
/// Task 5 must not turn this into an allow: Ranger's
/// `RangerDefaultPolicyEvaluator.evaluate` only accepts `SELF`,
/// `SELF_AND_ALL_DESCENDANTS` (and `DESCENDANT` under
/// `SELF_OR_DESCENDANTS` scope); `ANCESTOR` is accepted only for a
/// `RangerTagAccessRequest`.
#[test]
fn policy_naming_fewer_levels_than_the_request_is_an_ancestor_match() {
    let def = ServiceDef::parse(&trino_def());
    let policy = resources(&[("catalog", &["hive"]), ("schema", &["sales"])]);
    let req = request(&[
        ("catalog", "hive"),
        ("schema", "sales"),
        ("table", "sibling"),
        ("column", "v"),
    ]);

    assert_eq!(match_resource(&policy, &req, &def), MatchType::Ancestor);
}

/// The same grant with the column level named `*` is the case the live
/// measurement recorded as returning rows.
#[test]
fn policy_naming_all_four_levels_with_star_column_is_a_self_match() {
    let def = ServiceDef::parse(&trino_def());
    let policy = resources(&[
        ("catalog", &["hive"]),
        ("schema", &["sales"]),
        ("table", &["sibling"]),
        ("column", &["*"]),
    ]);
    let req = request(&[
        ("catalog", "hive"),
        ("schema", "sales"),
        ("table", "sibling"),
        ("column", "v"),
    ]);

    let got = match_resource(&policy, &req, &def);
    assert!(
        matches!(got, MatchType::Self_ | MatchType::SelfAndAllDescendants),
        "expected a self match, got {got:?}"
    );
}

/// The shorter-policy/longer-request direction's mirror: a policy naming
/// MORE levels than the request is a DESCENDANT match when the extra
/// levels name real values, and SELF_AND_ALL_DESCENDANTS when every extra
/// level is match-any. Source: the `resourceKeysSize < policyResources.size()`
/// tail of `getMatchType`, discriminating on `lastNonAnyMatcherIndex`.
#[test]
fn policy_naming_more_levels_than_the_request_splits_on_match_any() {
    let def = ServiceDef::parse(&trino_def());
    let req = request(&[("catalog", "hive"), ("schema", "sales"), ("table", "t")]);

    let named_column = resources(&[
        ("catalog", &["hive"]),
        ("schema", &["sales"]),
        ("table", &["t"]),
        ("column", &["id"]),
    ]);
    assert_eq!(
        match_resource(&named_column, &req, &def),
        MatchType::Descendant
    );

    let any_column = resources(&[
        ("catalog", &["hive"]),
        ("schema", &["sales"]),
        ("table", &["t"]),
        ("column", &["*"]),
    ]);
    assert_eq!(
        match_resource(&any_column, &req, &def),
        MatchType::SelfAndAllDescendants
    );
}

// ── 2. Case folding, driven by the service-def ───────────────────────────

/// `ignoreCase: true` in the resource's `matcherOptions` (which every
/// trino/starrocks resource declares) makes `HIVE` match `hive`. Source:
/// `RangerAbstractResourceMatcher.getMatcher` picks
/// `CaseInsensitiveStringMatcher` when `optIgnoreCase`, and
/// `getOptionIgnoreCase` defaults to **true** when the option is absent.
#[test]
fn ignore_case_from_the_service_def_folds_case() {
    let def = ServiceDef::parse(&trino_def());
    let policy = resources(&[("catalog", &["hive"])]);

    assert_eq!(
        match_resource(&policy, &request(&[("catalog", "HIVE")]), &def),
        MatchType::Self_
    );
    assert_eq!(
        match_resource(&policy, &request(&[("catalog", "hive")]), &def),
        MatchType::Self_
    );
}

/// The tag service-def declares `ignoreCase: false`, so the same policy
/// value there does NOT fold case. Drives the behaviour from the def
/// rather than from a hardcoded engine name.
#[test]
fn ignore_case_false_in_the_tag_def_does_not_fold_case() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../egret-policy/src/service-defs/tag.json"
    );
    let def = ServiceDef::parse(
        &serde_json::from_str::<Value>(&std::fs::read_to_string(path).expect("read tag def"))
            .expect("parse tag def"),
    );
    let policy = resources(&[("tag", &["PII"])]);

    assert_eq!(
        match_resource(&policy, &request(&[("tag", "PII")]), &def),
        MatchType::Self_
    );
    assert_eq!(
        match_resource(&policy, &request(&[("tag", "pii")]), &def),
        MatchType::None
    );
}

// ── 3. Real globbing, not a literal `*` special case ─────────────────────

/// `ord*` matches `orders`: Ranger compiles a trailing-`*` value into a
/// starts-with matcher, an embedded `*` or any `?` into a full wildcard
/// matcher (`FilenameUtils.wildcardMatch`), and a leading-`*` value into an
/// ends-with matcher. A matcher that only special-cased the literal `"*"`
/// would deny every one of these.
#[test]
fn glob_values_match_beyond_the_literal_star() {
    let def = ServiceDef::parse(&trino_def());
    let req_orders = request(&[("catalog", "orders")]);

    for pattern in ["ord*", "*ers", "o?ders", "or*e*", "*"] {
        assert_eq!(
            match_resource(&resources(&[("catalog", &[pattern])]), &req_orders, &def),
            MatchType::Self_,
            "pattern {pattern} should match `orders`"
        );
    }
    for pattern in ["ord", "ordr*", "o?ers", "*ing"] {
        assert_eq!(
            match_resource(&resources(&[("catalog", &[pattern])]), &req_orders, &def),
            MatchType::None,
            "pattern {pattern} should not match `orders`"
        );
    }
}

/// A resource whose def turns `wildCard` off treats `*` and `?` as literal
/// characters. Source: `getMatcher` only scans for `*`/`?` when
/// `optWildCard`, so every value compiles to a plain string matcher.
#[test]
fn wild_card_false_in_the_def_makes_glob_characters_literal() {
    let mut raw = trino_def();
    raw["resources"][0]["matcherOptions"]["wildCard"] = json!(false);
    let def = ServiceDef::parse(&raw);
    let prefix = resources(&[("catalog", &["ord*"])]);

    // With wildcards on this would be a starts-with match.
    assert_eq!(
        match_resource(&prefix, &request(&[("catalog", "orders")]), &def),
        MatchType::None
    );
    // The literal value still matches itself.
    assert_eq!(
        match_resource(&prefix, &request(&[("catalog", "ord*")]), &def),
        MatchType::Self_
    );
    // And `*` alone is no longer match-any: it matches nothing but itself,
    // and does not cover `hive`.
    assert_eq!(
        match_resource(
            &resources(&[("catalog", &["*"])]),
            &request(&[("catalog", "hive")]),
            &def
        ),
        MatchType::None
    );
}

/// A request value of `*` (or `""`, or an absent level) is
/// `isAllValuesRequested`, and `RangerDefaultResourceMatcher.getMatchType`
/// short-circuits it to NONE unless the POLICY is match-any — without ever
/// consulting a policy value. So a policy naming the literal `*` on a
/// `wildCard: false` resource still does not match a request for `*`; only
/// a genuinely match-any policy does.
///
/// Measured, not assumed: this expectation was written the other way round
/// first and the test failed, which is what sent us back to
/// `if (isMatchAny || (allValuesRequested && !isPrefixMatch)) { ret = isMatchAny ? SELF : NONE; }`.
#[test]
fn a_request_for_all_values_matches_only_a_match_any_policy() {
    let mut raw = trino_def();
    raw["resources"][0]["matcherOptions"]["wildCard"] = json!(false);
    let literal_star_def = ServiceDef::parse(&raw);
    assert_eq!(
        match_resource(
            &resources(&[("catalog", &["*"])]),
            &request(&[("catalog", "*")]),
            &literal_star_def
        ),
        MatchType::None
    );

    let def = ServiceDef::parse(&trino_def());
    assert_eq!(
        match_resource(
            &resources(&[("catalog", &["hive"])]),
            &request(&[("catalog", "*")]),
            &def
        ),
        MatchType::None
    );
    // Match-any policy: `*` on a wildcard-enabled resource.
    assert_eq!(
        match_resource(
            &resources(&[("catalog", &["*"])]),
            &request(&[("catalog", "*")]),
            &def
        ),
        MatchType::Self_
    );
}

// ── 4. isExcludes, including the carve-out ───────────────────────────────

/// `isExcludes: true` flips the un-excluded result. Source:
/// `RangerAbstractResourceMatcher.applyExcludes`, disassembled from
/// `ranger-plugins-common-2.5.0.jar`:
/// `if (!policyIsExcludes) return r; if (allValuesRequested && !isMatchAny) return r; return !r;`
#[test]
fn is_excludes_flips_a_named_value_match() {
    let def = ServiceDef::parse(&trino_def());
    let policy = excluding(&[("catalog", &["hive"])]);

    // Without excludes this would be SELF; the flip makes it NONE.
    assert_eq!(
        match_resource(&policy, &request(&[("catalog", "hive")]), &def),
        MatchType::None
    );
    // Without excludes this would be NONE; the flip makes it SELF.
    assert_eq!(
        match_resource(&policy, &request(&[("catalog", "other")]), &def),
        MatchType::Self_
    );
}

/// The one case where `isExcludes` does NOT flip: the request asks for all
/// values (`null`, `""` or `"*"`) while the policy is not match-any. The
/// un-flipped result there is NONE, so an excludes policy still does not
/// match a request for everything.
///
/// This is the mutation-checked assertion: inverting or deleting the
/// `allValuesRequested && !isMatchAny` guard turns each of these into
/// `Self_`.
#[test]
fn is_excludes_carve_out_does_not_flip_when_all_values_are_requested() {
    let def = ServiceDef::parse(&trino_def());
    let policy = excluding(&[("catalog", &["hive"])]);

    for all_values in ["*", ""] {
        assert_eq!(
            match_resource(&policy, &request(&[("catalog", all_values)]), &def),
            MatchType::None,
            "carve-out should leave the request for {all_values:?} unflipped"
        );
    }
}

/// The carve-out is guarded by `!isMatchAny`: an excludes policy that IS
/// match-any (values `["*"]`, or no values at all) flips even for a request
/// for all values. Without this the `!isMatchAny` half of the guard would
/// be free to be dropped.
#[test]
fn is_excludes_carve_out_does_not_apply_to_a_match_any_policy() {
    let def = ServiceDef::parse(&trino_def());
    let policy = excluding(&[("catalog", &["*"])]);

    assert_eq!(
        match_resource(&policy, &request(&[("catalog", "*")]), &def),
        MatchType::None,
        "match-any excludes flips SELF to NONE even for a request for all values"
    );
}

// ── 5. additionalResources ───────────────────────────────────────────────

/// `additionalResources` is a second, independent resource map that gets
/// its own matcher and its own chance to match. Source:
/// `RangerAbstractPolicyEvaluator.init` builds one
/// `RangerDefaultPolicyResourceEvaluator` per additional resource map and
/// `RangerDefaultPolicyEvaluator.evaluate` loops over all of them.
#[test]
fn additional_resources_get_an_independent_chance_to_match() {
    let def = ServiceDef::parse(&trino_def());
    let body = json!({
        "name": "two-tables",
        "service": "trino1",
        "policyType": 0,
        "resources": {
            "catalog": {"values": ["hive"]},
            "schema": {"values": ["a"]},
            "table": {"values": ["t1"]},
            "column": {"values": ["*"]}
        },
        "additionalResources": [{
            "catalog": {"values": ["hive"]},
            "schema": {"values": ["b"]},
            "table": {"values": ["t2"]},
            "column": {"values": ["*"]}
        }]
    });
    let policy: RangerPolicy = serde_json::from_value(body).expect("parse policy");

    let req = request(&[
        ("catalog", "hive"),
        ("schema", "b"),
        ("table", "t2"),
        ("column", "c"),
    ]);
    let matches = match_policy_resources(&policy, &req, &def);

    assert_eq!(matches.len(), 2, "one match per resource map");
    assert_eq!(matches[0].source, ResourceMapSource::Primary);
    assert_eq!(matches[0].match_type, MatchType::None);
    assert_eq!(matches[1].source, ResourceMapSource::Additional(0));
    assert_eq!(matches[1].match_type, MatchType::Self_);
}

// ── 6. The captured download's own service-def ───────────────────────────

/// The same T1 answer, driven by the `serviceDef` carried in a real
/// captured `/policies/download` envelope rather than by the embedded def.
#[test]
fn the_captured_envelope_service_def_drives_matching_too() {
    let envelope = captured_envelope();
    let def = ServiceDef::parse(&envelope["serviceDef"]);

    let ancestor = resources(&[("catalog", &["hive"]), ("schema", &["sales"])]);
    let req = request(&[
        ("catalog", "hive"),
        ("schema", "sales"),
        ("table", "orders"),
        ("column", "id"),
    ]);
    assert_eq!(match_resource(&ancestor, &req, &def), MatchType::Ancestor);

    let full = resources(&[
        ("catalog", &["hive"]),
        ("schema", &["sales"]),
        ("table", &["orders"]),
        ("column", &["*"]),
    ]);
    assert_eq!(match_resource(&full, &req, &def), MatchType::Self_);
}

/// A policy naming a level that is not in the request's hierarchy at all
/// (a sibling branch) does not match. Source: `getMatchingHierarchy`
/// returns null when neither key set contains the other, and `getMatchType`
/// leaves `ret` at NONE for an empty hierarchy.
#[test]
fn a_policy_on_a_sibling_branch_does_not_match() {
    let def = ServiceDef::parse(&trino_def());
    let policy = resources(&[("catalog", &["hive"]), ("sessionproperty", &["*"])]);
    let req = request(&[("catalog", "hive"), ("schema", "sales")]);

    assert_eq!(match_resource(&policy, &req, &def), MatchType::None);
}

// ── 7. The mask/row-filter hierarchy is NOT the access one ───────────────

/// A masking policy has exactly **one** resource hierarchy, the full
/// `catalog/schema/table/column` chain — not one per `isValidLeaf` prefix
/// the way an access policy does.
///
/// The reason is a single asymmetry in `ServiceDefUtil.mergeResourceDef`:
/// `parent` is retained from the access def, but `isValidLeaf` is
/// **replaced** by the `dataMaskDef` entry's (its `copyLeafValue` block is
/// an unconditional assignment — see `normalize_sub_def`'s doc). Every
/// `dataMaskDef` entry in both embedded defs omits `isValidLeaf`, so the
/// merge clears the `true` that `catalog`, `schema` and `table` carry in
/// `resources`, and none of them terminates a mask hierarchy any more.
///
/// StarRocks is the sharper case: all four of its access resources are
/// `isValidLeaf: true`, so retaining the base value would produce four mask
/// hierarchies where Ranger has one.
#[test]
fn mask_and_row_filter_hierarchies_drop_the_access_defs_valid_leaf_prefixes() {
    let trino = ServiceDef::parse(&trino_def());
    assert_eq!(
        trino.hierarchies(PolicyType::DataMask),
        [vec![
            "catalog".to_string(),
            "schema".into(),
            "table".into(),
            "column".into()
        ]],
        "masking has one hierarchy, the full chain"
    );
    assert_eq!(
        trino.hierarchies(PolicyType::RowFilter),
        [vec!["catalog".to_string(), "schema".into(), "table".into()]],
        "row-filtering has one hierarchy, the full chain"
    );
    // The access hierarchies are untouched by the merge: still all 13,
    // which is what the seeder in `egret-policy` builds its default
    // policies from.
    assert_eq!(trino.hierarchies(PolicyType::Access).len(), 13);

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../egret-policy/src/service-defs/starrocks.json"
    );
    let starrocks = ServiceDef::parse(
        &serde_json::from_str::<Value>(&std::fs::read_to_string(path).expect("read starrocks def"))
            .expect("parse starrocks def"),
    );
    assert_eq!(
        starrocks.hierarchies(PolicyType::DataMask),
        [vec![
            "catalog".to_string(),
            "database".into(),
            "table".into(),
            "column".into()
        ]],
    );
}

/// The same, off the captured `/policies/download` envelope rather than an
/// embedded def — its `dataMaskDef.resources` carry `matcherOptions` and
/// `uiHint` and nothing else, which is the shape the merge has to cope with.
#[test]
fn the_captured_envelopes_mask_hierarchy_is_also_one_chain() {
    let def = ServiceDef::parse(&captured_envelope()["serviceDef"]);
    assert_eq!(
        def.hierarchies(PolicyType::DataMask),
        [vec![
            "catalog".to_string(),
            "schema".into(),
            "table".into(),
            "column".into()
        ]],
    );
}

/// An empty policy resource map matches an empty request (Ranger's
/// `policyResources.size() == 0 && resourceKeysSize == 0` short-circuit),
/// and nothing else.
#[test]
fn empty_policy_resources_match_only_an_empty_request() {
    let def = ServiceDef::parse(&trino_def());
    let empty = BTreeMap::new();

    assert_eq!(
        match_resource(&empty, &RequestResource::default(), &def),
        MatchType::Self_
    );
    assert_eq!(
        match_resource(&empty, &request(&[("catalog", "hive")]), &def),
        MatchType::None
    );
}
