//! SP4d-2a Task 1: the `tagPolicies` refusal, narrowed to the tier that can
//! actually decide.
//!
//! # The fixture is captured, not invented
//!
//! `tests/fixtures/tag-policies-live.json` is a byte-for-byte copy of what a
//! live Trino plugin holds on disk:
//!
//! ```text
//! kubectl --context kind-egret exec -n egret-engines \
//!   sp3c2-trino-live-coordinator-855f5cc7bb-lzpg9 -- \
//!   cat /tmp/ranger/sp3c2-trino-live/policycache/trino_sp3c2-trino-live.json
//! ```
//!
//! Captured 2026-09-29: 37049 bytes, sha256
//! `5b92a23d1ab4ba0c7afc76a240f7ac1ca672f3116be2ca01cab8aa241bc29988`. Its
//! envelope is service 52 (`sp3c2-trino-live`) at `policyVersion` 116 with 31
//! resource policies, and its `tagPolicies` block is tag service 51
//! (`sp3c2-trino-live_tag`) at `policyVersion` 22 with **three** policies:
//!
//! | id | `policyType` | `resources.tag.values` | buckets |
//! |----|--------------|------------------------|---------|
//! | 8799 | 1 (mask) | `["pii"]` | 1 `dataMaskPolicyItems` |
//! | 8805 | 2 (row filter) | `["pii"]` | 1 `rowFilterPolicyItems` |
//! | 8839 | 0 (ACCESS) | `["tag"]` | 1 `denyPolicyItems`, 1 `denyExceptions` |
//!
//! Note for anyone re-capturing it: the sibling file
//! `trino_<service>_tag.json` in the same directory is **not** this. That one
//! is the tag STORE the enricher caches (`tags` / `serviceResources` /
//! `resourceToTagIds`, 2481 bytes here) — it carries no policies at all. The
//! tag POLICIES live inside the main policy cache, under `tagPolicies`.
//!
//! A fixture drawn from the same imagination as the code inherits its blind
//! spots, and the measurement that mattered most here came out of the capture
//! rather than out of the plan: the live download's `serviceDef` carries
//! `contextEnrichers`, while the repo's older capture
//! (`tests/ranger/fixtures/service-policies.json`) and the tag def embedded in
//! this very fixture both **omit the key entirely**. That is why
//! [`ServiceDef::has_tag_enricher`] is three-valued — see
//! `an_absent_context_enrichers_key_is_unknown_not_absent` below.
//!
//! # What was measured in the jars
//!
//! Every claim below was checked with `javap -c` against
//! `ranger-plugins-common` **2.5.0** (the version `trinodb/trino:470` loads)
//! and **2.8.0** (`starrocks/fe-ubuntu:4.1-latest`). Both
//! `RangerPolicyEngineImpl.evaluateTagPolicies` bodies are 256 instructions and
//! agree on every offset and branch target up to 604. There are exactly
//! **three** differences, none behavioural: `commons-lang` → `commons-lang3` in
//! `StringUtils.isNotEmpty`/`equals` at offsets 214 and 285, and an `ldc` →
//! `ldc_w` at 617 inside the trailing debug-log block, which shifts the tail
//! one byte so the final `ifeq`/`return` land at 668 in 2.8.0 against 667 in
//! 2.5.0. Every offset cited below is identical in both jars.
//!
//! * **Tag policies run FIRST.**
//!   `evaluatePoliciesForOneAccessTypeNoAudit` calls `evaluateTagPolicies` at
//!   offset 176, and only reaches the resource-policy loop
//!   (`hasResourcePolicies` at 343, `getLikelyMatchPolicyEvaluators` at 401)
//!   afterwards. The tag loop calls the full
//!   `RangerPolicyEvaluator.evaluate` (offset 467) and copies a determined
//!   result onto the shared result at 531 — so a tag policy can DENY before
//!   any resource policy is consulted.
//!
//! * **An empty block is inert.** `evaluateTagPolicies` computes `evaluators`
//!   at offsets 109-129 —
//!   `RangerPolicyRepository.getLikelyMatchPolicyEvaluators`, or `null` if the
//!   tag repository is itself null — then branches
//!   `CollectionUtils.isNotEmpty(evaluators)` / `ifeq 582` at 133-136. The
//!   only writes to the shared result (offsets 458, 508, 531, 557) are inside
//!   the loop the branch skips.
//!
//!   For a *realistic* empty block the live path is the empty LIST, not the
//!   null repository: `PolicyEngine.<init>` builds the tag repository whenever
//!   `tagPolicies != null`, `getServiceName()` is non-empty and
//!   `getServiceDef() != null` (offsets 471-500, **no `getPolicies()` call in
//!   the gate**). `isNotEmpty` is false for empty and for null alike, so both
//!   paths reach the same exit.
//!
//!   One correction to how this is usually stated: offset 582 is not a bare
//!   `return`. It reads `result.getIsAllowed()`, branches `ifeq 596` at 587,
//!   and on fall-through calls `setIsAccessDetermined(true)` — the
//!   `invokevirtual` at **593**; 590 is the `aload 5` opening the guarded
//!   block. That write cannot change anything on this path: the result comes
//!   from `createAccessResult` at offset 112 of the caller with `isAllowed`
//!   false, and the one block that can flip it first — the super-user
//!   short-circuit at caller offsets 133-165 — already sets
//!   `setIsAccessDetermined(true)` itself at 143. So the write is either
//!   unreached or idempotent, and the block is inert either way.
//!
//!   `ServicePolicies$TagPolicies` also carries a `serviceConfig` map in both
//!   jars, so an empty block could in principle smuggle one. Safe: its only
//!   call site is `RangerPolicyRepository` offset 279 → `buildAuditPolicyEvaluators`
//!   at 282. The super-user `RangerPolicyEngineImpl$ServiceConfig` is built
//!   from the TOP-LEVEL `ServicePolicies.getServiceConfig()` at 125-136, never
//!   from the tag block.
//!
//! * **No resolved tags ⇒ no evaluators.**
//!   `RangerPolicyRepository.getLikelyMatchPolicyEvaluators(request, tags,
//!   policyType, date)` opens `ret = Collections.EMPTY_LIST` (offset 0) and
//!   branches `isNotEmpty(tags)` / `ifeq 333` at 6-9, returning that empty
//!   list. Identical in both jars.
//!
//! * **The tag set comes from the enrichers, not from the engine type.** The
//!   set at offset 104 is
//!   `RangerAccessRequestUtil.getRequestTagsFromContext(request.getContext())`,
//!   and the only thing in `ranger-plugins-common` that writes that context
//!   key is `RangerTagEnricher.enrich`, via
//!   `RangerAccessRequestUtil.setRequestTagsInContext` (offset 199 of
//!   `enrich`). Each `RangerPolicyRepository` instantiates enrichers from its
//!   OWN `serviceDef`'s `contextEnrichers`. So the gate is the DEFS, never
//!   [`EngineType`]: egret's `trino.json` declares a `RangerTagEnricher` and
//!   its `starrocks.json` has `contextEnrichers: []`, and the day that one
//!   field changes an `EngineType::StarRocks` test would go silently wrong.
//!
//! * **There are TWO defs, and their enricher lists are UNIONED.** This is
//!   finding I-1 from the Task 1 review, and it is the reason the gate asks
//!   both. The full measured chain is on
//!   [`egret_ranger_eval::eval`]'s `tag_policies_can_decide` and is exercised
//!   by `the_block_s_own_service_def_can_supply_the_enricher` below; the short
//!   version is that `PolicyEngine.<init>` builds a second repository from
//!   `tagPolicies.getServiceDef()` and concatenates its enrichers with the
//!   resource repository's (offsets 619-702), and Ranger's own stock
//!   `ranger-servicedef-tag.json` — shipped in both jars — declares the tag
//!   enricher.

