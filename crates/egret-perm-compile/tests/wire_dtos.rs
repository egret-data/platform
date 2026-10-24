//! SP4d-2a Task 4: conversion tests for the hand-written wire DTOs that
//! mirror `egret_ranger_eval::eval`'s three-valued `Decision`, `DenyReason`,
//! `UndecidableCause`, and the mask / row-filter results — and, one level
//! deeper, the `Unmodelled` / `ServiceDefIssue` (and `ItemBucket` /
//! `TagPolicyBlock` / `PolicyType` / `GraphProblem`) payloads those causes
//! carry. All of it is mirrored exhaustively, not just the top level — see
//! `explain_wire`'s module doc for why an earlier `{:?}` shortcut one level
//! down was wrong.
//!
//! **Why one test per source variant.** The whole point of hand-writing
//! these conversions (rather than deriving `ToSchema` on the source types
//! directly, which would require adding `utoipa` to `egret-ranger-eval` and
//! giving up its `serde`-only discipline) is that the `From` impls match
//! with no wildcard arm — a variant added upstream fails the BUILD, not a
//! test. This file cannot exercise that compile-time property directly by
//! itself, but it can and does prove the mapping is correct for every
//! variant that exists today, so a reviewer diffing this file against a
//! future `eval.rs`/`model.rs`/`servicedef.rs` can see at a glance which arm
//! is missing. The compile-time property itself was verified by hand while
//! writing this: a temporary extra variant added to `UndecidableCause` (in
//! `eval.rs`) and, separately, to `Unmodelled` (in `model.rs`) each broke
//! `cargo build -p egret-perm-compile` with `error[E0004]: non-exhaustive
//! patterns`, pointing at the exact `match` in `explain_wire.rs` missing
//! that arm — then both were reverted (`git diff --stat -- crates/
//! egret-ranger-eval/` empty again). See the Task 4 report for the full
//! transcript.
//!
//! Pure conversions, no I/O — no `DATABASE_URL` needed.

use egret_perm_compile::explain_wire::{
    WireDecision, WireDenyReason, WireEngineType, WireGraphProblem, WireItemBucket,
    WireMaskDecision, WireMaskResult, WirePluginVersion, WirePolicyType, WireRowFilterDecision,
    WireRowFilterResult, WireServiceDefIssue, WireTagAttachmentProblem,
    WireTagNormalisationProblem, WireTagPolicyBlock, WireTagResourceProblem, WireUndecidableCause,
    WireUnmodelled,
};
use egret_ranger_eval::eval::{
    Decision, DenyReason, EngineType, MaskDecision, MaskResult, PluginVersion, RowFilterDecision,
    RowFilterResult, TagAttachmentProblem, TagNormalisationProblem, TagResourceProblem,
    TiedOutcome, UndecidableCause,
};
use egret_ranger_eval::model::{ItemBucket, TagPolicyBlock, Unmodelled};
use egret_ranger_eval::servicedef::{GraphProblem, PolicyType, ServiceDefIssue};

// ── DenyReason (3 variants) ─────────────────────────────────────────────

#[test]
fn deny_reason_deny_policy_item() {
    assert_eq!(
        WireDenyReason::from(DenyReason::DenyPolicyItem),
        WireDenyReason::DenyPolicyItem
    );
    assert_eq!(
        serde_json::to_value(WireDenyReason::DenyPolicyItem).unwrap(),
        serde_json::json!("deny_policy_item")
    );
}

#[test]
fn deny_reason_deny_all_else() {
    assert_eq!(
        WireDenyReason::from(DenyReason::DenyAllElse),
        WireDenyReason::DenyAllElse
    );
    assert_eq!(
        serde_json::to_value(WireDenyReason::DenyAllElse).unwrap(),
        serde_json::json!("deny_all_else")
    );
}

#[test]
fn deny_reason_no_policy_matched() {
    assert_eq!(
        WireDenyReason::from(DenyReason::NoPolicyMatched),
        WireDenyReason::NoPolicyMatched
    );
    assert_eq!(
        serde_json::to_value(WireDenyReason::NoPolicyMatched).unwrap(),
        serde_json::json!("no_policy_matched")
    );
}

// ── EngineType / PluginVersion (2 variants each; payloads of two causes) ─

#[test]
fn engine_type_both_variants() {
    assert_eq!(
        WireEngineType::from(EngineType::Trino),
        WireEngineType::Trino
    );
    assert_eq!(
        WireEngineType::from(EngineType::StarRocks),
        WireEngineType::StarRocks
    );
}

/// **One spelling of StarRocks per API surface** (SP4d-2a Task 5 review, I3).
/// `egret_query_api::Engine` already publishes `starrocks` on the same
/// document, and that same literal is what `policy.authz_services.type`
/// stores. A `rename_all = "snake_case"` on `WireEngineType` would render
/// `star_rocks` and force a client correlating the two to translate between
/// them. Pinned as a WIRE string, because nothing above would notice: the
/// `From` assertions compare Rust values and pass under either spelling.
#[test]
fn engine_type_serialises_the_same_spelling_the_rest_of_the_api_uses() {
    assert_eq!(
        serde_json::to_value(WireEngineType::Trino).unwrap(),
        serde_json::json!("trino")
    );
    assert_eq!(
        serde_json::to_value(WireEngineType::StarRocks).unwrap(),
        serde_json::json!("starrocks"),
        "`star_rocks` is a second spelling of a word this API already spells \
         `starrocks` (`egret_query_api::Engine`, `authz_services.type`)"
    );
    assert_eq!(
        serde_json::to_value(WireEngineType::StarRocks).unwrap(),
        serde_json::to_value(egret_query_api::Engine::StarRocks).unwrap(),
        "and the two must not be able to drift apart independently"
    );
}

