//! SP4d-2b Task 2: evaluating the `tagPolicies` block against a resolved tag
//! set.
//!
//! # Why this file lives in `src/` and not in `tests/`
//!
//! SP4d-2b Task 3. The three functions exercised here deliberately do NOT run
//! [`crate::eval::request_preamble`] — no super-user short-circuit, no
//! `SuperUsersUnknown`, no engine/service-def cross-check — on the premise that
//! the composed evaluator is their only caller. Task 3's caller enumeration
//! found exactly one caller that was not: this file, reaching them across the
//! crate boundary through `pub`. So they are `pub(crate)` now and the file came
//! inside with them, which is the boundary being right rather than a second
//! preamble call papering over its being wrong.
//!
//! Nothing else changed: the fixtures, the assertions and the measurements
//! below are Task 2's, and the paths are still `CARGO_MANIFEST_DIR`-relative.
//!
//! # The fixture is the one the engine is holding
//!
//! `tests/fixtures/tag-policies-live.json` is the policy cache captured from
//! the running Trino plugin (provenance and sha256 in `tests/tag_tiers.rs`).
//! Its `tagPolicies` block carries three real policies, and all three are
//! exercised here rather than only the one that is easy to assert:
//!
//! | id | `policyType` | `resources.tag.values` | what decides |
//! |------|----------------|------------------------|---|
//! | 8799 | 1 (mask) | `["pii"]` | `dataMaskPolicyItems`, `MASK` for `public` |
//! | 8805 | 2 (row filter) | `["pii"]` | `rowFilterPolicyItems`, `id > 2` for `public` |
//! | 8839 | 0 (ACCESS) | `["tag"]` | `denyPolicyItems` for `public`, `denyExceptions` for `admin` |
//!
//! # What was measured in the jars for this task
//!
//! With `javap -c` on `ranger-plugins-common` **2.5.0** and **2.8.0**, and — for
//! the two claims a disassembly cannot settle — by RUNNING the real classes on
//! the harness classpath at `tests/ranger`.
//!
//! * **The tag's match type is the admitted-set test, not the policy's.**
//!   `RangerDefaultPolicyEvaluator.evaluate` branches on
//!   `request instanceof RangerTagAccessRequest` (2.5.0 offsets 188-201, 2.8.0
//!   188-199) and reads `((RangerTagAccessRequest) request).getMatchType()` —
//!   the value `RangerTagAccessRequest`'s constructor took from
//!   `RangerTagForEval`, i.e. the enricher's verdict on the ATTACHMENT. The
//!   policy's own `getMatchType` (2.5.0 offset 247, 2.8.0 232) is on the OTHER
//!   branch and is never reached for a tag request.
//!
//! * **`ANCESTOR` is admitted, and `DESCENDANT`/`NONE` are not**, at the
//!   default `SELF` scope with a concrete access type — in both versions, by
//!   different routes:
//!   - 2.5.0 REWRITES `ANCESTOR` to `SELF` (offsets 206-219) and then tests
//!     `SELF || SELF_AND_ALL_DESCENDANTS` (317-338). The rewrite is also why
//!     `updateAccessResult`'s own `matchType != ANCESTOR` guard on the allow
//!     branch (offsets 142-146) never fires for a tag request.
//!   - 2.8.0 keeps the match type and adds
//!     `|| (matchType == ANCESTOR && request instanceof RangerTagAccessRequest)`
//!     (offsets 334-370), having dropped that `updateAccessResult` guard.
//!
//! * **Three comparators, chosen by a `tableswitch` on `policyType`** at
//!   offsets 237-298 of `RangerPolicyRepository.getLikelyMatchPolicyEvaluators`:
//!   `0` and `3` (audit) take `EVAL_ORDER_COMPARATOR`, `1` takes
//!   `MATCH_TYPE_COMPARATOR`, `2` takes `NAME_COMPARATOR`.
//!   `MATCH_TYPE_COMPARATOR` compares the TAG's match type first and only falls
//!   through to `NAME_COMPARATOR` on a tie (the `ifne 44` at offset 24), and
//!   `NAME_COMPARATOR` is **not** just the name: priority DESC, then `hasDeny`
//!   first, then `compareStrings(name)`.
//!
//! * **`MATCH_TYPE_COMPARATOR`'s order was RUN, not read.** Sorting every
//!   `MatchType` with the real comparator on both jars gives
//!   `SELF < SELF_AND_ALL_DESCENDANTS < ANCESTOR < DESCENDANT < NONE` — which is
//!   NOT the enum's declaration order (`NONE, SELF, DESCENDANT, ANCESTOR,
//!   SELF_AND_ALL_DESCENDANTS`), so a rank taken from `ordinal()` would reorder
//!   masks.
//!
//! * **The candidate filter for a tag request is the TRIE, and it is wider than
//!   the matcher.** Because `evaluate` never re-asks the matcher (above),
//!   `RangerResourceEvaluatorsRetriever` alone decides which tag policies run.
//!   Building a real `RangerResourceTrie` over the `tag` service-def and
//!   comparing it with `getMatchType`, on 2.5.0: they agree for a literal, for
//!   `*`/`**` and for `p*`-shaped values, and DISAGREE for `*ii` (parked at the
//!   root) and for `p*x` / `pi?` (parked at the literal prefix), which the trie
//!   returns for tags the matcher rejects. An `isExcludes` resource is worse: it
//!   lands in `getInheritedEvaluators()`, which the retriever unions into every
//!   lookup, so a policy excluding `pii` comes back as a candidate for `pii`
//!   itself. Those shapes are refused here rather than answered — see
//!   `a_wildcard_tag_value_is_refused_because_the_trie_is_wider`.

use std::collections::{BTreeMap, BTreeSet};

use crate::eval::{
    evaluate_tag_data_mask as evaluate_tag_data_mask_phase,
    evaluate_tag_policies as evaluate_tag_policies_phase,
    evaluate_tag_row_filter as evaluate_tag_row_filter_phase, AccessRequest, DenyReason,
    EnrichedTags, MaskDecision, ResolvedTag, RowFilterDecision, TagDecision, TagResourceProblem,
    TagServiceResource, UndecidableCause, UnenrichedTags,
};
use crate::model::{ServicePolicySet, TagPolicyBlock, Unmodelled};
use crate::resource::{MatchType, RequestResource};
use crate::servicedef::ServiceDef;
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

fn live_download() -> ServicePolicySet {
    serde_json::from_value(live_download_json()).expect("parse captured download")
}

/// The `tag` service-def the captured download carries, verbatim.
fn live_tag_def() -> ServiceDef {
    ServiceDef::parse(&live_download_json()["tagPolicies"]["serviceDef"])
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

/// An envelope with no resource policies at all and a `tagPolicies` block
/// carrying exactly `policies`, under the captured `tag` service-def. The
/// resource half is empty on purpose: this task evaluates ONE phase, and a
/// resource policy in the fixture could only obscure which phase answered.
fn tagged(policies: Value) -> ServicePolicySet {
    let block = json!({
        "serviceName": "sp3c2-trino-live_tag",
        "serviceId": 51,
        "policyVersion": 22,
        "serviceDef": live_download_json()["tagPolicies"]["serviceDef"],
        "policies": policies,
    });
    serde_json::from_value(json!({
        "serviceName": "sp3c2-trino-live",
        "policies": [],
        "serviceDef": {"name": "trino"},
        "tagPolicies": block,
    }))
    .expect("parse synthetic envelope")
}

/// One tag policy, spelled out rather than templated so each test's fixture is
/// readable where it is used.
fn tag_policy(id: i64, name: &str, policy_type: i32, tag_values: Value, extra: Value) -> Value {
    let mut policy = json!({
        "id": id,
        "name": name,
        "service": "sp3c2-trino-live_tag",
        "policyType": policy_type,
        "resources": {"tag": {"values": tag_values, "isExcludes": false, "isRecursive": false}},
    });
    for (k, v) in extra.as_object().expect("extra is an object") {
        policy[k] = v.clone();
    }
    policy
}

fn deny_select_for_public() -> Value {
    json!({
        "denyPolicyItems": [{
            "groups": ["public"],
            "accesses": [{"type": "select", "isAllowed": true}],
        }],
    })
}

fn allow_select_for_public() -> Value {
    json!({
        "policyItems": [{
            "groups": ["public"],
            "accesses": [{"type": "select", "isAllowed": true}],
        }],
    })
}

fn mask_for_public(mask_type: &str) -> Value {
    json!({
        "dataMaskPolicyItems": [{
            "groups": ["public"],
            "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": mask_type},
        }],
    })
}