use std::collections::{BTreeMap, BTreeSet};

use egret_ranger_eval::eval::{
    evaluate, evaluate_data_mask, evaluate_row_filter, AccessRequest, Decision, EngineConfig,
    MaskDecision, ResolvedTag, RowFilterDecision, TagServiceResource, UndecidableCause,
    UnenrichedTags,
};
use egret_ranger_eval::model::{ServicePolicySet, TagPolicyBlock, Unmodelled};
use egret_ranger_eval::resource::{MatchType, RequestResource};
use egret_ranger_eval::servicedef::ServiceDef;
use serde_json::{json, Value};

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

// ── fixtures and helpers ─────────────────────────────────────────────────

/// The captured live download (see the module doc).
fn live_download_json() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/tag-policies-live.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("read live tag fixture"))
        .expect("parse live tag fixture")
}

/// The captured `tagPolicies` block, verbatim — three real policies.
fn live_tag_policies() -> Value {
    live_download_json()["tagPolicies"].clone()
}

fn trino_def_json() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../egret-policy/src/service-defs/trino.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("read trino service-def"))
        .expect("parse trino service-def")
}

fn starrocks_def_json() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../egret-policy/src/service-defs/starrocks.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("read starrocks service-def"))
        .expect("parse starrocks service-def")
}

fn trino_def() -> ServiceDef {
    ServiceDef::parse(&trino_def_json())
}

/// Apache Ranger's own stock `tag` service-def, extracted verbatim from
/// `service-defs/ranger-servicedef-tag.json` inside
/// `ranger-plugins-common-2.5.0.jar` (2449 bytes, sha256
/// `0bd25d688a15e5578a5dc15c697d4743d51d53c75dc995bd8ac11ad284d211f0`). The
/// 2.8.0 jar's copy differs only by dropping the `enums` key (2411 bytes,
/// sha256 `1dde75b3d0645ffae1644dc7ea34aa97af266d4c492e45bb974631396d54fc99`)
/// and declares a byte-identical `contextEnrichers`.
///
/// It is taken from the jar rather than written here on purpose: this fixture
/// exists to anchor the I-1 test to Ranger's real shipped artefact rather than
/// to anyone's reading of it.
fn stock_tag_def_json() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/ranger-servicedef-tag-stock-2.5.0.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("read stock tag service-def"))
        .expect("parse stock tag service-def")
}