#[test]
fn plugin_version_every_variant() {
    assert_eq!(
        WirePluginVersion::from(PluginVersion::Ranger2_5_0),
        WirePluginVersion::Ranger2_5_0
    );
    assert_eq!(
        WirePluginVersion::from(PluginVersion::Ranger2_8_0),
        WirePluginVersion::Ranger2_8_0
    );
    assert_eq!(
        WirePluginVersion::from(PluginVersion::Ranger2_9_0),
        WirePluginVersion::Ranger2_9_0
    );
    assert_eq!(
        serde_json::to_value(WirePluginVersion::Ranger2_9_0).unwrap(),
        serde_json::json!("ranger_2_9_0")
    );
    assert_eq!(
        serde_json::to_value(WirePluginVersion::Ranger2_5_0).unwrap(),
        serde_json::json!("ranger_2_5_0")
    );
    assert_eq!(
        serde_json::to_value(WirePluginVersion::Ranger2_8_0).unwrap(),
        serde_json::json!("ranger_2_8_0")
    );
}

// ── UndecidableCause (19 variants) ───────────────────────────────────────

#[test]
fn cause_envelope_construct_carries_the_full_typed_construct() {
    let cause =
        UndecidableCause::EnvelopeConstruct(Unmodelled::TagPolicies(TagPolicyBlock::Policies(3)));
    let wire = WireUndecidableCause::from(&cause);
    assert_eq!(
        wire,
        WireUndecidableCause::EnvelopeConstruct {
            construct: WireUnmodelled::TagPolicies {
                block: WireTagPolicyBlock::Policies { count: 3 },
            },
        },
        "the operator-facing fact (how many tag policies) must survive as a \
         number, not be buried in a Debug string"
    );
}

#[test]
fn cause_policy_construct_carries_policy_id_name_and_typed_construct() {
    let cause = UndecidableCause::PolicyConstruct {
        policy_id: Some(42),
        policy_name: "p1".to_string(),
        construct: Unmodelled::ValiditySchedules,
    };
    let wire = WireUndecidableCause::from(&cause);
    assert_eq!(
        wire,
        WireUndecidableCause::PolicyConstruct {
            policy_id: Some(42),
            policy_name: "p1".to_string(),
            construct: WireUnmodelled::ValiditySchedules,
        }
    );
}

#[test]
fn cause_service_def_carries_the_full_typed_issue() {
    let issue = ServiceDefIssue::InvalidResourceGraph {
        policy_type: PolicyType::Access,
        reason: GraphProblem::NoSourcesOrSinks,
    };
    let cause = UndecidableCause::ServiceDef(issue);
    let wire = WireUndecidableCause::from(&cause);
    assert_eq!(
        wire,
        WireUndecidableCause::ServiceDef {
            issue: WireServiceDefIssue::InvalidResourceGraph {
                policy_type: WirePolicyType::Access,
                reason: WireGraphProblem::NoSourcesOrSinks,
            },
        }
    );
}

#[test]
fn cause_resource_owner_unknown() {
    let cause = UndecidableCause::ResourceOwnerUnknown {
        policy_id: Some(7),
        policy_name: "owner-policy".to_string(),
    };
    let wire = WireUndecidableCause::from(&cause);
    assert_eq!(
        wire,
        WireUndecidableCause::ResourceOwnerUnknown {
            policy_id: Some(7),
            policy_name: "owner-policy".to_string(),
        }
    );
}

#[test]
fn cause_super_users_unknown() {
    let wire = WireUndecidableCause::from(&UndecidableCause::SuperUsersUnknown);
    assert_eq!(wire, WireUndecidableCause::SuperUsersUnknown);
}

#[test]
fn cause_pseudo_access_type() {
    let cause = UndecidableCause::PseudoAccessType("_any".to_string());
    let wire = WireUndecidableCause::from(&cause);
    assert_eq!(
        wire,
        WireUndecidableCause::PseudoAccessType {
            access_type: "_any".to_string(),
        }
    );
}

#[test]
fn cause_access_type_case_mismatch() {
    let cause = UndecidableCause::AccessTypeCaseMismatch {
        policy_id: None,
        policy_name: "p2".to_string(),
        access_type: "Select".to_string(),
    };
    let wire = WireUndecidableCause::from(&cause);
    assert_eq!(
        wire,
        WireUndecidableCause::AccessTypeCaseMismatch {
            policy_id: None,
            policy_name: "p2".to_string(),
            access_type: "Select".to_string(),
        }
    );
}

