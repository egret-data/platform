//! SP4d-2b Task 3: the two phases composed, and Ranger's three-way priority
//! arbitration between them.
//!
//! # What was measured in the jars for this task, and where
//!
//! `RangerPolicyEngineImpl.evaluatePoliciesForOneAccessTypeNoAudit`, disassembled
//! with `javap -c -p` on `ranger-plugins-common` **2.5.0** and **2.8.0**. Across
//! the whole arbitration the two versions are identical at every offset, branch
//! target and comparison opcode, so one set of offsets serves both.
//!
//! FIX ROUND 1: not "byte-identical", which an earlier version of this doc
//! claimed and which is a measurement claim the measurement does not support —
//! at offset 127, OUTSIDE the arbitration, the `_super_user` compare is
//! `commons-lang`'s `StringUtils.equals(String, String)` in 2.5.0 and
//! `commons-lang3`'s `equals(CharSequence, CharSequence)` in 2.8.0. Same
//! semantics, no bearing on anything below, but the claim is scoped to what was
//! measured.
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
//!   allow branch (offsets 135-184). A resource DENY therefore always overwrites.
//!
//!   ⚠ **WHOLE-BRANCH REVIEW: this bullet used to continue "So a resource ALLOW
//!   landing on a result a tag policy already allowed writes nothing at all — the
//!   tag policy stays the reported one". That is FALSE, and it was a confident
//!   over-DENY.** The `if (!result.getIsAllowed())` guard is read against a FRESH
//!   per-access-type result out of `accessTypeResults`, not against the shared
//!   one, so it always passes; `getCompositeAccessResult` → `setAccessResultFrom`
//!   then copies `policyPriority` unconditionally onto the shared result, and the
//!   next lower-priority evaluator is shut out at offset 553's sibling, 505. That
//!   shape now REFUSES
//!   ([`UndecidableCause::AccessPriorityCompositeNotModelled`]) and is pinned by
//!   `a_tag_allow_under_a_higher_priority_resource_allow_refuses` below.
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

use std::collections::{BTreeMap, BTreeSet};

use egret_ranger_eval::eval::{
    evaluate, evaluate_data_mask, evaluate_row_filter, AccessRequest, Decision, DenyReason,
    EngineConfig, MaskDecision, PluginVersion, ResolvedTag, RowFilterDecision, TagServiceResource,
    UndecidableCause, UnenrichedTags,
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
        resolved_tags: Some(UnenrichedTags::new(vec![column_tag(
            "pii",
            MatchType::Self_,
        )])),
    }
}

