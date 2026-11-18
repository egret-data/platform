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
use uuid::Uuid;

pub use crate::members::MissingUsername;

/// The reserved principal view-lineage EXPLAIN runs as, for
/// `SnapshotInput::lineage_principal`: every snapshot the refresher and
/// explain build refuses it what `egret_perm_ir::lineage` does not keep
/// (governance SP2b Task 18), whatever the engine's delegation mode.
pub fn lineage_principal() -> String {
    egret_core::engine_principals::LINEAGE_PRINCIPAL.to_string()
}

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
    let ep = &lowered.loaded.ep;
    let data: Vec<Statement> = lowered
        .families
        .iter()
        .flat_map(|f| f.statements.iter())
        // What the matrix says OPA cannot enforce (browse, create, manage)
        // never enters the snapshot: an allow of any privilege on a
        // catalog/schema (levels `Unnamed`) counts as reaching every sibling
        // object in the evaluator.
        .filter_map(|s| egret_perm_ir::lower::grants::strip_unenforced(ep, s))
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

/// SP5b Task 5: a [`Snapshot`] of ONE embedded catalog's folded grants, for the
/// unified enforcement path (`egret_catalog_engine::unified_decide`).
///
/// This is NOT keyed by an authz service. The embedded catalog is a registry
/// catalog, not a `policy.authz_services` row, so there is no service whose
/// `build_snapshot_via` would read exactly this catalog's grants and nothing
/// else. Instead it builds a catalog-scoped [`SnapshotInput`] directly from the
/// store the fold wrote into: `perm.grants` + `perm.securable_owners` for this
/// `catalog_id`, the SAME rows `egret_perm::catalog_rbac_migrate::fold_catalog`
/// created, lowered by the SAME grant lowering a Ranger/OPA compile uses
/// ([`egret_perm_ir::lower::grants::lower_grants`]) and stripped of what a
/// target cannot enforce ([`strip_unenforced`]), so the decision matches a real
/// compile of these grants.
///
/// The enforcement point is Trino/Ranger: the catalog decision only ever asks
/// `read`/`write` (never stripped) and reach (any privilege), and `manage`
/// always implies `read`, so the engine/mechanism choice does not move a
/// catalog-client decision — Ranger merely keeps `create`/`manage` rather than
/// dropping them, which costs nothing here. No rules, no classified columns and
/// no infrastructure enter: the fold wrote grants and owners only, and the
/// catalog guard enforces grants alone (no masks). The directory is empty: the
/// fold's principals are resolved `users.username` values (`user` kind), which
/// need no expansion; a `role`-principal owner (the platform-admin fallback)
/// lowers to a skipped [`egret_perm_ir::statement::Lowering`], never an error,
/// and never matches a catalog client anyway.
///
/// `None` when `catalog_id` names no registry catalog.
pub async fn build_catalog_snapshot_via(
    conn: &mut sqlx::PgConnection,
    catalog_id: Uuid,
) -> Result<Option<Snapshot>, CompileError> {
    use egret_perm_ir::audience::Directory;
    use egret_perm_ir::ep::{EpInput, EpKind, Mechanism};

    let name: Option<String> =
        sqlx::query_scalar("SELECT name FROM registry.catalogs WHERE id = $1")
            .bind(catalog_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(CompileError::from_db)?;
    let Some(catalog_name) = name else {
        return Ok(None);
    };

    let mut grants = egret_perm::list_grants_for_catalogs_via(conn, &[catalog_id])
        .await
        .map_err(CompileError::from_perm)?;
    grants.extend(
        egret_perm::list_owners_for_catalogs_via(conn, &[catalog_id])
            .await
            .map_err(CompileError::from_perm)?
            .iter()
            .map(egret_perm::owner_grant),
    );

    let ep = EpInput {
        profile_id: 0,
        profile_name: catalog_name.clone(),
        kind: EpKind {
            engine: egret_perm::EngineType::Trino,
            mechanism: Mechanism::Ranger,
        },
        engines: Vec::new(),
        default_version: Err(NOT_READ_VERSION.to_string()),
        default_image_level: None,
        classification_channel: false,
        catalogs: [(catalog_id, catalog_name)].into_iter().collect(),
    };
    let lowered = egret_perm_ir::lower::grants::lower_grants(&ep, &grants, &Directory::default())?;
    let statements: Vec<Statement> = lowered
        .statements
        .iter()
        .filter_map(|s| egret_perm_ir::lower::grants::strip_unenforced(&ep, s))
        .collect();

    Snapshot::build(SnapshotInput {
        service_id: 0,
        key: (0, 0),
        statements,
        classified: Vec::new(),
        lineage_principal: lineage_principal(),
    })
    .map(Some)
    .map_err(|e| CompileError::Db(format!("catalog snapshot: {e}")))
}

/// Every classification on a column or whole table of ONE catalog — the
/// catalog-scoped twin of [`load_classified_columns_via`], which reaches the
/// catalog by service; the embedded catalog is bound to none.
pub async fn load_classified_columns_for_catalog_via(
    conn: &mut sqlx::PgConnection,
    catalog_id: Uuid,
) -> Result<Vec<ClassifiedColumn>, CompileError> {
    let rows = egret_policy::tag_assignments_for_catalogs_via(conn, &[catalog_id])
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

/// SP5c Task 1: the ENFORCEMENT snapshot of ONE embedded catalog — its rule
/// (mask, row filter) and withhold statements plus its classified objects,
/// the input of `Snapshot::table_read_restricted`. NO grants or owners:
/// those are decided by [`build_catalog_snapshot_via`], unchanged, so this
/// snapshot cannot move a grant decision.
///
/// Lowered exactly as a Trino/Ranger profile reaching this catalog lowers
/// them (`lower_rules`, `lower_withholds`, then `strip_unenforced`), on the
/// same synthetic EP as the grant snapshot, except
/// `classification_channel: true`: the iceberg-rest EP IS the classification
/// channel, so a classification-selected rule must lower to its native
/// `Target::Classification` statement rather than be skipped for want of a
/// channel. Rules are common (D9): the path rules read are this catalog's
/// (`list_rules_for_catalogs_via`), tag rules are every tag rule, and the
/// catalog's `classified` index limits which of this catalog's objects a
/// classification statement hits. A withhold is installation-wide on Trino.
///
/// Reads with [`MissingUsername::Omit`]: nothing here may write.
/// `None` when `catalog_id` names no registry catalog.
pub async fn build_catalog_enforcement_snapshot(
    conn: &mut sqlx::PgConnection,
    catalog_id: Uuid,
) -> Result<Option<Snapshot>, CompileError> {
    use egret_perm_ir::ep::{EpInput, EpKind, Mechanism};

    let name: Option<String> =
        sqlx::query_scalar("SELECT name FROM registry.catalogs WHERE id = $1")
            .bind(catalog_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(CompileError::from_db)?;
    let Some(catalog_name) = name else {
        return Ok(None);
    };
    let missing = MissingUsername::Omit;

    let rules = egret_perm::list_rules_for_catalogs_via(conn, &[catalog_id])
        .await
        .map_err(CompileError::from_perm)?;
    let directory =
        crate::members::expand_enumerated_principals(conn, &[], &rules, &[], missing).await?;
    let leaving = crate::table_columns::leaving_via(conn).await?;

    let ep = EpInput {
        profile_id: 0,
        profile_name: catalog_name.clone(),
        kind: EpKind {
            engine: egret_perm::EngineType::Trino,
            mechanism: Mechanism::Ranger,
        },
        engines: Vec::new(),
        default_version: Err(NOT_READ_VERSION.to_string()),
        default_image_level: None,
        classification_channel: true,
        catalogs: [(catalog_id, catalog_name)].into_iter().collect(),
    };

    let (rule_inputs, read_skips) = crate::ir_load::load_rule_inputs(conn, &ep, &rules).await?;
    let ruled = egret_perm_ir::lower::rules::lower_rules(
        &ep,
        &rules,
        &directory,
        &rule_inputs,
        read_skips,
        crate::trigger::DEFAULT_EXPANSION_CEILING,
        &leaving,
    )?;
    let gates = crate::ir_load::load_withhold_inputs(conn, &ep, 0, missing).await?;
    let withheld = egret_perm_ir::lower::withhold::lower_withholds(
        &ep,
        &gates,
        egret_ranger_eval::resource::fold_identifier,
        crate::trigger::DEFAULT_EXPANSION_CEILING,
        &leaving,
    )?;

    // FAIL CLOSED. A mask, row filter or withhold that was skipped instead of
    // lowered emits no statement, so `table_read_restricted` would answer
    // `false` for a table that IS restricted: a leak at the catalog. The only
    // skips that leave the restriction in force are the ones whose own detail
    // says a deny was still compiled (`NotMeasured`: enforced as a deny;
    // `ViewNotMeasured`/`TableNotInCatalog`: the whole-table deny is compiled,
    // only the view guard is not). Anything else is refused: the caller
    // treats a build error as "cannot enforce", never as "nothing restricts".
    refuse_dropped_restrictions(
        ruled
            .lowerings
            .iter()
            .chain(withheld.lowered.lowerings.iter()),
    )?;

    let statements: Vec<Statement> = ruled
        .statements
        .iter()
        .chain(withheld.lowered.statements.iter())
        .filter_map(|s| egret_perm_ir::lower::grants::strip_unenforced(&ep, s))
        .collect();

    Snapshot::build(SnapshotInput {
        service_id: 0,
        key: (0, 0),
        statements,
        classified: load_classified_columns_for_catalog_via(conn, catalog_id).await?,
        lineage_principal: lineage_principal(),
    })
    .map(Some)
    .map_err(|e| CompileError::Db(format!("catalog enforcement snapshot: {e}")))
}

/// The fail-closed check of [`build_catalog_enforcement_snapshot`]: `Err`
/// for any skip that is not one of the three whose detail says the deny was
/// still compiled.
fn refuse_dropped_restrictions<'a>(
    skips: impl Iterator<Item = &'a egret_perm_ir::statement::Lowering>,
) -> Result<(), CompileError> {
    use egret_perm_ir::statement::ReasonCode;
    match skips.into_iter().find(|l| {
        !matches!(
            l.code,
            ReasonCode::NotMeasured | ReasonCode::ViewNotMeasured | ReasonCode::TableNotInCatalog
        )
    }) {
        Some(l) => Err(CompileError::Db(format!(
            "catalog enforcement snapshot: a restricting rule was skipped, so the catalog \
             cannot be enforced ({:?}: {})",
            l.code, l.detail
        ))),
        None => Ok(()),
    }
}

/// The `default_version` a catalog snapshot's EP carries: unread, exactly as the
/// grant-lowering unit tests pass it. `read`/`write` are never consulted and
/// Ranger keeps `create`/`manage` without a version, so no gate reads it.
const NOT_READ_VERSION: &str = "not read";

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
        lineage_principal: lineage_principal(),
    })
    .map(Some)
    .map_err(|e| CompileError::Db(format!("OPA snapshot: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_perm_ir::statement::{Lowering, ReasonCode, Source};

    fn skip(code: ReasonCode) -> Lowering {
        Lowering {
            source: Source::Rule {
                rule_id: Uuid::nil(),
                rule_name: "r".into(),
            },
            code,
            detail: "d".into(),
        }
    }

    #[test]
    fn a_dropped_mask_or_filter_is_refused() {
        for code in [
            ReasonCode::NoClassificationChannel,
            ReasonCode::UnexpandedPrincipal,
            ReasonCode::UnknownClassification,
            ReasonCode::EmptyAudience,
        ] {
            assert!(refuse_dropped_restrictions([skip(code)].iter()).is_err());
        }
    }

    #[test]
    fn a_skip_that_still_compiled_its_deny_is_accepted() {
        for code in [
            ReasonCode::NotMeasured,
            ReasonCode::ViewNotMeasured,
            ReasonCode::TableNotInCatalog,
        ] {
            assert!(refuse_dropped_restrictions([skip(code)].iter()).is_ok());
        }
    }
}
