//! SP4d-2b Task 4: the refusals that keep the tag phase honest, and the two
//! enricher behaviours (D9) that are ANSWERS rather than refusals.
//!
//! # What is asserted here, and why each is worth an assertion today
//!
//! Every refusal in this file is **inert today, and none of them is inert
//! because of a schema constraint.** `ServiceTags::from_assignments` writes each
//! wire tag as `{"type": <tags.name>, "owner": 0}` — no `attributes`, no
//! `options`, no `validityPeriods` — so those three cannot arrive from egret's
//! own writer. The LEVEL COUNT is different, and fix round 1 corrected this
//! file's claim about it: `catalog_tag_assignments` has no `CHECK`, `NOT NULL`
//! does not exclude `''`, the assign routes validate nothing, and
//! `from_assignments` omits an empty level — so `{"schema": "", "table": ""}`
//! really does produce a one-level `resourceElements`. What makes the refusal
//! inert is the READ path: **nothing constructs an
//! `egret_ranger_eval::eval::ResolvedTag` outside this crate's tests**, and the
//! `egret_policy::ResolvedTag` that Task 7 will map from is selected by exact
//! `schema_name`/`table_name` equality, so a `('', '')` row cannot come back for
//! a real resource. See the ⚠ block on `TagAttachmentProblem::ResourceLevels`.
//!
//! Asserting the refusals anyway is what stops a hand-written `/tags/download`
//! shim, an imported payload or a future egret feature from changing the answer
//! silently: with the assertion, the day one of those arrives the verdict says
//! so; without it, the day one of those arrives the verdict is confidently
//! wrong.
//!
//! # The two behaviours that are NOT refusals (D9)
//!
//! `RangerTagEnricher.processServiceTags` drops a service resource whose key set
//! no policy type validates, and removes its `resourceToTagIds` entry with it —
//! so the tag does not merely lose one attachment, it stops existing for that
//! resource. That is Ranger's own answer, reproduced, not a refusal. And
//! `isEnabled` on a service resource is never read at all; the field is modelled
//! precisely so that its inertness can be asserted rather than inferred from its
//! absence. Both measured with `javap -c -p` on `ranger-plugins-common` 2.5.0
//! and 2.8.0 — see [`egret_ranger_eval::eval`]'s `enriched_tags`.
//!
//! # Fixture provenance
//!
//! The `tag` service-def is the one the live Trino plugin was holding
//! (`tests/fixtures/tag-policies-live.json`, provenance and sha256 in
//! `tests/tag_tiers.rs`); the resource def is the shipped
//! `egret-policy/src/service-defs/trino.json`. Nothing here invents a
//! service-def, because the hierarchies of the RESOURCE def are exactly what
//! decides D9's drop.

use std::collections::{BTreeMap, BTreeSet};

use egret_ranger_eval::eval::{
    evaluate, evaluate_data_mask, evaluate_row_filter, AccessRequest, Decision, DenyReason,
    EngineConfig, MaskDecision, ResolvedTag, RowFilterDecision, TagAttachmentProblem,
    TagNormalisationProblem, TagServiceResource, UndecidableCause,
};
use egret_ranger_eval::model::{ServicePolicySet, Unmodelled};
use egret_ranger_eval::resource::{MatchType, RequestResource};
use egret_ranger_eval::servicedef::ServiceDef;
use serde_json::{json, Value};

// ── fixtures ─────────────────────────────────────────────────────────────

fn live_download_json() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/tag-policies-live.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("read live tag fixture"))
        .expect("parse live tag fixture")
}

fn tag_def_json() -> Value {
    live_download_json()["tagPolicies"]["serviceDef"].clone()
}

fn trino_def() -> ServiceDef {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../egret-policy/src/service-defs/trino.json"
    );
    ServiceDef::parse(
        &serde_json::from_str::<Value>(&std::fs::read_to_string(path).expect("read trino def"))
            .expect("parse trino def"),
    )
}

/// An envelope with no resource policies and a `tagPolicies` block carrying
/// exactly `tag_policies`. The resource half is empty throughout this file:
/// every assertion here is about phase 1, and a resource policy could only
/// obscure which phase answered.
fn envelope(tag_policies: Value) -> ServicePolicySet {
    serde_json::from_value(json!({
        "serviceName": "trino",
        "policies": [],
        "serviceDef": {"name": "trino"},
        "tagPolicies": {
            "serviceName": "trino_tag",
            "serviceId": 99,
            "policyVersion": 1,
            "policies": tag_policies,
            "serviceDef": tag_def_json(),
        },
    }))
    .expect("parse envelope")
}

/// A tag policy on the LITERAL tag value `pii` — literal because
/// `resource::trie_reproduces_matcher` classifies a wildcard as unreproducible
/// and refuses it, which would answer every test below for the wrong reason.
fn tag_policy(id: i64, name: &str, policy_type: i32, bucket: &str, items: Value) -> Value {
    let mut p = json!({
        "id": id, "name": name, "service": "trino_tag", "policyType": policy_type,
        "isEnabled": true, "policyPriority": 0,
        "resources": { "tag": {"values": ["pii"]} },
    });
    p[bucket] = items;
    p
}

/// The one tag ACCESS policy every access test in this file uses: `pii` allows
/// `select` to alice, and nothing else in the envelope can allow anything. So
/// `Allowed{"tag-allow"}` means the tag reached the evaluator and
/// `Denied{NoPolicyMatched}` means it did not — a two-valued observation with
/// no third reading.
fn tag_allow() -> ServicePolicySet {
    envelope(json!([tag_policy(
        10,
        "tag-allow",
        0,
        "policyItems",
        json!([{ "users": ["alice"], "accesses": [{"type": "select", "isAllowed": true}] }]),
    )]))
}

fn no_superusers() -> EngineConfig {
    EngineConfig::without_super_users()
}

/// `hive.sales.orders.id`, carrying exactly the attachments handed in.
fn request(tags: Vec<ResolvedTag>) -> AccessRequest {
    AccessRequest {
        user: "alice".to_string(),
        groups: BTreeSet::new(),
        roles: BTreeSet::new(),
        access_type: "select".to_string(),
        resource: RequestResource::new()
            .with("catalog", "hive")
            .with("schema", "sales")
            .with("table", "orders")
            .with("column", "id"),
        owner: None,
        resolved_tags: Some(tags),
    }
}

/// The attachment `ServiceTags::from_assignments` writes for a COLUMN-level
/// assignment: four `resourceElements` levels, an enabled service resource, and
/// none of `attributes` / `options` / `validityPeriods`. Every refusal in this
/// file is inert for it, which is what makes it the control.
fn column_tag(name: &str) -> ResolvedTag {
    ResolvedTag {
        name: name.to_string(),
        match_type: MatchType::Self_,
        attributes: BTreeMap::new(),
        options: BTreeSet::new(),
        validity_periods: 0,
        attached_through: TagServiceResource {
            levels: levels(&["catalog", "schema", "table", "column"]),
            is_enabled: true,
        },
    }
}