/// One attachment shaped the way `ServiceTags::from_assignments` writes a
/// COLUMN-level one: four `resourceElements` levels, an enabled service
/// resource, and none of `attributes` / `options` / `validityPeriods` — the
/// payload egret itself emits, so every attachment refusal SP4d-2b Task 4 added
/// is inert for it.
fn column_tag(name: &str, match_type: MatchType) -> ResolvedTag {
    ResolvedTag {
        name: name.to_string(),
        match_type,
        attributes: BTreeMap::new(),
        options: BTreeSet::new(),
        validity_periods: 0,
        attached_through: TagServiceResource {
            levels: ["catalog", "schema", "table", "column"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            is_enabled: true,
        },
    }
}

fn no_superusers() -> EngineConfig {
    EngineConfig::without_super_users()
}

/// The same config with the plugin version stated as **2.8.0**.
///
/// SP4d-2b Task 5. One composed shape — a tag DENY reopened by a STRICTLY
/// higher-priority resource ALLOW — is answered DIFFERENTLY by the two plugin
/// versions (measured on both real engines; see
/// `UndecidableCause::TagDenyReopenedDiffersByPluginVersion`). This crate
/// reproduces 2.8.0's answer and refuses on 2.5.0 and on an unstated version, so
/// every test of that shape has to say which engine it is about — and
/// `plugin_version: None` refusing IS the contract, not an inconvenience.
///
/// `engine` stays `None` on purpose: naming StarRocks (the engine that runs
/// 2.8.0) would make the trino service-def these tests use an
/// `EngineMismatch`, and the shape under test has nothing to do with the
/// resource vocabulary.
fn plugin_2_8_0() -> EngineConfig {
    EngineConfig {
        plugin_version: Some(PluginVersion::Ranger2_8_0),
        ..EngineConfig::without_super_users()
    }
}

fn select_for_alice() -> Value {
    json!([{ "users": ["alice"], "accesses": [{"type": "select", "isAllowed": true}] }])
}

fn select_for_dave() -> Value {
    json!([{ "users": ["dave"], "accesses": [{"type": "select", "isAllowed": true}] }])
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

/// A row-filter resource policy, and a request to match it.
///
/// Both are TABLE-granular on purpose: Trino's `rowFilterDef.resources` names
/// `catalog`/`schema`/`table` and no `column`, so a policy naming a column
/// matches no row-filter hierarchy and can never yield. The control at the end
/// of `a_tag_row_filter_stands_against_a_same_priority_resource_filter` is what
/// caught that — the first version of that test shut out a resource policy that
/// could not have decided anything anyway.
fn row_filter_resource_policy(id: i64, name: &str, priority: i32, expr: &str) -> Value {
    json!({
        "id": id, "name": name, "service": "trino", "policyType": 2,
        "isEnabled": true, "policyPriority": priority,
        "resources": {
            "catalog": {"values": ["hive"]},
            "schema":  {"values": ["sales"]},
            "table":   {"values": ["orders"]},
        },
        "rowFilterPolicyItems": filter_items(expr),
    })
}

fn table_request() -> AccessRequest {
    AccessRequest {
        resource: RequestResource::new()
            .with("catalog", "hive")
            .with("schema", "sales")
            .with("table", "orders"),
        ..request()
    }
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
///
/// ⚠ **This is TRUE ON 2.8.0 ONLY, and the differential gate is what
/// established that.** The log line is 2.5.0's own, and 2.5.0 does NOT do what
/// it says: measured on both engines, the identical download answers `Allowed`
/// by the resource policy on 2.8.0 and `Denied` by the TAG policy on 2.5.0. So
/// the config here states the version, and the sibling below asserts the refusal
/// the other two answers force.
#[test]
fn a_tag_deny_is_reopened_by_a_strictly_higher_priority_resource_allow_on_2_8_0() {
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

    let d = evaluate(&set, &request(), &trino_def(), &plugin_2_8_0());
    assert!(
        matches!(&d, Decision::Allowed { policy_name, .. } if policy_name == "res-allow"),
        "on 2.8.0 a strictly higher-priority resource allow must reopen a tag deny: {}",
        named(&d)
    );

    // …and on 2.5.0, and on an unstated version, the two engines disagree so the
    // crate must REFUSE rather than pick one. Both halves here, because a refusal
    // that fired only for `None` would leave 2.5.0 answering 2.8.0's verdict.
    for engine in [
        EngineConfig {
            plugin_version: Some(PluginVersion::Ranger2_5_0),
            ..EngineConfig::without_super_users()
        },
        no_superusers(),
    ] {
        let d = evaluate(&set, &request(), &trino_def(), &engine);
        match &d {
            Decision::Undecidable { why } => assert!(
                why.iter().any(|c| matches!(
                    c,
                    UndecidableCause::TagDenyReopenedDiffersByPluginVersion {
                        resource_policy_name,
                        tag_policy_name,
                        ..
                    } if resource_policy_name == "res-allow" && tag_policy_name == "tag-deny"
                )),
                "the refusal must name both policies: {why:?}"
            ),
            other => panic!(
                "the two plugin versions answer this shape differently, so {:?} must refuse: \
                 {other:?}",
                engine.plugin_version
            ),
        }
    }
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

// ── the accessTypeResults composite, offsets 469/505 read against a priority
//    this crate does not raise (whole-branch review CRITICAL) ───────────────

/// **The CRITICAL.** A tag ALLOW at priority 0, a resource policy at priority 1
/// that also allows, and a resource policy at priority 0 that denies.
///
/// MEASURED on both real engines, which answer `Allowed, policyId 1` — and the
/// control below shows the tag ALLOW is the sole trigger. This crate answered
/// `Denied` by the priority-0 denier, because it modelled the allow branch's
/// `if (!result.getIsAllowed())` guard as "writes nothing" and so never raised
/// `policyPriority` to 1, leaving the denier inside offset 505's window.
///
/// It REFUSES rather than models the composite; the ruling and its three reasons
/// are on the variant. This test is the JVM-free half of the pin — the
/// differential's `check_measured_priority_composite` is the half that re-measures
/// the engines.
#[test]
fn a_tag_allow_under_a_higher_priority_resource_allow_refuses() {
    let mut allower = resource_policy(1, "p1", 1, "policyItems", select_for_alice());
    allower["isDenyAllElse"] = json!(true);
    let set = envelope(
        json!([
            allower,
            resource_policy(2, "p2", 0, "denyPolicyItems", select_for_alice()),
        ]),
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
    let Decision::Undecidable { why } = &d else {
        panic!(
            "a tag allow under a strictly higher-priority resource allow must REFUSE: {}. \
             `Denied` is the over-deny this refusal exists for; `Allowed` means the allow arm \
             now writes unconditionally, which is a MODEL of the composite and needs open \
             item 0's audit first",
            named(&d)
        );
    };
    assert!(
        matches!(
            why.as_slice(),
            [UndecidableCause::AccessPriorityCompositeNotModelled {
                tag_policy_id: Some(10),
                resource_policy_id: Some(1),
                tag_priority: 0,
                resource_priority: 1,
                ..
            }]
        ),
        "the refusal must name the tag policy whose allow was standing and the resource policy \
         whose priority Ranger composites, with both priorities: {why:?}"
    );
}

/// The CONTROL for the test above: the same two resource policies with NO tag
/// block. Both engines answer `Allowed, policyId 1`, and so must this crate —
/// through the ordinary resource loop, where the allow arm DOES write because
/// nothing had allowed yet.
///
/// Without this, "ranger allows" would be consistent with the resource policies
/// alone and the finding would not be located on the tag ALLOW.
#[test]
fn the_same_two_resource_policies_with_no_tag_block_answer_allowed() {
    let mut allower = resource_policy(1, "p1", 1, "policyItems", select_for_alice());
    allower["isDenyAllElse"] = json!(true);
    let set = envelope(
        json!([
            allower,
            resource_policy(2, "p2", 0, "denyPolicyItems", select_for_alice()),
        ]),
        json!([]),
    );

    let d = evaluate(&set, &request(), &trino_def(), &no_superusers());
    assert!(
        matches!(
            &d,
            Decision::Allowed {
                policy_id: Some(1),
                ..
            }
        ),
        "the control must answer Allowed by policy 1: {}",
        named(&d)
    );
}

/// The refusal is NARROW: at one priority the composite writes a priority equal to
/// the one already there, so nothing is shut out that this crate does not shut out,
/// and the crate must still ANSWER.
///
/// This is the boundary the hold-out comment in `differential.rs` used to get
/// wrong — it claimed an honest refusal here would also swallow T5's four rows.
/// It does not: those are a tag DENY, and this arm only fires on a tag ALLOW.
#[test]
fn a_tag_allow_with_everything_at_one_priority_still_answers() {
    let set = envelope(
        json!([
            resource_policy(1, "p1", 0, "policyItems", select_for_alice()),
            resource_policy(2, "p2", 0, "denyPolicyItems", select_for_alice()),
        ]),
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
            Decision::Denied {
                policy_id: Some(2),
                ..
            }
        ),
        "with no priority gap the composite is a no-op for the arbitration, so the same-priority \
         denier still decides and the crate must answer: {}",
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
/// * deny arm: the higher-priority resource policy ALLOWS alice and is
///   `hasDeny()` (its deny bucket names `bob`), so it writes an allow that
///   offset 553 cannot determine for, the loop runs out, and offset 612 puts
///   the tag deny back over it. Delete the restore and this answers `Allowed`.
/// * allow arm: the higher-priority resource policy carries `denyPolicyItems`
///   for `bob` and no allow items. It is `hasDeny()`, so offset 553 cannot
///   fire either, and the loop again ends undetermined.
#[test]
fn nothing_else_determines_so_the_tag_verdict_is_restored() {
    let mut res_allows_but_cannot_determine = resource_policy(
        1,
        "res-allow-deny-capable",
        9,
        "policyItems",
        select_for_alice(),
    );
    res_allows_but_cannot_determine["denyPolicyItems"] = select_for_bob();
    let deny_side = envelope(
        json!([res_allows_but_cannot_determine]),
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
    let tag = |values: Value| {
        let mut p = tag_policy(
            10,
            "tag-filter",
            0,
            2,
            "rowFilterPolicyItems",
            filter_items("id > 2"),
        );
        p["resources"]["tag"]["values"] = values;
        p
    };
    let set = envelope(
        json!([row_filter_resource_policy(1, "res-filter", 0, "id > 9")]),
        json!([tag(json!(["pii"]))]),
    );

    let d = evaluate_row_filter(&set, &table_request(), &trino_def(), &no_superusers());
    match &d {
        RowFilterDecision::Filtered(f) => {
            assert_eq!(f.policy_name, "tag-filter", "{d:?}");
            assert_eq!(f.filter_expr, "id > 2", "{d:?}");
        }
        other => panic!("a same-priority resource filter must not displace the tag one: {other:?}"),
    }

    // The control WITHOUT which the assertion above is unfalsifiable: shutting
    // out a resource policy that could never have yielded proves nothing. Same
    // envelope, tag policy pointed at another tag, so the tag phase yields
    // nothing and the resource filter must answer.
    let alone = envelope(
        json!([row_filter_resource_policy(1, "res-filter", 0, "id > 9")]),
        json!([tag(json!(["other"]))]),
    );
    let d = evaluate_row_filter(&alone, &table_request(), &trino_def(), &no_superusers());
    match &d {
        RowFilterDecision::Filtered(f) => assert_eq!(f.filter_expr, "id > 9", "{d:?}"),
        other => panic!("the resource filter must be able to yield at all: {other:?}"),
    }

    // And the other side of the `>=`: a STRICTLY higher-priority resource
    // filter does run, and splits the reported id from the expression.
    let split = envelope(
        json!([row_filter_resource_policy(1, "res-filter", 1, "id > 9")]),
        json!([tag(json!(["pii"]))]),
    );
    let d = evaluate_row_filter(&split, &table_request(), &trino_def(), &no_superusers());
    assert!(
        matches!(
            &d,
            RowFilterDecision::Undecidable { why }
                if why.iter().any(|c| matches!(
                    c,
                    UndecidableCause::TagSubResultSplitByResourcePolicy { .. }
                ))
        ),
        "{d:?}"
    );
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

/// **The SAME band with the two policies' NAMES SWAPPED, and the reason this
/// test exists at all.**
///
/// SP4d-2b Task 5. The sibling test above names the allower `a-allow` and the
/// denier `b-deny`, so this crate's name tie-break puts the ALLOWER first — and
/// the refusal used to live in the allow arm of the loop alone. With the names
/// the other way round the DENIER runs first, the deny arm set `determined` and
/// broke the loop, and the crate answered `Denied` over a band whose winner
/// Ranger picks with `getEvalOrder()`. Measured on the real 2.8.0 engine by the
/// differential gate the first time the corpus generated a composed duel: 6 of
/// 387 compared cases, every one `egret said Denied / ranger said Allowed`.
///
/// So the refusal was not order-independent; only its SCAN was. Reaching the
/// scan depended on a tie-break, which means exactly half the shape was refused
/// and which half was decided by the alphabet.
///
/// Nothing here is new except two names. That is the point: the test above and
/// this one differ by the one thing the defect turned on, and the test above
/// could not have failed. Both must pass, and a fix that only satisfies one of
/// them has reinstated the blind spot from the other side.
#[test]
fn the_same_band_refuses_when_this_crate_s_order_puts_the_denier_first() {
    let contested = json!([
        {
            // `a-deny` now sorts FIRST, where `b-deny` sorted last.
            "id": 1, "name": "a-deny", "service": "trino", "policyType": 0,
            "isEnabled": true, "policyPriority": 5,
            "resources": {
                "catalog": {"values": ["hive"]}, "schema": {"values": ["sales"]},
                "table": {"values": ["orders"]}, "column": {"values": ["*"]},
            },
            "denyPolicyItems": select_for_alice(),
        },
        {
            "id": 2, "name": "b-allow", "service": "trino", "policyType": 0,
            "isEnabled": true, "policyPriority": 5,
            "resources": {
                "catalog": {"values": ["hive"]}, "schema": {"values": ["sales"]},
                "table": {"values": ["orders"]}, "column": {"values": ["*"]},
            },
            "denyPolicyItems": select_for_bob(),
            "policyItems": select_for_alice(),
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
                    // The DENIER is the member reached first, so it is named
                    // first — the pair is the same pair, reported from the other
                    // end.
                    if policy_ids == &vec![Some(1), Some(2)]
            )),
            "{why:?}"
        ),
        other => panic!(
            "the band's order still decides the answer when the denier sorts first, so it \
             must still refuse — answering {other:?} is the fail this gate measured"
        ),
    }

    // The same control as the sibling test: with no tag verdict the `>` arm lets
    // the allower run after the deny and the answer is stable. Without this, a
    // "fix" that refused every same-priority band would pass the assertion above
    // and would refuse the ordinary case too.
    let untagged = envelope(contested, json!([]));
    let d = evaluate(&untagged, &request(), &trino_def(), &no_superusers());
    assert!(
        matches!(&d, Decision::Denied { policy_name, .. } if policy_name == "a-deny"),
        "without a tag deny the same band is order-independent: {}",
        named(&d)
    );
}

/// The mirror of the "cannot disagree" control, for the deny side: a band whose
/// members all DENY gives the same answer in every order, so it must answer.
///
/// Written with the fix, and required by it: the check now runs on a deny as
/// well as on an allow, so "refuses on a denying rival" is no longer the whole
/// rule and "refuses whenever the band has two deciding members" would pass
/// every other test in this file.
#[test]
fn a_tag_deny_over_a_band_of_two_deniers_still_answers() {
    let set = envelope(
        json!([
            {
                "id": 1, "name": "a-deny", "service": "trino", "policyType": 0,
                "isEnabled": true, "policyPriority": 5,
                "resources": {
                    "catalog": {"values": ["hive"]}, "schema": {"values": ["sales"]},
                    "table": {"values": ["orders"]}, "column": {"values": ["*"]},
                },
                "denyPolicyItems": select_for_alice(),
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
        matches!(&d, Decision::Denied { .. }),
        "two deniers in one band agree in every order, so this must answer: {}",
        named(&d)
    );
}

/// The refusal is on a DENYING rival, not on the band having more than one
/// member: a band whose other members allow (or decide nothing) gives the same
/// answer in every order, so it must still answer.
///
/// ⚠ **On 2.8.0**, and stated because fix round 1 measured the other version.
/// Two deny-capable allowers at one priority above a tag deny answer `Allowed`
/// on 2.8.0 — the second one trips offset 469, `determined` is set and the
/// restore never runs — and `Denied` on 2.5.0, where a tag deny is never
/// reopened at all. The sibling below asserts the refusal the 2.5.0 answer
/// forces.
#[test]
fn a_tag_deny_over_a_band_that_cannot_disagree_still_answers_on_2_8_0() {
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

    let d = evaluate(&set, &request(), &trino_def(), &plugin_2_8_0());
    assert!(
        matches!(&d, Decision::Allowed { .. }),
        "two allowers in one band agree in every order, so on 2.8.0 this must answer: {}",
        named(&d)
    );

    // …and on 2.5.0 and on an unstated version it must REFUSE, because 2.5.0
    // answers `Denied` by the tag policy for this exact download. MEASURED on
    // both engines: two deny-capable allowers at priority 5 over a tag deny at 0
    // give `Allowed, policyId 1` on 2.8.0 and `Denied, policyId 900` on 2.5.0.
    for engine in [
        EngineConfig {
            plugin_version: Some(PluginVersion::Ranger2_5_0),
            ..EngineConfig::without_super_users()
        },
        no_superusers(),
    ] {
        let d = evaluate(&set, &request(), &trino_def(), &engine);
        assert!(
            matches!(
                &d,
                Decision::Undecidable { why }
                    if why.iter().any(|c| matches!(
                        c,
                        UndecidableCause::TagDenyReopenedDiffersByPluginVersion { .. }
                    ))
            ),
            "the two versions answer this download differently, so {:?} must refuse: {}",
            engine.plugin_version,
            named(&d)
        );
    }
}

// ── offset 553 on the mask path: what Ranger actually REACHES ────────────

/// **FIX ROUND 1, the review's measured over-refusal.**
///
/// A tag mask leaves `isAllowed` true on the shared result, so offset 553
/// (`if (result.getIsAllowed() && !evaluator.hasDeny()) setIsAccessDetermined(true)`)
/// fires for the first strictly-higher evaluator with no deny items of its own
/// **whether or not it yielded** — and `RangerDefaultPolicyEvaluator.evaluate`'s
/// 380-384 guard then skips every later evaluator's items.
///
/// So the priority-5 policy here writes nothing, determines anyway, and the
/// priority-1 policy that WOULD have yielded never runs. The order that decides
/// this is between priority bands, which is fully determined and which this
/// crate already sorts by — so the answer is the tag mask, not a refusal.
///
/// The first version of this package's split refusal walked every evaluator
/// above the tag's priority and refused here.
#[test]
fn a_reachable_non_yielder_shuts_out_the_lower_priority_yielder_behind_it() {
    let mut high = resource_policy(
        1,
        "res-mask-high",
        5,
        "dataMaskPolicyItems",
        json!([{
            "users": ["bob"],
            "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": "HASH"},
        }]),
    );
    high["policyType"] = json!(1);
    let mut low = resource_policy(
        2,
        "res-mask-low",
        1,
        "dataMaskPolicyItems",
        mask_items("HASH"),
    );
    low["policyType"] = json!(1);

    let set = envelope(
        json!([high, low.clone()]),
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
        other => panic!("offset 553 makes this deterministic, so it must answer: {other:?}"),
    }

    // The control WITHOUT which the assertion above proves nothing about 553:
    // the priority-1 policy really can yield, and does when it is reachable.
    let alone = envelope(
        json!([low]),
        json!([tag_policy(
            10,
            "tag-mask",
            0,
            1,
            "dataMaskPolicyItems",
            mask_items("MASK")
        )]),
    );
    let d = evaluate_data_mask(&alone, &request(), &trino_def(), &no_superusers());
    assert!(
        matches!(
            &d,
            MaskDecision::Undecidable { why }
                if why.iter().any(|c| matches!(
                    c,
                    UndecidableCause::TagSubResultSplitByResourcePolicy {
                        resource_policy_id: Some(2), ..
                    }
                ))
        ),
        "with nothing shutting it out, the priority-1 policy splits the result: {d:?}"
    );
}

/// The other half of rule 2: a strictly-higher evaluator that yields nothing but
/// IS deny-capable does **not** determine at 553, so the loop continues and the
/// yielder behind it is reached after all — and that one does split.
///
/// One `denyPolicyItems` entry is the whole difference between this test and the
/// one above, which is what makes `hasDeny` the load-bearing field rather than
/// "it wrote nothing".
#[test]
fn a_deny_capable_non_yielder_does_not_shut_out_the_yielder_behind_it() {
    let mut high = resource_policy(
        1,
        "res-mask-high",
        5,
        "dataMaskPolicyItems",
        json!([{
            "users": ["bob"],
            "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": "HASH"},
        }]),
    );
    high["policyType"] = json!(1);
    high["denyPolicyItems"] = select_for_bob();
    let mut low = resource_policy(
        2,
        "res-mask-low",
        1,
        "dataMaskPolicyItems",
        mask_items("HASH"),
    );
    low["policyType"] = json!(1);

    let set = envelope(
        json!([high, low]),
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
    assert!(
        matches!(
            &d,
            MaskDecision::Undecidable { why }
                if why.iter().any(|c| matches!(
                    c,
                    UndecidableCause::TagSubResultSplitByResourcePolicy {
                        resource_policy_id: Some(2), ..
                    }
                ))
        ),
        "a hasDeny() evaluator cannot determine at 553, so the yielder is reached: {d:?}"
    );
}

/// Rule 3: the determining band is taken WHOLE, because which of its members
/// ran first is `getEvalOrder()`. Two priority-5 policies with no deny items —
/// one yields, one does not — so the answer depends on an order this crate
/// cannot compute, and it refuses rather than picking the member it happened to
/// sort first.
#[test]
fn the_determining_band_is_taken_whole_because_its_order_is_unknown() {
    let mut quiet = resource_policy(
        1,
        "a-quiet",
        5,
        "dataMaskPolicyItems",
        json!([{
            "users": ["bob"],
            "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": "HASH"},
        }]),
    );
    quiet["policyType"] = json!(1);
    let mut yielder = resource_policy(2, "b-yields", 5, "dataMaskPolicyItems", mask_items("HASH"));
    yielder["policyType"] = json!(1);

    let set = envelope(
        json!([quiet, yielder]),
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
    assert!(
        matches!(
            &d,
            MaskDecision::Undecidable { why }
                if why.iter().any(|c| matches!(
                    c,
                    UndecidableCause::TagSubResultSplitByResourcePolicy {
                        resource_policy_id: Some(2), ..
                    }
                ))
        ),
        "`a-quiet` sorts first here, but Ranger's order is getEvalOrder(): {d:?}"
    );
}

// ── offset 553 reads no matcher, so Ranger's candidate filter decides ────

/// An unrelated resource policy on another table. Literal values, the request's
/// own four keys, no `isExcludes`, no `isRecursive` — the shape
/// `EgretResourceTrieCandidacyTest` measures as **absent** from
/// `getLikelyMatchPolicyEvaluators`'s answer.
fn unrelated_resource_policy(
    id: i64,
    name: &str,
    priority: i32,
    bucket: &str,
    items: Value,
) -> Value {
    let mut p = json!({
        "id": id, "name": name, "service": "trino", "policyType": 0,
        "isEnabled": true, "policyPriority": priority,
        "resources": {
            "catalog": {"values": ["hive"]},
            "schema":  {"values": ["otherdb"]},
            "table":   {"values": ["othertable"]},
            "column":  {"values": ["*"]},
        },
    });
    p[bucket] = items;
    p
}

/// **FIX ROUND 2, the CRITICAL the re-review measured.**
///
/// Offset 553 is in the ENGINE loop, after `evaluate` has returned, and reads
/// only `hasDeny()` — never the matcher. So an evaluator that matched nothing
/// still ends the evaluation, and with a tag ALLOW seeding `isAllowed` before
/// the resource loop starts it can do so before any resource policy has matched
/// anything at all.
///
/// The crate evaluates every enabled policy of the type; Ranger evaluates its
/// trie's answer. A policy on another table is in the first list and not the
/// second, so without a candidate-set rule this request answers `Allowed` where
/// Ranger denies — a fail-open, and one this package's own composition created:
/// before the phases were composed, `isAllowed` could only become true through a
/// MATCHED policy.
///
/// `hive/otherdb/othertable/*` is measured absent from Ranger's list
/// (`tests/ranger/src/test/java/EgretResourceTrieCandidacyTest.java`), so this
/// case is not merely un-refused — it is ANSWERED, and answered Ranger's way.
#[test]
fn an_unrelated_policy_cannot_end_the_evaluation_at_553() {
    let set = envelope(
        json!([
            unrelated_resource_policy(1, "res-unrelated", 5, "policyItems", select_for_dave()),
            resource_policy(2, "res-deny", 1, "denyPolicyItems", select_for_alice()),
        ]),
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
        "a policy Ranger's trie never returns must not end the evaluation: {}",
        named(&d)
    );

    // The control: the SAME two policies, with the unrelated one moved onto the
    // request's own table. Now it really is in Ranger's list, 553 really does
    // fire, and the deny behind it really is shut out — so the assertion above
    // is about candidacy and not about priority.
    let mut on_target = resource_policy(1, "res-on-target", 5, "policyItems", select_for_dave());
    on_target["policyItems"] = select_for_dave();
    let set = envelope(
        json!([
            on_target,
            resource_policy(2, "res-deny", 1, "denyPolicyItems", select_for_alice()),
        ]),
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
        matches!(&d, Decision::Allowed { policy_name, .. } if policy_name == "tag-allow"),
        "a MATCHED non-deny policy does end the evaluation at 553: {}",
        named(&d)
    );
}

/// The other side of the rule: a policy whose candidacy the crate cannot
/// establish, where it decides the request, refuses.
///
/// `hive/sales/*ders/*` is measured PRESENT in Ranger's list while
/// `getMatchType` says `SELF`… so for the refusal the shape has to be one the
/// matcher REJECTS and the trie may still hold. An `isExcludes` level is exactly
/// that: measured present (it lands in `getInheritedEvaluators()`, unioned into
/// every lookup) with `getMatchType` `NONE`.
#[test]
fn a_policy_whose_candidacy_is_unprovable_refuses_when_it_decides() {
    // FIX ROUND 4. `level_excluded_by_trie` declines to reason from THREE
    // shapes, and until this round only the first was pinned — dropping either
    // of the other two survived the whole suite, in the fail-open direction.
    // Each row below is one shape, measured `present|NONE` in the jar at both
    // plugin versions by `EgretResourceTrieCandidacyTest`: Ranger holds the
    // policy and stops at 553, the crate's matcher rejects it, neither half can
    // be proved, so it refuses rather than letting the deny behind it run.
    let shapes: [(&str, Value); 3] = [
        // `isExcludes` — `addInheritedEvaluator`, unioned into every lookup.
        (
            "isExcludes on the table level",
            json!({"values": ["orders"], "isExcludes": true}),
        ),
        // `isRecursive` — `insert` ends `if (isWildcard || isRecursive)
        // addWildcardEvaluator(...)` and the caller passes `getIsRecursive()`
        // with no `recursiveSupported` check, so `order` is a PREFIX in the trie
        // and comes back for `orders`, while the matcher — which never reads the
        // flag — tests it exactly.
        (
            "isRecursive on the table level",
            json!({"values": ["order"], "isRecursive": true}),
        ),
        // A leading wildcard — `getNonWildcardPrefix` is empty, so the value is
        // parked at the trie ROOT and comes back for every request value.
        (
            "a leading-wildcard table value",
            json!({"values": ["*xyz"]}),
        ),
    ];

    let tag = json!([tag_policy(
        10,
        "tag-allow",
        0,
        0,
        "policyItems",
        select_for_alice()
    )]);
    for (label, table_level) in shapes {
        let mut unprovable =
            resource_policy(1, "res-unprovable", 5, "policyItems", select_for_dave());
        unprovable["resources"]["table"] = table_level;
        let set = envelope(
            json!([
                unprovable,
                resource_policy(2, "res-deny", 1, "denyPolicyItems", select_for_alice()),
            ]),
            tag.clone(),
        );

        let d = evaluate(&set, &request(), &trino_def(), &no_superusers());
        assert!(
            matches!(
                &d,
                Decision::Undecidable { why }
                    if why.iter().any(|c| matches!(
                        c,
                        UndecidableCause::ResourceCandidacyNotReproducible {
                            policy_id: Some(1), ..
                        }
                    ))
            ),
            "{label} is in Ranger's list and not in the matcher's: {}",
            named(&d)
        );
    }

    // The control the three rows need: the SAME policy with a plain literal that
    // does not match is provably ABSENT, so the deny behind it runs. Without it,
    // "it refused" could be a blanket on anything at priority 5.
    let mut literal = resource_policy(1, "res-literal", 5, "policyItems", select_for_dave());
    literal["resources"]["table"] = json!({"values": ["othertable"]});
    let set = envelope(
        json!([
            literal,
            resource_policy(2, "res-deny", 1, "denyPolicyItems", select_for_alice()),
        ]),
        tag,
    );
    let d = evaluate(&set, &request(), &trino_def(), &no_superusers());
    assert!(
        matches!(
            &d,
            Decision::Denied { policy_name, reason: DenyReason::DenyPolicyItem, .. }
                if policy_name == "res-deny"
        ),
        "a plain literal mismatch IS provable, so the deny behind it runs: {}",
        named(&d)
    );
}

/// …and it refuses ONLY when it decides. The same unprovable policy, with
/// nothing behind it that could have answered differently, must still answer —
/// otherwise the refusal is a blanket on every download carrying an `isExcludes`
/// policy anywhere.
#[test]
fn an_unprovable_candidacy_that_changes_no_verdict_still_answers() {
    let mut excludes = resource_policy(1, "res-excludes", 5, "policyItems", select_for_dave());
    excludes["resources"]["table"] = json!({"values": ["orders"], "isExcludes": true});
    let set = envelope(
        json!([excludes]),
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
        matches!(&d, Decision::Allowed { policy_name, .. } if policy_name == "tag-allow"),
        "both branches of the unknown give the same verdict, so this answers: {}",
        named(&d)
    );
}

/// **The mask twin of the CRITICAL** — the defect fix round 1 introduced by
/// latching `determining_band` on a candidate Ranger may never reach.
///
/// Ranger's data-mask trie returns only the priority-1 policy, which yields, so
/// Ranger produces exactly the id/data split
/// [`egret_ranger_eval::eval::UndecidableCause::TagSubResultSplitByResourcePolicy`]
/// exists to refuse. Before the fix the crate answered `Masked(tag-mask)`
/// instead, because the unrelated priority-5 policy shut the yielder out.
#[test]
fn an_unrelated_mask_policy_does_not_shut_out_a_yielder_ranger_reaches() {
    let mut unrelated = unrelated_resource_policy(
        1,
        "res-unrelated",
        5,
        "dataMaskPolicyItems",
        mask_items("HASH"),
    );
    unrelated["policyType"] = json!(1);
    let mut low = resource_policy(
        2,
        "res-mask-low",
        1,
        "dataMaskPolicyItems",
        mask_items("HASH"),
    );
    low["policyType"] = json!(1);

    let set = envelope(
        json!([unrelated, low]),
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
    assert!(
        matches!(
            &d,
            MaskDecision::Undecidable { why }
                if why.iter().any(|c| matches!(
                    c,
                    UndecidableCause::TagSubResultSplitByResourcePolicy {
                        resource_policy_id: Some(2), ..
                    }
                ))
        ),
        "a policy Ranger's trie never returns must not end the walk: {d:?}"
    );
}

/// The row-filter twin, which the re-review noted was untested only because the
/// defect was.
#[test]
fn an_unrelated_row_filter_policy_does_not_shut_out_a_yielder_ranger_reaches() {
    let unrelated = json!({
        "id": 1, "name": "res-unrelated", "service": "trino", "policyType": 2,
        "isEnabled": true, "policyPriority": 5,
        "resources": {
            "catalog": {"values": ["hive"]},
            "schema":  {"values": ["otherdb"]},
            "table":   {"values": ["othertable"]},
        },
        "rowFilterPolicyItems": filter_items("id > 99"),
    });
    let set = envelope(
        json!([
            unrelated,
            row_filter_resource_policy(2, "res-filter-low", 1, "id > 9")
        ]),
        json!([tag_policy(
            10,
            "tag-filter",
            0,
            2,
            "rowFilterPolicyItems",
            filter_items("id > 2")
        )]),
    );
    let d = evaluate_row_filter(&set, &table_request(), &trino_def(), &no_superusers());
    assert!(
        matches!(
            &d,
            RowFilterDecision::Undecidable { why }
                if why.iter().any(|c| matches!(
                    c,
                    UndecidableCause::TagSubResultSplitByResourcePolicy {
                        resource_policy_id: Some(2), ..
                    }
                ))
        ),
        "the row-filter path has the same candidate-set rule: {d:?}"
    );
}

// ── ANCESTOR is PRESENT in Ranger's list, so it determines ───────────────

/// **FIX ROUND 3, the re-review's measured over-refusal.**
///
/// An ordinary table-level grant — `catalog/schema/table` with no `column` —
/// against a column-level request. The crate's matcher calls that `ANCESTOR`,
/// which is not an allow, so it decides nothing. But
/// `EgretResourceTrieCandidacyTest` measures it **present** in
/// `getLikelyMatchPolicyEvaluators`'s answer: `RangerResourceTrie` adds a policy
/// declaring no value for a level as an INHERITED evaluator of that level when
/// `isAncestorOf(resourceDef)`, and inherited evaluators are unioned into every
/// lookup. So Ranger runs it, offset 553 fires, and the deny behind it never
/// runs.
///
/// Fix round 2 proved that row and then did not use it: `trie_excludes` requires
/// the policy to declare exactly the request's keys, so a three-key policy fell
/// through to "cannot tell" and refused. That is a refusal at a place where
/// reproduction was available AND already measured — the expensive kind.
///
/// Three fixtures, differing only in the resource map, so this is about the
/// hierarchy relation and nothing else.
#[test]
fn a_table_level_grant_is_in_rangers_list_and_ends_the_evaluation() {
    let table_level = |id: i64, name: &str, priority: i32, schema: &str, table: &str| {
        json!({
            "id": id, "name": name, "service": "trino", "policyType": 0,
            "isEnabled": true, "policyPriority": priority,
            "resources": {
                "catalog": {"values": ["hive"]},
                "schema":  {"values": [schema]},
                "table":   {"values": [table]},
            },
            "policyItems": select_for_dave(),
        })
    };
    let deny = resource_policy(2, "res-deny", 1, "denyPolicyItems", select_for_alice());
    let tag = json!([tag_policy(
        10,
        "tag-allow",
        0,
        0,
        "policyItems",
        select_for_alice()
    )]);

    // 1. ANCESTOR of the request: present, so 553 fires and the tag allow stands.
    let set = envelope(
        json!([
            table_level(1, "res-table-level", 5, "sales", "orders"),
            deny.clone()
        ]),
        tag.clone(),
    );
    let d = evaluate(&set, &request(), &trino_def(), &no_superusers());
    assert!(
        matches!(&d, Decision::Allowed { policy_name, .. } if policy_name == "tag-allow"),
        "an ANCESTOR policy is in Ranger's list and determines at 553: {}",
        named(&d)
    );

    // 2. The same SHAPE on another branch: `NONE`, not `ANCESTOR`, and measured
    //    ABSENT — so the rule is the hierarchy relation, not "declares fewer
    //    levels". The deny behind it must therefore still run.
    let set = envelope(
        json!([
            table_level(1, "res-elsewhere", 5, "otherdb", "othertable"),
            deny.clone()
        ]),
        tag.clone(),
    );
    let d = evaluate(&set, &request(), &trino_def(), &no_superusers());
    assert!(
        matches!(
            &d,
            Decision::Denied { policy_name, reason: DenyReason::DenyPolicyItem, .. }
                if policy_name == "res-deny"
        ),
        "a three-key policy on another branch is NOT an ancestor of this request: {}",
        named(&d)
    );

    // 3. The four-key version of fixture 1, which was already right before this
    //    round — so a regression here would tell the two rules apart.
    let set = envelope(
        json!([
            resource_policy(1, "res-four-key", 5, "policyItems", select_for_dave()),
            deny,
        ]),
        tag,
    );
    let d = evaluate(&set, &request(), &trino_def(), &no_superusers());
    assert!(
        matches!(&d, Decision::Allowed { policy_name, .. } if policy_name == "tag-allow"),
        "the SELF-matching version determines, as it always did: {}",
        named(&d)
    );
}

/// The mask twin: an ancestor-level mask policy is in Ranger's data-mask list
/// too, so it ends the walk and the tag mask stands rather than the yielder
/// behind it splitting the result.
#[test]
fn a_table_level_mask_policy_ends_the_sub_walk_too() {
    let ancestor = json!({
        "id": 1, "name": "res-mask-ancestor", "service": "trino", "policyType": 1,
        "isEnabled": true, "policyPriority": 5,
        "resources": {
            "catalog": {"values": ["hive"]},
            "schema":  {"values": ["sales"]},
        },
        "dataMaskPolicyItems": json!([{
            "users": ["bob"],
            "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": "HASH"},
        }]),
    });
    let mut low = resource_policy(
        2,
        "res-mask-low",
        1,
        "dataMaskPolicyItems",
        mask_items("HASH"),
    );
    low["policyType"] = json!(1);

    let set = envelope(
        json!([ancestor, low]),
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
        other => panic!("an ancestor mask policy is in Ranger's list and ends the walk: {other:?}"),
    }
}

/// **A fail-open a SURVIVING mutation found, and no review did.**
///
/// The mutation was "read a level the policy does not declare as a proof of
/// absence". It survived the whole suite, because a policy declaring a prefix of
/// the request's levels normally matches at `ANCESTOR` and never reaches
/// `trie_excludes` at all.
///
/// The shape that does reach it: a prefix policy carrying `isExcludes` on one of
/// the levels it DOES declare. Its match type is `NONE` — excludes inverts the
/// level — so the crate cannot call it a candidate; and it is present in
/// Ranger's list twice over, inherited at `schema` because of the excludes and
/// at `table`/`column` because of `isAncestorOf`. Measured as
/// `excludes-schema-partial | hive/EXCL:sales/-/-` → `present|NONE` in
/// `EgretResourceTrieCandidacyTest`, at both plugin versions.
///
/// So the crate can prove neither presence nor absence, and it decides the
/// request: refuse. Reading the undeclared `table`/`column` levels as absence
/// would instead skip it and answer `Denied`, which is Ranger's opposite.
#[test]
fn an_undeclared_level_is_not_a_proof_of_absence() {
    let partial_excludes = json!({
        "id": 1, "name": "res-partial-excludes", "service": "trino", "policyType": 0,
        "isEnabled": true, "policyPriority": 5,
        "resources": {
            "catalog": {"values": ["hive"]},
            "schema":  {"values": ["sales"], "isExcludes": true},
        },
        "policyItems": select_for_dave(),
    });
    let set = envelope(
        json!([
            partial_excludes,
            resource_policy(2, "res-deny", 1, "denyPolicyItems", select_for_alice()),
        ]),
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
            Decision::Undecidable { why }
                if why.iter().any(|c| matches!(
                    c,
                    UndecidableCause::ResourceCandidacyNotReproducible { policy_id: Some(1), .. }
                ))
        ),
        "a level the policy does not declare is the inherited path, never absence: {}",
        named(&d)
    );
}

// ── the candidate filter reaches the ARBITRATION (SP4d-2b Task 5) ─────────

/// **A defect the differential gate found, and its minimal reproducer.**
///
/// The three-way arbitration sets `determined` from the PRIORITY alone and never
/// asks the matcher, so a resource policy Ranger's trie removed could end the
/// loop, skip the restore at offsets 599-632, and change the verdict. The crate
/// used to iterate every enabled policy of the type there.
///
/// MEASURED on the real 2.8.0 engine, this exact shape: `Denied, policyId 2` —
/// the allowing policy wrote its id and `setIsAllowed(false)` in the restore then
/// cleared `isAllowed` without touching it — and the same answer with the third
/// policy REMOVED, because its evaluator is never in the loop's list at all.
/// Before the fix this crate answered `Allowed` with it and `Denied` without it:
/// the entire divergence was a policy neither engine ever evaluates.
///
/// The `excluded` policy's `schema` value is a LITERAL that does not match the
/// request, which is what makes `trie_excludes` able to prove absence. A wildcard
/// there would leave the crate unable to prove it and the skip would not fire —
/// which is the point of the second assertion below.
#[test]
fn a_trie_excluded_policy_cannot_end_the_loop_and_skip_the_restore() {
    let allower_with_deny = json!({
        "id": 2, "name": "res-allow-deny-capable", "service": "trino", "policyType": 0,
        "isEnabled": true, "policyPriority": 1,
        "resources": {
            "catalog": {"values": ["hive"]}, "schema": {"values": ["sales"]},
            "table": {"values": ["orders"]}, "column": {"values": ["*"]},
        },
        "policyItems": select_for_alice(),
        // Deny items that match NOBODY: `hasDeny()` is true, so offset 553
        // cannot determine for this policy and the loop must run out.
        "denyPolicyItems": select_for_bob(),
    });
    // Same hierarchy depth, a literal `schema` the request does not carry, and
    // NO deny items — so before the fix it triggered offset 469 and broke the
    // loop.
    let trie_excluded = json!({
        "id": 3, "name": "res-other-schema", "service": "trino", "policyType": 0,
        "isEnabled": true, "policyPriority": 0,
        "resources": {
            "catalog": {"values": ["hive"]}, "schema": {"values": ["otherschema"]},
            "table": {"values": ["orders"]}, "column": {"values": ["*"]},
        },
        "policyItems": select_for_alice(),
    });
    let tag_deny = json!([tag_policy(
        10,
        "tag-deny",
        0,
        0,
        "denyPolicyItems",
        select_for_alice()
    )]);

    let with_excluded = envelope(
        json!([allower_with_deny.clone(), trie_excluded]),
        tag_deny.clone(),
    );
    let d = evaluate(&with_excluded, &request(), &trino_def(), &no_superusers());
    assert!(
        matches!(&d, Decision::Denied { policy_name, .. } if policy_name == "tag-deny"),
        "a policy Ranger's trie does not hold must not end the loop, so the restore must \
         put the tag deny back: {}",
        named(&d)
    );

    // The control, and the half that makes the assertion above mean something:
    // WITHOUT the excluded policy the answer must be the SAME. Ranger's answer
    // does not move when a policy it never evaluates is added or removed, and
    // that invariance is the whole finding — before the fix these two differed.
    let without = envelope(json!([allower_with_deny]), tag_deny);
    let d_without = evaluate(&without, &request(), &trino_def(), &no_superusers());
    assert_eq!(
        format!("{d:?}"),
        format!("{d_without:?}"),
        "adding a policy Ranger's trie excludes must not change the verdict"
    );
}