/// A `trino` resource def that explicitly declares NO context enrichers — the
/// shape egret's `starrocks.json` has, and the only shape that can license the
/// enricher narrowing.
fn def_without_enrichers() -> ServiceDef {
    let mut def_json = trino_def_json();
    def_json["contextEnrichers"] = json!([]);
    ServiceDef::parse(&def_json)
}

/// The live captured `tagPolicies` block with its `serviceDef`'s
/// `contextEnrichers` replaced by `enrichers`. The three policies stay exactly
/// as captured; only the def under test changes.
fn live_tag_policies_with_enrichers(enrichers: Value) -> Value {
    let mut block = live_tag_policies();
    block["serviceDef"]["contextEnrichers"] = enrichers;
    block
}

/// An envelope with one plain allow policy, plus whatever `extra` top-level
/// keys the case under test needs.
fn envelope(extra: Value) -> ServicePolicySet {
    let mut body = json!({
        "serviceName": "trino",
        "policies": [{
            "id": 1, "name": "p", "service": "trino", "policyType": 0,
            "resources": {
                "catalog": {"values": ["hive"]},
                "schema":  {"values": ["sales"]},
                "table":   {"values": ["orders"]},
                "column":  {"values": ["*"]},
            },
            "policyItems": [{
                "users": ["alice"],
                "accesses": [{"type": "select", "isAllowed": true}],
            }],
        }],
    });
    for (k, v) in extra.as_object().expect("extra is an object") {
        body[k] = v.clone();
    }
    serde_json::from_value(body).expect("parse envelope")
}

/// A request for `hive.sales.orders.id`, the shape a Trino column check has.
/// `resolved_tags` is `None` — "the caller did not resolve tags".
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
        resolved_tags: None,
    }
}

fn no_superusers() -> EngineConfig {
    EngineConfig::without_super_users()
}

fn causes(d: &Decision) -> Vec<UndecidableCause> {
    match d {
        Decision::Undecidable { why } => why.clone(),
        _ => Vec::new(),
    }
}

/// **The instrument, and what SP4d-2b Task 3 changed about it.**
///
/// Until Task 3 the composed evaluator REFUSED a `tagPolicies` block it could
/// not show inert, and every test below asserted that refusal by naming the
/// block's policy count. Task 3 EVALUATES the block instead, so that refusal is
/// gone and "the gate let the tag phase through" needs a different observable.
///
/// It has one, and a sharper one: every test below hands the evaluator
/// `request()`, whose `resolved_tags` is `None`. `tag_candidates` refuses that
/// with [`UndecidableCause::TagsUnresolved`] — "the caller did not resolve",
/// which is never the empty set. So a `TagsUnresolved` and nothing else means
/// the phase RAN, and an answer means the gate narrowed. Each test's claim is
/// unchanged; what it now proves is that the phase ran, rather than that a
/// construct was reported.
fn assert_tag_phase_ran(d: &Decision, label: &str) {
    assert_eq!(
        causes(d),
        vec![UndecidableCause::TagsUnresolved],
        "{label}: the tag phase must run against this envelope: {d:?}"
    );
}

// ── tier (a): the block is absent ────────────────────────────────────────

/// Control. Nothing to consider, so nothing is reported and the decision
/// stands.
#[test]
fn tier_a_absent_block_reports_nothing() {
    let set = envelope(json!({}));
    assert!(set.unmodelled.is_empty(), "{:?}", set.unmodelled);

    let d = evaluate(&set, &request(), &trino_def(), &no_superusers());
    assert!(
        matches!(d, Decision::Allowed { .. }),
        "absent tagPolicies must not make anything undecidable: {d:?}"
    );
}

// ── tier (b): the block is present with zero policies ────────────────────

/// The valuable tier. `bind_trino_profile` creates `{name}_tag` with zero
/// policies and egret's `/policies/download` embeds the block regardless
/// (`crates/egret-policy/src/ranger/api/download.rs`), so every auto-bound
/// Trino profile lands here — and `evaluateTagPolicies` is measured to return
/// at `CollectionUtils.isNotEmpty(evaluators)` without writing to the shared
/// result (module doc). Refusing it costs every such profile an answer and
/// buys nothing.
#[test]
fn tier_b_empty_block_is_decidable() {
    let set = envelope(json!({
        "tagPolicies": {
            "serviceName": "trino_tag", "serviceId": 9, "policyVersion": 1,
            "serviceDef": {"name": "tag"},
            "policies": [],
        },
    }));
    assert!(
        !set.unmodelled
            .iter()
            .any(|u| matches!(u, Unmodelled::TagPolicies(_))),
        "an empty tagPolicies block is provably inert: {:?}",
        set.unmodelled
    );

    let d = evaluate(&set, &request(), &trino_def(), &no_superusers());
    assert!(
        matches!(d, Decision::Allowed { .. }),
        "an empty tagPolicies block must not force Undecidable: {d:?}"
    );
}