#[test]
fn cause_mask_expression() {
    let cause = UndecidableCause::MaskExpression {
        policy_id: Some(3),
        policy_name: "mask-policy".to_string(),
        item_index: 2,
        expr: "${{ ctx.tbl }}".to_string(),
    };
    let wire = WireUndecidableCause::from(&cause);
    assert_eq!(
        wire,
        WireUndecidableCause::MaskExpression {
            policy_id: Some(3),
            policy_name: "mask-policy".to_string(),
            item_index: 2,
            expr: "${{ ctx.tbl }}".to_string(),
        }
    );
}

#[test]
fn cause_row_filter_expression() {
    let cause = UndecidableCause::RowFilterExpression {
        policy_id: Some(4),
        policy_name: "rf-policy".to_string(),
        item_index: 1,
        expr: "${{ ctx.user }}".to_string(),
    };
    let wire = WireUndecidableCause::from(&cause);
    assert_eq!(
        wire,
        WireUndecidableCause::RowFilterExpression {
            policy_id: Some(4),
            policy_name: "rf-policy".to_string(),
            item_index: 1,
            expr: "${{ ctx.user }}".to_string(),
        }
    );
}

#[test]
fn cause_row_filter_double_brackets() {
    let cause = UndecidableCause::RowFilterDoubleBrackets {
        policy_id: Some(5),
        policy_name: "rf-db-policy".to_string(),
        item_index: 0,
        expr: "a [[b]] c".to_string(),
    };
    let wire = WireUndecidableCause::from(&cause);
    assert_eq!(
        wire,
        WireUndecidableCause::RowFilterDoubleBrackets {
            policy_id: Some(5),
            policy_name: "rf-db-policy".to_string(),
            item_index: 0,
            expr: "a [[b]] c".to_string(),
        }
    );
}

#[test]
fn cause_eval_order_ambiguous_keeps_every_tied_id() {
    let cause = UndecidableCause::EvalOrderAmbiguous {
        policy_ids: vec![Some(1), None, Some(3)],
        policy_names: vec!["a".to_string(), "b".to_string(), "c".to_string()],
        outcomes: Vec::new(),
    };
    let wire = WireUndecidableCause::from(&cause);
    assert_eq!(
        wire,
        WireUndecidableCause::EvalOrderAmbiguous {
            policy_ids: vec![Some(1), None, Some(3)],
            policy_names: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            outcomes: Vec::new(),
        }
    );
    // An access tie has no outcomes, and the wire omits the empty list.
    assert!(serde_json::to_value(&wire)
        .unwrap()
        .get("outcomes")
        .is_none());
}

#[test]
fn cause_eval_order_ambiguous_carries_each_tied_outcome_in_order() {
    let cause = UndecidableCause::EvalOrderAmbiguous {
        policy_ids: vec![Some(1), Some(2)],
        policy_names: vec!["a".to_string(), "b".to_string()],
        outcomes: vec![
            TiedOutcome::Mask {
                mask_type: Some("MASK_HASH".to_string()),
                masked_value: None,
                mask_condition: None,
            },
            TiedOutcome::RowFilter {
                filter_expr: "region = 'us'".to_string(),
            },
        ],
    };
    let wire = serde_json::to_value(WireUndecidableCause::from(&cause)).unwrap();
    assert_eq!(
        wire["outcomes"],
        serde_json::json!([
            {"kind": "mask", "mask_type": "MASK_HASH", "masked_value": null, "mask_condition": null},
            {"kind": "row_filter", "filter_expr": "region = 'us'"},
        ])
    );
}

#[test]
fn cause_engine_mismatch() {
    let cause = UndecidableCause::EngineMismatch {
        declared: EngineType::Trino,
        service_def_name: "starrocks".to_string(),
    };
    let wire = WireUndecidableCause::from(&cause);
    assert_eq!(
        wire,
        WireUndecidableCause::EngineMismatch {
            declared: WireEngineType::Trino,
            service_def_name: "starrocks".to_string(),
        }
    );
}

#[test]
fn cause_plugin_version_mismatch() {
    let cause = UndecidableCause::PluginVersionMismatch {
        engine: EngineType::StarRocks,
        declared: PluginVersion::Ranger2_5_0,
    };
    let wire = WireUndecidableCause::from(&cause);
    assert_eq!(
        wire,
        WireUndecidableCause::PluginVersionMismatch {
            engine: WireEngineType::StarRocks,
            declared: WirePluginVersion::Ranger2_5_0,
        }
    );
}

#[test]
fn cause_tags_unresolved() {
    let wire = WireUndecidableCause::from(&UndecidableCause::TagsUnresolved);
    assert_eq!(wire, WireUndecidableCause::TagsUnresolved);
    assert_eq!(
        serde_json::to_value(wire).unwrap(),
        serde_json::json!({"kind": "tags_unresolved"})
    );
}

