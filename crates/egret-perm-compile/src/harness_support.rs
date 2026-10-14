//! Test support for `egret-ranger-eval`'s differential (view governance, Task 3).
//!
//! The differential asks the real Ranger jars about the bodies egret emits, so
//! it must be handed the bodies the compiler's own functions produce, not a
//! hand-copied JSON. `fold_clearance_mask` and `mv_deny_inputs` are
//! crate-private; these wrappers are the single, doc-hidden crossing. They add
//! no logic of their own.

use serde_json::Value;
use uuid::Uuid;

use egret_perm::{CompileError, EngineType};
use egret_policy::admin::access_to_ranger_json;
use egret_ranger_eval::eval::PluginVersion;

use crate::clearance::mv_deny_inputs;
use crate::clearance_mask::{clearance_mask_name, fold_clearance_mask, PendingMask};
use crate::rules::{stamp_service_type, TaggedColumn};

/// The StarRocks clearance mask body for `column`, folded over `others`.
pub fn starrocks_clearance_mask(
    service: &str,
    tag_id: Uuid,
    column: &TaggedColumn,
    cleared: &[String],
    others: &[Value],
    plugin: PluginVersion,
) -> Result<Value, CompileError> {
    fold_clearance_mask(
        &clearance_mask_name(tag_id, column),
        service,
        column,
        cleared,
        others,
        EngineType::StarRocks,
        plugin,
    )
}

/// The `create materialized view` deny bodies for the given (column, cleared)
/// pairs, stamped as `compile_clearance` stamps them: at most one, on
/// `default_catalog`/`*` (A3 revised 2026-10-14).
pub fn starrocks_mv_denies(service: &str, pending: &[(TaggedColumn, Vec<String>)]) -> Vec<Value> {
    let pending: Vec<PendingMask> = pending
        .iter()
        .map(|(column, cleared)| PendingMask {
            tag_id: Uuid::nil(),
            column: column.clone(),
            cleared: cleared.clone(),
        })
        .collect();
    mv_deny_inputs(&pending, service)
        .into_iter()
        .map(|(_, input)| stamp_service_type(access_to_ranger_json(&input), EngineType::StarRocks))
        .collect()
}