/// The `resourceElements` KEY SET, which is what
/// [`TagServiceResource::levels`] is: a duplicate key is unrepresentable there,
/// by type, since fix round 1.
fn levels(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

/// The same names as the `Vec` [`TagAttachmentProblem::ResourceLevels`] carries
/// for reporting — sorted, because the cause is built from the set.
fn level_names(names: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = names.iter().map(|s| s.to_string()).collect();
    v.sort();
    v
}

fn causes(decision: &Decision) -> Vec<UndecidableCause> {
    match decision {
        Decision::Undecidable { why } => why.clone(),
        _ => Vec::new(),
    }
}

// ── 1. a tag carrying `options` or `validityPeriods` ─────────────────────

/// `RangerTagForEval.isApplicable` reads `TAG_VALIDITY_PERIODS` back out of
/// `options` when the map holds anything and the constructor built no validity
/// evaluators (2.5.0 offsets 62-128), and
/// `RangerPolicyRepository.getLikelyMatchPolicyEvaluators` then SKIPS the tag
/// when `isApplicable(accessTime)` is false (offset 62). So an option key — any
/// option key, recognised or not — sits one branch from removing this tag from
/// the evaluation, and the branch's answer depends on
/// `RangerAccessRequest.getAccessTime()`, which the plugin sets and this crate
/// does not model.
#[test]
fn a_tag_carrying_options_refuses() {
    let mut tag = column_tag("pii");
    tag.options.insert("TAG_VALIDITY_PERIODS".to_string());

    let d = evaluate(
        &tag_allow(),
        &request(vec![tag]),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        causes(&d).contains(&UndecidableCause::TagAttachmentNotReproducible {
            tag: "pii".to_string(),
            problem: TagAttachmentProblem::Options(vec!["TAG_VALIDITY_PERIODS".to_string()]),
        }),
        "an option key must refuse, naming the key: {d:?}"
    );

    // The control, and it is the same fixture with the map emptied: without it
    // the assertion above is satisfied by anything that refuses.
    let d = evaluate(
        &tag_allow(),
        &request(vec![column_tag("pii")]),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        matches!(&d, Decision::Allowed { policy_name, .. } if policy_name == "tag-allow"),
        "egret's own wire tag carries no options and must ANSWER: {d:?}"
    );
}

/// `RangerTagForEval`'s constructor turns every `validityPeriods` entry into a
/// `RangerValidityScheduleEvaluator` (offset 88) — and that happens whether or
/// not `options` is set, which is why this is a second field and not a second
/// reading of the first.
#[test]
fn a_tag_carrying_validity_periods_refuses() {
    let mut tag = column_tag("pii");
    tag.validity_periods = 2;

    let d = evaluate(
        &tag_allow(),
        &request(vec![tag]),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        causes(&d).contains(&UndecidableCause::TagAttachmentNotReproducible {
            tag: "pii".to_string(),
            problem: TagAttachmentProblem::ValidityPeriods(2),
        }),
        "a validity period must refuse, naming how many: {d:?}"
    );
}

/// The refusal fires only where the filter it stands in for could change THIS
/// decision. A `DESCENDANT` attachment never reaches the policy items at the
/// default `SELF` scope (`tag_match_is_admitted`), so whether Ranger's
/// `isApplicable` would have skipped it cannot matter — and refusing for it
/// would be pure over-refusal.
#[test]
fn an_unadmitted_tag_carrying_options_does_not_refuse() {
    let mut tag = column_tag("pii");
    tag.match_type = MatchType::Descendant;
    tag.options.insert("TAG_VALIDITY_PERIODS".to_string());

    let d = evaluate(
        &tag_allow(),
        &request(vec![tag]),
        &trino_def(),
        &no_superusers(),
    );
    assert_eq!(
        d,
        Decision::Denied {
            policy_id: None,
            policy_name: String::new(),
            reason: DenyReason::NoPolicyMatched,
        },
        "a DESCENDANT attachment writes nothing whatever its payload carried: {d:?}"
    );
}

/// **The mutation that found this test.** Removing the admission guard from the
/// attachment sweep SURVIVED the whole suite, because the only case then
/// covered was a request whose tags are ALL unadmitted — and in that case
/// `tag_candidates` returns at `admitted.is_empty()` before it ever looks at
/// `why`, so the extra causes are silently discarded and the verdict is
/// unchanged. The narrowing is only observable in a MIXED request, which is the
/// one nothing exercised.
///
/// It is also the shape this package keeps producing, one turn further on: a
/// refusal that a return routes around, where the routing is invisible because
/// the guard above it happens to make the refusal empty.
#[test]
fn an_unadmitted_tag_does_not_refuse_beside_an_admitted_one() {
    let mut descendant = column_tag("pii");
    descendant.match_type = MatchType::Descendant;
    descendant
        .options
        .insert("TAG_VALIDITY_PERIODS".to_string());
    descendant.validity_periods = 3;
    descendant.attached_through.levels = levels(&["catalog", "schema"]);

    let d = evaluate(
        &tag_allow(),
        &request(vec![column_tag("pii"), descendant]),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        matches!(&d, Decision::Allowed { policy_name, .. } if policy_name == "tag-allow"),
        "an attachment Ranger's `evaluate` never lets reach the policy items cannot \
         make this decision undecidable, however loaded its payload: {d:?}"
    );
}

/// **A SURVIVING MUTATION found this gap — fix round 1.** Reducing
/// `dedup_causes` to "keep only the first cause" passed the whole suite, because
/// nothing asserted that two DISTINCT causes both reach the caller. A helper
/// added to collapse duplicates that silently collapses findings instead would
/// be strictly worse than the duplicates it was added for: the refusal would
/// still be a refusal, and the operator would be handed one reason where there
/// are two, with no signal that anything was dropped.
///
/// Both directions, in one test: two distinct problems on the same attachment
/// survive as two causes, and the same problem on two attachments that Ranger
/// keeps apart is reported once.
#[test]
fn distinct_causes_all_survive_and_only_duplicates_collapse() {
    let mut tag = column_tag("pii");
    tag.options.insert("TAG_VALIDITY_PERIODS".to_string());
    tag.validity_periods = 1;

    let d = evaluate(
        &tag_allow(),
        &request(vec![tag.clone()]),
        &trino_def(),
        &no_superusers(),
    );
    let why = causes(&d);
    assert!(
        why.contains(&UndecidableCause::TagAttachmentNotReproducible {
            tag: "pii".to_string(),
            problem: TagAttachmentProblem::Options(vec!["TAG_VALIDITY_PERIODS".to_string()]),
        }) && why.contains(&UndecidableCause::TagAttachmentNotReproducible {
            tag: "pii".to_string(),
            problem: TagAttachmentProblem::ValidityPeriods(1),
        }),
        "two distinct problems on one attachment are two findings, not one: {why:?}"
    );

    // And a DUPLICATE is collapsed. Two attachments of `pii` that differ only in
    // the service resource are ONE member of Ranger's set, and the option key is
    // the same, so the same cause is derived twice.
    let mut second = tag.clone();
    second.attached_through.levels = levels(&["catalog", "schema", "table"]);
    let d = evaluate(
        &tag_allow(),
        &request(vec![tag, second]),
        &trino_def(),
        &no_superusers(),
    );
    let options_causes = causes(&d)
        .into_iter()
        .filter(|c| {
            matches!(
                c,
                UndecidableCause::TagAttachmentNotReproducible {
                    problem: TagAttachmentProblem::Options(_),
                    ..
                }
            )
        })
        .count();
    assert_eq!(
        options_causes, 1,
        "one finding reported twice must reach the caller once: {d:?}"
    );
}

// ── 2. a tag policy referencing a tag ATTRIBUTE ──────────────────────────

/// A tag attribute is reachable from a policy through exactly two doors, and
/// this is the first: a `${{ … }}` request-time expression. Measured — the only
/// names the script context binds for tags are `TAG`, `TAGS`, `TAGNAMES` and
/// the `ctx.tagAttr*` helpers, all of them inside
/// `RangerRequestScriptEvaluator`'s preamble
/// (`…TAG=_ctx.tag;TAGS=_ctx.tags;TAGNAMES=_ctx.tagNames;`), which only
/// `RangerRequestExprResolver` and the script condition evaluators run.
///
/// The refusal is `MaskExpression`, which already existed; what this asserts is
/// that it is REACHABLE on the tag path, where the policy's own matcher is
/// never consulted and `PolicyView::for_tag` therefore has to assert `matched`
/// by construction. An absent attribute expands to the EMPTY STRING, not to
/// "no constraint" — so omitting this would produce a mask value that is a
/// value, and a different one from Ranger's (SP4d-2b design, D8).
#[test]
fn a_tag_mask_policy_referencing_a_tag_attribute_refuses() {
    let set = envelope(json!([tag_policy(
        11,
        "tag-mask",
        1,
        "dataMaskPolicyItems",
        json!([{
            "users": ["alice"],
            "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": "CUSTOM", "valueExpr": "${{TAG.classification}}"},
        }]),
    )]));

    let d = evaluate_data_mask(
        &set,
        &request(vec![column_tag("pii")]),
        &trino_def(),
        &no_superusers(),
    );
    match &d {
        MaskDecision::Undecidable { why } => assert!(
            why.iter().any(|c| matches!(
                c,
                UndecidableCause::MaskExpression { policy_name, expr, .. }
                    if policy_name == "tag-mask" && expr.contains("TAG.classification")
            )),
            "the mask expression must be named: {why:?}"
        ),
        other => panic!("a tag mask referencing a tag attribute must refuse: {other:?}"),
    }

    // Control: the same policy with a literal mask type answers.
    let set = envelope(json!([tag_policy(
        11,
        "tag-mask",
        1,
        "dataMaskPolicyItems",
        json!([{
            "users": ["alice"],
            "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": "MASK"},
        }]),
    )]));
    let d = evaluate_data_mask(
        &set,
        &request(vec![column_tag("pii")]),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        matches!(&d, MaskDecision::Masked(m) if m.mask_type.as_deref() == Some("MASK")),
        "a tag mask naming no expression must ANSWER: {d:?}"
    );
}