/// `policies` absent, or explicitly `null`, is the same inert shape: Ranger's
/// `getLikelyMatchPolicyEvaluators` returns `EMPTY_LIST`/`null` either way and
/// `isNotEmpty` is false for both.
#[test]
fn tier_b_missing_or_null_policies_list_is_decidable() {
    for block in [
        json!({"serviceName": "trino_tag"}),
        json!({"policies": null}),
    ] {
        let set = envelope(json!({"tagPolicies": block.clone()}));
        assert!(
            !set.unmodelled
                .iter()
                .any(|u| matches!(u, Unmodelled::TagPolicies(_))),
            "{block} carries no policies: {:?}",
            set.unmodelled
        );
    }
}

/// …but a block whose shape this reader cannot count policies in is NOT
/// shown inert. Reporting it as "zero policies" would fail open on any future
/// wire change.
#[test]
fn a_block_this_reader_cannot_count_still_refuses() {
    for block in [
        json!("tagPolicies-as-a-string"),
        json!([]),
        json!({"policies": {"not": "an array"}}),
    ] {
        let set = envelope(json!({"tagPolicies": block.clone()}));
        assert!(
            set.unmodelled
                .contains(&Unmodelled::TagPolicies(TagPolicyBlock::Unreadable)),
            "{block} is not countable and must refuse: {:?}",
            set.unmodelled
        );

        // This is the ONE tier whose refusal is still the envelope construct
        // itself after SP4d-2b Task 3: `ServicePolicySet::tag_policies` is
        // empty for a block whose `policies` array did not bind, so silence
        // would read as "the policies decided nothing". `tag_candidates`
        // raises it, before every other return in the phase.
        let d = evaluate(&set, &request(), &trino_def(), &no_superusers());
        assert_eq!(
            causes(&d),
            vec![UndecidableCause::EnvelopeConstruct(
                Unmodelled::TagPolicies(TagPolicyBlock::Unreadable)
            )],
            "{block} must force Undecidable naming the unreadable block: {d:?}"
        );
    }
}

/// **FIX ROUND 1, MINOR 2.** The `Unreadable` refusal was asserted only through
/// `evaluate`. It reaches a MASK and a ROW-FILTER decision by the same two
/// routes — `request_preamble`'s envelope sweep and `tag_candidates`'s, which
/// since this round skip the same thing — and `evaluateTagPolicies` runs for
/// policy types 1 and 2 as well, so an unreadable block can hide a tag mask
/// exactly as it can hide a tag deny.
///
/// Asserted as an EXACT cause vector on both, which is also what pins that the
/// two sweeps' overlap is deduplicated rather than reported twice.
#[test]
fn a_block_this_reader_cannot_count_refuses_a_mask_and_a_row_filter_too() {
    let set = envelope(json!({"tagPolicies": json!("tagPolicies-as-a-string")}));
    let expected = vec![UndecidableCause::EnvelopeConstruct(
        Unmodelled::TagPolicies(TagPolicyBlock::Unreadable),
    )];

    match evaluate_data_mask(&set, &request(), &trino_def(), &no_superusers()) {
        MaskDecision::Undecidable { why } => assert_eq!(why, expected),
        other => panic!("an unreadable tag block must refuse a mask: {other:?}"),
    }
    match evaluate_row_filter(&set, &request(), &trino_def(), &no_superusers()) {
        RowFilterDecision::Undecidable { why } => assert_eq!(why, expected),
        other => panic!("an unreadable tag block must refuse a row filter: {other:?}"),
    }
}

// ── tier (c): the block is present and non-empty ─────────────────────────