/// SP4d-2b Task 4. All three `TagAttachmentProblem` arms. Each carries a
/// DIFFERENT payload shape (a list of keys, a count, a list of levels) and the
/// three would be indistinguishable on the wire if any two collapsed — the
/// remedies are "drop the tag's validity construct", "the same", and "this
/// payload did not come from egret's own writer", which is not a remedy at all
/// but a provenance question.
#[test]
fn cause_tag_attachment_not_reproducible_every_problem() {
    let cases = [
        (
            TagAttachmentProblem::Options(vec!["TAG_VALIDITY_PERIODS".to_string()]),
            WireTagAttachmentProblem::Options {
                keys: vec!["TAG_VALIDITY_PERIODS".to_string()],
            },
            serde_json::json!({"kind": "options", "keys": ["TAG_VALIDITY_PERIODS"]}),
        ),
        (
            TagAttachmentProblem::ValidityPeriods(2),
            WireTagAttachmentProblem::ValidityPeriods { count: 2 },
            serde_json::json!({"kind": "validity_periods", "count": 2}),
        ),
        (
            TagAttachmentProblem::ResourceLevels(vec!["catalog".to_string(), "schema".to_string()]),
            WireTagAttachmentProblem::ResourceLevels {
                levels: vec!["catalog".to_string(), "schema".to_string()],
            },
            serde_json::json!({"kind": "resource_levels", "levels": ["catalog", "schema"]}),
        ),
    ];
    for (problem, expected, expected_json) in cases {
        assert_eq!(WireTagAttachmentProblem::from(&problem), expected);
        assert_eq!(
            serde_json::to_value(WireTagAttachmentProblem::from(&problem)).unwrap(),
            expected_json
        );
        let cause = UndecidableCause::TagAttachmentNotReproducible {
            tag: "pii".to_string(),
            problem: problem.clone(),
        };
        assert_eq!(
            WireUndecidableCause::from(&cause),
            WireUndecidableCause::TagAttachmentNotReproducible {
                tag: "pii".to_string(),
                problem: expected,
            }
        );
    }
}

/// SP4d-2b Task 2. All three `TagResourceProblem` arms, because each names a
/// different remedy for the steward: drop the extra resource level, drop the
/// `isExcludes`, or spell the tag value out.
#[test]
fn cause_tag_resource_not_reproducible_every_problem() {
    let cases = [
        (
            TagResourceProblem::NotOnlyTheTagResource,
            WireTagResourceProblem::NotOnlyTheTagResource,
            serde_json::json!({"kind": "not_only_the_tag_resource"}),
        ),
        (
            TagResourceProblem::Excludes,
            WireTagResourceProblem::Excludes,
            serde_json::json!({"kind": "excludes"}),
        ),
        (
            TagResourceProblem::ValueNotTriePrefixExact("p*x".to_string()),
            WireTagResourceProblem::ValueNotTriePrefixExact {
                value: "p*x".to_string(),
            },
            serde_json::json!({"kind": "value_not_trie_prefix_exact", "value": "p*x"}),
        ),
        (
            TagResourceProblem::Recursive,
            WireTagResourceProblem::Recursive,
            serde_json::json!({"kind": "recursive"}),
        ),
    ];
    for (problem, expected, expected_json) in cases {
        assert_eq!(WireTagResourceProblem::from(&problem), expected);
        assert_eq!(
            serde_json::to_value(WireTagResourceProblem::from(&problem)).unwrap(),
            expected_json
        );
        let cause = UndecidableCause::TagResourceNotReproducible {
            policy_id: Some(8839),
            policy_name: "clearance".to_string(),
            problem: problem.clone(),
        };
        assert_eq!(
            WireUndecidableCause::from(&cause),
            WireUndecidableCause::TagResourceNotReproducible {
                policy_id: Some(8839),
                policy_name: "clearance".to_string(),
                problem: expected,
            }
        );
    }
}

/// SP4d-2b fix round 1, review question 3.
#[test]
fn cause_tag_sub_policy_is_deny_capable() {
    let cause = UndecidableCause::TagSubPolicyIsDenyCapable {
        policy_id: Some(8799),
        policy_name: "mask-with-deny".to_string(),
    };
    let wire = WireUndecidableCause::from(&cause);
    assert_eq!(
        wire,
        WireUndecidableCause::TagSubPolicyIsDenyCapable {
            policy_id: Some(8799),
            policy_name: "mask-with-deny".to_string(),
        }
    );
    assert_eq!(
        serde_json::to_value(wire).unwrap(),
        serde_json::json!({
            "kind": "tag_sub_policy_is_deny_capable",
            "policy_id": 8799,
            "policy_name": "mask-with-deny"
        })
    );
}

/// **SP4d-2b Task 5.** The three shapes of `TagNormalisationProblem`, each on the
/// wire as its own `kind`.
///
/// `mask_type_absent` carries no payload and is the one that matters most: it is
/// not "egret cannot decide this", it is "the engine's plugin throws while
/// loading this download". A wire shape that folded it onto the other two would
/// leave the UI nothing to distinguish an editable name from an incident.
#[test]
fn cause_tag_policy_not_normalisable_every_problem() {
    let cases = [
        (
            TagNormalisationProblem::NamespacedAccessType("trino:select".to_string()),
            WireTagNormalisationProblem::NamespacedAccessType {
                access_type: "trino:select".to_string(),
            },
            serde_json::json!({"kind": "namespaced_access_type", "access_type": "trino:select"}),
        ),
        (
            TagNormalisationProblem::NamespacedMaskType("hive:MASK_HASH".to_string()),
            WireTagNormalisationProblem::NamespacedMaskType {
                mask_type: "hive:MASK_HASH".to_string(),
            },
            serde_json::json!({"kind": "namespaced_mask_type", "mask_type": "hive:MASK_HASH"}),
        ),
        (
            TagNormalisationProblem::MaskTypeAbsent,
            WireTagNormalisationProblem::MaskTypeAbsent,
            serde_json::json!({"kind": "mask_type_absent"}),
        ),
    ];
    for (problem, expected, expected_json) in cases {
        assert_eq!(WireTagNormalisationProblem::from(&problem), expected);
        assert_eq!(
            serde_json::to_value(WireTagNormalisationProblem::from(&problem)).unwrap(),
            expected_json
        );
        let cause = UndecidableCause::TagPolicyNotNormalisable {
            policy_id: Some(8901),
            policy_name: "clearance".to_string(),
            problem: problem.clone(),
        };
        assert_eq!(
            WireUndecidableCause::from(&cause),
            WireUndecidableCause::TagPolicyNotNormalisable {
                policy_id: Some(8901),
                policy_name: "clearance".to_string(),
                problem: expected,
            }
        );
    }
}