fn row_filter_for_public(expr: &str) -> Value {
    json!({
        "rowFilterPolicyItems": [{
            "groups": ["public"],
            "accesses": [{"type": "select", "isAllowed": true}],
            "rowFilterInfo": {"filterExpr": expr},
        }],
    })
}

/// A `select` on `hive.sales.orders.id` by `user`, carrying `tags` as the
/// resolved set. The RESOURCE is irrelevant to the tag phase — Ranger builds a
/// `RangerTagResource` holding only `{tag: <name>}` — and is realistic here so
/// that it cannot accidentally be what a tag policy matched.
/// The request's own set, presented to the phase as already enriched.
///
/// **SP4d-2b Task 7 made the enrichment ordering a type.** The phase now takes
/// `Option<&EnrichedTags>`, which `crate::eval::enriched_tags` produces from the
/// RESOURCE service-def — and these tests hold only the TAG def, because the
/// enricher's drop is the COMPOSED evaluator's step and is pinned where it
/// happens (`tests/tag_refusals.rs`, `tests/tag_tiers.rs`). So the fixture goes
/// in unchanged, which is exactly what Task 2 did when the parameter was the
/// request itself. The three adapters below keep that statement in ONE place
/// instead of at thirty call sites, and the `Option` — "the caller did not
/// resolve", never the empty set — is carried through untouched.
fn enriched(request: &AccessRequest) -> Option<EnrichedTags> {
    request
        .resolved_tags
        .as_ref()
        .map(|tags| EnrichedTags::already_enriched(tags.as_slice().to_vec()))
}

fn evaluate_tag_policies(
    policies: &ServicePolicySet,
    request: &AccessRequest,
    tag_service_def: &ServiceDef,
) -> TagDecision {
    evaluate_tag_policies_phase(
        policies,
        request,
        enriched(request).as_ref(),
        tag_service_def,
    )
}

fn evaluate_tag_data_mask(
    policies: &ServicePolicySet,
    request: &AccessRequest,
    tag_service_def: &ServiceDef,
) -> MaskDecision {
    evaluate_tag_data_mask_phase(
        policies,
        request,
        enriched(request).as_ref(),
        tag_service_def,
    )
}

fn evaluate_tag_row_filter(
    policies: &ServicePolicySet,
    request: &AccessRequest,
    tag_service_def: &ServiceDef,
) -> RowFilterDecision {
    evaluate_tag_row_filter_phase(
        policies,
        request,
        enriched(request).as_ref(),
        tag_service_def,
    )
}

fn request(user: &str, tags: Option<Vec<ResolvedTag>>) -> AccessRequest {
    AccessRequest {
        user: user.to_string(),
        groups: BTreeSet::new(),
        roles: BTreeSet::new(),
        access_type: "select".to_string(),
        resource: RequestResource::new()
            .with("catalog", "hive")
            .with("schema", "sales")
            .with("table", "orders")
            .with("column", "id"),
        owner: None,
        resolved_tags: tags.map(UnenrichedTags::new),
    }
}

