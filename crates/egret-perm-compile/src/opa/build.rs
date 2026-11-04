//! One profile's OPA snapshot (governance SP2b, D3): the four statement
//! families [`crate::trigger::lower_ep_via`] lowers for the Ranger compile,
//! the infrastructure statements (`load_infra_inputs` + `lower_infra`, SP2a),
//! and the classified columns of the profile's catalogs, read at the caller's
//! transaction and frozen into an [`egret_perm_eval::Snapshot`].
//!
//! **Infrastructure is lowered on its own** ([`lower_infra_via`], C4): when
//! the data lowering fails, the refresher splices fresh infrastructure into
//! the previous snapshot (`Snapshot::with_infra`), so the SP2a kill switch
//! still removes a disabled admin's all-access.
//!
//! **Nothing here writes** when called with [`MissingUsername::Omit`] (I9):
//! the refresher builds in a `READ ONLY` transaction, and a person with no
//! username yet is left out rather than minted for.
//!
//! **Why leaving them out is safe — and what it depends on.** A login or
//! first-query mint recompiles nothing (`crate::members::role_usernames`'
//! doc, design R5/R6), so a username appearing later does not by itself move
//! this snapshot's key. What closes the gap is the compile: every recompile
//! first mints a username for every person the model names
//! (`crate::members::premint_model_usernames`, called by every
//! `crate::trigger` recompile entry after the profile locks), and a
//! recompile that fails rolls back with the write that caused it. So no
//! committed state holds an unnamed person the model names — in particular
//! none in a grant-`Deny` include or an `Allow` except, the only positions
//! where leaving a person out would loosen the answer. `Omit` therefore
//! drops only people a Ranger compile of the same state would not name
//! either (legacy rows written before R5, a transient row no recompile has
//! seen), and the Ranger artifact agrees until the next recompile mints them
//! and bumps the key. **This holds only while every model and membership
//! write keeps running the Ranger recompile, premint included, for OPA
//! profiles too**; `tests/opa_snapshot.rs`
//! (`an_unnamed_deny_holder_is_denied_once_a_recompile_names_them`) pins it.

use egret_perm::CompileError;
use egret_perm_eval::{ClassifiedColumn, Snapshot, SnapshotInput};
use egret_perm_ir::statement::Statement;

pub use crate::members::MissingUsername;

/// What one snapshot is built from, before `Snapshot::build`.
pub struct SnapshotParts {
    pub key: (i64, i64),
    /// grants, engine privileges, rules, withholds — everything but infrastructure.
    pub data: Vec<Statement>,
    pub infra: Vec<Statement>,
    pub classified: Vec<ClassifiedColumn>,
}

/// Every classification on a column or a whole table of the catalogs bound to
/// `service_id`: the rows the tag download serves, through the store's one
/// spelling of engine reachability (`catalogs_for_service_via`) and its
/// assignment read (`tag_assignments_for_catalogs_via`) — no SQL of its own
/// (C3). A table-level assignment is stored with `column_name = ''`
/// (`migrations/0081_catalog_tag_targets.sql`, `NOT NULL DEFAULT ''`), never
/// NULL; it becomes `column: None`.
pub async fn load_classified_columns_via(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<Vec<ClassifiedColumn>, CompileError> {
    let catalogs = egret_policy::catalogs_for_service_via(conn, service_id)
        .await
        .map_err(crate::db_error::from_store)?;
    let catalog_ids: Vec<uuid::Uuid> = catalogs.iter().map(|c| c.id).collect();
    let rows = egret_policy::tag_assignments_for_catalogs_via(conn, &catalog_ids)
        .await
        .map_err(crate::db_error::from_store)?;
    Ok(rows
        .into_iter()
        .map(|r| ClassifiedColumn {
            catalog: r.catalog,
            schema: r.schema_name,
            table: r.table_name,
            column: (!r.column_name.is_empty()).then_some(r.column_name),
            classification: r.tag_name,
        })
        .collect())
}

/// The infrastructure statements alone (`load_infra_inputs` + `lower_infra`), for the refresher's
/// splice when the data lowering fails (C4). None: not trino/starrocks.
pub async fn lower_infra_via(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    missing: MissingUsername,
) -> Result<Option<Vec<Statement>>, CompileError> {
    let Some(loaded) = crate::ir_load::load_infra_inputs(conn, service_id, missing).await? else {
        return Ok(None);
    };
    Ok(Some(egret_perm_ir::lower::infra::lower_infra(
        &loaded.inputs,
    )))
}

/// None: the service is not trino/starrocks. Reads at the caller's transaction.
///
/// **The caller must hold one `REPEATABLE READ` snapshot** for the whole call
/// (the refresher's `REPEATABLE READ, READ ONLY` transaction, D3), so that
/// the key and every statement come from one point in time. The key is read
/// FIRST: should a caller run this under `READ COMMITTED` anyway, a commit
/// landing mid-build leaves content newer than its key — one extra rebuild
/// next period — never content older than its key, which the refresher would
/// keep serving because the key would not move again.
pub async fn snapshot_parts_via(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    missing: MissingUsername,
) -> Result<Option<SnapshotParts>, CompileError> {
    let key: (i64, i64) = sqlx::query_as(
        "SELECT policy_version, tag_version FROM policy.authz_services WHERE id = $1",
    )
    .bind(service_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(CompileError::from_db)?
    .ok_or_else(|| CompileError::Db(format!("OPA snapshot: no profile {service_id}")))?;
    let Some(lowered) = crate::trigger::lower_ep_via(conn, service_id, missing).await? else {
        return Ok(None);
    };
    let data: Vec<Statement> = lowered
        .families
        .into_iter()
        .flat_map(|f| f.statements)
        .collect();
    let Some(infra) = lower_infra_via(conn, service_id, missing).await? else {
        return Ok(None);
    };
    let classified = load_classified_columns_via(conn, service_id).await?;
    Ok(Some(SnapshotParts {
        key,
        data,
        infra,
        classified,
    }))
}

/// `snapshot_parts_via` then `Snapshot::build` over data ++ infra. The same
/// `REPEATABLE READ` requirement as [`snapshot_parts_via`].
pub async fn build_snapshot_via(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    missing: MissingUsername,
) -> Result<Option<Snapshot>, CompileError> {
    let Some(parts) = snapshot_parts_via(conn, service_id, missing).await? else {
        return Ok(None);
    };
    let mut statements = parts.data;
    statements.extend(parts.infra);
    Snapshot::build(SnapshotInput {
        service_id,
        key: parts.key,
        statements,
        classified: parts.classified,
    })
    .map(Some)
    .map_err(|e| CompileError::Db(format!("OPA snapshot: {e}")))
}