/// The priority arbitration keeps the three lists POSITIONAL.
///
/// A steward acts on the PRIORITIES, so losing them — or letting them drift out
/// of step with the names — would leave nothing to change. Asserted on the JSON
/// as well as on the enum, because the positional contract is a property of the
/// serialisation and not of the Rust type.
#[test]
fn cause_tag_priority_arbitration_keeps_names_and_priorities_in_step() {
    let cause = UndecidableCause::TagPriorityArbitrationNotModelled {
        policy_ids: vec![Some(8910), None],
        policy_names: vec!["low".to_string(), "high".to_string()],
        priorities: vec![0, 3],
    };
    assert_eq!(
        serde_json::to_value(WireUndecidableCause::from(&cause)).unwrap(),
        serde_json::json!({
            "kind": "tag_priority_arbitration_not_modelled",
            "policy_ids": [8910, null],
            "policy_names": ["low", "high"],
            "priorities": [0, 3],
        })
    );
}

/// The whole-branch review's CRITICAL has its OWN named variant, and the two
/// policies are NAMED rather than positional.
///
/// # ⚠ This assertion changed DIRECTION, and the reason is recorded here
///
/// It read the opposite until SP4d debt Task 7: it pinned this cause as riding
/// [`WireUndecidableCause::TagPriorityArbitrationNotModelled`]'s three positional
/// lists under that variant's own `kind`. That alias was real, deliberate and
/// documented — it let the Critical land without racing `openapi.json`, the
/// generated client and the exhaustive TSX switch — and the final SP4d round then
/// SPLIT it, for the one-word reason the `From` arm in `explain_wire.rs` states:
/// the arbitration sentence opens "Several **tag** policies …", and here one of
/// the two is a RESOURCE policy, so the alias put a wrong NOUN in front of a
/// steward. The test was added by #611, the same squashed commit that performed
/// the split, and it is DB-free — nothing hid it but CI being out of quota with
/// `just check` as the only gate.
///
/// **The code is the current side, confirmed from the contract and not from a
/// comment:** `WireUndecidableCause::AccessPriorityCompositeNotModelled` exists
/// with six named fields, `openapi.json` carries the
/// `access_priority_composite_not_modelled` discriminator, the generated client's
/// `wireUndecidableCause.ts` has that arm, and `ExplainAccess.tsx` switches on it
/// with a sentence of its own. Rewriting the assertion to match the code would be
/// papering over a defect if any of those four disagreed; all four agree.
///
/// What survives unchanged is the property the old assertion was for: which policy
/// is the TAG one must be readable off the wire. A `Vec` cannot say that, so the
/// pairing is now pinned by the field names instead of by position — and that is
/// asserted on the JSON, not only on the enum, because it is a property of the
/// serialisation.
#[test]
fn cause_access_priority_composite_is_its_own_named_variant() {
    let cause = UndecidableCause::AccessPriorityCompositeNotModelled {
        tag_policy_id: Some(901),
        tag_policy_name: "clearance-allow".to_string(),
        tag_priority: 0,
        resource_policy_id: Some(1),
        resource_policy_name: "override-grant".to_string(),
        resource_priority: 1,
    };
    assert_eq!(
        serde_json::to_value(WireUndecidableCause::from(&cause)).unwrap(),
        serde_json::json!({
            "kind": "access_priority_composite_not_modelled",
            "tag_policy_id": 901,
            "tag_policy_name": "clearance-allow",
            "tag_priority": 0,
            "resource_policy_id": 1,
            "resource_policy_name": "override-grant",
            "resource_priority": 1,
        }),
        "the composite must NOT serialise as `tag_priority_arbitration_not_modelled` \
         any more: that `kind` is the one whose page sentence calls both policies tag \
         policies, and one of these two is a resource policy"
    );
}