/// The second door, and the one an attribute is most often read through:
/// `RangerScriptConditionEvaluator`, reached from a policy-level `conditions`
/// list. The refusal is `Unmodelled::PolicyConditions`, and the assertion is
/// again that it REACHES the tag path.
#[test]
fn a_tag_policy_whose_condition_could_read_a_tag_attribute_refuses() {
    let mut policy = tag_policy(
        12,
        "tag-cond",
        0,
        "policyItems",
        json!([{ "users": ["alice"], "accesses": [{"type": "select", "isAllowed": true}] }]),
    );
    policy["conditions"] = json!([{
        "type": "expression",
        "values": ["TAG.classification == 'restricted'"],
    }]);

    let d = evaluate(
        &envelope(json!([policy])),
        &request(vec![column_tag("pii")]),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        causes(&d).iter().any(|c| matches!(
            c,
            UndecidableCause::PolicyConstruct { policy_name, construct: Unmodelled::PolicyConditions, .. }
                if policy_name == "tag-cond"
        )),
        "a policy-level condition on a TAG policy must refuse: {d:?}"
    );
}

/// The third door, and the one `collect_causes` reads out of a DIFFERENT bucket:
/// an ITEM-level `conditions` list. `construct_affects` maps
/// `Unmodelled::ItemConditions` onto the bucket the decision actually reads, so
/// a condition in `policyItems` refuses an ACCESS request while one in
/// `allowExceptions` of the same policy does not — and that mapping had never
/// been exercised through `PolicyView::for_tag`, which asserts `matched` by
/// construction instead of computing it.
#[test]
fn a_tag_policy_item_condition_refuses_on_the_tag_path() {
    let policy = tag_policy(
        15,
        "tag-item-cond",
        0,
        "policyItems",
        json!([{
            "users": ["alice"],
            "accesses": [{"type": "select", "isAllowed": true}],
            "conditions": [{"type": "expression", "values": ["TAG.classification == 'x'"]}],
        }]),
    );
    let d = evaluate(
        &envelope(json!([policy])),
        &request(vec![column_tag("pii")]),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        causes(&d).iter().any(|c| matches!(
            c,
            UndecidableCause::PolicyConstruct {
                policy_name,
                construct: Unmodelled::ItemConditions { .. },
                ..
            } if policy_name == "tag-item-cond"
        )),
        "an item condition on a TAG policy must refuse: {d:?}"
    );
}

/// The row-filter twin of `a_tag_mask_policy_referencing_a_tag_attribute_refuses`.
/// It is a separate assertion because it runs through a separate bucket
/// (`rowFilterPolicyItems`), a separate cause, and the third of the three tag
/// phases.
#[test]
fn a_tag_row_filter_policy_referencing_a_tag_attribute_refuses() {
    let table_request = |tags: Vec<ResolvedTag>| AccessRequest {
        // TABLE-granular: Trino's `rowFilterDef.resources` names
        // `catalog`/`schema`/`table` and no `column`, so a column request could
        // never yield and the assertion would measure nothing.
        resource: RequestResource::new()
            .with("catalog", "hive")
            .with("schema", "sales")
            .with("table", "orders"),
        ..request(tags)
    };
    let filter_policy = |expr: &str| {
        envelope(json!([tag_policy(
            16,
            "tag-filter",
            2,
            "rowFilterPolicyItems",
            json!([{
                "users": ["alice"],
                "accesses": [{"type": "select", "isAllowed": true}],
                "rowFilterInfo": {"filterExpr": expr},
            }]),
        )]))
    };

    // Control first, or the refusal below is indistinguishable from "nothing
    // could ever yield here".
    let d = evaluate_row_filter(
        &filter_policy("id > 2"),
        &table_request(vec![column_tag("pii")]),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        matches!(&d, RowFilterDecision::Filtered(f) if f.filter_expr == "id > 2"),
        "the control must FILTER: {d:?}"
    );

    let d = evaluate_row_filter(
        &filter_policy("dept = '${{TAG.classification}}'"),
        &table_request(vec![column_tag("pii")]),
        &trino_def(),
        &no_superusers(),
    );
    match &d {
        RowFilterDecision::Undecidable { why } => assert!(
            why.iter().any(|c| matches!(
                c,
                UndecidableCause::RowFilterExpression { policy_name, expr, .. }
                    if policy_name == "tag-filter" && expr.contains("TAG.classification")
            )),
            "the filter expression must be named: {why:?}"
        ),
        other => panic!("a tag row filter referencing a tag attribute must refuse: {other:?}"),
    }
}

