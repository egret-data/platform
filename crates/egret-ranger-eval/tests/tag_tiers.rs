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
//! and **2.8.0** (`starrocks/fe-ubuntu:4.1-latest`). The two disassemblies of
//! `RangerPolicyEngineImpl.evaluateTagPolicies` are identical instruction for
//! instruction — 256 instructions, same offsets, same branch targets — apart
//! from `commons-lang` → `commons-lang3` in `StringUtils.isNotEmpty`/`equals`
//! at offsets 214 and 285, which changes no behaviour here.
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
//! * **An empty block is inert.** `evaluateTagPolicies` computes
//!   `evaluators` at offsets 109-129 (null repository ⇒ `null`; otherwise
//!   `RangerPolicyRepository.getLikelyMatchPolicyEvaluators`), then branches
//!   `CollectionUtils.isNotEmpty(evaluators)` / `ifeq 582` at 133-136. The
//!   only writes to the shared result (offsets 508, 531, 557) are inside the
//!   loop the branch skips.
//!
//!   One correction to how this is usually stated: offset 582 is not a bare
//!   `return`. It reads `result.getIsAllowed()` and, if true, calls
//!   `setIsAccessDetermined(true)` at 590. That write cannot change anything
//!   on this path: the result comes from `createAccessResult` at offset 112
//!   of the caller with `isAllowed` false, and the one block that can flip it
//!   first — the super-user short-circuit at caller offsets 133-165 — already
//!   sets `setIsAccessDetermined(true)` itself at 143. So the write is either
//!   unreached or idempotent, and the block is inert either way.
//!
//! * **No resolved tags ⇒ no evaluators.**
//!   `RangerPolicyRepository.getLikelyMatchPolicyEvaluators(request, tags,
//!   policyType, date)` opens `ret = Collections.EMPTY_LIST` (offset 0) and
//!   branches `isNotEmpty(tags)` / `ifeq 333` at 6-9, returning that empty
//!   list. Identical in both jars.
//!
//! * **The tag set comes from the enricher, not from the engine type.** The
//!   set at offset 104 is
//!   `RangerAccessRequestUtil.getRequestTagsFromContext(request.getContext())`,
//!   and the only thing in `ranger-plugins-common` that writes that context
//!   key is `RangerTagEnricher.enrich`, via
//!   `RangerAccessRequestUtil.setRequestTagsInContext` (offset 199 of
//!   `enrich`). `RangerPolicyRepository` instantiates enrichers from the
//!   service-def's `contextEnrichers`. So the gate is the DEF, never
//!   [`EngineType`]: egret's `trino.json` declares a `RangerTagEnricher` and
//!   its `starrocks.json` has `contextEnrichers: []`, and the day that one
//!   field changes an `EngineType::StarRocks` test would go silently wrong.

use std::collections::BTreeSet;

use egret_ranger_eval::eval::{evaluate, AccessRequest, Decision, EngineConfig, UndecidableCause};
use egret_ranger_eval::model::{ServicePolicySet, TagPolicyBlock, Unmodelled};
use egret_ranger_eval::resource::RequestResource;
use egret_ranger_eval::servicedef::ServiceDef;
use serde_json::{json, Value};

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

fn tag_causes(d: &Decision) -> Vec<TagPolicyBlock> {
    causes(d)
        .into_iter()
        .filter_map(|c| match c {
            UndecidableCause::EnvelopeConstruct(Unmodelled::TagPolicies(b)) => Some(b),
            _ => None,
        })
        .collect()
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

        let d = evaluate(&set, &request(), &trino_def(), &no_superusers());
        assert!(
            !tag_causes(&d).is_empty(),
            "{block} must force Undecidable: {d:?}"
        );
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
    assert_eq!(
        tag_causes(&d),
        vec![TagPolicyBlock::Policies(3)],
        "the live tag block must force Undecidable naming its 3 policies: {d:?}"
    );
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
        resolved_tags: Some(BTreeSet::new()),
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
    assert_eq!(
        tag_causes(&d),
        vec![TagPolicyBlock::Policies(3)],
        "resolved_tags: None must NOT be read as an empty tag set: {d:?}"
    );
}

/// A non-empty resolved tag set is no narrowing at all — those tags are
/// exactly what the tag policies would be evaluated against.
#[test]
fn a_non_empty_resolved_tag_set_still_refuses() {
    let set = envelope(json!({"tagPolicies": live_tag_policies()}));
    let req = AccessRequest {
        resolved_tags: Some(BTreeSet::from(["pii".to_string()])),
        ..request()
    };
    let d = evaluate(&set, &req, &trino_def(), &no_superusers());
    assert_eq!(tag_causes(&d), vec![TagPolicyBlock::Policies(3)], "{d:?}");
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
/// This is gated on the DEF, not on [`EngineType`]: gating on
/// `EngineType::StarRocks` would be silently wrong the day `contextEnrichers`
/// changes.
#[test]
fn a_def_without_the_tag_enricher_makes_a_non_empty_block_decidable() {
    let set = envelope(json!({"tagPolicies": live_tag_policies()}));
    let mut def_json = trino_def_json();
    def_json["contextEnrichers"] = json!([]);
    let def = ServiceDef::parse(&def_json);
    assert_eq!(def.has_tag_enricher(), Some(false));

    let d = evaluate(&set, &request(), &def, &no_superusers());
    assert!(
        matches!(d, Decision::Allowed { .. }),
        "no tag enricher means no tag evaluator can run: {d:?}"
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
    assert_eq!(
        tag_causes(&d),
        vec![TagPolicyBlock::Policies(3)],
        "an unknown enricher must not take the narrowing: {d:?}"
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