/// One attachment shaped the way `ServiceTags::from_assignments` writes a
/// COLUMN-level one: four `resourceElements` levels, an enabled service
/// resource, and none of `attributes` / `options` / `validityPeriods` — the
/// payload egret itself emits, so every attachment refusal SP4d-2b Task 4 added
/// is inert for it. A test that wants one of those refusals fills the field.
fn tag(name: &str, match_type: MatchType) -> ResolvedTag {
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

fn causes(decision: &TagDecision) -> Vec<UndecidableCause> {
    match decision {
        TagDecision::Undecidable { why } => why.clone(),
        _ => Vec::new(),
    }
}

// ── the captured block is read, not counted ──────────────────────────────

/// Before this task the block's policies were counted for the refusal and then
/// dropped. Pins that they are now READ, with the ids and policy types the
/// plugin holds.
#[test]
fn the_captured_download_carries_its_three_tag_policies() {
    let set = live_download();
    let carried: Vec<(Option<i64>, i32)> = set
        .tag_policies
        .iter()
        .map(|p| (p.id, p.policy_type))
        .collect();
    assert_eq!(
        carried,
        vec![(Some(8799), 1), (Some(8805), 2), (Some(8839), 0)],
        "the captured tagPolicies block must reach the evaluator intact"
    );
}

// ── the live ACCESS policy, 8839 ─────────────────────────────────────────

/// Case 1. The tag the live clearance names is `tag`, its `denyPolicyItems`
/// name the group `public` — which `matchUserGroupAndOwner` matches for every
/// user — and `select` is one of its three access types.
#[test]
fn a_tag_in_the_resolved_set_lets_the_live_clearance_deny() {
    let set = live_download();
    let d = evaluate_tag_policies(
        &set,
        &request("alice", Some(vec![tag("tag", MatchType::Self_)])),
        &live_tag_def(),
    );
    assert_eq!(
        d,
        TagDecision::Denied {
            policy_id: Some(8839),
            policy_name: "perm-clearance-3205756d-543e-49f5-adfb-75f31112b5c2".to_string(),
            priority: 0,
            reason: DenyReason::DenyPolicyItem,
        },
        "the live clearance must deny a request carrying its tag"
    );
}

/// The other half of the same policy: `denyExceptions` names `admin`, and an
/// exception VETOES the deny rather than competing with it — the policy then
/// has no allow bucket, so it determines nothing at all.
#[test]
fn the_live_clearance_s_deny_exception_leaves_it_undetermined() {
    let set = live_download();
    let d = evaluate_tag_policies(
        &set,
        &request("admin", Some(vec![tag("tag", MatchType::Self_)])),
        &live_tag_def(),
    );
    assert_eq!(
        d,
        TagDecision::NotDetermined,
        "a vetoed deny is not an allow, and 8839 carries no policyItems"
    );
}

/// Case 2. `pii` is a real tag in this fixture — 8799 and 8805 name it — so
/// this is not "no tags", it is "not THIS policy's tag". 8839 names `tag`.
#[test]
fn a_tag_outside_the_resolved_set_cannot_decide() {
    let set = live_download();
    let d = evaluate_tag_policies(
        &set,
        &request("alice", Some(vec![tag("pii", MatchType::Self_)])),
        &live_tag_def(),
    );
    assert_eq!(
        d,
        TagDecision::NotDetermined,
        "8839 names the tag `tag`, and `pii` is not it"
    );
}

// ── the admitted set ─────────────────────────────────────────────────────

/// Case 3. **`ANCESTOR` is admitted here and is not, for a resource policy.**
///
/// This is the row that egret's live data cannot exercise — every live tag
/// assignment is column-level — and the one the package's correctness turns on:
/// a tag attached to the TABLE governs a request naming a column.
///
/// `SELF_AND_ALL_DESCENDANTS` rides along in the same assertion because it is
/// the only other admitted member.
#[test]
fn an_ancestor_tag_is_admitted_on_the_tag_path() {
    let set = live_download();
    for match_type in [
        MatchType::Self_,
        MatchType::SelfAndAllDescendants,
        MatchType::Ancestor,
    ] {
        let d = evaluate_tag_policies(
            &set,
            &request("alice", Some(vec![tag("tag", match_type)])),
            &live_tag_def(),
        );
        assert!(
            matches!(
                d,
                TagDecision::Denied {
                    policy_id: Some(8839),
                    ..
                }
            ),
            "{match_type:?} must be admitted on the tag path: {d:?}"
        );
    }
}

/// The contrast, in the same crate and the same enum: for a RESOURCE policy
/// `ANCESTOR` is not an allow. `RangerDefaultPolicyEvaluator.evaluate` admits it
/// only for a `RangerTagAccessRequest`, and a live Trino measurement confirmed
/// the resource side. Without this assertion "ANCESTOR is admitted" could be
/// read as a property of the enum rather than of the request kind.
#[test]
fn ancestor_is_still_not_an_allow_for_a_resource_policy() {
    let policy_resources = [(
        "catalog".to_string(),
        serde_json::from_value(json!({"values": ["hive"]})).expect("level"),
    )]
    .into_iter()
    .collect();
    let requested = RequestResource::new()
        .with("catalog", "hive")
        .with("schema", "sales");
    assert_eq!(
        crate::resource::match_resource(&policy_resources, &requested, &trino_def()),
        MatchType::Ancestor,
        "the fixture must actually produce an ANCESTOR match, or it tests nothing"
    );
}

/// Case 4. `DESCENDANT` and `NONE` are in neither version's admitted set at the
/// default `SELF` scope. `DESCENDANT` joins it only under `isAccessTypeAny()`
/// (which is [`UndecidableCause::PseudoAccessType`] here) or
/// `SELF_OR_DESCENDANTS` (which [`AccessRequest`] cannot express); `NONE` never
/// does.
#[test]
fn a_descendant_or_none_tag_is_not_admitted() {
    let set = live_download();
    for match_type in [MatchType::Descendant, MatchType::None] {
        let d = evaluate_tag_policies(
            &set,
            &request("alice", Some(vec![tag("tag", match_type)])),
            &live_tag_def(),
        );
        assert_eq!(
            d,
            TagDecision::NotDetermined,
            "{match_type:?} must NOT reach the policy items"
        );
    }
}

// ── the TAG def drives matching, and it is case-sensitive ────────────────

/// Case 5. `crates/egret-policy/src/service-defs/tag.json` and the def embedded
/// in the captured download both set **`matcherOptions.ignoreCase: false`** on
/// the single `tag` resource, where `trino.json` and `starrocks.json` leave
/// resource matching case-INSENSITIVE. So `TAG` is not `tag`.
///
/// The control is what makes this a test of the DEF rather than of a constant:
/// the same request against the same policies, with only `ignoreCase` flipped
/// in the def, denies. A hard-coded case rule would pass the first half and
/// fail the second.
#[test]
fn tag_matching_is_case_sensitive_because_the_tag_def_says_so() {
    let set = live_download();
    let upper = request("alice", Some(vec![tag("TAG", MatchType::Self_)]));

    assert_eq!(
        evaluate_tag_policies(&set, &upper, &live_tag_def()),
        TagDecision::NotDetermined,
        "ignoreCase is false on the tag resource, so `TAG` must not match `tag`"
    );

    let mut def_json = live_download_json();
    def_json["tagPolicies"]["serviceDef"]["resources"][0]["matcherOptions"]["ignoreCase"] =
        json!("true");
    let case_insensitive = ServiceDef::parse(&def_json["tagPolicies"]["serviceDef"]);
    assert!(
        matches!(
            evaluate_tag_policies(&set, &upper, &case_insensitive),
            TagDecision::Denied {
                policy_id: Some(8839),
                ..
            }
        ),
        "with ignoreCase true the SAME request must match — otherwise the \
         case rule is not coming from the def"
    );
}

/// The def that decides is the BLOCK's, not the service's. Handed `trino.json`,
/// no hierarchy contains `tag`, `init` fails and every match is `NONE` — so a
/// phase that reached for the resource def would answer "nothing applies" with
/// full confidence.
#[test]
fn the_resource_def_cannot_stand_in_for_the_tag_def() {
    let set = live_download();
    let d = evaluate_tag_policies(
        &set,
        &request("alice", Some(vec![tag("tag", MatchType::Self_)])),
        &trino_def(),
    );
    assert_eq!(d, TagDecision::NotDetermined, "{d:?}");
}

// ── the live MASK and ROW-FILTER policies, 8799 and 8805 ─────────────────

#[test]
fn the_live_mask_policy_masks_a_pii_tagged_column() {
    let set = live_download();
    let d = evaluate_tag_data_mask(
        &set,
        &request("alice", Some(vec![tag("pii", MatchType::Self_)])),
        &live_tag_def(),
    );
    match d {
        MaskDecision::Masked(m) => {
            assert_eq!(m.policy_id, Some(8799));
            assert_eq!(m.mask_type.as_deref(), Some("MASK"));
            assert_eq!(m.masked_value, None);
            assert_eq!(m.mask_condition, None);
        }
        other => panic!("expected the live MASK policy to decide: {other:?}"),
    }
}

#[test]
fn the_live_row_filter_policy_filters_a_pii_tagged_table() {
    let set = live_download();
    let d = evaluate_tag_row_filter(
        &set,
        &request("alice", Some(vec![tag("pii", MatchType::Self_)])),
        &live_tag_def(),
    );
    match d {
        RowFilterDecision::Filtered(f) => {
            assert_eq!(f.policy_id, Some(8805));
            assert_eq!(f.filter_expr, "id > 2");
        }
        other => panic!("expected the live row-filter policy to decide: {other:?}"),
    }
}

/// The three lists are disjoint: `RangerPolicyRepository.init` files a policy by
/// its RAW `policyType`, so the mask policy cannot answer an access request and
/// the clearance cannot answer a mask request — even though all three sit in
/// one block and `pii` is in the set.
#[test]
fn the_three_policy_types_cannot_answer_each_other_s_question() {
    let set = live_download();
    let req = request(
        "alice",
        Some(vec![
            tag("pii", MatchType::Self_),
            tag("tag", MatchType::Self_),
        ]),
    );
    assert_eq!(
        evaluate_tag_data_mask(
            &set,
            &request("alice", Some(vec![tag("tag", MatchType::Self_)])),
            &live_tag_def()
        ),
        MaskDecision::NoMask,
        "the clearance is policyType 0 and cannot mask"
    );
    assert!(
        matches!(
            evaluate_tag_policies(&set, &req, &live_tag_def()),
            TagDecision::Denied {
                policy_id: Some(8839),
                ..
            }
        ),
        "only the ACCESS policy answers the access question"
    );
}

// ── the three orderings ──────────────────────────────────────────────────

/// **MASK orders by the TAG's match type, and that key comes FIRST — before
/// priority.**
///
/// The `ANCESTOR` tag's policy has the EARLIER name (`a-ancestor` against
/// `z-self`), so only `MATCH_TYPE_COMPARATOR`, with the match type as its first
/// key, picks `z-self`.
///
/// This is the shape SP4d-2b's D4 exists for: two attachments of tags at
/// different levels produce a different masking answer, so a resolution that
/// deduped by tag name — or a phase that inherited the resolver's order —
/// would silently report the wrong mask.
///
/// ⚠ SP4d-2b Task 5: the two policies used to carry DIFFERENT priorities (1 and
/// 0), to make "every other key point the other way". They are levelled now,
/// because a multi-priority tag candidate set is refused —
/// [`UndecidableCause::TagPriorityArbitrationNotModelled`] — and the test would
/// be asserting the refusal rather than the comparator. Nothing is lost: with
/// the priorities equal, `a-ancestor` still sorts before `z-self` by NAME, so
/// the match type is still the only key that can pick `z-self`, which is the
/// whole claim.
#[test]
fn mask_evaluators_are_ordered_by_tag_match_type_before_the_name() {
    let set = tagged(json!([
        tag_policy(
            1,
            "a-ancestor",
            1,
            json!(["a"]),
            mask_for_public("MASK_HASH")
        ),
        tag_policy(2, "z-self", 1, json!(["b"]), mask_for_public("MASK_NULL")),
    ]));
    let d = evaluate_tag_data_mask(
        &set,
        &request(
            "alice",
            Some(vec![
                tag("a", MatchType::Ancestor),
                tag("b", MatchType::Self_),
            ]),
        ),
        &live_tag_def(),
    );
    match d {
        MaskDecision::Masked(m) => assert_eq!(
            (m.policy_id, m.mask_type.as_deref()),
            (Some(2), Some("MASK_NULL")),
            "SELF sorts before ANCESTOR, and the match type outranks the name"
        ),
        other => panic!("expected a mask: {other:?}"),
    }
}

/// **ROW FILTER does NOT order by match type** — `NAME_COMPARATOR` never looks
/// at the tag. The same fixture as the mask test, with the policy types changed
/// to 2 and the priorities levelled: the `ANCESTOR` tag's policy wins, on its
/// name, exactly where the mask ordering chose the other one.
#[test]
fn row_filter_evaluators_ignore_the_tag_match_type() {
    let set = tagged(json!([
        tag_policy(
            1,
            "a-ancestor",
            2,
            json!(["a"]),
            row_filter_for_public("a = 1")
        ),
        tag_policy(2, "z-self", 2, json!(["b"]), row_filter_for_public("b = 1")),
    ]));
    let d = evaluate_tag_row_filter(
        &set,
        &request(
            "alice",
            Some(vec![
                tag("a", MatchType::Ancestor),
                tag("b", MatchType::Self_),
            ]),
        ),
        &live_tag_def(),
    );
    match d {
        RowFilterDecision::Filtered(f) => assert_eq!(
            (f.policy_id, f.filter_expr.as_str()),
            (Some(1), "a = 1"),
            "row-filter order is the policy name, not the tag's match type"
        ),
        other => panic!("expected a row filter: {other:?}"),
    }
}

/// **REWRITTEN, SP4d-2b Task 5.** This asserted that `NAME_COMPARATOR`'s first
/// key is priority DESC and that the alphabetically-later, higher-priority
/// policy therefore wins — `Filtered { policy_id: 2, "b = 1" }`.
///
/// The comparator claim is still true and is still what `sort_tag_candidates`
/// implements. What was NOT true is that the comparator is the only thing
/// priority reaches in this phase: the differential gate measured
/// `RangerDefaultPolicyEvaluator.evaluatePolicyItems` arbitrating a per-access-type
/// result map by `policyPriority` ACROSS tag evaluators, and this crate does not
/// model that. So a tag candidate set whose deciding members sit at more than one
/// priority now REFUSES, and this fixture is one.
///
/// The assertion is inverted rather than the fixture levelled, because a
/// row-filter test with equal priorities is already
/// `row_filter_evaluators_ignore_the_tag_match_type` just below — this one's only
/// content was the priority key, so the refusal is now its content.
#[test]
fn a_multi_priority_row_filter_candidate_set_refuses() {
    let set = tagged(json!([
        tag_policy(
            1,
            "a-ancestor",
            2,
            json!(["a"]),
            row_filter_for_public("a = 1")
        ),
        tag_policy(2, "z-self", 2, json!(["b"]), {
            let mut v = row_filter_for_public("b = 1");
            v["policyPriority"] = json!(1);
            v
        }),
    ]));
    let d = evaluate_tag_row_filter(
        &set,
        &request(
            "alice",
            Some(vec![tag("a", MatchType::Self_), tag("b", MatchType::Self_)]),
        ),
        &live_tag_def(),
    );
    match d {
        RowFilterDecision::Undecidable { why } => assert!(
            why.iter().any(|c| matches!(
                c,
                UndecidableCause::TagPriorityArbitrationNotModelled { priorities, .. }
                    if priorities.contains(&0) && priorities.contains(&1)
            )),
            "the refusal must name both priorities it could not arbitrate: {why:?}"
        ),
        other => panic!(
            "two deciding tag row-filter policies at different priorities must refuse, \
             not answer: {other:?}"
        ),
    }
}

/// The control for the refusal above, and the thing that stops it from being
/// "refuse whenever two tag policies decide": the SAME two policies at ONE
/// priority still answer, by the comparator.
#[test]
fn a_single_priority_row_filter_candidate_set_still_answers() {
    let set = tagged(json!([
        tag_policy(
            1,
            "a-ancestor",
            2,
            json!(["a"]),
            row_filter_for_public("a = 1")
        ),
        tag_policy(2, "z-self", 2, json!(["b"]), row_filter_for_public("b = 1")),
    ]));
    let d = evaluate_tag_row_filter(
        &set,
        &request(
            "alice",
            Some(vec![tag("a", MatchType::Self_), tag("b", MatchType::Self_)]),
        ),
        &live_tag_def(),
    );
    match d {
        RowFilterDecision::Filtered(f) => assert_eq!(
            (f.policy_id, f.filter_expr.as_str()),
            (Some(1), "a = 1"),
            "at one priority the NAME decides, and `a-ancestor` sorts first"
        ),
        other => panic!("expected a row filter: {other:?}"),
    }
}

/// **REWRITTEN, SP4d-2b Task 5, and the rewrite is a LOSS that is recorded
/// rather than hidden.**
///
/// This asserted `Allowed { policy_id: 1 }` — a higher-priority ALLOW carrying no
/// deny items DETERMINES (offsets 472-493), so the lower-priority deny is never
/// reached. **That answer is CORRECT**: the differential gate measured the real
/// 2.8.0 engine on this family of shapes and it answers Allowed by the
/// higher-priority allow.
///
/// It refuses now anyway, because the crate was right here for the wrong reason.
/// The old doc ended *"the tag loop, unlike the resource loop, compares no
/// priorities of its own, so the comparator is the only thing that can produce
/// this"* — and that is the sentence the gate falsified. Priority is arbitrated
/// again, after the comparator, in
/// `RangerDefaultPolicyEvaluator.evaluatePolicyItems`'s per-access-type result
/// map; with the allower's deny bucket non-empty (so it does NOT determine) the
/// crate answered `Denied` where Ranger answers `Allowed`, and the comparator
/// model cannot tell the two fixtures apart.
///
/// So the refusal covers a shape this crate answered correctly. **That is a
/// measured over-refusal**, named here so the follow-up task that models the
/// arbitration has the row to aim at:
///
/// | shape | Ranger (measured, 2.8.0) | this crate today |
/// |---|---|---|
/// | allow@1 without deny items, deny@0 | Allowed by the allow | REFUSES (was: Allowed ✓) |
/// | allow@1 WITH deny items, deny@0 | Allowed by the allow | REFUSES (was: Denied ✗) |
/// | allow@0, deny@0 | Denied by the deny | Denied ✓ |
/// | allow@0, deny@1 | Denied by the deny | Denied ✓ |
#[test]
fn a_multi_priority_access_candidate_set_refuses() {
    let higher_allow = tag_policy(1, "z-allow", 0, json!(["t"]), {
        let mut v = allow_select_for_public();
        v["policyPriority"] = json!(1);
        v
    });
    let lower_deny = tag_policy(2, "a-deny", 0, json!(["t"]), deny_select_for_public());
    let set = tagged(json!([higher_allow, lower_deny]));
    let d = evaluate_tag_policies(
        &set,
        &request("alice", Some(vec![tag("t", MatchType::Self_)])),
        &live_tag_def(),
    );
    match &d {
        TagDecision::Undecidable { why } => assert!(
            why.iter().any(|c| matches!(
                c,
                UndecidableCause::TagPriorityArbitrationNotModelled { priorities, .. }
                    if priorities.contains(&0) && priorities.contains(&1)
            )),
            "the refusal must name both priorities it could not arbitrate: {why:?}"
        ),
        other => panic!(
            "two deciding tag ACCESS policies at different priorities must refuse: {other:?}"
        ),
    }

    // The control: the SAME two policies at ONE priority answer, and answer
    // DENIED — the comparator puts the `hasDeny` policy first at equal priority,
    // which is the rule this phase does still reproduce.
    let level_allow = tag_policy(1, "z-allow", 0, json!(["t"]), allow_select_for_public());
    let level_deny = tag_policy(2, "a-deny", 0, json!(["t"]), deny_select_for_public());
    let level = tagged(json!([level_allow, level_deny]));
    let d = evaluate_tag_policies(
        &level,
        &request("alice", Some(vec![tag("t", MatchType::Self_)])),
        &live_tag_def(),
    );
    assert!(
        matches!(
            &d,
            TagDecision::Denied {
                policy_id: Some(2),
                ..
            }
        ),
        "at one priority the deny wins and the phase still answers: {d:?}"
    );
}

/// ...and at EQUAL priority `hasDeny` comes first, which is what stops an allow
/// hiding a same-priority deny.
///
/// The names run the OTHER way here on purpose — `a-allow` before `z-deny` —
/// because with the deny already first alphabetically the assertion would hold
/// under a name-only ordering too, and would be testing nothing.
#[test]
fn access_evaluators_put_deny_capable_policies_first_at_equal_priority() {
    let set = tagged(json!([
        tag_policy(1, "a-allow", 0, json!(["t"]), allow_select_for_public()),
        tag_policy(2, "z-deny", 0, json!(["t"]), deny_select_for_public()),
    ]));
    let d = evaluate_tag_policies(
        &set,
        &request("alice", Some(vec![tag("t", MatchType::Self_)])),
        &live_tag_def(),
    );
    assert!(
        matches!(
            d,
            TagDecision::Denied {
                policy_id: Some(2),
                ..
            }
        ),
        "hasDeny sorts first at equal priority: {d:?}"
    );
}

/// **An allow from a policy that HAS deny items does not DETERMINE**, so a
/// later deny still wins — offsets 472-493, `if (tagEvalResult.getIsAllowed()
/// && !evaluator.hasDeny()) setIsAccessDetermined(true)`.
///
/// This is the one rule in the tag loop that no ordering test can reach, and
/// dropping it turns a deny into an allow. The fixture puts the allowing policy
/// FIRST — equal priority, both `hasDeny`, and `a-both` sorts before `z-deny` —
/// so the deny is only reachable if the allow declines to determine.
#[test]
fn an_allow_from_a_deny_capable_policy_does_not_close_the_question() {
    let set = tagged(json!([
        tag_policy(
            1,
            "a-both",
            0,
            json!(["t"]),
            json!({
                // Present, so `hasDeny()` is true — but it names nobody in this
                // request, so it does not itself deny.
                "denyPolicyItems": [{
                    "users": ["nobody"],
                    "accesses": [{"type": "select", "isAllowed": true}],
                }],
                "policyItems": [{
                    "groups": ["public"],
                    "accesses": [{"type": "select", "isAllowed": true}],
                }],
            })
        ),
        tag_policy(2, "z-deny", 0, json!(["t"]), deny_select_for_public()),
    ]));
    let d = evaluate_tag_policies(
        &set,
        &request("alice", Some(vec![tag("t", MatchType::Self_)])),
        &live_tag_def(),
    );
    assert!(
        matches!(
            d,
            TagDecision::Denied {
                policy_id: Some(2),
                ..
            }
        ),
        "an allow recorded by a deny-capable policy must leave the question \
         open for the later deny: {d:?}"
    );
}

// ── the refusals this phase owes ─────────────────────────────────────────

/// `None` is "the caller did not resolve tags", never the empty set that
/// narrows — and with policies in the block there is no honest answer without
/// it.
#[test]
fn an_unresolved_tag_set_refuses() {
    let set = live_download();
    let d = evaluate_tag_policies(&set, &request("alice", None), &live_tag_def());
    assert_eq!(causes(&d), vec![UndecidableCause::TagsUnresolved], "{d:?}");
}

/// An EXPLICITLY empty set is the licence `getLikelyMatchPolicyEvaluators`
/// gives at its `isNotEmpty(tags)` branch: nothing to evaluate, and that is an
/// answer rather than a refusal.
#[test]
fn an_empty_resolved_tag_set_determines_nothing_without_refusing() {
    let set = live_download();
    let d = evaluate_tag_policies(&set, &request("alice", Some(Vec::new())), &live_tag_def());
    assert_eq!(d, TagDecision::NotDetermined, "{d:?}");
}

/// The measured trie/matcher divergence. `p*x` is parked in the trie at the
/// literal prefix `p`, so Ranger evaluates this policy for the tag `pii` while
/// `wildcardMatch("p*x", "pii")` is false — and the policy is a DENY, so
/// answering from the matcher alone would be a confident allow the engine
/// denies.
///
/// The control matters as much as the case: `p*`, whose trie key IS its
/// `starts_with` prefix, must still be answered.
#[test]
fn a_wildcard_tag_value_is_refused_because_the_trie_is_wider() {
    let refused = tagged(json!([tag_policy(
        7,
        "wildcard-deny",
        0,
        json!(["p*x"]),
        deny_select_for_public()
    )]));
    let d = evaluate_tag_policies(
        &refused,
        &request("alice", Some(vec![tag("pii", MatchType::Self_)])),
        &live_tag_def(),
    );
    assert_eq!(
        causes(&d),
        vec![UndecidableCause::TagResourceNotReproducible {
            policy_id: Some(7),
            policy_name: "wildcard-deny".to_string(),
            problem: TagResourceProblem::ValueNotTriePrefixExact("p*x".to_string()),
        }],
        "{d:?}"
    );

    let answered = tagged(json!([tag_policy(
        8,
        "prefix-deny",
        0,
        json!(["p*"]),
        deny_select_for_public()
    )]));
    let d = evaluate_tag_policies(
        &answered,
        &request("alice", Some(vec![tag("pii", MatchType::Self_)])),
        &live_tag_def(),
    );
    assert!(
        matches!(
            d,
            TagDecision::Denied {
                policy_id: Some(8),
                ..
            }
        ),
        "a trailing-`*` value keys the trie on exactly its starts_with prefix \
         and must still be answered: {d:?}"
    );
}

/// `isExcludes` lands in the trie's `inheritedEvaluators`, which the retriever
/// unions into EVERY lookup — measured: a policy excluding `pii` came back as a
/// candidate for the tag `pii` itself, where the matcher answers `NONE`.
#[test]
fn an_excludes_tag_resource_is_refused() {
    let mut policy = tag_policy(9, "excl", 0, json!(["pii"]), deny_select_for_public());
    policy["resources"]["tag"]["isExcludes"] = json!(true);
    let set = tagged(json!([policy]));
    let d = evaluate_tag_policies(
        &set,
        &request("alice", Some(vec![tag("pii", MatchType::Self_)])),
        &live_tag_def(),
    );
    assert_eq!(
        causes(&d),
        vec![UndecidableCause::TagResourceNotReproducible {
            policy_id: Some(9),
            policy_name: "excl".to_string(),
            problem: TagResourceProblem::Excludes,
        }],
        "{d:?}"
    );
}

/// A tag policy naming a level the tag service-def does not have. The retriever
/// consults only the tries for keys the REQUEST carries, and a tag request
/// carries exactly `tag` — so Ranger still evaluates this policy while this
/// crate's `init` finds no hierarchy holding both keys and answers `NONE`.
#[test]
fn a_tag_policy_naming_another_resource_is_refused() {
    let mut policy = tag_policy(
        10,
        "two-levels",
        0,
        json!(["pii"]),
        deny_select_for_public(),
    );
    policy["resources"]["column"] = json!({"values": ["*"]});
    let set = tagged(json!([policy]));
    let d = evaluate_tag_policies(
        &set,
        &request("alice", Some(vec![tag("pii", MatchType::Self_)])),
        &live_tag_def(),
    );
    assert_eq!(
        causes(&d),
        vec![UndecidableCause::TagResourceNotReproducible {
            policy_id: Some(10),
            policy_name: "two-levels".to_string(),
            problem: TagResourceProblem::NotOnlyTheTagResource,
        }],
        "{d:?}"
    );
}

/// The refusal sweep is gated on there being an ADMITTED tag, because Ranger
/// runs nothing at all without one: an unreproducible policy that no tag could
/// have reached must not cost an answer.
#[test]
fn an_unreproducible_policy_no_tag_could_reach_costs_no_answer() {
    let set = tagged(json!([tag_policy(
        7,
        "wildcard-deny",
        0,
        json!(["p*x"]),
        deny_select_for_public()
    )]));
    for tags in [
        Vec::new(),
        vec![tag("pii", MatchType::Descendant)],
        vec![tag("pii", MatchType::None)],
    ] {
        let d = evaluate_tag_policies(&set, &request("alice", Some(tags.clone())), &live_tag_def());
        assert_eq!(d, TagDecision::NotDetermined, "{tags:?} -> {d:?}");
    }
}

/// A policy this decision's TYPE would not evaluate cannot refuse it either:
/// the three evaluator lists are disjoint, so an unreproducible MASK policy is
/// invisible to an access question.
#[test]
fn an_unreproducible_policy_of_another_type_costs_no_answer() {
    let set = tagged(json!([tag_policy(
        11,
        "wildcard-mask",
        1,
        json!(["p*x"]),
        mask_for_public("MASK")
    )]));
    let d = evaluate_tag_policies(
        &set,
        &request("alice", Some(vec![tag("pii", MatchType::Self_)])),
        &live_tag_def(),
    );
    assert_eq!(d, TagDecision::NotDetermined, "{d:?}");
}

// ── fix round 1: the inputs the phase did not look at ────────────────────

/// **CRITICAL 1.** `isRecursive: true` is the same trie widening as a wildcard
/// value, through a field the first version of the sweep did not read.
///
/// MEASURED, byte-identical in both jars: `RangerResourceTrie.insert` ends
/// `if (isWildcard || isRecursive) addWildcardEvaluator(...)`, and the caller
/// passes `resource.getIsRecursive()`. A wildcard evaluator at node `N` is
/// returned for every request value with prefix `N`, while
/// `RangerDefaultResourceMatcher` contains zero references to `isRecursive` in
/// either jar. Live on 2.5.0: a policy on `pii` with `isRecursive: true` is a
/// candidate for the tag `piix`, where `getMatchType` is `NONE`.
///
/// Before the fix this request was `NotDetermined` with **no cause at all**,
/// which composes into an `Allowed` where Ranger denies.
#[test]
fn a_recursive_tag_resource_is_refused() {
    let mut policy = tag_policy(11, "rec", 0, json!(["pii"]), deny_select_for_public());
    policy["resources"]["tag"]["isRecursive"] = json!(true);
    let set = tagged(json!([policy]));
    let d = evaluate_tag_policies(
        &set,
        &request("alice", Some(vec![tag("piix", MatchType::Self_)])),
        &live_tag_def(),
    );
    assert_eq!(
        causes(&d),
        vec![UndecidableCause::TagResourceNotReproducible {
            policy_id: Some(11),
            policy_name: "rec".to_string(),
            problem: TagResourceProblem::Recursive,
        }],
        "{d:?}"
    );

    // The tag the policy literally names is refused too — the refusal is the
    // FLAG, not a per-request comparison. That is the documented over-refusal.
    let d = evaluate_tag_policies(
        &set,
        &request("alice", Some(vec![tag("pii", MatchType::Self_)])),
        &live_tag_def(),
    );
    assert!(matches!(d, TagDecision::Undecidable { .. }), "{d:?}");
}

/// **IMPORTANT 1.** `tag_match_is_admitted` is written for one request shape —
/// `SELF` scope, concrete access type. Under `isAccessTypeAny()`
/// `RangerDefaultPolicyEvaluator.evaluate` takes a different branch where
/// `DESCENDANT` IS admitted (2.5.0 offsets 260-284, 2.8.0 268-331), so the
/// phase's own admitted set stops being the right one.
///
/// `PseudoAccessType` already existed — but only in `request_preamble`, which
/// this phase deliberately does not run.
#[test]
fn a_pseudo_access_type_refuses_because_it_changes_the_admitted_set() {
    let set = live_download();
    for pseudo in ["_any", "_admin", "_super_user"] {
        let req = AccessRequest {
            access_type: pseudo.to_string(),
            ..request("alice", Some(vec![tag("tag", MatchType::Descendant)]))
        };
        let d = evaluate_tag_policies(&set, &req, &live_tag_def());
        assert_eq!(
            causes(&d),
            vec![UndecidableCause::PseudoAccessType(pseudo.to_string())],
            "{pseudo} must refuse: {d:?}"
        );
    }
    // The control: with a concrete access type the same DESCENDANT tag is an
    // answer, not a refusal — so the refusal is the access type, not the tag.
    assert_eq!(
        evaluate_tag_policies(
            &set,
            &request("alice", Some(vec![tag("tag", MatchType::Descendant)])),
            &live_tag_def()
        ),
        TagDecision::NotDetermined
    );
}

/// **IMPORTANT 2.** An `Unreadable` block leaves `tag_policies` EMPTY, which
/// from `of_kind` alone is indistinguishable from "carries none of this type".
/// "I could not read the policies" must never look like "the policies decided
/// nothing".
#[test]
fn an_unreadable_tag_policies_block_refuses() {
    let set: ServicePolicySet = serde_json::from_value(json!({
        "serviceName": "sp3c2-trino-live",
        "policies": [],
        "serviceDef": {"name": "trino"},
        // `policies` is an object, not a list — a shape the reader does not
        // understand, so it has NOT been shown inert.
        "tagPolicies": {
            "serviceName": "t", "serviceDef": {"name": "tag"},
            "policies": {"unexpected": "shape"},
        },
    }))
    .expect("parse envelope");
    assert!(set.tag_policies.is_empty());
    assert!(set
        .unmodelled
        .contains(&Unmodelled::TagPolicies(TagPolicyBlock::Unreadable)));

    for d in [
        evaluate_tag_policies(
            &set,
            &request("alice", Some(vec![tag("pii", MatchType::Self_)])),
            &live_tag_def(),
        ),
        evaluate_tag_policies(&set, &request("alice", Some(Vec::new())), &live_tag_def()),
    ] {
        assert_eq!(
            causes(&d),
            vec![UndecidableCause::EnvelopeConstruct(
                Unmodelled::TagPolicies(TagPolicyBlock::Unreadable)
            )],
            "{d:?}"
        );
    }
}

/// The other three envelope constructs, which the first version left to a
/// caller that may not run `request_preamble`.
///
/// `securityZones` is the one that corrupts this phase's own answer:
/// `evaluateTagPolicies` has its own zone gate, and in a non-default zone it
/// evaluates ONLY tag policies whose `zoneName` matches the request's — so a
/// zone-less tag policy this crate evaluates is one Ranger skips.
#[test]
fn the_other_envelope_constructs_refuse_too() {
    let cases = [
        (
            json!({"securityZones": {"z": {"resources": []}}}),
            Unmodelled::SecurityZones,
        ),
        (
            json!({"policyDeltas": [{"id": 1}]}),
            Unmodelled::PolicyDeltas,
        ),
        (
            json!({"serviceConfig": {"ranger.plugin.super.users": "root"}}),
            Unmodelled::ServiceConfig,
        ),
    ];
    for (extra, expected) in cases {
        let mut body = json!({
            "serviceName": "sp3c2-trino-live",
            "policies": [],
            "serviceDef": {"name": "trino"},
            "tagPolicies": {
                "serviceName": "t",
                "serviceDef": live_download_json()["tagPolicies"]["serviceDef"],
                "policies": [tag_policy(1, "p", 0, json!(["pii"]), deny_select_for_public())],
            },
        });
        for (k, v) in extra.as_object().expect("object") {
            body[k] = v.clone();
        }
        let set: ServicePolicySet = serde_json::from_value(body).expect("parse envelope");
        let d = evaluate_tag_policies(
            &set,
            &request("alice", Some(vec![tag("pii", MatchType::Self_)])),
            &live_tag_def(),
        );
        assert!(
            causes(&d).contains(&UndecidableCause::EnvelopeConstruct(expected.clone())),
            "{expected:?} must refuse: {d:?}"
        );
    }
}

/// **MINOR 3.** `NAME_COMPARATOR` has no policy-id key, so two distinct
/// policies tying on (match type, priority, hasDeny, name) are ordered by
/// Ranger's repository order and by this crate's id stand-in — which need not
/// agree. The tie is representable: `policy.authz_policies` is
/// `UNIQUE (service_id, name)` on the COLUMN, while the download's `name` comes
/// from the stored body, which `enabled_policies_for_service` never rewrites.
#[test]
fn two_mask_policies_tied_on_every_key_ranger_has_refuse() {
    let set = tagged(json!([
        tag_policy(
            1,
            "same-name",
            1,
            json!(["a"]),
            mask_for_public("MASK_HASH")
        ),
        tag_policy(
            2,
            "same-name",
            1,
            json!(["b"]),
            mask_for_public("MASK_NULL")
        ),
    ]));
    let d = evaluate_tag_data_mask(
        &set,
        &request(
            "alice",
            Some(vec![tag("a", MatchType::Self_), tag("b", MatchType::Self_)]),
        ),
        &live_tag_def(),
    );
    assert_eq!(
        d,
        MaskDecision::Undecidable {
            why: vec![UndecidableCause::EvalOrderAmbiguous {
                policy_ids: vec![Some(1), Some(2)],
                policy_names: vec!["same-name".to_string(), "same-name".to_string()],
            }]
        },
        "{d:?}"
    );

    // The control: give them different names and the comparator orders them,
    // so the refusal is the TIE and not the pair.
    let named = tagged(json!([
        tag_policy(1, "a-first", 1, json!(["a"]), mask_for_public("MASK_HASH")),
        tag_policy(2, "z-second", 1, json!(["b"]), mask_for_public("MASK_NULL")),
    ]));
    let d = evaluate_tag_data_mask(
        &named,
        &request(
            "alice",
            Some(vec![tag("a", MatchType::Self_), tag("b", MatchType::Self_)]),
        ),
        &live_tag_def(),
    );
    assert!(
        matches!(d, MaskDecision::Masked(ref m) if m.policy_id == Some(1)),
        "{d:?}"
    );

    // ...and the SAME policy reached through two tags of the same match type
    // ties with itself, which is not an ambiguity.
    let one = tagged(json!([tag_policy(
        1,
        "only",
        1,
        json!(["a", "b"]),
        mask_for_public("MASK")
    )]));
    let d = evaluate_tag_data_mask(
        &one,
        &request(
            "alice",
            Some(vec![tag("a", MatchType::Self_), tag("b", MatchType::Self_)]),
        ),
        &live_tag_def(),
    );
    assert!(
        matches!(d, MaskDecision::Masked(ref m) if m.policy_id == Some(1)),
        "one policy under two tags is not a tie: {d:?}"
    );
}

/// **Review question 3.** A deny-capable mask policy that yields is the point
/// where Ranger can start reporting one policy's id with another's mask, because
/// its `updateAccessResult` writes the mask fields unconditionally and the
/// identity only `if (!result.getIsAllowed())`.
///
/// The first version documented this instead of refusing it, because egret's
/// compiler emits neither bucket. Nothing ENFORCES that: `create_policy` /
/// `update_policy` take `Json<Value>` and `enabled_policies_for_service`
/// re-serves the stored body verbatim.
#[test]
fn a_deny_capable_mask_policy_that_yields_is_refused() {
    let mut policy = tag_policy(
        3,
        "mask-with-deny",
        1,
        json!(["pii"]),
        mask_for_public("MASK"),
    );
    policy["denyPolicyItems"] = json!([{
        "users": ["nobody"],
        "accesses": [{"type": "select", "isAllowed": true}],
    }]);
    let set = tagged(json!([policy]));
    let d = evaluate_tag_data_mask(
        &set,
        &request("alice", Some(vec![tag("pii", MatchType::Self_)])),
        &live_tag_def(),
    );
    assert_eq!(
        d,
        MaskDecision::Undecidable {
            why: vec![UndecidableCause::TagSubPolicyIsDenyCapable {
                policy_id: Some(3),
                policy_name: "mask-with-deny".to_string(),
            }]
        },
        "{d:?}"
    );

    // `isDenyAllElse` reaches `hasDeny()` by the other arm.
    let mut policy = tag_policy(
        4,
        "mask-deny-all",
        1,
        json!(["pii"]),
        mask_for_public("MASK"),
    );
    policy["isDenyAllElse"] = json!(true);
    let set = tagged(json!([policy]));
    assert!(
        matches!(
            evaluate_tag_data_mask(
                &set,
                &request("alice", Some(vec![tag("pii", MatchType::Self_)])),
                &live_tag_def()
            ),
            MaskDecision::Undecidable { .. }
        ),
        "isDenyAllElse must reach hasDeny() too"
    );

    // The control: a deny-capable mask policy that does NOT yield cannot start
    // the split, so it is not refused.
    let mut policy = tag_policy(
        5,
        "mask-with-deny",
        1,
        json!(["pii"]),
        mask_for_public("MASK"),
    );
    policy["denyPolicyItems"] = json!([{
        "users": ["nobody"],
        "accesses": [{"type": "select", "isAllowed": true}],
    }]);
    policy["dataMaskPolicyItems"] = json!([{
        "users": ["someone-else"],
        "accesses": [{"type": "select", "isAllowed": true}],
        "dataMaskInfo": {"dataMaskType": "MASK"},
    }]);
    let set = tagged(json!([policy]));
    assert_eq!(
        evaluate_tag_data_mask(
            &set,
            &request("alice", Some(vec![tag("pii", MatchType::Self_)])),
            &live_tag_def()
        ),
        MaskDecision::NoMask,
        "a deny-capable policy whose item does not match cannot split anything"
    );
}

// ── fix round 2: the early return that routed around the refusals ────────

/// An envelope with `extra` merged in, and a `tagPolicies` block carrying
/// exactly `block_policies` under the captured tag def.
fn envelope_with(extra: Value, block_policies: Value) -> ServicePolicySet {
    let mut body = json!({
        "serviceName": "sp3c2-trino-live",
        "policies": [],
        "serviceDef": {"name": "trino"},
        "tagPolicies": {
            "serviceName": "sp3c2-trino-live_tag",
            "serviceDef": live_download_json()["tagPolicies"]["serviceDef"],
            "policies": block_policies,
        },
    });
    for (k, v) in extra.as_object().expect("extra is an object") {
        body[k] = v.clone();
    }
    serde_json::from_value(body).expect("parse envelope")
}

/// **FIX ROUND 2.** The envelope sweep used to sit BELOW the
/// `of_kind.is_empty()` return, so `PolicyDeltas` could not fire in the one
/// state it exists to catch.
///
/// `Unmodelled::PolicyDeltas` means the download is incremental, so `policies`
/// is "partial (or absent)" — an EMPTY tag block is not an edge case for that
/// refusal, it is its signature. The refusal was routed around by exactly the
/// condition it refuses, which is the C1/I1/I2 shape one layer up.
///
/// All four constructs are asserted at an empty block, not just the one that
/// was measured wrong, because the bypass was structural rather than
/// per-construct.
#[test]
fn an_envelope_construct_refuses_even_with_no_tag_policies_to_evaluate() {
    let cases = [
        (
            json!({"policyDeltas": [{"id": 1}]}),
            Unmodelled::PolicyDeltas,
        ),
        (
            json!({"serviceConfig": {"ranger.plugin.super.users": "root"}}),
            Unmodelled::ServiceConfig,
        ),
        (
            json!({"securityZones": {"z": {"resources": []}}}),
            Unmodelled::SecurityZones,
        ),
    ];
    for (extra, expected) in cases {
        let set = envelope_with(extra, json!([]));
        assert!(
            set.tag_policies.is_empty(),
            "the block must be EMPTY, or this tests the wrong branch"
        );
        let d = evaluate_tag_policies(
            &set,
            &request("alice", Some(vec![tag("pii", MatchType::Self_)])),
            &live_tag_def(),
        );
        assert_eq!(
            causes(&d),
            vec![UndecidableCause::EnvelopeConstruct(expected.clone())],
            "{expected:?} must refuse with no tag policies present: {d:?}"
        );
    }
}

/// The control the reorder must not cost: a block with zero policies and a
/// clean envelope is still an ANSWER.
///
/// This is the tier every auto-bound Trino profile lands in —
/// `bind_trino_profile` creates `{name}_tag` with zero policies — so turning it
/// into a refusal would take an answer away from every such profile and buy
/// nothing.
#[test]
fn an_empty_block_with_a_clean_envelope_is_still_an_answer() {
    let set = envelope_with(json!({}), json!([]));
    for tags in [
        None,
        Some(Vec::new()),
        Some(vec![tag("pii", MatchType::Self_)]),
    ] {
        let d = evaluate_tag_policies(&set, &request("alice", tags.clone()), &live_tag_def());
        assert_eq!(d, TagDecision::NotDetermined, "{tags:?} -> {d:?}");
    }
}

/// The other half of the reorder's ruling: the pseudo-access-type refusal stays
/// BELOW the `of_kind` return on purpose.
///
/// With no evaluator to run, Ranger's `isAccessTypeAny` branch is unreachable
/// and neither side determines anything — so refusing there would cost every
/// tagless download an answer for nothing. Pinned so that "move it up for
/// symmetry" is a failing test rather than a plausible tidy-up.
#[test]
fn a_pseudo_access_type_costs_nothing_when_there_is_nothing_to_evaluate() {
    let set = envelope_with(json!({}), json!([]));
    let req = AccessRequest {
        access_type: "_any".to_string(),
        ..request("alice", Some(vec![tag("pii", MatchType::Descendant)]))
    };
    assert_eq!(
        evaluate_tag_policies(&set, &req, &live_tag_def()),
        TagDecision::NotDetermined
    );

    // ...and it still refuses the moment there IS something to evaluate.
    let live = envelope_with(
        json!({}),
        json!([tag_policy(
            1,
            "p",
            0,
            json!(["pii"]),
            deny_select_for_public()
        )]),
    );
    assert_eq!(
        causes(&evaluate_tag_policies(&live, &req, &live_tag_def())),
        vec![UndecidableCause::PseudoAccessType("_any".to_string())]
    );
}

// ── the tag set is a set, and its identity includes the match type ───────

/// `RangerTagForEval`'s `equals`/`hashCode` include `matchType`, so the tag `a`
/// reached at two hierarchy levels is TWO members of Ranger's set — and a
/// resolution or a phase that collapsed them by NAME would report a different
/// mask.
///
/// **This pins the `matchType` component only, and it is not the whole
/// identity.** SP4d-2b Task 4 widened the key to all five components Ranger
/// compares (`type`, `attributes`, `matchType`, `options`, `validityPeriods`);
/// the other four are pinned in `eval::tests::
/// the_tag_identity_is_all_five_components_ranger_compares`, as a unit test,
/// because no verdict this crate can return distinguishes them — see that
/// test's own doc for why, rather than reading its absence here as a gap.
///
/// The fixture is built so that dropping one of the two `a` members is the only
/// thing that can change the answer. `z-on-a` is reachable at `SELF` and at
/// `ANCESTOR`; `a-on-b` only at `ANCESTOR`, and it sorts FIRST under
/// `NAME_COMPARATOR`. So:
///
/// * keeping both `a` members ⇒ the rank-0 (`SELF`) band holds `z-on-a` alone
///   ⇒ `MASK_HASH`;
/// * collapsing them to the `ANCESTOR` one ⇒ the rank-0 band is empty, rank 2
///   holds both, and the name decides ⇒ `MASK_NULL`.
#[test]
fn the_tag_set_s_identity_is_the_name_and_the_match_type() {
    let set = tagged(json!([
        tag_policy(1, "z-on-a", 1, json!(["a"]), mask_for_public("MASK_HASH")),
        tag_policy(2, "a-on-b", 1, json!(["b"]), mask_for_public("MASK_NULL")),
    ]));
    let two_levels = request(
        "alice",
        Some(vec![
            tag("a", MatchType::Ancestor),
            tag("a", MatchType::Self_),
            tag("b", MatchType::Ancestor),
        ]),
    );
    let d = evaluate_tag_data_mask(&set, &two_levels, &live_tag_def());
    let both_kept = match d {
        MaskDecision::Masked(m) => m,
        other => panic!("expected a mask: {other:?}"),
    };
    assert_eq!(
        (both_kept.policy_id, both_kept.mask_type.as_deref()),
        (Some(1), Some("MASK_HASH")),
        "the SELF member of `a` must survive alongside its ANCESTOR member"
    );

    // ...and the collapse this guards against really would answer differently,
    // so the assertion above is not true by accident.
    let name_deduped = request(
        "alice",
        Some(vec![
            tag("a", MatchType::Ancestor),
            tag("b", MatchType::Ancestor),
        ]),
    );
    assert!(
        matches!(
            evaluate_tag_data_mask(&set, &name_deduped, &live_tag_def()),
            MaskDecision::Masked(ref m) if m.policy_id == Some(2)
        ),
        "the fixture must distinguish the two, or it tests nothing"
    );

    // A duplicate that IS a duplicate — same name AND same match type — is one
    // member, which is the other half of `RangerTagForEval`'s identity.
    let with_duplicate = request(
        "alice",
        Some(vec![
            tag("a", MatchType::Ancestor),
            tag("a", MatchType::Self_),
            tag("a", MatchType::Self_),
            tag("b", MatchType::Ancestor),
        ]),
    );
    assert_eq!(
        evaluate_tag_data_mask(&set, &with_duplicate, &live_tag_def()),
        MaskDecision::Masked(both_kept),
        "a duplicate (name, matchType) is one member, not two"
    );
}