/// **A FAIL-OPEN found by trying to write this pin — fix round 1.**
///
/// The fourth door is a `${{ … }}` expression in the policy's own RESOURCE
/// value. On the resource path `collect_causes` reports
/// `Unmodelled::ResourceValueExpressionMarker` **whether or not the policy
/// matched**, precisely because the marker is what makes `matched` unreliable.
/// The tag path could not reach that rule: `tag_candidates` builds a
/// `PolicyView` only for policies whose `resources.tag` covers an admitted tag
/// NAME, compared as a literal — so a policy whose tag value is an expression
/// matched nothing, became no candidate, and its marker was never reported.
///
/// Ranger does not read it as a literal. `ResourceMatcher`'s constructor builds
/// a `RangerRequestExprResolver` whenever `hasExpressions(value)` (offsets
/// 17-41) and `getExpandedValue(evalContext)` calls
/// `resolver.resolveExpressions(request)` (offsets 22-26), so the value is
/// resolved PER REQUEST and can match a tag this crate said it could not. The
/// policy vanished from the candidate set with a confident answer behind it.
///
/// Fixed by reporting the marker from the per-`of_kind` sweep, where candidacy
/// has not been decided yet.
#[test]
fn a_tag_policy_whose_tag_value_is_an_expression_refuses() {
    let mut policy = tag_policy(
        17,
        "tag-expr-value",
        0,
        "denyPolicyItems",
        json!([{ "users": ["alice"], "accesses": [{"type": "select", "isAllowed": true}] }]),
    );
    policy["resources"] = json!({ "tag": {"values": ["${{TAG.classification}}"]} });

    // The deny policy is the one that matters: with the marker unreported this
    // returned `Denied{NoPolicyMatched}` for a policy Ranger may have matched
    // and denied — a confident answer standing in for an unknown one.
    let d = evaluate(
        &envelope(json!([policy])),
        &request(vec![column_tag("pii")]),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        causes(&d).iter().any(|c| matches!(
            c,
            UndecidableCause::PolicyConstruct {
                policy_name,
                construct: Unmodelled::ResourceValueExpressionMarker { .. },
                ..
            } if policy_name == "tag-expr-value"
        )),
        "a tag policy whose tag value is resolved per request must refuse: {d:?}"
    );
}

/// **FIX ROUND 2, MINOR A — the same conflation as the fail-open above, one
/// construct over.** `collect_causes`' `always` list means "regardless of
/// `matched`", and the tag path builds a `PolicyView` only for CANDIDATES, so on
/// this path it means "regardless of `matched`, among policies whose tag value
/// already matched". A zoned tag policy naming a tag nobody resolved was
/// therefore observed and dropped.
///
/// Both compositions the re-review measured are here. The first alone answers
/// `Denied{NoPolicyMatched}`; the second is the one that would matter, because
/// Ranger's associated-zone arm (offset 285,
/// `equals(requestZone, policy.getZoneName())`) skips every zone-LESS tag policy
/// and so would never have reached the allow.
///
/// It was not a live fail-open — the associated arm needs a non-empty
/// `securityZones`, which already refuses at the envelope — and that is exactly
/// why it is worth a test: the verdict was right through a guard other than the
/// one the comment credited.
#[test]
fn a_zoned_tag_policy_refuses_even_when_no_resolved_tag_reaches_it() {
    let zoned = |id: i64, name: &str| {
        let mut p = tag_policy(
            id,
            name,
            0,
            "denyPolicyItems",
            json!([{ "users": ["alice"], "accesses": [{"type": "select", "isAllowed": true}] }]),
        );
        // `phi`, which the request does NOT carry — so this policy is not a
        // candidate and `collect_causes` never sees it.
        p["resources"] = json!({ "tag": {"values": ["phi"]} });
        p["zoneName"] = json!("z1");
        p
    };

    let expected = |name: &str| UndecidableCause::PolicyConstruct {
        policy_id: Some(80),
        policy_name: name.to_string(),
        construct: Unmodelled::ZoneName("z1".to_string()),
    };

    // 1. The zoned policy alone.
    let d = evaluate(
        &envelope(json!([zoned(80, "zoned-deny")])),
        &request(vec![column_tag("pii")]),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        causes(&d).contains(&expected("zoned-deny")),
        "a zoned tag policy no resolved tag reaches must still refuse: {d:?}"
    );

    // 2. Beside a zone-LESS allow the crate would otherwise report. This is the
    //    composition that shows what the silence cost: Ranger's associated-zone
    //    arm skips the zone-less policy, so `Allowed{70}` was an answer Ranger
    //    need not agree with.
    let allow = tag_policy(
        70,
        "zoneless-allow",
        0,
        "policyItems",
        json!([{ "users": ["alice"], "accesses": [{"type": "select", "isAllowed": true}] }]),
    );
    let d = evaluate(
        &envelope(json!([allow, zoned(80, "zoned-deny")])),
        &request(vec![column_tag("pii")]),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        causes(&d).contains(&expected("zoned-deny")),
        "a zone-less allow must not be reported while a zoned policy sits unread \
         beside it: {d:?}"
    );

    // 3. The control, on the same fixture: with no `zoneName` the second policy
    //    is an ordinary dormant tag policy and the allow ANSWERS. Without this,
    //    the two assertions above are satisfied by refusing every download that
    //    carries a second tag policy at all.
    let mut unzoned = zoned(80, "zoned-deny");
    unzoned
        .as_object_mut()
        .expect("policy object")
        .remove("zoneName");
    let allow = tag_policy(
        70,
        "zoneless-allow",
        0,
        "policyItems",
        json!([{ "users": ["alice"], "accesses": [{"type": "select", "isAllowed": true}] }]),
    );
    let d = evaluate(
        &envelope(json!([allow, unzoned])),
        &request(vec![column_tag("pii")]),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        matches!(
            &d,
            Decision::Allowed {
                policy_id: Some(70),
                ..
            }
        ),
        "the zone is what refuses, not the presence of a second policy: {d:?}"
    );
}

/// The control for the fix above: a LITERAL tag value that does not match the
/// resolved tag must still be an ANSWER, not a refusal. Without this, reporting
/// the marker over every policy of the kind would be indistinguishable from
/// refusing every download that carries a dormant tag policy.
#[test]
fn a_literal_tag_value_that_does_not_match_still_answers() {
    let mut dormant = tag_policy(
        18,
        "tag-other",
        0,
        "denyPolicyItems",
        json!([{ "users": ["alice"], "accesses": [{"type": "select", "isAllowed": true}] }]),
    );
    dormant["resources"] = json!({ "tag": {"values": ["phi"]} });

    let d = evaluate(
        &envelope(json!([dormant])),
        &request(vec![column_tag("pii")]),
        &trino_def(),
        &no_superusers(),
    );
    assert_eq!(
        d,
        Decision::Denied {
            policy_id: None,
            policy_name: String::new(),
            reason: DenyReason::NoPolicyMatched,
        },
        "a literal tag value this crate can compare decides nothing and refuses nothing: {d:?}"
    );
}

