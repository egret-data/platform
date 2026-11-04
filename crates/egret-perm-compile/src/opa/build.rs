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
pub async fn snapshot_parts_via(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    missing: MissingUsername,
) -> Result<Option<SnapshotParts>, CompileError> {
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
    let key: (i64, i64) = sqlx::query_as(
        "SELECT policy_version, tag_version FROM policy.authz_services WHERE id = $1",
    )
    .bind(service_id)
    .fetch_one(&mut *conn)
    .await
    .map_err(CompileError::from_db)?;
    Ok(Some(SnapshotParts {
        key,
        data,
        infra,
        classified,
    }))
}

/// `snapshot_parts_via` then `Snapshot::build` over data ++ infra.
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
