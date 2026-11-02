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

use crate::clearance::{mv_deny_inputs, starrocks_deny, withheld_accesses, WHOLE_TABLE_COLUMN};
use crate::clearance_mask::{clearance_mask_name, fold_clearance_mask};
use crate::ranger::rules::stamp_service_type;
use crate::rules::TaggedColumn;

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
    let cleared: Vec<Vec<String>> = pending.iter().map(|(_, c)| c.clone()).collect();
    mv_deny_inputs(&cleared, service)
        .into_iter()
        .map(|(_, input)| stamp_service_type(access_to_ranger_json(&input), EngineType::StarRocks))
        .collect()
}

/// The whole-table clearance deny (D2, user decision U1) on
/// `{catalog}.{schema}.{table}`, excepting `cleared`, as `compile_clearance`
/// builds it: `select` denied to `{public}` on `column: *`.
pub fn starrocks_table_clearance_deny(
    service: &str,
    tag_id: Uuid,
    catalog: &str,
    schema: &str,
    table: &str,
    cleared: &[String],
) -> Result<Value, CompileError> {
    let all = TaggedColumn::new(catalog, schema, table, WHOLE_TABLE_COLUMN)?;
    let (_, body) = starrocks_deny(
        tag_id,
        service,
        &all,
        &withheld_accesses(EngineType::StarRocks)?,
        cleared.to_vec(),
    );
    Ok(body)
}

/// What one profile compiles to, for the golden corpus (governance
/// sub-project 1): every desired policy as `(service name, policy name, body)`
/// in the compiler's own order, every skip as `(kind, id, reason)` in order,
/// and the services the recompile sweeps.
pub struct GoldenCompiled {
    pub desired: Vec<(String, String, Value)>,
    pub skipped: Vec<(String, Uuid, String)>,
    pub sweep: Vec<i64>,
}

/// Compile `service_id` on `conn` without writing anything. `None` when the
/// compiler does not target the service (a tag service, say).
pub async fn golden_desired_in_tx(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<Option<GoldenCompiled>, CompileError> {
    let Some(c) = crate::trigger::compile_desired(conn, service_id).await? else {
        return Ok(None);
    };
    let mut names: std::collections::HashMap<i64, String> = std::collections::HashMap::new();
    for sid in &c.sweep {
        let n: String = sqlx::query_scalar("SELECT name FROM policy.authz_services WHERE id = $1")
            .bind(sid)
            .fetch_one(&mut *conn)
            .await
            .map_err(CompileError::from_db)?;
        names.insert(*sid, n);
    }
    Ok(Some(GoldenCompiled {
        desired: c
            .desired
            .iter()
            .map(|((sid, name), dp)| (names[sid].clone(), name.clone(), dp.body.clone()))
            .collect(),
        skipped: c
            .skipped
            .iter()
            .map(|s| (format!("{:?}", s.source_kind), s.id, s.reason.clone()))
            .collect(),
        sweep: c.sweep.clone(),
    }))
}

/// What `ir_load::load_ep` read for one profile, for the loader's own test
/// (governance sub-project 1, Task 4): the enforcement point and the sweep set.
pub struct LoadedForTest {
    pub ep: egret_perm_ir::ep::EpInput,
    pub sweep: Vec<i64>,
}

/// Run `ir_load::load_ep` on `conn`. `None` when the compiler does not target
/// the service.
pub async fn load_ep_for_test(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<Option<LoadedForTest>, CompileError> {
    Ok(crate::ir_load::load_ep(conn, service_id)
        .await?
        .map(|l| LoadedForTest {
            ep: l.ep,
            sweep: l.sweep,
        }))
}