// ── 3. a tag service resource with fewer than three levels ───────────────

/// `ServiceTags::from_assignments` omits an empty `column`/`schema`/`table`
/// level, and `catalog_tag_assignments` makes `schema_name`/`table_name`
/// `NOT NULL` with no empty-string convention — only `column_name` has one. So
/// three levels is the floor for anything egret authored, and fewer means the
/// payload came from somewhere else, whose resolution rules are not the ones
/// SP4d-2b measured (design D2, the user's scope ruling).
///
/// Note what this is NOT: it is not Ranger dropping the resource.
/// `hierarchyHasAllResources` is a SUBSET test, so `{catalog, schema}` validates
/// happily against Trino's `catalog/schema/table/column` chain and Ranger keeps
/// it. The sibling test below is the shape Ranger really does drop, and the two
/// must not be confused for one rule.
#[test]
fn a_tag_service_resource_with_fewer_than_three_levels_refuses() {
    let mut tag = column_tag("pii");
    tag.attached_through.levels = levels(&["catalog", "schema"]);

    let d = evaluate(
        &tag_allow(),
        &request(vec![tag]),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        causes(&d).contains(&UndecidableCause::TagAttachmentNotReproducible {
            tag: "pii".to_string(),
            problem: TagAttachmentProblem::ResourceLevels(level_names(&["catalog", "schema"])),
        }),
        "a schema-level attachment must refuse, naming the shape: {d:?}"
    );

    // Control: three levels — the table-level attachment egret DOES write —
    // answers.
    let mut tag = column_tag("pii");
    tag.attached_through.levels = levels(&["catalog", "schema", "table"]);
    let d = evaluate(
        &tag_allow(),
        &request(vec![tag]),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        matches!(&d, Decision::Allowed { policy_name, .. } if policy_name == "tag-allow"),
        "a table-level attachment is one egret writes and must ANSWER: {d:?}"
    );

    // FIX ROUND 1, MINOR 1 — the ONE-level shape, asserted on its own. It is the
    // case a `Vec<String>` `levels` got wrong: `["catalog", "catalog",
    // "catalog"]` counted three and answered, for an attachment whose real key
    // set is `{catalog}`. `levels` is a `BTreeSet<String>` now, so the two cannot
    // disagree — and the shape the bug let through is pinned here rather than
    // left to the type alone.
    let mut tag = column_tag("pii");
    tag.attached_through.levels = levels(&["catalog"]);
    let d = evaluate(
        &tag_allow(),
        &request(vec![tag]),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        causes(&d).contains(&UndecidableCause::TagAttachmentNotReproducible {
            tag: "pii".to_string(),
            problem: TagAttachmentProblem::ResourceLevels(level_names(&["catalog"])),
        }),
        "a one-level attachment — which Ranger KEEPS — must refuse: {d:?}"
    );
}

/// The refusal does not depend on the order the caller listed the attachments
/// in. `attached_through` is deliberately outside `ResolvedTag::identity` —
/// `RangerTagForEval` carries no service resource — so one tag reaching the
/// request through both a modelled and an unmodelled resource is ONE member of
/// Ranger's set, and a sweep folded into the dedup loop would refuse or answer
/// according to which the caller happened to write first.
#[test]
fn the_shape_refusal_does_not_depend_on_the_callers_order() {
    let mut odd = column_tag("pii");
    odd.attached_through.levels = levels(&["catalog", "schema"]);
    let ordinary = column_tag("pii");

    for (label, tags) in [
        ("odd first", vec![odd.clone(), ordinary.clone()]),
        ("ordinary first", vec![ordinary, odd]),
    ] {
        let d = evaluate(&tag_allow(), &request(tags), &trino_def(), &no_superusers());
        assert!(
            causes(&d).contains(&UndecidableCause::TagAttachmentNotReproducible {
                tag: "pii".to_string(),
                problem: TagAttachmentProblem::ResourceLevels(level_names(&["catalog", "schema"])),
            }),
            "{label}: the verdict must not turn on the caller's Vec order: {d:?}"
        );
    }
}

// ── 4. the existing per-policy refusals, on the TAG path ─────────────────

/// `Unmodelled::ValiditySchedules` and `Unmodelled::PolicyConditions` were
/// built for the resource path, where `PolicyView::build` computes `matched`
/// from the policy's own matcher. A tag request never asks that matcher — the
/// match type comes off the ATTACHMENT — so `PolicyView::for_tag` asserts
/// `matched` by construction, and whether `collect_causes` still reports these
/// is a property of that second constructor, not of the first.
#[test]
fn a_tag_policy_carrying_validity_schedules_refuses() {
    let mut policy = tag_policy(
        13,
        "tag-window",
        0,
        "policyItems",
        json!([{ "users": ["alice"], "accesses": [{"type": "select", "isAllowed": true}] }]),
    );
    policy["validitySchedules"] = json!([{ "startTime": "2020/01/01 00:00:00" }]);

    let d = evaluate(
        &envelope(json!([policy])),
        &request(vec![column_tag("pii")]),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        causes(&d).iter().any(|c| matches!(
            c,
            UndecidableCause::PolicyConstruct { policy_name, construct: Unmodelled::ValiditySchedules, .. }
                if policy_name == "tag-window"
        )),
        "a validity schedule on a TAG policy must refuse: {d:?}"
    );
}

/// The per-policy sweep must not fire for a tag policy no tag reaches: Ranger
/// never builds an evaluator for it, so its constructs cannot decide anything.
/// Without this the test above is satisfied by a sweep over every policy in the
/// block, which would refuse every download carrying one dormant tag policy.
#[test]
fn a_tag_policy_no_resolved_tag_reaches_does_not_refuse() {
    let mut dormant = tag_policy(
        14,
        "tag-window-other",
        0,
        "policyItems",
        json!([{ "users": ["alice"], "accesses": [{"type": "select", "isAllowed": true}] }]),
    );
    dormant["resources"] = json!({ "tag": {"values": ["phi"]} });
    dormant["validitySchedules"] = json!([{ "startTime": "2020/01/01 00:00:00" }]);

    let allow = tag_policy(
        10,
        "tag-allow",
        0,
        "policyItems",
        json!([{ "users": ["alice"], "accesses": [{"type": "select", "isAllowed": true}] }]),
    );

    let d = evaluate(
        &envelope(json!([allow, dormant])),
        &request(vec![column_tag("pii")]),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        matches!(&d, Decision::Allowed { policy_name, .. } if policy_name == "tag-allow"),
        "a tag policy the resolved set cannot reach decides nothing: {d:?}"
    );
}

// ── 5. control: the envelope refusals are not narrowed by any of this ────

