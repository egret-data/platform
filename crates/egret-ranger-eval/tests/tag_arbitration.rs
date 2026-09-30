//! SP4d-2b Task 3: the two phases composed, and Ranger's three-way priority
//! arbitration between them.
//!
//! # What was measured in the jars for this task, and where
//!
//! `RangerPolicyEngineImpl.evaluatePoliciesForOneAccessTypeNoAudit`, disassembled
//! with `javap -c -p` on `ranger-plugins-common` **2.5.0** and **2.8.0**. The
//! method is byte-identical between the two versions at every offset cited
//! below — same branch targets, same comparison opcodes — so one set of offsets
//! serves both:
//!
//! ```java
//! //  61-165: isSuperUser / _super_user short-circuit; sets policyPriority = MAX_VALUE
//! //     176: evaluateTagPolicies(request, policyType, zoneName, tagRepository, result)
//! // 291-312: isAllowedByTags = result.getIsAccessDetermined() &&  result.getIsAllowed()
//! // 314-335: isDeniedByTags  = result.getIsAccessDetermined() && !result.getIsAllowed()
//! // 337-350: if (!policyEngine.hasResourcePolicies(policyRepository)) skip everything
//! //     391: result.setIsAccessDetermined(false)                      // THE CLEAR
//! //     401: evaluators = policyRepository.getLikelyMatchPolicyEvaluators(request, policyType)
//! //     437: if (!evaluator.isApplicable(accessTime)) continue;
//! //     452: if (isDeniedByTags) {
//! //     469:     if (result.getPolicyPriority() >= evaluator.getPolicyPriority())   // if_icmplt 538
//! //     472:         result.setIsAccessDetermined(true);
//! //     481: } else if (result.getIsAllowed()) {
//! //     489:     if (policyType == POLICY_TYPE_ACCESS) {
//! //     505:         if (result.getPolicyPriority() >  evaluator.getPolicyPriority())  // if_icmple 538
//! //     508:             result.setIsAccessDetermined(true);
//! //     517:     } else {
//! //     529:         if (result.getPolicyPriority() >= evaluator.getPolicyPriority())  // if_icmplt 538
//! //     532:             result.setIsAccessDetermined(true);
//! //     543: evaluator.evaluate(request, result);   // its items are guarded by
//! //                                                 // !getIsAccessDetermined()
//! //                                                 // (RangerDefaultPolicyEvaluator
//! //                                                 //  offsets 380-384)
//! //     553: if (result.getIsAllowed() && !evaluator.hasDeny()) setIsAccessDetermined(true);
//! // 599-632: if (!getIsAccessDetermined()) {                          // THE RESTORE
//! //              if      (isDeniedByTags)  setIsAllowed(false);
//! //              else if (isAllowedByTags) setIsAllowed(true);
//! //          }
//! ```
//!
//! The three asymmetries are the three comparison opcodes above, and they are
//! one character apart in Rust:
//!
//! | tag verdict | resource policy type | opcode | stands the tag verdict |
//! |---|---|---|---|
//! | deny | any | `if_icmplt 538` at 469 | `>=` |
//! | allow | access (`policyType == 0`) | `if_icmple 538` at 505 | `>` |
//! | allow | mask / row filter | `if_icmplt 538` at 529 | `>=` |
//!
//! ## Two facts the loop's shape depends on, also measured
//!
//! * `RangerAccessResult.setIsAllowed(false)` calls `setIsAccessDetermined(true)`
//!   itself (offsets 0-6), which is why a deny ends the evaluation.
//! * `RangerDefaultPolicyEvaluator.updateAccessResult(result, matchType,
//!   isAllowed, reason)` writes the policy's id, priority and reason
//!   **unconditionally** on the deny branch (offsets 83-132, guarded only by
//!   `matchType != DESCENDANT`) and **only `if (!result.getIsAllowed())`** on the
//!   allow branch (offsets 135-184). So a resource ALLOW landing on a result a
//!   tag policy already allowed writes nothing at all — the tag policy stays the
//!   reported one — while a resource DENY always overwrites.
//!
//! ## `hasResourcePolicies` is deliberately NOT modelled, and that is measured
//!
//! `PolicyEngine.hasResourcePolicies` is `repository != null &&
//! isNotEmpty(repository.getPolicies())` — the RAW envelope list, before any
//! `policyType` or `isEnabled` filtering. Its `false` arm skips the clear, the
//! loop and the restore in one jump (offset 350 → 715). That is
//! indistinguishable from its `true` arm with an empty evaluator list: the clear
//! happens, no evaluator runs, and the restore puts the tag verdict back
//! unchanged. Both arms therefore produce the same `isAllowed` / `policyId` /
//! `policyPriority`, so the crate reproduces only one of them.
//! `a_download_with_no_resource_policies_at_all_keeps_the_tag_verdict` pins that
//! equivalence rather than leaving it as prose.