/// The real captured block: three policies, one of them `policyType` 0 with
/// `denyPolicyItems`. Tag policies run before every resource policy and can
/// deny, so this must refuse — and the cause must NAME the count, not be a
/// generic envelope refusal.
#[test]
fn tier_c_live_block_refuses_and_names_the_count() {
    let set = envelope(json!({"tagPolicies": live_tag_policies()}));
    assert_eq!(
        set.unmodelled
            .iter()
            .filter_map(|u| match u {
                Unmodelled::TagPolicies(b) => Some(b.clone()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![TagPolicyBlock::Policies(3)],
        "{:?}",
        set.unmodelled
    );

    let d = evaluate(&set, &request(), &trino_def(), &no_superusers());
    assert_tag_phase_ran(&d, "the live tag block's 3 policies must be evaluated");
}

/// The fixture's own envelope, parsed whole rather than spliced — the same
/// refusal, from the artefact exactly as the plugin holds it.
#[test]
fn the_captured_download_refuses_as_itself() {
    let set: ServicePolicySet =
        serde_json::from_value(live_download_json()).expect("parse captured download");
    assert!(
        set.unmodelled
            .contains(&Unmodelled::TagPolicies(TagPolicyBlock::Policies(3))),
        "{:?}",
        set.unmodelled
    );
}

// ── the empty-resolved-tag-set narrowing, and its trap ──────────────────

/// `getLikelyMatchPolicyEvaluators` returns `EMPTY_LIST` at `isNotEmpty(tags)`
/// (module doc), so a request whose resolved tag set is EMPTY cannot be
/// touched by any tag policy.
///
/// ⚠ This is only sound because the caller had to say so explicitly. See
/// `an_unresolved_tag_set_must_not_take_the_narrowing`.
#[test]
fn an_empty_resolved_tag_set_is_decidable() {
    let set = envelope(json!({"tagPolicies": live_tag_policies()}));
    let req = AccessRequest {
        resolved_tags: Some(UnenrichedTags::new(Vec::new())),
        ..request()
    };
    let d = evaluate(&set, &req, &trino_def(), &no_superusers());
    assert!(
        matches!(d, Decision::Allowed { .. }),
        "an empty resolved tag set leaves no tag evaluator to run: {d:?}"
    );
}

/// **The fail-open trap.** `None` means "the caller did not resolve tags",
/// which is NOT "there are none" — and the difference is not academic:
/// `tag_keys_for_resource_via` (`crates/egret-policy/src/store.rs`) matches
/// `schema_name`/`table_name`/`column_name` by exact string equality with no
/// hierarchy roll-up, while Ranger's `RangerTagEnricher` walks the hierarchy
/// and emits ancestor tags that a tag request admits. So egret can report "no
/// tags" for `report.country` while the engine enforces a tag policy attached
/// to the table `report`.
///
/// Every live tag assignment today is column-level, so the ancestor path is
/// not exercised by current data — which is exactly how treating `None` as
/// empty would ship green and wrong.
#[test]
fn an_unresolved_tag_set_must_not_take_the_narrowing() {
    let set = envelope(json!({"tagPolicies": live_tag_policies()}));
    let d = evaluate(&set, &request(), &trino_def(), &no_superusers());
    assert_tag_phase_ran(
        &d,
        "resolved_tags: None must NOT be read as an empty tag set",
    );
}

/// A non-empty resolved tag set is no narrowing at all — those tags are
/// exactly what the tag policies are evaluated against, and since SP4d-2b
/// Task 3 that sentence is MEASURABLE rather than a reason to refuse.
///
/// Both halves are asserted against the live block, whose only ACCESS policy
/// (8839, a clearance deny for `public` with an exception for `admin`) covers
/// the tag value `"tag"`:
///
/// * a request tagged `pii` reaches no ACCESS tag candidate, so the tag phase
///   determines nothing and the envelope's resource policy answers;
/// * the same request tagged `tag` reaches 8839, which denies.
///
/// One fixture, one changed tag name, opposite answers — which is what "the
/// tags are what the policies are evaluated against" actually claims.
#[test]
fn a_non_empty_resolved_tag_set_is_what_the_tag_policies_see() {
    let set = envelope(json!({"tagPolicies": live_tag_policies()}));
    let tagged = |name: &str| AccessRequest {
        resolved_tags: Some(UnenrichedTags::new(vec![column_tag(
            name,
            MatchType::Self_,
        )])),
        ..request()
    };

    let d = evaluate(&set, &tagged("pii"), &trino_def(), &no_superusers());
    assert!(
        matches!(&d, Decision::Allowed { policy_name, .. } if policy_name == "p"),
        "no ACCESS tag policy covers `pii`, so the resource policy answers: {d:?}"
    );

    let d = evaluate(&set, &tagged("tag"), &trino_def(), &no_superusers());
    assert!(
        matches!(
            &d,
            Decision::Denied {
                policy_id: Some(8839),
                ..
            }
        ),
        "the live clearance policy covers the tag `tag` and denies: {d:?}"
    );
}

// ── the enricher gate is the service-def, never the engine type ──────────

/// egret's `trino.json` declares a `RangerTagEnricher`; its `starrocks.json`
/// has `contextEnrichers: []`. The measured consequence: two StarRocks FEs
/// never wrote a `_tag.json`, while a Trino pod wrote one within seconds for
/// a service with no tag service bound.
#[test]
fn the_embedded_defs_disagree_about_the_tag_enricher() {
    assert_eq!(
        ServiceDef::parse(&trino_def_json()).has_tag_enricher(),
        Some(true),
        "trino.json declares org.apache.ranger.plugin.contextenricher.RangerTagEnricher"
    );
    assert_eq!(
        ServiceDef::parse(&starrocks_def_json()).has_tag_enricher(),
        Some(false),
        "starrocks.json has contextEnrichers: []"
    );
}

/// With no enricher, nothing ever populates the request's tag context, so
/// `isNotEmpty(tags)` is false and every tag evaluator is skipped — a
/// non-empty block does not force `Undecidable`.
///
/// This is gated on the DEFS, not on [`EngineType`]: gating on
/// `EngineType::StarRocks` would be silently wrong the day `contextEnrichers`
/// changes.
///
/// **Both** defs must say so. Ranger unions the two enricher lists (see
/// `the_block_s_own_service_def_can_supply_the_enricher` for the measured
/// chain), so this test has to disclaim the tag def's enrichers as well as the
/// resource def's. An earlier version of this test passed the live block
/// unchanged and so only disclaimed the resource def — it was blessing a
/// fail-open gap, because the live block's `serviceDef` merely happens to omit
/// `contextEnrichers` rather than declaring none.
#[test]
fn defs_without_the_tag_enricher_make_a_non_empty_block_decidable() {
    let set = envelope(json!({
        "tagPolicies": live_tag_policies_with_enrichers(json!([])),
    }));
    let def = def_without_enrichers();
    assert_eq!(def.has_tag_enricher(), Some(false));

    let d = evaluate(&set, &request(), &def, &no_superusers());
    assert!(
        matches!(d, Decision::Allowed { .. }),
        "no tag enricher on either def means no tag evaluator can run: {d:?}"
    );
}

/// **I-1: the block's OWN `serviceDef` can supply the enricher, so the
/// resource def alone is not enough to license the narrowing.**
///
/// Measured in `ranger-plugins-common` 2.5.0 and 2.8.0 (offsets are 2.5.0's;
/// 2.8.0 has the same shape at slightly different offsets):
///
/// * `PolicyEngine.<init>` builds the tag repository when
///   `!disableTagPolicyEvaluation && tagPolicies != null &&
///   !StringUtils.isEmpty(tagPolicies.getServiceName()) &&
///   tagPolicies.getServiceDef() != null` — offsets 471-500. **There is no
///   `getPolicies()` call in that gate at all**, in either jar.
/// * `RangerPolicyRepository(TagPolicies, ctx, componentDef, componentName)`
///   sets `this.serviceDef` from `tagPolicies.getServiceDef()` (offsets 33-48)
///   and `this.zoneName = null` (offset 30).
/// * `buildContextEnrichers` reads `this.serviceDef.getContextEnrichers()`
///   under an `isEmpty(zoneName)` guard — offsets 12-38. For the tag
///   repository that guard is satisfied by construction.
/// * `PolicyEngine.<init>` then sets
///   `allContextEnrichers = tagPolicyRepository.getContextEnrichers() ++
///   policyRepository.getContextEnrichers()` — offsets 619-702, a real
///   `ArrayList.addAll` union, not a choice between them.
/// * `RangerDefaultRequestProcessor.enrich` iterates
///   `PolicyEngine.getAllContextEnrichers()` (offset 4) and calls
///   `RangerContextEnricher.enrich` on each (offset 110); `preProcess` calls
///   `enrich` at offset 363.
///
/// And the payload that makes it reachable: **Apache Ranger's own stock
/// `service-defs/ranger-servicedef-tag.json`, bundled inside
/// `ranger-plugins-common` 2.5.0 AND 2.8.0, declares `RangerTagEnricher`** —
/// see [`stock_tag_def_json`], which is that file verbatim.
///
/// So a non-empty block plus a resource def with no tag enricher plus a
/// `tagPolicies.serviceDef` carrying the stock def means the engine DOES
/// enrich and DOES evaluate tag policies. Narrowing on the resource def alone
/// would answer `Allowed` for exactly that case.
///
/// What keeps this from biting egret today is DATA, not the measurement:
/// `crates/egret-policy/src/service-defs/tag.json` happens to omit
/// `contextEnrichers`. `download.rs` embeds that def for **any** engine type
/// with a `tagService` in config, and the differential corpus and
/// `examples/live_gate.rs` consume downloads captured elsewhere — so "safe
/// because of a field nobody set" is the whole of today's protection.
#[test]
fn the_block_s_own_service_def_can_supply_the_enricher() {
    let stock = stock_tag_def_json();
    // The premise, asserted rather than described: Ranger's shipped tag def
    // really does declare the enricher.
    assert_eq!(
        ServiceDef::parse(&stock).has_tag_enricher(),
        Some(true),
        "the stock tag def must declare RangerTagEnricher"
    );

    let set = envelope(json!({
        "tagPolicies": live_tag_policies_with_enrichers(stock["contextEnrichers"].clone()),
    }));
    let def = def_without_enrichers();
    assert_eq!(def.has_tag_enricher(), Some(false));

    let d = evaluate(&set, &request(), &def, &no_superusers());
    assert_tag_phase_ran(&d, "the tag def's own enricher must still admit the phase");
}

/// The same refusal with the stock def dropped in whole, rather than only its
/// `contextEnrichers` — the shape a real `tagPolicies` block from a stock
/// Ranger admin has.
#[test]
fn a_block_carrying_the_whole_stock_tag_def_refuses() {
    let mut block = live_tag_policies();
    block["serviceDef"] = stock_tag_def_json();
    let set = envelope(json!({"tagPolicies": block}));

    let d = evaluate(&set, &request(), &def_without_enrichers(), &no_superusers());
    assert_tag_phase_ran(&d, "a block carrying the whole stock tag def");
}

/// A tag `serviceDef` whose `contextEnrichers` key is ABSENT is unknown, not
/// "declares none" — the same provenance rule the resource def follows, and
/// the live captured block is exactly this shape.
#[test]
fn an_absent_tag_def_enricher_key_is_unknown_not_absent() {
    // The live block, unmodified: its serviceDef is egret's tag.json, which
    // omits contextEnrichers.
    let live = live_tag_policies();
    assert!(
        live["serviceDef"].get("contextEnrichers").is_none(),
        "the captured block's tag def is expected to omit contextEnrichers"
    );
    assert_eq!(
        ServiceDef::parse(&live["serviceDef"]).has_tag_enricher(),
        None
    );

    let set = envelope(json!({"tagPolicies": live}));
    let d = evaluate(&set, &request(), &def_without_enrichers(), &no_superusers());
    assert_tag_phase_ran(
        &d,
        "an unknown tag-def enricher must not take the narrowing",
    );
}

/// Neither def alone is sufficient: the narrowing needs `Some(false)` from
/// both, so each of the three non-(false,false) combinations refuses.
#[test]
fn either_def_declaring_an_enricher_blocks_the_narrowing() {
    let with = stock_tag_def_json()["contextEnrichers"].clone();
    let cases: [(&str, ServiceDef, Value); 3] = [
        (
            "resource none / tag enricher",
            def_without_enrichers(),
            with.clone(),
        ),
        ("resource enricher / tag none", trino_def(), json!([])),
        ("resource enricher / tag enricher", trino_def(), with),
    ];
    for (label, def, tag_enrichers) in cases {
        let set = envelope(json!({
            "tagPolicies": live_tag_policies_with_enrichers(tag_enrichers),
        }));
        let d = evaluate(&set, &request(), &def, &no_superusers());
        assert_tag_phase_ran(&d, label);
    }
}

/// The request-side escape is independent of both defs: no resolved tags means
/// `isNotEmpty(tags)` is false however many enrichers ran, so it still narrows
/// even with the stock tag def in place.
#[test]
fn an_empty_resolved_tag_set_narrows_even_with_both_enrichers() {
    let set = envelope(json!({
        "tagPolicies": live_tag_policies_with_enrichers(
            stock_tag_def_json()["contextEnrichers"].clone()
        ),
    }));
    let req = AccessRequest {
        resolved_tags: Some(UnenrichedTags::new(Vec::new())),
        ..request()
    };
    let d = evaluate(&set, &req, &trino_def(), &no_superusers());
    assert!(
        matches!(d, Decision::Allowed { .. }),
        "an empty resolved tag set leaves no tag evaluator to run: {d:?}"
    );
}

/// A def declaring only some OTHER enricher is still a def with no tag
/// enricher — the predicate is the enricher class, not merely the presence of
/// the list.
#[test]
fn a_def_with_only_another_enricher_has_no_tag_enricher() {
    let mut def_json = trino_def_json();
    def_json["contextEnrichers"] = json!([{
        "itemId": 1, "name": "GeoEnricher",
        "enricher": "org.apache.ranger.plugin.contextenricher.RangerCountryProviderEnricher",
    }]);
    assert_eq!(ServiceDef::parse(&def_json).has_tag_enricher(), Some(false));
}

/// **An absent `contextEnrichers` key is UNKNOWN, not "no enricher".**
///
/// Measured, and this is why the accessor is three-valued: the live download
/// captured for this task carries the key on its `trino` def, while the
/// repo's older capture (`tests/ranger/fixtures/service-policies.json`) and
/// the `tag` def embedded inside the live fixture itself both omit it
/// entirely. Reading absence as `false` would let a def that really does
/// declare an enricher take the narrowing — fail open.
#[test]
fn an_absent_context_enrichers_key_is_unknown_not_absent() {
    let mut def_json = trino_def_json();
    def_json
        .as_object_mut()
        .expect("def is an object")
        .remove("contextEnrichers");
    let def = ServiceDef::parse(&def_json);
    assert_eq!(
        def.has_tag_enricher(),
        None,
        "an absent key is unknown, not false"
    );

    let set = envelope(json!({"tagPolicies": live_tag_policies()}));
    let d = evaluate(&set, &request(), &def, &no_superusers());
    assert_tag_phase_ran(
        &d,
        "an unknown resource-def enricher must not take the narrowing",
    );

    // The shapes that produced this measurement, asserted rather than
    // described, so a re-capture that changes them fails here.
    let tag_def = ServiceDef::parse(&live_download_json()["tagPolicies"]["serviceDef"]);
    assert_eq!(tag_def.name(), "tag");
    assert_eq!(tag_def.has_tag_enricher(), None);
    let older: Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/ranger/fixtures/service-policies.json"
        ))
        .expect("read older capture"),
    )
    .expect("parse older capture");
    assert_eq!(
        ServiceDef::parse(&older["serviceDef"]).has_tag_enricher(),
        None
    );
}

