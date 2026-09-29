//! SP4d-2a Task 4: conversion tests for the hand-written wire DTOs that
//! mirror `egret_ranger_eval::eval`'s three-valued `Decision`, `DenyReason`,
//! `UndecidableCause`, and the mask / row-filter results.
//!
//! **Why one test per source variant.** The whole point of hand-writing
//! these conversions (rather than deriving `ToSchema` on the source types
//! directly, which would require adding `utoipa` to `egret-ranger-eval` and
//! giving up its `serde`-only discipline) is that the `From` impls match
//! with no wildcard arm — a variant added upstream fails the BUILD, not a
//! test. This file cannot exercise that compile-time property directly, but
//! it can and does prove the mapping is correct for every variant that
//! exists today, so a reviewer diffing this file against a future
//! `eval.rs` can see at a glance which arm is missing.
//!
//! Pure conversions, no I/O — no `DATABASE_URL` needed.

use egret_perm_compile::explain_wire::{
    WireDecision, WireDenyReason, WireEngineType, WireMaskDecision, WireMaskResult,
    WirePluginVersion, WireRowFilterDecision, WireRowFilterResult, WireUndecidableCause,
};
use egret_ranger_eval::eval::{
    Decision, DenyReason, EngineType, MaskDecision, MaskResult, PluginVersion, RowFilterDecision,
    RowFilterResult, UndecidableCause,
};
use egret_ranger_eval::model::Unmodelled;
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

#[test]
fn plugin_version_both_variants() {
    assert_eq!(
        WirePluginVersion::from(PluginVersion::Ranger2_5_0),
        WirePluginVersion::Ranger2_5_0
    );
    assert_eq!(
        WirePluginVersion::from(PluginVersion::Ranger2_8_0),
        WirePluginVersion::Ranger2_8_0
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

// ── UndecidableCause (13 variants) ───────────────────────────────────────

#[test]
fn cause_envelope_construct_carries_a_description() {
    let cause = UndecidableCause::EnvelopeConstruct(Unmodelled::ServiceConfig);
    let wire = WireUndecidableCause::from(&cause);
    match wire {
        WireUndecidableCause::EnvelopeConstruct { construct } => {
            assert_eq!(construct, format!("{:?}", Unmodelled::ServiceConfig));
        }
        other => panic!("expected EnvelopeConstruct, got {other:?}"),
    }
}

#[test]
fn cause_policy_construct_carries_policy_id_name_and_description() {
    let cause = UndecidableCause::PolicyConstruct {
        policy_id: Some(42),
        policy_name: "p1".to_string(),
        construct: Unmodelled::ValiditySchedules,
    };
    let wire = WireUndecidableCause::from(&cause);
    match wire {
        WireUndecidableCause::PolicyConstruct {
            policy_id,
            policy_name,
            construct,
        } => {
            assert_eq!(policy_id, Some(42));
            assert_eq!(policy_name, "p1");
            assert_eq!(construct, format!("{:?}", Unmodelled::ValiditySchedules));
        }
        other => panic!("expected PolicyConstruct, got {other:?}"),
    }
}

#[test]
fn cause_service_def_carries_a_description() {
    let issue = ServiceDefIssue::InvalidResourceGraph {
        policy_type: PolicyType::Access,
        reason: GraphProblem::NoSourcesOrSinks,
    };
    let cause = UndecidableCause::ServiceDef(issue.clone());
    let wire = WireUndecidableCause::from(&cause);
    match wire {
        WireUndecidableCause::ServiceDef { issue: got } => {
            assert_eq!(got, format!("{issue:?}"));
        }
        other => panic!("expected ServiceDef, got {other:?}"),
    }
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
    };
    let wire = WireUndecidableCause::from(&cause);
    assert_eq!(
        wire,
        WireUndecidableCause::EvalOrderAmbiguous {
            policy_ids: vec![Some(1), None, Some(3)],
            policy_names: vec!["a".to_string(), "b".to_string(), "c".to_string()],
        }
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
        measured: PluginVersion::Ranger2_8_0,
    };
    let wire = WireUndecidableCause::from(&cause);
    assert_eq!(
        wire,
        WireUndecidableCause::PluginVersionMismatch {
            engine: WireEngineType::StarRocks,
            declared: WirePluginVersion::Ranger2_5_0,
            measured: WirePluginVersion::Ranger2_8_0,
        }
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
