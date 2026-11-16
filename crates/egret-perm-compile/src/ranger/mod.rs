//! The Ranger target (governance sub-project 1): renders the neutral
//! statements `egret_perm_ir::lower` produces into Ranger policy bodies. It
//! renders shape only; every decision (scope, skip, refusal) was made by the
//! lowering.

use std::collections::BTreeMap;

use egret_perm::CompileError;
use egret_perm_ir::statement::{Lowered, Lowering, Source};
use egret_policy::admin::access_to_ranger_json;
use serde_json::Value;
use uuid::Uuid;

use crate::ir_load::Loaded;
use crate::trigger::{
    body_name, target_service_id, DesiredPolicy, PolicyKey, SkippedItem, SourceKind,
};

pub(crate) mod completion;
pub(crate) mod engine_privileges;
pub(crate) mod grants;
pub(crate) mod infra;
pub(crate) mod rules;
pub(crate) mod withhold;

/// One statement rendered for one Ranger service: the policy body, its name
/// (the desired-set key) and where it came from.
pub(crate) struct Rendered {
    pub body: Value,
    pub name: String,
    pub source_kind: SourceKind,
    pub grant_id: Option<Uuid>,
    pub rule_id: Option<Uuid>,
}

impl Rendered {
    pub(crate) fn into_desired(self) -> DesiredPolicy {
        DesiredPolicy {
            body: self.body,
            source_kind: self.source_kind,
            grant_id: self.grant_id,
            rule_id: self.rule_id,
        }
    }
}

/// A lowering as the recompile reports it: the source's kind and id, and the
/// lowering's text verbatim.
pub(crate) fn skipped(l: &Lowering) -> SkippedItem {
    let (source_kind, id) = match &l.source {
        Source::Grant { grant_id } => (SourceKind::Grant, *grant_id),
        Source::Rule { rule_id, .. } => (SourceKind::Rule, *rule_id),
        Source::Withhold {
            classification_id, ..
        } => (SourceKind::Clearance, *classification_id),
        Source::EnginePrivilege { engine_grant_id } => (SourceKind::EngineGrant, *engine_grant_id),
        // `lower_infra` produces no lowerings, so this arm is never reached.
        Source::Infra { .. } => (SourceKind::Infra, Uuid::nil()),
    };
    SkippedItem {
        source_kind,
        id,
        reason: l.detail.clone(),
    }
}

/// Render one family's lowering for the profile `l` loaded: each statement
/// into the desired set, under the service its body names (one the profile may
/// write into, `l.writable`), and each lowering into `skips`, in order. An
/// engine grant's skip is also logged here, the lowering having no service
/// id.
pub(crate) fn emit(
    l: &Loaded,
    lowered: Lowered,
    desired: &mut BTreeMap<PolicyKey, DesiredPolicy>,
    skips: &mut Vec<SkippedItem>,
) -> Result<(), CompileError> {
    let engine = l.ep.kind.engine;
    let profile = l.profile_name.as_str();
    for s in &lowered.statements {
        if egret_perm_ir::lower::grants::is_browse_only(s) {
            continue; // lowered: no Ranger access type to emit (a direct browse DENY is dropped too: nothing enforces browse yet)
        }
        let (key, policy) = match &s.source {
            Source::Grant { .. } => {
                let r = grants::render(s, profile, engine)?;
                let target = target_service_id(
                    &r.body,
                    &l.writable,
                    &format!("grant {}", r.grant_id.expect("a grant statement")),
                )?;
                ((target, r.name.clone()), r.into_desired())
            }
            Source::EnginePrivilege { engine_grant_id } => {
                let input = engine_privileges::render(s, profile, engine)?;
                let name = input.name.clone();
                let body = rules::stamp_service_type(access_to_ranger_json(&input), engine);
                let target = target_service_id(
                    &body,
                    &l.writable,
                    &format!("engine grant {engine_grant_id}"),
                )?;
                (
                    (target, name),
                    DesiredPolicy {
                        body,
                        source_kind: SourceKind::EngineGrant,
                        grant_id: None,
                        rule_id: None,
                    },
                )
            }
            Source::Rule { rule_id, rule_name } => {
                let body = rules::render(s, profile, l.tag_service.as_deref(), engine)?;
                let what = format!("rule {rule_name:?}");
                let name = body_name(&body, &what)?;
                // A TAG body names the tag service here, a path body the
                // profile — the routing that SP4a Task 10 found missing.
                let target = target_service_id(&body, &l.writable, &what)?;
                (
                    (target, name),
                    DesiredPolicy {
                        body,
                        source_kind: SourceKind::Rule,
                        grant_id: None,
                        rule_id: Some(*rule_id),
                    },
                )
            }
            Source::Withhold { .. } => {
                let (name, body) = withhold::render(s, &l.ep, profile, l.tag_service.as_deref())?;
                let target = target_service_id(&body, &l.writable, &format!("clearance {name}"))?;
                (
                    (target, name),
                    DesiredPolicy {
                        body,
                        source_kind: SourceKind::Clearance,
                        grant_id: None,
                        rule_id: None,
                    },
                )
            }
            Source::Infra { .. } => {
                return Err(CompileError::Db(
                    "infrastructure statements are converged by infra::converge_infra_in_tx, never emitted"
                        .into(),
                ));
            }
        };
        desired.insert(key, policy);
    }
    for low in &lowered.lowerings {
        let item = skipped(low);
        if item.source_kind == SourceKind::EngineGrant {
            tracing::warn!(
                service_id = l.ep.profile_id,
                engine_grant_id = %item.id,
                reason = %item.reason,
                "engine grant skipped during recompile"
            );
        }
        skips.push(item);
    }
    Ok(())
}

