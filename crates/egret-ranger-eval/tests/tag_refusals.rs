//! SP4d-2b Task 4: the refusals that keep the tag phase honest, and the two
//! enricher behaviours (D9) that are ANSWERS rather than refusals.
//!
//! # What is asserted here, and why each is worth an assertion today
//!
//! Every refusal in this file is **inert for the payload egret emits**.
//! `ServiceTags::from_assignments` writes each wire tag as
//! `{"type": <tags.name>, "owner": 0}` — no `attributes`, no `options`, no
//! `validityPeriods` — and each service resource from
//! `catalog_tag_assignments`, whose `schema_name` and `table_name` are
//! `NOT NULL` with no empty-string convention, so its `resourceElements` always
//! carries at least `catalog`/`schema`/`table`. Asserting the refusals anyway
//! is what stops a hand-written `/tags/download` shim, an imported payload or a
//! future egret feature from changing the answer silently: with the assertion,
//! the day one of those arrives the verdict says so; without it, the day one of
//! those arrives the verdict is confidently wrong.
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
    evaluate, evaluate_data_mask, AccessRequest, Decision, DenyReason, EngineConfig, MaskDecision,
    ResolvedTag, TagAttachmentProblem, TagServiceResource, UndecidableCause,
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

fn levels(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
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
            problem: TagAttachmentProblem::ResourceLevels(levels(&["catalog", "schema"])),
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
                problem: TagAttachmentProblem::ResourceLevels(levels(&["catalog", "schema"])),
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
