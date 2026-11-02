//! The Ranger target (governance sub-project 1): renders the neutral
//! statements `egret_perm_ir::lower` produces into Ranger policy bodies. It
//! renders shape only; every decision (scope, skip, refusal) was made by the
//! lowering.

use egret_perm_ir::statement::{Lowering, Source};
use serde_json::Value;
use uuid::Uuid;

use crate::trigger::{DesiredPolicy, SkippedItem, SourceKind};

pub(crate) mod grants;

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
    };
    SkippedItem {
        source_kind,
        id,
        reason: l.detail.clone(),
    }
}