#[cfg(test)]
mod browse_lowering_tests {
    use super::*;
    use egret_perm::{
        Action, Effect, EngineType, Grant, GrantOrigin, Principal, PrincipalKind, PrincipalSet,
        Scope,
    };
    use egret_perm_ir::audience::Directory;
    use egret_perm_ir::ep::{EpInput, EpKind, Mechanism};

    #[test]
    fn implied_browse_is_lowered_and_the_read_policy_is_unchanged() {
        let c = Uuid::from_u128(9);
        let ep = EpInput {
            profile_id: 1,
            profile_name: "p".into(),
            kind: EpKind {
                engine: EngineType::Trino,
                mechanism: Mechanism::Ranger,
            },
            engines: vec![],
            default_version: Err("unread".into()),
            default_image_level: None,
            classification_channel: false,
            catalogs: [(c, "sales".to_string())].into_iter().collect(),
        };
        let g = Grant {
            id: Uuid::from_u128(1),
            effect: Effect::Allow,
            actions: vec![Action::Read],
            scope: Scope {
                catalog_id: c,
                schema: Some("s".into()),
                table: Some("t".into()),
                column: None,
            },
            principals: PrincipalSet {
                include: vec![Principal {
                    kind: PrincipalKind::User,
                    r#ref: "a".into(),
                }],
                exclude: vec![],
            },
            origin: GrantOrigin::Explicit,
        };
        let lowered = egret_perm_ir::lower::grants::lower_grants(
            &ep,
            std::slice::from_ref(&g),
            &Directory::default(),
        )
        .unwrap();
        assert_eq!(
            lowered.statements.len(),
            4,
            "the Read plus browse on catalog, schema and table"
        );
        let kept: Vec<_> = lowered
            .statements
            .iter()
            .filter(|s| !egret_perm_ir::lower::grants::is_browse_only(s))
            .collect();
        assert_eq!(kept.len(), 1, "every implied browse is lowered away");
        assert!(matches!(
            &kept[0].effect,
            egret_perm_ir::statement::Effect::Allow { privileges }
                if privileges == &[egret_perm_ir::statement::Privilege::Read]
        ));
        let body = grants::render(kept[0], "trino-1", EngineType::Trino).unwrap();
        let direct =
            crate::ranger::grants::compile_grant(&g, "sales", "trino-1", EngineType::Trino)
                .unwrap();
        assert_eq!(body.name, direct.name);
        // Every statement, implied browse included, renders without error or is skipped.
        for s in &lowered.statements {
            if !egret_perm_ir::lower::grants::is_browse_only(s) {
                grants::render(s, "trino-1", EngineType::Trino).unwrap();
            }
        }
    }
}