/// The version split names BOTH policies, and they are not interchangeable: one
/// is the TAG policy that denies and one is the RESOURCE policy that allows, and
/// a UI that swapped them would send a steward to the wrong service entirely.
///
/// It also carries the plugin version, in BOTH of its states (fix round 1,
/// IMPORTANT 4). The UI branches on it because the two need different advice —
/// a known 2.5.0 has nothing to record, and a `null` does — so a wire shape that
/// dropped it would force one sentence to cover both, which is the defect that
/// finding names.
#[test]
fn cause_tag_deny_reopened_carries_both_policies_and_the_plugin_version() {
    let cause = |plugin_version| UndecidableCause::TagDenyReopenedDiffersByPluginVersion {
        plugin_version,
        tag_policy_id: Some(8920),
        tag_policy_name: "clearance-deny".to_string(),
        resource_policy_id: Some(55),
        resource_policy_name: "grant-orders".to_string(),
    };
    assert_eq!(
        serde_json::to_value(WireUndecidableCause::from(&cause(Some(
            PluginVersion::Ranger2_5_0
        ))))
        .unwrap(),
        serde_json::json!({
            "kind": "tag_deny_reopened_differs_by_plugin_version",
            "plugin_version": "ranger_2_5_0",
            "tag_policy_id": 8920,
            "tag_policy_name": "clearance-deny",
            "resource_policy_id": 55,
            "resource_policy_name": "grant-orders",
        })
    );
    // The `None` arm is the one the UI's other branch renders, and a `Serialize`
    // that skipped the key instead of emitting `null` would make the TypeScript
    // field optional-and-absent rather than nullable — a different discriminant.
    assert_eq!(
        serde_json::to_value(WireUndecidableCause::from(&cause(None))).unwrap()["plugin_version"],
        serde_json::Value::Null
    );
}

// ── Decision (3 variants) — and the Vec<why> is not collapsed ───────────

#[test]
fn decision_allowed() {
    let decision = Decision::Allowed {
        policy_id: Some(1),
        policy_name: "allow-me".to_string(),
    };
    let wire = WireDecision::from(&decision);
    assert_eq!(
        wire,
        WireDecision::Allowed {
            policy_id: Some(1),
            policy_name: "allow-me".to_string(),
        }
    );
}

#[test]
fn decision_denied() {
    let decision = Decision::Denied {
        policy_id: Some(2),
        policy_name: "deny-me".to_string(),
        reason: DenyReason::DenyAllElse,
    };
    let wire = WireDecision::from(&decision);
    assert_eq!(
        wire,
        WireDecision::Denied {
            policy_id: Some(2),
            policy_name: "deny-me".to_string(),
            reason: WireDenyReason::DenyAllElse,
        }
    );
}

#[test]
fn decision_undecidable_keeps_every_cause_not_just_the_first() {
    let decision = Decision::Undecidable {
        why: vec![
            UndecidableCause::SuperUsersUnknown,
            UndecidableCause::PseudoAccessType("_admin".to_string()),
        ],
    };
    let wire = WireDecision::from(&decision);
    match wire {
        WireDecision::Undecidable { why } => {
            assert_eq!(why.len(), 2, "collapsing to one cause loses information");
            assert_eq!(why[0], WireUndecidableCause::SuperUsersUnknown);
            assert_eq!(
                why[1],
                WireUndecidableCause::PseudoAccessType {
                    access_type: "_admin".to_string(),
                }
            );
        }
        other => panic!("expected Undecidable, got {other:?}"),
    }
}

// ── MaskDecision (3 variants) ────────────────────────────────────────────

#[test]
fn mask_decision_no_mask() {
    let wire = WireMaskDecision::from(&MaskDecision::NoMask);
    assert_eq!(wire, WireMaskDecision::NoMask);
}

#[test]
fn mask_decision_masked() {
    let result = MaskResult {
        policy_id: Some(9),
        policy_name: "mask-policy".to_string(),
        mask_type: Some("MASK_HASH".to_string()),
        masked_value: None,
        mask_condition: None,
    };
    let wire = WireMaskDecision::from(&MaskDecision::Masked(result));
    assert_eq!(
        wire,
        WireMaskDecision::Masked {
            result: WireMaskResult {
                policy_id: Some(9),
                policy_name: "mask-policy".to_string(),
                mask_type: Some("MASK_HASH".to_string()),
                masked_value: None,
                mask_condition: None,
            },
        }
    );
}

#[test]
fn mask_decision_undecidable() {
    let wire = WireMaskDecision::from(&MaskDecision::Undecidable {
        why: vec![UndecidableCause::SuperUsersUnknown],
    });
    match wire {
        WireMaskDecision::Undecidable { why } => {
            assert_eq!(why, vec![WireUndecidableCause::SuperUsersUnknown]);
        }
        other => panic!("expected Undecidable, got {other:?}"),
    }
}

// ── RowFilterDecision (3 variants) ───────────────────────────────────────

#[test]
fn row_filter_decision_no_filter() {
    let wire = WireRowFilterDecision::from(&RowFilterDecision::NoFilter);
    assert_eq!(wire, WireRowFilterDecision::NoFilter);
}

#[test]
fn row_filter_decision_filtered() {
    let result = RowFilterResult {
        policy_id: Some(11),
        policy_name: "rf-policy".to_string(),
        filter_expr: "region = 'us'".to_string(),
    };
    let wire = WireRowFilterDecision::from(&RowFilterDecision::Filtered(result));
    assert_eq!(
        wire,
        WireRowFilterDecision::Filtered {
            result: WireRowFilterResult {
                policy_id: Some(11),
                policy_name: "rf-policy".to_string(),
                filter_expr: "region = 'us'".to_string(),
            },
        }
    );
}