/// A `tagPolicies.serviceDef` that is missing, `null`, or not an object at all
/// must parse without panicking and answer UNKNOWN — so it refuses.
///
/// This is not hypothetical plumbing: `ServicePolicySet::tag_service_def` is
/// `Value::Null` for every envelope with no `tagPolicies` block, so
/// `ServiceDef::parse` is handed a `Null` on the ordinary path. The direction
/// is what matters — a non-object def is the *least* trustworthy input there
/// is, and answering `Some(false)` for one would license the narrowing on
/// garbage.
#[test]
fn a_tag_service_def_that_is_not_an_object_is_unknown() {
    for v in [
        json!(null),
        json!("a string"),
        json!([]),
        json!(7),
        json!({}),
    ] {
        assert_eq!(
            ServiceDef::parse(&v).has_tag_enricher(),
            None,
            "{v} must be UNKNOWN, never Some(false)"
        );
    }

    // And end to end: a block whose `serviceDef` is a non-object refuses even
    // when the resource def explicitly declares no enricher.
    let mut block = live_tag_policies();
    block["serviceDef"] = json!("not a def");
    let set = envelope(json!({"tagPolicies": block}));
    let d = evaluate(&set, &request(), &def_without_enrichers(), &no_superusers());
    assert_tag_phase_ran(&d, "a tag service-def that is not an object");
}