/// `Unmodelled::ServiceConfig` carries `ranger.plugin.super.users`, and a
/// super-user hit sets `isAllowed` with `policyPriority = Integer.MAX_VALUE`
/// before any policy is consulted. The tag phase's own sweep refuses it too,
/// deliberately. Asserted explicitly so that a later narrowing of the tag path
/// cannot take it along by accident.
#[test]
fn service_config_still_refuses_on_the_tag_path() {
    let mut envelope_json = json!({
        "serviceName": "trino",
        "policies": [],
        "serviceDef": {"name": "trino"},
        "serviceConfig": {"ranger.plugin.super.users": "root"},
        "tagPolicies": {
            "serviceName": "trino_tag",
            "serviceId": 99,
            "policyVersion": 1,
            "serviceDef": tag_def_json(),
        },
    });
    envelope_json["tagPolicies"]["policies"] = json!([tag_policy(
        10,
        "tag-allow",
        0,
        "policyItems",
        json!([{ "users": ["alice"], "accesses": [{"type": "select", "isAllowed": true}] }]),
    )]);
    let set: ServicePolicySet = serde_json::from_value(envelope_json).expect("parse envelope");

    let d = evaluate(
        &set,
        &request(vec![column_tag("pii")]),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        causes(&d).contains(&UndecidableCause::EnvelopeConstruct(
            Unmodelled::ServiceConfig
        )),
        "serviceConfig must still refuse: {d:?}"
    );
}

// ── D9: the two enricher behaviours that are ANSWERS ─────────────────────

/// `createRangerServiceResourceMatcher` returns null when no policy type
/// validates the key set, and `processServiceTags` then calls
/// `iter.remove()` (offset 151) AND
/// `serviceTags.getResourceToTagIds().remove(res.getId())` (offsets 158-167).
/// Both halves matter: the resource going is not enough on its own, because the
/// tag link surviving would leave the tag attached to nothing. Reproduced by
/// dropping the attachment, which after resolution is the same object.
///
/// `database` is StarRocks' middle level; on Trino's def it appears in no
/// hierarchy of any of `RangerPolicy.POLICY_TYPES`, which is exactly the
/// mismatched-download shape this rule exists for.
#[test]
fn a_service_resource_validating_under_no_policy_type_is_dropped_with_its_tag_link() {
    let mut tag = column_tag("pii");
    tag.attached_through.levels = levels(&["catalog", "database", "table", "column"]);

    let d = evaluate(
        &tag_allow(),
        &request(vec![tag]),
        &trino_def(),
        &no_superusers(),
    );
    assert_eq!(
        d,
        Decision::Denied {
            policy_id: None,
            policy_name: String::new(),
            reason: DenyReason::NoPolicyMatched,
        },
        "the attachment and its tag link are dropped, so `pii` reaches no evaluator: {d:?}"
    );

    // And it is a DROP, not a refusal: the answer is Ranger's, with no cause.
    assert!(
        causes(&d).is_empty(),
        "a dropped resource is an answer, not an `Undecidable`: {d:?}"
    );
}

/// The drop is ordered BEFORE the shape refusal, and that ordering is Ranger's:
/// a resource the enricher removed never becomes a `RangerTagForEval`, so there
/// is nothing left for a refusal to be about. `{bogus}` is both invalid AND
/// shorter than three levels, so it is the one payload that can tell the two
/// rules apart.
#[test]
fn a_dropped_resource_is_not_also_refused_for_its_shape() {
    let mut tag = column_tag("pii");
    tag.attached_through.levels = levels(&["bogus"]);

    let d = evaluate(
        &tag_allow(),
        &request(vec![tag]),
        &trino_def(),
        &no_superusers(),
    );
    assert_eq!(
        d,
        Decision::Denied {
            policy_id: None,
            policy_name: String::new(),
            reason: DenyReason::NoPolicyMatched,
        },
        "the drop runs first, so no shape refusal is raised: {d:?}"
    );
}

/// The SAME ten rows the Java probe measures against the real
/// `RangerTagEnricher.createRangerServiceResourceMatcher`
/// (`tests/ranger/src/test/java/EgretTagServiceResourceValidationTest.java`,
/// green at 2.5.0 and 2.8.0 with an identical table). The Rust rule is a
/// hand-written copy of that method; asserting it only against the prose that
/// produced it would be a correlated oracle, so the two sides carry one table
/// and it is measured on one of them.
///
/// Read behaviourally rather than through the private predicate: `kept` means
/// the attachment reaches `tag-allow` and the request is `Allowed`, `dropped`
/// means it does not exist and nothing matches. Rows `catalog,schema` and
/// `catalog` are `kept` by Ranger and REFUSED by this crate for provenance —
/// so they are exercised through a request that also carries a modelled
/// attachment, where "kept" is observable as "the shape refusal is raised for
/// it", not as an allow.
#[test]
fn the_drop_table_matches_the_one_measured_on_the_jar() {
    // (levels, kept by Ranger?)
    let rows: [(&[&str], bool); 10] = [
        (&["catalog", "schema", "table", "column"], true),
        (&["catalog", "schema", "table"], true),
        (&["catalog", "schema"], true),
        (&["catalog"], true),
        (&[], true),
        (&["catalog", "database", "table", "column"], false),
        (&["database"], false),
        (&["bogus"], false),
        (&["catalog", "schema", "table", "column", "bogus"], false),
        (&["catalog", "queryid"], false),
    ];

    for (names, kept) in rows {
        let mut tag = column_tag("pii");
        tag.attached_through.levels = levels(names);
        let d = evaluate(
            &tag_allow(),
            &request(vec![tag]),
            &trino_def(),
            &no_superusers(),
        );

        // A KEPT attachment reaches the phase: it either answers (three or more
        // levels) or is refused for its shape (fewer). A DROPPED one does
        // neither — it is not there at all.
        let reached = !matches!(
            d,
            Decision::Denied {
                reason: DenyReason::NoPolicyMatched,
                ..
            }
        );
        assert_eq!(
            reached, kept,
            "levels {names:?}: Ranger keeps it? {kept}; this crate reached the phase? \
             {reached} — {d:?}"
        );
    }
}

/// The MASK path runs the enricher too, and it is a second call site rather
/// than the same one: `evaluate` and `evaluate_sub` each hold the resource def
/// and each rewrites the request for its own phase-1 call. A test only on the
/// ACCESS path would leave the sub path free to skip the drop entirely — which
/// is the shape this package keeps finding, two guards that must agree written
/// in two places.
#[test]
fn the_mask_path_drops_the_same_service_resources() {
    let set = envelope(json!([tag_policy(
        11,
        "tag-mask",
        1,
        "dataMaskPolicyItems",
        json!([{
            "users": ["alice"],
            "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": "MASK"},
        }]),
    )]));

    // Control first: the attachment egret writes reaches the tag mask.
    let d = evaluate_data_mask(
        &set,
        &request(vec![column_tag("pii")]),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        matches!(&d, MaskDecision::Masked(m) if m.policy_name == "tag-mask"),
        "the control must MASK, or the assertion below measures nothing: {d:?}"
    );

    let mut tag = column_tag("pii");
    tag.attached_through.levels = levels(&["catalog", "database", "table", "column"]);
    let d = evaluate_data_mask(&set, &request(vec![tag]), &trino_def(), &no_superusers());
    assert_eq!(
        d,
        MaskDecision::NoMask,
        "a dropped attachment reaches no tag mask either: {d:?}"
    );
}

