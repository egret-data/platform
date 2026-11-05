//! Test support for `egret-ranger-eval`'s differential (view governance, Task 3).
//!
//! The differential asks the real Ranger jars about the bodies egret emits, so
//! it must be handed the bodies the compiler's own functions produce, not a
//! hand-copied JSON. `fold_clearance_mask` and the `ranger::withhold`
//! renderers are crate-private; these wrappers are the single, doc-hidden crossing. They add
//! no logic of their own.

use serde_json::Value;
use uuid::Uuid;

use egret_perm::{CompileError, EngineType};
use egret_policy::admin::access_to_ranger_json;
use egret_ranger_eval::eval::PluginVersion;

use egret_perm_ir::ep::{EpKind, Mechanism};
use egret_perm_ir::lower::withhold::{cleared_for_every, withheld_privileges};

use crate::clearance_mask::{clearance_mask_name, fold_clearance_mask};
use crate::ranger::grants::read_write_accesses;
use crate::ranger::rules::stamp_service_type;
use crate::ranger::withhold::{mv_deny_input, starrocks_deny, MV_CATALOG, WHOLE_TABLE_COLUMN};
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
/// pairs, as the recompile builds them: at most one, on `default_catalog`/`*`
/// (A3 revised 2026-10-14), excepting the users cleared for every pair
/// (`egret_perm_ir::lower::withhold::cleared_for_every`).
pub fn starrocks_mv_denies(service: &str, pending: &[(TaggedColumn, Vec<String>)]) -> Vec<Value> {
    let cleared: Vec<Vec<String>> = pending.iter().map(|(_, c)| c.clone()).collect();
    cleared_for_every(&cleared)
        .into_iter()
        .map(|cleared| {
            let (_, input) = mv_deny_input(MV_CATALOG, cleared, service);
            stamp_service_type(access_to_ranger_json(&input), EngineType::StarRocks)
        })
        .collect()
}

/// The whole-table clearance deny (D2, user decision U1) on
/// `{catalog}.{schema}.{table}`, excepting `cleared`, as the recompile
/// builds it: the privileges the IR withholds on StarRocks (`select`) denied
/// to `{public}` on `column: *`.
pub fn starrocks_table_clearance_deny(
    service: &str,
    tag_id: Uuid,
    catalog: &str,
    schema: &str,
    table: &str,
    cleared: &[String],
) -> Result<Value, CompileError> {
    let all = TaggedColumn::new(catalog, schema, table, WHOLE_TABLE_COLUMN)?;
    let privileges = withheld_privileges(EpKind {
        engine: EngineType::StarRocks,
        mechanism: Mechanism::Ranger,
    });
    let (_, body) = starrocks_deny(
        tag_id,
        service,
        &all,
        &read_write_accesses(&privileges, EngineType::StarRocks, "harness_support")?,
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
    Ok(
        crate::ir_load::load_ep(conn, service_id, crate::members::MissingUsername::Mint)
            .await?
            .map(|l| LoadedForTest {
                ep: l.ep,
                sweep: l.sweep,
            }),
    )
}

/// The OPA snapshot of `service_id` as the refresher builds it: lowered with
/// `MissingUsername::Omit`, so nothing is minted (governance SP2b Task 6).
pub async fn build_snapshot_for_test(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<Option<egret_perm_eval::Snapshot>, CompileError> {
    crate::opa::build::build_snapshot_via(conn, service_id, crate::members::MissingUsername::Omit)
        .await
}

/// The policy download `routes::served_artifact_via` evaluates for explain, as
/// the JSON the plugin receives: `tagPolicies` included, and the lineage
/// transform applied when the profile's first engine is eligible (governance
/// SP2b Task 18, the OPA differential's Ranger side).
pub async fn served_artifact_json_via(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<Value, CompileError> {
    let svc = egret_policy::get_service_by_id_via(conn, service_id)
        .await
        .map_err(crate::db_error::from_store)?;
    let envelope = crate::routes::served_envelope_via(conn, &svc)
        .await
        .map_err(|e| CompileError::Db(format!("served artifact: {e}")))?;
    serde_json::to_value(&envelope).map_err(|e| CompileError::Db(format!("served artifact: {e}")))
}