use std::collections::BTreeSet;

use egret_ranger_eval::eval::{
    evaluate, evaluate_data_mask, evaluate_row_filter, AccessRequest, Decision, DenyReason,
    EngineConfig, MaskDecision, ResolvedTag, RowFilterDecision, UndecidableCause,
};
use egret_ranger_eval::model::ServicePolicySet;
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

/// The `tag` service-def the live capture carries, verbatim — a single `tag`
/// level with `wildCard: true`, `ignoreCase: false`, and the Trino access-type
/// vocabulary.
fn tag_def_json() -> Value {
    live_download_json()["tagPolicies"]["serviceDef"].clone()
}

fn tag_def() -> ServiceDef {
    ServiceDef::parse(&tag_def_json())
}

fn trino_def_json() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../egret-policy/src/service-defs/trino.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("read trino def"))
        .expect("parse trino def")
}

fn trino_def() -> ServiceDef {
    ServiceDef::parse(&trino_def_json())
}

/// An envelope carrying both phases: `policies` for the resource phase and a
/// `tagPolicies` block for the tag phase, with the live `tag` def.
fn envelope(resource_policies: Value, tag_policies: Value) -> ServicePolicySet {
    serde_json::from_value(json!({
        "serviceName": "trino",
        "policies": resource_policies,
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

/// `hive.sales.orders.id` carrying the tag `pii` at `SELF` — one admitted tag,
/// so the tag phase runs and neither narrowing on
/// `AccessRequest::resolved_tags` applies.
fn request() -> AccessRequest {
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
        resolved_tags: Some(vec![ResolvedTag {
            name: "pii".to_string(),
            match_type: MatchType::Self_,
        }]),
    }
}

fn no_superusers() -> EngineConfig {
    EngineConfig::without_super_users()
}

fn select_for_alice() -> Value {
    json!([{ "users": ["alice"], "accesses": [{"type": "select", "isAllowed": true}] }])
}

fn select_for_bob() -> Value {
    json!([{ "users": ["bob"], "accesses": [{"type": "select", "isAllowed": true}] }])
}

/// A resource policy on `hive.sales.orders.*`. `bucket` is the item bucket the
/// items go into, so one helper builds an allow, a deny, or a mask.
fn resource_policy(id: i64, name: &str, priority: i32, bucket: &str, items: Value) -> Value {
    let mut p = json!({
        "id": id, "name": name, "service": "trino", "policyType": 0,
        "isEnabled": true, "policyPriority": priority,
        "resources": {
            "catalog": {"values": ["hive"]},
            "schema":  {"values": ["sales"]},
            "table":   {"values": ["orders"]},
            "column":  {"values": ["*"]},
        },
    });
    p[bucket] = items;
    p
}

/// A tag policy on the tag `pii` — a literal value, which
/// `resource::trie_reproduces_matcher` classifies as `Equals` and therefore
/// answers rather than refusing.
fn tag_policy(
    id: i64,
    name: &str,
    priority: i32,
    policy_type: i32,
    bucket: &str,
    items: Value,
) -> Value {
    let mut p = json!({
        "id": id, "name": name, "service": "trino_tag", "policyType": policy_type,
        "isEnabled": true, "policyPriority": priority,
        "resources": { "tag": {"values": ["pii"]} },
    });
    p[bucket] = items;
    p
}

fn mask_items(mask_type: &str) -> Value {
    json!([{
        "users": ["alice"],
        "accesses": [{"type": "select", "isAllowed": true}],
        "dataMaskInfo": {"dataMaskType": mask_type},
    }])
}

fn filter_items(expr: &str) -> Value {
    json!([{
        "users": ["alice"],
        "accesses": [{"type": "select", "isAllowed": true}],
        "rowFilterInfo": {"filterExpr": expr},
    }])
}

fn named(d: &Decision) -> String {
    match d {
        Decision::Allowed { policy_name, .. } => format!("allowed by {policy_name}"),
        Decision::Denied {
            policy_name,
            reason,
            ..
        } => format!("denied by {policy_name} ({reason:?})"),
        Decision::Undecidable { why } => format!("undecidable {why:?}"),
    }
}

// ── arm 1: a tag DENY, against `>=` ──────────────────────────────────────

/// **The `>=` at offset 469.** A resource policy whose priority is not
/// STRICTLY higher than the tag deny's never runs at all: the arbitration sets
/// `isAccessDetermined` before `evaluate`, and
/// `RangerDefaultPolicyEvaluator.evaluate` then skips `evaluatePolicyItems`
/// (offsets 380-384). So the same-priority resource ALLOW below cannot be
/// reached, and the tag deny is the answer.
///
/// Mutating `>=` to `>` here lets the resource policy run and allow — the
/// fail-open direction, which is why this arm is the one with the tighter
/// comparison.
#[test]
fn a_tag_deny_stands_against_a_same_priority_resource_allow() {
    let set = envelope(
        json!([resource_policy(
            1,
            "res-allow",
            0,
            "policyItems",
            select_for_alice()
        )]),
        json!([tag_policy(
            10,
            "tag-deny",
            0,
            0,
            "denyPolicyItems",
            select_for_alice()
        )]),
    );

    let d = evaluate(&set, &request(), &trino_def(), &no_superusers());
    assert!(
        matches!(
            &d,
            Decision::Denied { policy_name, reason: DenyReason::DenyPolicyItem, .. }
                if policy_name == "tag-deny"
        ),
        "a same-priority resource allow must not reach a tag deny: {}",
        named(&d)
    );
}

/// The other side of the same comparison, and the case the engine logs in
/// prose at offset 224: *"access denied by a tag policy. Higher priority
/// resource policies will be evaluated to check for allow."*
#[test]
fn a_tag_deny_is_reopened_by_a_strictly_higher_priority_resource_allow() {
    let set = envelope(
        json!([resource_policy(
            1,
            "res-allow",
            1,
            "policyItems",
            select_for_alice()
        )]),
        json!([tag_policy(
            10,
            "tag-deny",
            0,
            0,
            "denyPolicyItems",
            select_for_alice()
        )]),
    );

    let d = evaluate(&set, &request(), &trino_def(), &no_superusers());
    assert!(
        matches!(&d, Decision::Allowed { policy_name, .. } if policy_name == "res-allow"),
        "a strictly higher-priority resource allow must reopen a tag deny: {}",
        named(&d)
    );
}

// ── arm 2: a tag ALLOW versus a resource ACCESS policy, against `>` ───────

/// **The `>` at offset 505.** Ranger's own log line at offset 264: *"access
/// allowed by a tag policy. Same or higher priority resource policies will be
/// evaluated to check for deny."* A same-priority resource deny therefore RUNS,
/// and `updateAccessResult`'s deny branch overwrites unconditionally.
///
/// Mutating `>` to `>=` closes the question at equal priority and turns this
/// deny into an allow.
#[test]
fn a_tag_allow_is_closed_by_a_same_priority_resource_deny() {
    let set = envelope(
        json!([resource_policy(
            1,
            "res-deny",
            0,
            "denyPolicyItems",
            select_for_alice()
        )]),
        json!([tag_policy(
            10,
            "tag-allow",
            0,
            0,
            "policyItems",
            select_for_alice()
        )]),
    );

    let d = evaluate(&set, &request(), &trino_def(), &no_superusers());
    assert!(
        matches!(
            &d,
            Decision::Denied { policy_name, reason: DenyReason::DenyPolicyItem, .. }
                if policy_name == "res-deny"
        ),
        "a same-priority resource deny must still close a tag allow: {}",
        named(&d)
    );
}

/// The other side: a LOWER-priority resource deny is shut out, because `>`
/// holds. Without this the `>`/`>=` pair is only half pinned — a mutation to
/// `<` would keep the test above green.
#[test]
fn a_tag_allow_shuts_out_a_lower_priority_resource_deny() {
    let set = envelope(
        json!([resource_policy(
            1,
            "res-deny",
            0,
            "denyPolicyItems",
            select_for_alice()
        )]),
        json!([tag_policy(
            10,
            "tag-allow",
            5,
            0,
            "policyItems",
            select_for_alice()
        )]),
    );

    let d = evaluate(&set, &request(), &trino_def(), &no_superusers());
    assert!(
        matches!(&d, Decision::Allowed { policy_name, .. } if policy_name == "tag-allow"),
        "a lower-priority resource deny must not reach a tag allow: {}",
        named(&d)
    );
}

// ── the restore at offsets 599-632 ───────────────────────────────────────

/// **The restore, both arms.** A strictly higher-priority resource policy runs
/// — the arbitration does not stop it — but DECIDES nothing, so the loop falls
/// off its end with `isAccessDetermined` still false and the tag verdict is put
/// back.
///
/// The two fixtures are shaped so the restore is the only thing that can
/// produce the answer:
///
/// * deny arm: the higher-priority resource policy's only item names `bob`, so
///   nothing matches, `isAllowed` stays false and no evaluator determines.
/// * allow arm: the higher-priority resource policy carries `denyPolicyItems`
///   for `bob` and no allow items. It is `hasDeny()`, so offset 553's
///   "an allow with no deny items of its own determines" cannot fire either,
///   and the loop again ends undetermined.
#[test]
fn nothing_else_determines_so_the_tag_verdict_is_restored() {
    let deny_side = envelope(
        json!([resource_policy(
            1,
            "res-irrelevant",
            9,
            "policyItems",
            select_for_bob()
        )]),
        json!([tag_policy(
            10,
            "tag-deny",
            0,
            0,
            "denyPolicyItems",
            select_for_alice()
        )]),
    );
    let d = evaluate(&deny_side, &request(), &trino_def(), &no_superusers());
    assert!(
        matches!(
            &d,
            Decision::Denied { policy_name, reason: DenyReason::DenyPolicyItem, .. }
                if policy_name == "tag-deny"
        ),
        "an undetermined resource phase must restore the tag deny: {}",
        named(&d)
    );

    let allow_side = envelope(
        json!([resource_policy(
            1,
            "res-irrelevant",
            9,
            "denyPolicyItems",
            select_for_bob()
        )]),
        json!([tag_policy(
            10,
            "tag-allow",
            0,
            0,
            "policyItems",
            select_for_alice()
        )]),
    );
    let d = evaluate(&allow_side, &request(), &trino_def(), &no_superusers());
    assert!(
        matches!(&d, Decision::Allowed { policy_name, .. } if policy_name == "tag-allow"),
        "an undetermined resource phase must restore the tag allow: {}",
        named(&d)
    );
}

/// `hasResourcePolicies` false. Measured to be indistinguishable from an empty
/// evaluator list (module doc), which is why the crate models one arm and this
/// test holds it to that.
#[test]
fn a_download_with_no_resource_policies_at_all_keeps_the_tag_verdict() {
    let set = envelope(
        json!([]),
        json!([tag_policy(
            10,
            "tag-deny",
            0,
            0,
            "denyPolicyItems",
            select_for_alice()
        )]),
    );

    let d = evaluate(&set, &request(), &trino_def(), &no_superusers());
    assert!(
        matches!(&d, Decision::Denied { policy_name, .. } if policy_name == "tag-deny"),
        "with no resource policies the tag verdict is the answer: {}",
        named(&d)
    );
}

/// The negative control for the whole composition: the tag phase determines
/// nothing (its policy covers a tag the request does not carry), so the
/// resource phase decides alone and every existing resource-path rule still
/// applies.
#[test]
fn a_tag_phase_that_determines_nothing_leaves_the_resource_phase_alone() {
    let mut tag = tag_policy(10, "tag-deny", 0, 0, "denyPolicyItems", select_for_alice());
    tag["resources"]["tag"]["values"] = json!(["other"]);
    let set = envelope(
        json!([resource_policy(
            1,
            "res-allow",
            0,
            "policyItems",
            select_for_alice()
        )]),
        json!([tag]),
    );

    let d = evaluate(&set, &request(), &trino_def(), &no_superusers());
    assert!(
        matches!(&d, Decision::Allowed { policy_name, .. } if policy_name == "res-allow"),
        "a tag policy covering another tag must not change the resource answer: {}",
        named(&d)
    );
}

// ── arm 3: a tag ALLOW versus a resource MASK / ROW-FILTER, against `>=` ──

/// **The `>=` at offset 529.** A mask tag policy leaves the shared result
/// `isAllowed` (its item evaluator reports allow unconditionally), so a
/// same-priority resource mask policy is shut out and the TAG's mask is the
/// answer.
///
/// This is the arm one character away from arm 2, and the mutation that swaps
/// them is the one the brief calls the easiest thing here to get backwards.
#[test]
fn a_tag_mask_stands_against_a_same_priority_resource_mask() {
    let mut res = resource_policy(1, "res-mask", 0, "dataMaskPolicyItems", mask_items("HASH"));
    res["policyType"] = json!(1);
    let set = envelope(
        json!([res]),
        json!([tag_policy(
            10,
            "tag-mask",
            0,
            1,
            "dataMaskPolicyItems",
            mask_items("MASK")
        )]),
    );

    let d = evaluate_data_mask(&set, &request(), &trino_def(), &no_superusers());
    match &d {
        MaskDecision::Masked(m) => {
            assert_eq!(m.policy_name, "tag-mask", "{d:?}");
            assert_eq!(m.mask_type.as_deref(), Some("MASK"), "{d:?}");
        }
        other => panic!("a same-priority resource mask must not displace the tag mask: {other:?}"),
    }
}

/// The row-filter half of the same arm — a different comparator orders the
/// candidates (`NAME_COMPARATOR` rather than `MATCH_TYPE_COMPARATOR`), and the
/// arbitration is the same opcode.
#[test]
fn a_tag_row_filter_stands_against_a_same_priority_resource_filter() {
    let mut res = resource_policy(
        1,
        "res-filter",
        0,
        "rowFilterPolicyItems",
        filter_items("id > 9"),
    );
    res["policyType"] = json!(2);
    let set = envelope(
        json!([res]),
        json!([tag_policy(
            10,
            "tag-filter",
            0,
            2,
            "rowFilterPolicyItems",
            filter_items("id > 2")
        )]),
    );

    let d = evaluate_row_filter(&set, &request(), &trino_def(), &no_superusers());
    match &d {
        RowFilterDecision::Filtered(f) => {
            assert_eq!(f.policy_name, "tag-filter", "{d:?}");
            assert_eq!(f.filter_expr, "id > 2", "{d:?}");
        }
        other => panic!("a same-priority resource filter must not displace the tag one: {other:?}"),
    }
}

/// The other side of arm 3, and the reason it is a REFUSAL rather than an
/// answer.
///
/// A strictly higher-priority resource mask policy does run. Its item evaluator
/// writes `maskType`/`maskedValue`/`maskCondition` **unconditionally**
/// (`RangerDefaultDataMaskPolicyItemEvaluator.updateAccessResult` offsets 11-88)
/// and then calls `RangerPolicyEvaluator.updateAccessResult(..., true, ...)`,
/// whose allow branch is guarded by `if (!result.getIsAllowed())` — and the tag
/// phase already set `isAllowed`. So Ranger returns the TAG policy's id with the
/// RESOURCE policy's mask, and [`egret_ranger_eval::eval::MaskResult`] bundles
/// the two and cannot say that.
#[test]
fn a_strictly_higher_priority_resource_mask_splits_the_id_from_the_mask() {
    let mut res = resource_policy(1, "res-mask", 1, "dataMaskPolicyItems", mask_items("HASH"));
    res["policyType"] = json!(1);
    let set = envelope(
        json!([res]),
        json!([tag_policy(
            10,
            "tag-mask",
            0,
            1,
            "dataMaskPolicyItems",
            mask_items("MASK")
        )]),
    );

    let d = evaluate_data_mask(&set, &request(), &trino_def(), &no_superusers());
    match &d {
        MaskDecision::Undecidable { why } => assert!(
            why.iter().any(|c| matches!(
                c,
                UndecidableCause::TagSubResultSplitByResourcePolicy {
                    resource_policy_id: Some(1),
                    ..
                }
            )),
            "{why:?}"
        ),
        other => panic!("the id/data split must refuse, not be guessed at: {other:?}"),
    }
}

/// The refusal above is on the resource policy YIELDING, not on its existing.
/// A strictly higher-priority resource mask policy whose items do not match
/// writes nothing, so the tag mask still stands and the answer is not lost.
#[test]
fn a_higher_priority_resource_mask_that_yields_nothing_leaves_the_tag_mask() {
    let mut res = resource_policy(
        1,
        "res-mask",
        1,
        "dataMaskPolicyItems",
        json!([{
            "users": ["bob"],
            "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": "HASH"},
        }]),
    );
    res["policyType"] = json!(1);
    let set = envelope(
        json!([res]),
        json!([tag_policy(
            10,
            "tag-mask",
            0,
            1,
            "dataMaskPolicyItems",
            mask_items("MASK")
        )]),
    );

    let d = evaluate_data_mask(&set, &request(), &trino_def(), &no_superusers());
    match &d {
        MaskDecision::Masked(m) => assert_eq!(m.policy_name, "tag-mask", "{d:?}"),
        other => panic!("a non-yielding resource mask must leave the tag mask: {other:?}"),
    }
}

/// The negative control for the mask composition: with no tag mask to stand on,
/// the resource mask decides exactly as it did before this task.
#[test]
fn a_resource_mask_decides_alone_when_the_tag_phase_yields_nothing() {
    let mut res = resource_policy(1, "res-mask", 0, "dataMaskPolicyItems", mask_items("HASH"));
    res["policyType"] = json!(1);
    let mut tag = tag_policy(
        10,
        "tag-mask",
        0,
        1,
        "dataMaskPolicyItems",
        mask_items("MASK"),
    );
    tag["resources"]["tag"]["values"] = json!(["other"]);
    let set = envelope(json!([res]), json!([tag]));

    let d = evaluate_data_mask(&set, &request(), &trino_def(), &no_superusers());
    match &d {
        MaskDecision::Masked(m) => {
            assert_eq!(m.policy_name, "res-mask", "{d:?}");
            assert_eq!(m.mask_type.as_deref(), Some("HASH"), "{d:?}");
        }
        other => panic!("the resource mask path must be unchanged: {other:?}"),
    }
}

// ── the preamble runs first, and exactly once ────────────────────────────

/// **Step 7's test.** The super-user short-circuit is at caller offsets 114-165
/// and `evaluateTagPolicies` is at 176, so a super-user is allowed before any
/// tag policy is consulted. The tag policy here would deny `alice` outright, so
/// if the tag phase ran first — or if its verdict were consulted at all — this
/// test reports a deny.
#[test]
fn a_super_user_short_circuits_before_the_tag_phase_can_deny() {
    let set = envelope(
        json!([]),
        json!([tag_policy(
            10,
            "tag-deny",
            0,
            0,
            "denyPolicyItems",
            select_for_alice()
        )]),
    );
    // The control: without the super-user list, this same request is denied.
    let control = evaluate(&set, &request(), &trino_def(), &no_superusers());
    assert!(
        matches!(&control, Decision::Denied { policy_name, .. } if policy_name == "tag-deny"),
        "the fixture must deny for a non-super-user, or this test proves nothing: {}",
        named(&control)
    );

    let engine = EngineConfig {
        super_users: Some(["alice".to_string()].into_iter().collect()),
        super_groups: Some(BTreeSet::new()),
        ..EngineConfig::default()
    };
    let d = evaluate(&set, &request(), &trino_def(), &engine);
    assert!(
        matches!(&d, Decision::Allowed { policy_id: Some(-1), policy_name, .. } if policy_name == "superuser"),
        "a super-user must be allowed before the tag phase runs: {}",
        named(&d)
    );
}

/// The same ordering for a super-GROUP, and for the mask and row-filter
/// requests — where a super-user's columns come back UNMASKED, so a tag mask
/// policy must not be consulted either.
#[test]
fn a_super_group_short_circuits_the_tag_mask_and_row_filter_too() {
    let mut req = request();
    req.groups = ["admins".to_string()].into_iter().collect();
    let engine = EngineConfig {
        super_users: Some(BTreeSet::new()),
        super_groups: Some(["admins".to_string()].into_iter().collect()),
        ..EngineConfig::default()
    };

    let masked = envelope(
        json!([]),
        json!([tag_policy(
            10,
            "tag-mask",
            0,
            1,
            "dataMaskPolicyItems",
            json!([{
                "groups": ["admins"],
                "accesses": [{"type": "select", "isAllowed": true}],
                "dataMaskInfo": {"dataMaskType": "MASK"},
            }])
        )]),
    );
    assert!(
        matches!(
            evaluate_data_mask(&masked, &req, &trino_def(), &no_superusers()),
            MaskDecision::Masked(_)
        ),
        "the fixture must mask for a non-super-user, or this test proves nothing"
    );
    assert_eq!(
        evaluate_data_mask(&masked, &req, &trino_def(), &engine),
        MaskDecision::NoMask,
        "a super-user's columns come back unmasked"
    );

    let filtered = envelope(
        json!([]),
        json!([tag_policy(
            11,
            "tag-filter",
            0,
            2,
            "rowFilterPolicyItems",
            json!([{
                "groups": ["admins"],
                "accesses": [{"type": "select", "isAllowed": true}],
                "rowFilterInfo": {"filterExpr": "id > 2"},
            }])
        )]),
    );
    assert!(
        matches!(
            evaluate_row_filter(&filtered, &req, &trino_def(), &no_superusers()),
            RowFilterDecision::Filtered(_)
        ),
        "the fixture must filter for a non-super-user, or this test proves nothing"
    );
    assert_eq!(
        evaluate_row_filter(&filtered, &req, &trino_def(), &engine),
        RowFilterDecision::NoFilter,
        "a super-user's rows come back unfiltered"
    );
}

/// The preamble's OTHER refusals also still precede the tag phase, and are
/// reported ONCE even though the tag phase sweeps the same envelope. A
/// duplicated cause would be the first sign the two sweeps had drifted into
/// two different rules.
#[test]
fn an_envelope_construct_is_reported_once_by_the_two_phases_together() {
    let mut body = json!({
        "serviceName": "trino",
        "policies": [resource_policy(1, "res-allow", 0, "policyItems", select_for_alice())],
        "tagPolicies": {
            "serviceName": "trino_tag",
            "serviceId": 99,
            "policyVersion": 1,
            "policies": [tag_policy(10, "tag-deny", 0, 0, "denyPolicyItems", select_for_alice())],
            "serviceDef": tag_def_json(),
        },
    });
    body["policyDeltas"] = json!([{"id": 7}]);
    let set: ServicePolicySet = serde_json::from_value(body).expect("parse envelope");

    let d = evaluate(&set, &request(), &trino_def(), &no_superusers());
    let why = match &d {
        Decision::Undecidable { why } => why.clone(),
        other => panic!("policyDeltas must refuse: {other:?}"),
    };
    let deltas = why
        .iter()
        .filter(|c| {
            matches!(
                c,
                UndecidableCause::EnvelopeConstruct(
                    egret_ranger_eval::model::Unmodelled::PolicyDeltas
                )
            )
        })
        .count();
    assert_eq!(deltas, 1, "reported once, not once per phase: {why:?}");
}

/// The tag phase's own refusals reach the composed answer. `resolved_tags:
/// None` is the sharpest of them — "the caller did not resolve", which must
/// never be read as the empty set — and before this task the composed evaluator
/// refused one layer earlier, on the envelope construct alone.
#[test]
fn a_tag_phase_refusal_becomes_the_composed_refusal() {
    let set = envelope(
        json!([resource_policy(
            1,
            "res-allow",
            0,
            "policyItems",
            select_for_alice()
        )]),
        json!([tag_policy(
            10,
            "tag-deny",
            0,
            0,
            "denyPolicyItems",
            select_for_alice()
        )]),
    );
    let req = AccessRequest {
        resolved_tags: None,
        ..request()
    };

    let d = evaluate(&set, &req, &trino_def(), &no_superusers());
    assert!(
        matches!(&d, Decision::Undecidable { why } if why.contains(&UndecidableCause::TagsUnresolved)),
        "an unresolved tag set must refuse even though a resource policy allows: {}",
        named(&d)
    );
}

/// The gate is the tag service-def as well as the request: with no enricher on
/// either def nothing ever writes a tag into the request context, so the tag
/// phase does not run and the resource policy answers. This is the one path on
/// which `evaluate` does NOT call the tag phase, and it must not become a
/// refusal.
#[test]
fn neither_def_declaring_an_enricher_keeps_the_tag_phase_out_of_it() {
    let mut tag_block_def = tag_def_json();
    tag_block_def["contextEnrichers"] = json!([]);
    let mut resource_def_json = trino_def_json();
    resource_def_json["contextEnrichers"] = json!([]);

    let set: ServicePolicySet = serde_json::from_value(json!({
        "serviceName": "trino",
        "policies": [resource_policy(1, "res-allow", 0, "policyItems", select_for_alice())],
        "tagPolicies": {
            "serviceName": "trino_tag",
            "serviceId": 99,
            "policyVersion": 1,
            // A tag policy that WOULD deny, so the gate is the only thing
            // standing between this request and a deny.
            "policies": [tag_policy(10, "tag-deny", 0, 0, "denyPolicyItems", select_for_alice())],
            "serviceDef": tag_block_def,
        },
    }))
    .expect("parse envelope");
    let def = ServiceDef::parse(&resource_def_json);
    assert_eq!(def.has_tag_enricher(), Some(false));
    assert_eq!(tag_def().has_tag_enricher(), None, "the live def is silent");

    let d = evaluate(&set, &request(), &def, &no_superusers());
    assert!(
        matches!(&d, Decision::Allowed { policy_name, .. } if policy_name == "res-allow"),
        "with no enricher on either def the tag phase cannot run: {}",
        named(&d)
    );
}

// ── what a tag DENY does to the resource phase's own eval order ──────────

/// **A defect the composition introduces, and the refusal that closes it.**
///
/// Under a tag DENY, offset 469's `>=` reads the LIVE
/// `result.getPolicyPriority()`, not the tag's — and a resource policy that
/// ALLOWS moves it (`updateAccessResult`'s allow branch fires because the tag
/// deny left `isAllowed` false). So the allow shuts out the rest of its own
/// `(priority, hasDeny)` band, and whichever of an allowing and a denying
/// member of that band runs FIRST decides the request:
///
/// * allow first → `priority` becomes 5, the deny hits `5 >= 5`, its items are
///   skipped, the answer is ALLOWED;
/// * deny first → determined immediately, the answer is DENIED.
///
/// The order inside a band is `getEvalOrder()`, which this crate does not
/// compute, so neither answer is defensible and the case refuses.
///
/// Neither of the other two arms can reach this: with a tag ALLOW the allow
/// branch does not fire at all, and with no tag verdict offset 505's `>` lets
/// the same-priority rival run anyway — which the sibling test below measures
/// on the SAME two policies.
#[test]
fn a_tag_deny_makes_a_resource_band_s_eval_order_decide_and_that_refuses() {
    let contested = json!([
        {
            "id": 1, "name": "a-allow", "service": "trino", "policyType": 0,
            "isEnabled": true, "policyPriority": 5,
            "resources": {
                "catalog": {"values": ["hive"]}, "schema": {"values": ["sales"]},
                "table": {"values": ["orders"]}, "column": {"values": ["*"]},
            },
            // `hasDeny()` — the bucket is non-empty — but it denies BOB, so the
            // policy allows alice without determining.
            "denyPolicyItems": select_for_bob(),
            "policyItems": select_for_alice(),
        },
        {
            "id": 2, "name": "b-deny", "service": "trino", "policyType": 0,
            "isEnabled": true, "policyPriority": 5,
            "resources": {
                "catalog": {"values": ["hive"]}, "schema": {"values": ["sales"]},
                "table": {"values": ["orders"]}, "column": {"values": ["*"]},
            },
            "denyPolicyItems": select_for_alice(),
        },
    ]);

    let set = envelope(
        contested.clone(),
        json!([tag_policy(
            10,
            "tag-deny",
            0,
            0,
            "denyPolicyItems",
            select_for_alice()
        )]),
    );
    let d = evaluate(&set, &request(), &trino_def(), &no_superusers());
    match &d {
        Decision::Undecidable { why } => assert!(
            why.iter().any(|c| matches!(
                c,
                UndecidableCause::EvalOrderAmbiguous { policy_ids, .. }
                    if policy_ids == &vec![Some(1), Some(2)]
            )),
            "{why:?}"
        ),
        other => panic!("the band's order decides the answer, so it must refuse: {other:?}"),
    }

    // The control, on the SAME two resource policies: with no tag verdict the
    // `>` arm lets `b-deny` run after `a-allow` and the answer is stable.
    let untagged = envelope(contested, json!([]));
    let d = evaluate(&untagged, &request(), &trino_def(), &no_superusers());
    assert!(
        matches!(&d, Decision::Denied { policy_name, .. } if policy_name == "b-deny"),
        "without a tag deny the same band is order-independent: {}",
        named(&d)
    );
}

/// The refusal is on a DENYING rival, not on the band having more than one
/// member: a band whose other members allow (or decide nothing) gives the same
/// answer in every order, so it must still answer.
#[test]
fn a_tag_deny_over_a_band_that_cannot_disagree_still_answers() {
    let set = envelope(
        json!([
            {
                "id": 1, "name": "a-allow", "service": "trino", "policyType": 0,
                "isEnabled": true, "policyPriority": 5,
                "resources": {
                    "catalog": {"values": ["hive"]}, "schema": {"values": ["sales"]},
                    "table": {"values": ["orders"]}, "column": {"values": ["*"]},
                },
                "denyPolicyItems": select_for_bob(),
                "policyItems": select_for_alice(),
            },
            {
                "id": 2, "name": "b-also-allow", "service": "trino", "policyType": 0,
                "isEnabled": true, "policyPriority": 5,
                "resources": {
                    "catalog": {"values": ["hive"]}, "schema": {"values": ["sales"]},
                    "table": {"values": ["orders"]}, "column": {"values": ["*"]},
                },
                "denyPolicyItems": select_for_bob(),
                "policyItems": select_for_alice(),
            },
        ]),
        json!([tag_policy(
            10,
            "tag-deny",
            0,
            0,
            "denyPolicyItems",
            select_for_alice()
        )]),
    );

    let d = evaluate(&set, &request(), &trino_def(), &no_superusers());
    assert!(
        matches!(&d, Decision::Allowed { .. }),
        "two allowers in one band agree in every order, so this must answer: {}",
        named(&d)
    );
}