/// Tier (a) and tier (b) leave `tag_service_def` as `Value::Null`, and both
/// still narrow — i.e. the `Null` def is reached on the ordinary path and does
/// not accidentally block the narrowing that tiers (a) and (b) are entitled to.
#[test]
fn an_absent_block_leaves_a_null_tag_service_def_and_still_narrows() {
    let absent = envelope(json!({}));
    assert!(absent.tag_service_def.is_null());

    let empty = envelope(json!({
        "tagPolicies": {"serviceName": "trino_tag", "policies": []},
    }));
    assert!(empty.tag_service_def.is_null());

    for set in [absent, empty] {
        let d = evaluate(&set, &request(), &trino_def(), &no_superusers());
        assert!(matches!(d, Decision::Allowed { .. }), "{d:?}");
    }
}

// ── the control that keeps the package honest ───────────────────────────

/// `Unmodelled::ServiceConfig` still forces `Undecidable`.
///
/// This test exists so a future narrowing cannot take it by accident. The two
/// refusals look alike and have OPPOSITE risk: an empty `tagPolicies` is a
/// measured no-op, while an unmodelled `serviceConfig` fails **OPEN** —
/// `RangerPolicyEngineImpl.<init>` builds a `ServiceConfig` from
/// `ServicePolicies.getServiceConfig()` (offsets 125-136, identical at 2.5.0
/// and 2.8.0), that class reads `ranger.plugin.super.users`, and
/// `isSuperUser` ORs it against the plugin config, yielding an allow at
/// `policyPriority = Integer.MAX_VALUE` that nothing downstream can override.
/// See the ⚠ block on [`Unmodelled::ServiceConfig`].
#[test]
fn service_config_still_forces_undecidable() {
    let set = envelope(json!({"serviceConfig": {"ranger.plugin.super.users": "root"}}));
    assert!(
        set.unmodelled.contains(&Unmodelled::ServiceConfig),
        "{:?}",
        set.unmodelled
    );

    let d = evaluate(&set, &request(), &trino_def(), &no_superusers());
    assert!(
        causes(&d).iter().any(|c| matches!(
            c,
            UndecidableCause::EnvelopeConstruct(Unmodelled::ServiceConfig)
        )),
        "serviceConfig must still refuse: {d:?}"
    );
}

/// The same for the other two envelope constructs this task did not touch, so
/// a narrowing that over-reaches into them is caught here too.
#[test]
fn security_zones_and_policy_deltas_still_force_undecidable() {
    for (key, body, want) in [
        (
            "securityZones",
            json!({"zone-a": {"name": "zone-a"}}),
            Unmodelled::SecurityZones,
        ),
        (
            "policyDeltas",
            json!([{"id": 99, "changeType": 1, "policy": {"id": 99}}]),
            Unmodelled::PolicyDeltas,
        ),
    ] {
        let set = envelope(json!({key: body}));
        assert!(set.unmodelled.contains(&want), "{:?}", set.unmodelled);

        let d = evaluate(&set, &request(), &trino_def(), &no_superusers());
        assert!(
            causes(&d)
                .iter()
                .any(|c| matches!(c, UndecidableCause::EnvelopeConstruct(u) if *u == want)),
            "{key} must still refuse: {d:?}"
        );
    }
}