#[test]
fn row_filter_decision_undecidable() {
    let wire = WireRowFilterDecision::from(&RowFilterDecision::Undecidable {
        why: vec![UndecidableCause::SuperUsersUnknown],
    });
    match wire {
        WireRowFilterDecision::Undecidable { why } => {
            assert_eq!(why, vec![WireUndecidableCause::SuperUsersUnknown]);
        }
        other => panic!("expected Undecidable, got {other:?}"),
    }
}

// ── ItemBucket (6 variants) ──────────────────────────────────────────────

#[test]
fn item_bucket_every_variant_round_trips() {
    let cases = [
        (ItemBucket::PolicyItems, WireItemBucket::PolicyItems),
        (ItemBucket::DenyPolicyItems, WireItemBucket::DenyPolicyItems),
        (ItemBucket::AllowExceptions, WireItemBucket::AllowExceptions),
        (ItemBucket::DenyExceptions, WireItemBucket::DenyExceptions),
        (
            ItemBucket::DataMaskPolicyItems,
            WireItemBucket::DataMaskPolicyItems,
        ),
        (
            ItemBucket::RowFilterPolicyItems,
            WireItemBucket::RowFilterPolicyItems,
        ),
    ];
    for (source, expected) in cases {
        assert_eq!(WireItemBucket::from(source), expected);
    }
}

// ── TagPolicyBlock (2 variants) ──────────────────────────────────────────

#[test]
fn tag_policy_block_policies_carries_the_count_as_a_number() {
    let wire = WireTagPolicyBlock::from(&TagPolicyBlock::Policies(7));
    assert_eq!(wire, WireTagPolicyBlock::Policies { count: 7 });
    assert_eq!(
        serde_json::to_value(&wire).unwrap(),
        serde_json::json!({"kind": "policies", "count": 7})
    );
}

#[test]
fn tag_policy_block_unreadable() {
    let wire = WireTagPolicyBlock::from(&TagPolicyBlock::Unreadable);
    assert_eq!(wire, WireTagPolicyBlock::Unreadable);
}

// ── Unmodelled (15 variants) ──────────────────────────────────────────────

#[test]
fn unmodelled_validity_schedules() {
    assert_eq!(
        WireUnmodelled::from(&Unmodelled::ValiditySchedules),
        WireUnmodelled::ValiditySchedules
    );
}

#[test]
fn unmodelled_policy_conditions() {
    assert_eq!(
        WireUnmodelled::from(&Unmodelled::PolicyConditions),
        WireUnmodelled::PolicyConditions
    );
}

#[test]
fn unmodelled_item_conditions() {
    let source = Unmodelled::ItemConditions {
        bucket: ItemBucket::DenyExceptions,
        index: 4,
    };
    assert_eq!(
        WireUnmodelled::from(&source),
        WireUnmodelled::ItemConditions {
            bucket: WireItemBucket::DenyExceptions,
            index: 4,
        }
    );
}

#[test]
fn unmodelled_zone_name() {
    let source = Unmodelled::ZoneName("finance-zone".to_string());
    assert_eq!(
        WireUnmodelled::from(&source),
        WireUnmodelled::ZoneName {
            zone_name: "finance-zone".to_string(),
        }
    );
}

#[test]
fn unmodelled_security_zones() {
    assert_eq!(
        WireUnmodelled::from(&Unmodelled::SecurityZones),
        WireUnmodelled::SecurityZones
    );
}

#[test]
fn unmodelled_tag_policies_carries_the_block_not_a_string() {
    let source = Unmodelled::TagPolicies(TagPolicyBlock::Policies(3));
    assert_eq!(
        WireUnmodelled::from(&source),
        WireUnmodelled::TagPolicies {
            block: WireTagPolicyBlock::Policies { count: 3 },
        }
    );
}

#[test]
fn unmodelled_policy_deltas() {
    assert_eq!(
        WireUnmodelled::from(&Unmodelled::PolicyDeltas),
        WireUnmodelled::PolicyDeltas
    );
}

#[test]
fn unmodelled_service_config() {
    assert_eq!(
        WireUnmodelled::from(&Unmodelled::ServiceConfig),
        WireUnmodelled::ServiceConfig
    );
}

#[test]
fn unmodelled_mask_expression_marker() {
    let source = Unmodelled::MaskExpressionMarker {
        bucket: ItemBucket::DataMaskPolicyItems,
        item_index: 2,
        expr: "${{ ctx.tbl }}".to_string(),
    };
    assert_eq!(
        WireUnmodelled::from(&source),
        WireUnmodelled::MaskExpressionMarker {
            bucket: WireItemBucket::DataMaskPolicyItems,
            item_index: 2,
            expr: "${{ ctx.tbl }}".to_string(),
        }
    );
}

#[test]
fn unmodelled_mask_condition_expr() {
    let source = Unmodelled::MaskConditionExpr {
        bucket: ItemBucket::DataMaskPolicyItems,
        item_index: 1,
    };
    assert_eq!(
        WireUnmodelled::from(&source),
        WireUnmodelled::MaskConditionExpr {
            bucket: WireItemBucket::DataMaskPolicyItems,
            item_index: 1,
        }
    );
}