/// The oracle for the THREE-policy-type loop — fix round 1, ruling 2.
///
/// `service_resource_validates` asks all of `{Access, DataMask, RowFilter}`
/// because `createRangerServiceResourceMatcher` loops
/// `RangerPolicy.POLICY_TYPES`. On **every service-def egret ships** that loop
/// is unobservable: `trino.json`'s and `starrocks.json`'s `dataMaskDef` /
/// `rowFilterDef` resource names are subsets of their access names (checked:
/// mask `{catalog, schema|database, table, column}`, filter the same minus
/// `column`, access a superset of both), so a key set valid for masking is
/// valid for access and a one-type loop answers identically. `tag.json` has one
/// resource. "Right because Ranger does it" is the unfalsifiable assertion this
/// package keeps catching, so the loop needs a case where the types disagree.
///
/// **No real def discriminates, so this def is CONSTRUCTED**, and it is
/// constructed by adding ONE resource to the real `trino.json`: a `dataMaskDef`
/// entry named `maskonly` with no counterpart in `resources`. Everything else
/// is the shipped file.
///
/// The same case is measured against Ranger itself in
/// `tests/ranger/.../EgretTagServiceResourceValidationTest`
/// (`aKeySetValidUnderMaskingOnlySurvivesBecauseTheLoopVisitsAllThreeTypes`),
/// which confirms the real `createRangerServiceResourceMatcher` KEEPS it —
/// `{maskonly}` is a one-element DATA_MASK hierarchy there because the raw
/// `dataMaskDef` resources carry no `parent` and each is its own root.
#[test]
fn a_key_set_valid_only_for_masking_survives_the_three_policy_type_loop() {
    let mut def_json = serde_json::from_str::<Value>(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../egret-policy/src/service-defs/trino.json"
        ))
        .expect("read trino def"),
    )
    .expect("parse trino def");
    def_json["dataMaskDef"]["resources"]
        .as_array_mut()
        .expect("trino.json declares dataMaskDef.resources")
        .push(json!({
            "name": "maskonly",
            "type": "string",
            "level": 10,
            "mandatory": true,
            "isValidLeaf": true,
            "matcher": "org.apache.ranger.plugin.resourcematcher.RangerDefaultResourceMatcher",
        }));
    let synthetic = ServiceDef::parse(&def_json);

    let mut tag = column_tag("pii");
    tag.attached_through.levels = levels(&["maskonly"]);

    // KEPT — but it is a ONE-level key set, so what proves it was kept is the
    // shape REFUSAL firing, not an allow. A dropped attachment produces neither.
    let d = evaluate(
        &tag_allow(),
        &request(vec![tag]),
        &synthetic,
        &no_superusers(),
    );
    assert!(
        causes(&d).contains(&UndecidableCause::TagAttachmentNotReproducible {
            tag: "pii".to_string(),
            problem: TagAttachmentProblem::ResourceLevels(level_names(&["maskonly"])),
        }),
        "`maskonly` validates under policy type 1 and no other, so the attachment \
         survives exactly when the loop consults the masking hierarchies: {d:?}"
    );

    // The falsifier on the SAME def: a name in no hierarchy of any type still
    // drops, so the row above is about `maskonly` being mask-only and not about
    // the def having become permissive.
    let mut tag = column_tag("pii");
    tag.attached_through.levels = levels(&["bogus"]);
    let d = evaluate(
        &tag_allow(),
        &request(vec![tag]),
        &synthetic,
        &no_superusers(),
    );
    assert_eq!(
        d,
        Decision::Denied {
            policy_id: None,
            policy_name: String::new(),
            reason: DenyReason::NoPolicyMatched,
        },
        "a name in no hierarchy of any policy type must still drop: {d:?}"
    );
}

/// `isEnabled` appears nowhere in `processServiceTags`: the loop branches only
/// on whether `createRangerServiceResourceMatcher` returned null. A disabled
/// service resource therefore enriches exactly like an enabled one — and a
/// reader who "fixed" that by filtering on it would silently lose every tag on
/// any payload that ever sets the field.
#[test]
fn is_enabled_on_a_service_resource_is_never_consulted() {
    let enabled = evaluate(
        &tag_allow(),
        &request(vec![column_tag("pii")]),
        &trino_def(),
        &no_superusers(),
    );

    let mut tag = column_tag("pii");
    tag.attached_through.is_enabled = false;
    let disabled = evaluate(
        &tag_allow(),
        &request(vec![tag]),
        &trino_def(),
        &no_superusers(),
    );

    assert_eq!(
        enabled, disabled,
        "isEnabled is not read, so the two verdicts must be identical"
    );
    assert!(
        matches!(&enabled, Decision::Allowed { policy_name, .. } if policy_name == "tag-allow"),
        "and both must be the tag ALLOW, or the equality is between two nothings: {enabled:?}"
    );
}

// ── 5. the TAG repository's own normalisation (SP4d-2b Task 5) ────────────
//
// `RangerPolicyRepository`'s `ServicePolicies$TagPolicies` constructor calls
// `normalizeAndPrunePolicies`, and **no other constructor in either jar does**.
// Every assertion below was written after the differential gate made the real
// 2.5.0 and 2.8.0 engines demonstrate the behaviour, not from reading the class:
// the crash in particular is a measured
// `NullPointerException ... at RangerPolicyRepository.normalizeAndPrunePolicyItems`.

/// A tag MASK policy whose item carries no `dataMaskInfo` at all.
///
/// On the RESOURCE path this is legal and is SP4d-1's pinned finding F2 — the
/// item DETERMINES with a null mask type. In the TAG block it makes the plugin
/// throw while building its repository, so there is no engine and no request of
/// any type is answered. The two paths differ because only one of them
/// normalises.
#[test]
fn a_tag_mask_item_with_no_mask_type_refuses_because_the_plugin_crashes() {
    let set = envelope(json!([tag_policy(
        20,
        "tag-mask-null",
        1,
        "dataMaskPolicyItems",
        json!([{ "users": ["alice"], "accesses": [{"type": "select", "isAllowed": true}] }]),
    )]));

    let d = evaluate_data_mask(
        &set,
        &request(vec![column_tag("pii")]),
        &trino_def(),
        &no_superusers(),
    );
    assert_eq!(
        d,
        MaskDecision::Undecidable {
            why: vec![UndecidableCause::TagPolicyNotNormalisable {
                policy_id: Some(20),
                policy_name: "tag-mask-null".to_string(),
                problem: TagNormalisationProblem::MaskTypeAbsent,
            }]
        },
        "a null dataMaskType in the tag block is a constructor NPE, not a mask"
    );
}