#[test]
fn unmodelled_row_filter_expression_marker() {
    let source = Unmodelled::RowFilterExpressionMarker {
        bucket: ItemBucket::RowFilterPolicyItems,
        item_index: 0,
        expr: "${{ ctx.user }}".to_string(),
    };
    assert_eq!(
        WireUnmodelled::from(&source),
        WireUnmodelled::RowFilterExpressionMarker {
            bucket: WireItemBucket::RowFilterPolicyItems,
            item_index: 0,
            expr: "${{ ctx.user }}".to_string(),
        }
    );
}

#[test]
fn unmodelled_resource_value_expression_marker() {
    let source = Unmodelled::ResourceValueExpressionMarker {
        additional_index: Some(2),
        resource_type: "column".to_string(),
        value: "{col}".to_string(),
    };
    assert_eq!(
        WireUnmodelled::from(&source),
        WireUnmodelled::ResourceValueExpressionMarker {
            additional_index: Some(2),
            resource_type: "column".to_string(),
            value: "{col}".to_string(),
        }
    );
}

#[test]
fn unmodelled_user_expression_marker() {
    let source = Unmodelled::UserExpressionMarker {
        bucket: ItemBucket::PolicyItems,
        index: 0,
        user: "{OWNER}".to_string(),
    };
    assert_eq!(
        WireUnmodelled::from(&source),
        WireUnmodelled::UserExpressionMarker {
            bucket: WireItemBucket::PolicyItems,
            index: 0,
            user: "{OWNER}".to_string(),
        }
    );
}

#[test]
fn unmodelled_explicit_deny() {
    let source = Unmodelled::ExplicitDeny {
        bucket: ItemBucket::AllowExceptions,
        index: 3,
        access_type: "select".to_string(),
    };
    assert_eq!(
        WireUnmodelled::from(&source),
        WireUnmodelled::ExplicitDeny {
            bucket: WireItemBucket::AllowExceptions,
            index: 3,
            access_type: "select".to_string(),
        }
    );
}

#[test]
fn unmodelled_delegate_admin() {
    let source = Unmodelled::DelegateAdmin {
        bucket: ItemBucket::PolicyItems,
        index: 1,
    };
    assert_eq!(
        WireUnmodelled::from(&source),
        WireUnmodelled::DelegateAdmin {
            bucket: WireItemBucket::PolicyItems,
            index: 1,
        }
    );
}

// ── PolicyType (3 variants) ──────────────────────────────────────────────

#[test]
fn policy_type_every_variant_round_trips() {
    assert_eq!(
        WirePolicyType::from(PolicyType::Access),
        WirePolicyType::Access
    );
    assert_eq!(
        WirePolicyType::from(PolicyType::DataMask),
        WirePolicyType::DataMask
    );
    assert_eq!(
        WirePolicyType::from(PolicyType::RowFilter),
        WirePolicyType::RowFilter
    );
}

// ── GraphProblem (3 variants) ────────────────────────────────────────────

#[test]
fn graph_problem_no_sources_or_sinks() {
    assert_eq!(
        WireGraphProblem::from(&GraphProblem::NoSourcesOrSinks),
        WireGraphProblem::NoSourcesOrSinks
    );
}

#[test]
fn graph_problem_sink_is_not_a_valid_leaf() {
    let source = GraphProblem::SinkIsNotAValidLeaf {
        resource: "table".to_string(),
    };
    assert_eq!(
        WireGraphProblem::from(&source),
        WireGraphProblem::SinkIsNotAValidLeaf {
            resource: "table".to_string(),
        }
    );
}

#[test]
fn graph_problem_broken_parent_chain() {
    let source = GraphProblem::BrokenParentChain {
        resource: "schema".to_string(),
    };
    assert_eq!(
        WireGraphProblem::from(&source),
        WireGraphProblem::BrokenParentChain {
            resource: "schema".to_string(),
        }
    );
}

// ── ServiceDefIssue (4 variants; InvalidResourceGraph already covered by
// cause_service_def_carries_the_full_typed_issue above) ──────────────────

#[test]
fn service_def_issue_unmodelled_matcher_class() {
    let source = ServiceDefIssue::UnmodelledMatcherClass {
        resource: "table".to_string(),
        matcher: "com.example.CustomMatcher".to_string(),
    };
    assert_eq!(
        WireServiceDefIssue::from(&source),
        WireServiceDefIssue::UnmodelledMatcherClass {
            resource: "table".to_string(),
            matcher: "com.example.CustomMatcher".to_string(),
        }
    );
}

#[test]
fn service_def_issue_quoted_case_sensitive() {
    let source = ServiceDefIssue::QuotedCaseSensitive {
        resource: "column".to_string(),
    };
    assert_eq!(
        WireServiceDefIssue::from(&source),
        WireServiceDefIssue::QuotedCaseSensitive {
            resource: "column".to_string(),
        }
    );
}

#[test]
fn service_def_issue_chained_implied_grants() {
    let source = ServiceDefIssue::ChainedImpliedGrants {
        implying: "all".to_string(),
        implied: "select".to_string(),
    };
    assert_eq!(
        WireServiceDefIssue::from(&source),
        WireServiceDefIssue::ChainedImpliedGrants {
            implying: "all".to_string(),
            implied: "select".to_string(),
        }
    );
}