/// …and the crash is not scoped to the decision's own policy type, or to an
/// ENABLED policy. Both halves matter and both are measured: the constructor
/// runs before `init()` (so `skipBuildingPolicyEvaluator` has dropped nothing
/// yet) and the throw leaves no engine (so an ACCESS request fails too).
///
/// This is the test that would fail if someone "tidied" the sweep by moving it
/// below the `of_kind` filter or adding `.filter(|p| p.is_enabled)` — both of
/// which look like obvious cleanups and both of which would restore a confident
/// answer where the real plugin serves none.
#[test]
fn the_tag_mask_crash_refuses_an_access_request_and_a_disabled_policy_too() {
    let mut policy = tag_policy(
        21,
        "tag-mask-null-disabled",
        1,
        "dataMaskPolicyItems",
        json!([{ "users": ["alice"], "accesses": [{"type": "select", "isAllowed": true}] }]),
    );
    policy["isEnabled"] = json!(false);
    // Beside a perfectly good tag ACCESS policy, so "refused" cannot be read as
    // "nothing was there to decide".
    let set = envelope(json!([
        policy,
        tag_policy(
            10,
            "tag-allow",
            0,
            "policyItems",
            json!([{ "users": ["alice"], "accesses": [{"type": "select", "isAllowed": true}] }]),
        ),
    ]));

    let d = evaluate(
        &set,
        &request(vec![column_tag("pii")]),
        &trino_def(),
        &no_superusers(),
    );
    assert_eq!(
        causes(&d),
        vec![UndecidableCause::TagPolicyNotNormalisable {
            policy_id: Some(21),
            policy_name: "tag-mask-null-disabled".to_string(),
            problem: TagNormalisationProblem::MaskTypeAbsent,
        }],
        "the constructor throws before init() and for every policy type: {d:?}"
    );
}

/// An access type carrying a `:` — the namespaced spelling a Ranger-authored
/// tag policy uses (`trino:select`).
///
/// **This is the fail-open direction.** Ranger strips `"<componentDefName>:"`
/// and the item then grants `select`; this crate compares the literal
/// `"trino:select"` against `"select"`, matches nothing, and would answer
/// `Denied{NoPolicyMatched}` for an ALLOW policy and — the direction that
/// matters — would MISS a deny.
#[test]
fn a_namespaced_access_type_on_a_tag_policy_refuses() {
    let set = envelope(json!([tag_policy(
        30,
        "tag-deny-namespaced",
        0,
        "denyPolicyItems",
        json!([{ "users": ["alice"], "accesses": [{"type": "trino:select", "isAllowed": true}] }]),
    )]));

    let d = evaluate(
        &set,
        &request(vec![column_tag("pii")]),
        &trino_def(),
        &no_superusers(),
    );
    assert_eq!(
        causes(&d),
        vec![UndecidableCause::TagPolicyNotNormalisable {
            policy_id: Some(30),
            policy_name: "tag-deny-namespaced".to_string(),
            problem: TagNormalisationProblem::NamespacedAccessType("trino:select".to_string()),
        }],
        "a `:` in a tag policy's access type is Ranger's namespace, not a literal: {d:?}"
    );
}

/// The same for `dataMaskType`, where Ranger's DROP arm removes the whole ITEM
/// rather than one access entry.
#[test]
fn a_namespaced_mask_type_on_a_tag_policy_refuses() {
    let set = envelope(json!([tag_policy(
        31,
        "tag-mask-namespaced",
        1,
        "dataMaskPolicyItems",
        json!([{
            "users": ["alice"],
            "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": "hive:MASK_HASH"},
        }]),
    )]));

    let d = evaluate_data_mask(
        &set,
        &request(vec![column_tag("pii")]),
        &trino_def(),
        &no_superusers(),
    );
    assert_eq!(
        d,
        MaskDecision::Undecidable {
            why: vec![UndecidableCause::TagPolicyNotNormalisable {
                policy_id: Some(31),
                policy_name: "tag-mask-namespaced".to_string(),
                problem: TagNormalisationProblem::NamespacedMaskType("hive:MASK_HASH".to_string()),
            }]
        },
        "a `:` in a tag policy's dataMaskType is Ranger's namespace: {d:?}"
    );
}

/// The one arm of the normalisation this crate REPRODUCES: a tag policy with no
/// items and `isAuditEnabled: false` is not in Ranger's repository at all, so
/// its `isDenyAllElse` cannot deny.
///
/// `isDenyAllElse` is not one of the six buckets `isPolicyNeedsPruning` tests,
/// which is the whole reason the prune is observable. Without the filter this
/// answers `Denied{DenyAllElse}` where the real plugin, having pruned the
/// policy, reaches no tag verdict — fail-CLOSED, which is why nothing noticed.
#[test]
fn an_audit_disabled_tag_policy_with_no_items_is_pruned_and_cannot_deny() {
    let mut policy = tag_policy(40, "tag-deny-all-else", 0, "policyItems", json!([]));
    policy["isDenyAllElse"] = json!(true);
    policy["isAuditEnabled"] = json!(false);

    let pruned = evaluate(
        &envelope(json!([policy.clone()])),
        &request(vec![column_tag("pii")]),
        &trino_def(),
        &no_superusers(),
    );
    assert_eq!(
        pruned,
        Decision::Denied {
            policy_id: None,
            policy_name: String::new(),
            reason: DenyReason::NoPolicyMatched,
        },
        "the pruned policy must reach no verdict at all, not deny: {pruned:?}"
    );

    // The falsifier, on the SAME policy with auditing left at its default: the
    // prune is about `isAuditEnabled`, not about the policy having no items.
    // Without this, a bug that dropped every item-less tag policy would pass
    // the assertion above.
    let mut audited = policy;
    audited["isAuditEnabled"] = json!(true);
    let kept = evaluate(
        &envelope(json!([audited])),
        &request(vec![column_tag("pii")]),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        matches!(
            &kept,
            Decision::Denied { reason: DenyReason::DenyAllElse, policy_name, .. }
                if policy_name == "tag-deny-all-else"
        ),
        "with auditing on, the identical policy is kept and DOES deny-all-else: {kept:?}"
    );
}

/// An item with an EMPTY `accesses` list does NOT empty its bucket, so a policy
/// carrying one is not pruned however `isAuditEnabled` reads.
///
/// `CollectionUtils.isNotEmpty(accesses)` guards the whole item-pruning block at
/// offset 67 of `normalizeAndPrunePolicyItems`, so such an item survives. The
/// tempting "an item with no accesses grants nothing, treat the bucket as empty"
/// shortcut would prune a policy Ranger keeps.
#[test]
fn an_item_with_no_accesses_does_not_make_its_bucket_empty() {
    let mut policy = tag_policy(
        41,
        "tag-empty-accesses",
        0,
        "policyItems",
        json!([{ "users": ["alice"], "accesses": [] }]),
    );
    policy["isDenyAllElse"] = json!(true);
    policy["isAuditEnabled"] = json!(false);

    let d = evaluate(
        &envelope(json!([policy])),
        &request(vec![column_tag("pii")]),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        matches!(
            &d,
            Decision::Denied { reason: DenyReason::DenyAllElse, policy_name, .. }
                if policy_name == "tag-empty-accesses"
        ),
        "the bucket is non-empty, so the policy is NOT pruned and still denies-all-else: {d:?}"
    );
}
