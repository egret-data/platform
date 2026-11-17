//! SP5c Task 4: the preview / enable / disable operations for per-catalog
//! iceberg-rest RULE enforcement (plan → switch → undo).
//!
//! Rule enforcement (deny a table carrying a mask / row filter / withhold at the
//! embedded catalog) operates on the UNIFIED snapshot. It is therefore a hard
//! precondition that the catalog's grants already run on the unified store
//! ([`Source::Unified`], SP5b's switch); on a legacy-RBAC catalog it would be a
//! no-op, so [`switch`] refuses.
//!
//! # What the gate reports
//!
//! - **`new_denies`** (primary): over the SP5b sample population (the VALID
//!   clients' service accounts, see [`catalog_migrate_gate`]) × the catalog's
//!   named tables (tables named by a legacy grant and tables targeted by a rule),
//!   every `(principal, table)` whose READ the grants allow but whose rule
//!   enforcement would deny (`Snapshot::table_read_restricted`, exemptions
//!   honoured). These are the intended allow→deny flips the admin reviews; they
//!   are reported, never a refusal.
//! - **`grants_identical`**: SP5c never touches the grant decision path, so
//!   grants are structurally unchanged. The defense-in-depth check runs only
//!   where it is meaningful: while the catalog still has legacy
//!   `catalog.catalog_grants` rows, SP5b's legacy-vs-unified comparison
//!   ([`catalog_migrate_gate::equivalence_report`]) must be clean. Once legacy is
//!   dropped (post-`finalize`) there is nothing to diverge from and it is `true`.
//!   A catalog that is not on the unified source is reported `false` (nothing is
//!   proven) and `switch` refuses it on the precondition first.
//!
//! `undo` is a plain reversible flag flip: SP5c drops nothing, so there is no
//! `finalize`.

use serde::Serialize;
use sqlx::PgConnection;
use uuid::Uuid;

use egret_perm_compile::opa::build::{
    build_catalog_snapshot_via, load_classified_columns_for_catalog_via,
};

use crate::catalog_migrate_gate::{equivalence_report, Divergence, EquivalenceReport, GateError};
use crate::store::CatalogStore;
use crate::unified_decide::{
    catalog_grant_source, covers, CatalogEnforcementSnapshotCache, Source,
};

/// One `(principal, table)` whose read flips from allow to deny when rule
/// enforcement is switched on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RuleDeny {
    /// The service-account username that is denied.
    pub principal: String,
    pub catalog: String,
    /// The dot-joined namespace.
    pub schema: String,
    pub table: String,
}

/// The preview of switching a catalog's rule enforcement on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IcebergRestPlan {
    /// Whether the catalog's grants are on the unified store (the precondition).
    pub grant_source_unified: bool,
    /// `false` ⇒ `switch` is refused (grants not proven unchanged).
    pub grants_identical: bool,
    /// Legacy-vs-unified grant disagreements; empty when identical or skipped.
    pub grant_divergences: Vec<Divergence>,
    /// The intended allow→deny flips (reported, not refused).
    pub new_denies: Vec<RuleDeny>,
    /// `true` when every restricting rule resolved to enumerated tables, so
    /// `new_denies` is the whole story. `false` means a catalog-wide or
    /// schema-wide path rule exists (or nothing was computed): ADDITIONAL tables
    /// beyond `new_denies` will be denied, and an empty/short list must not be
    /// read as "nothing will be denied".
    pub preview_complete: bool,
}

/// Does the warehouse still have legacy `catalog.catalog_grants` rows?
async fn has_legacy_grants(
    conn: &mut PgConnection,
    warehouse_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM catalog.catalog_grants g \
           JOIN catalog.catalog_roles r ON r.id = g.catalog_role_id \
          WHERE r.warehouse_id = $1)",
    )
    .bind(warehouse_id)
    .fetch_one(&mut *conn)
    .await
}

/// The grant-equivalence verdict: `(identical, divergences)`. Compared only while
/// legacy rows remain; otherwise vacuously identical.
async fn grant_check(
    store: &CatalogStore,
    catalog_id: Uuid,
    warehouse_id: Uuid,
) -> Result<(bool, Vec<Divergence>), GateError> {
    let mut conn = store.pool().acquire().await?;
    let legacy = has_legacy_grants(&mut conn, warehouse_id).await?;
    drop(conn);
    if !legacy {
        return Ok((true, Vec::new()));
    }
    let report: EquivalenceReport = equivalence_report(store, catalog_id).await?;
    Ok((report.clean(), report.divergences))
}

/// The distinct usernames of the warehouse's VALID clients' service accounts
/// (same filter as SP5b's sampler: active owner + workspace, not revoked/expired).
async fn sample_principals(
    conn: &mut PgConnection,
    warehouse_id: Uuid,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT DISTINCT u.username::text \
           FROM catalog.catalog_clients c \
           JOIN service_accounts sa ON sa.user_id = c.service_account_id \
           JOIN users u ON u.id = sa.user_id \
           JOIN workspaces ws ON ws.id = c.workspace_id \
          WHERE c.warehouse_id = $1 \
            AND u.status = 'active' AND ws.status = 'active' \
            AND c.revoked_at IS NULL \
            AND (c.expires_at IS NULL OR c.expires_at > now()) \
          ORDER BY 1",
    )
    .bind(warehouse_id)
    .fetch_all(&mut *conn)
    .await
}

/// The candidate tables `(namespace levels, table)` plus whether they cover every
/// restricting rule. Candidates are the UNION of: tables named by a legacy table
/// grant (while it exists), tables named by a path rule, and every table carrying
/// a tag/classification in the catalog (tag rules carry no path, so the tagged
/// tables are where they land). A path rule naming no table (schema-wide or
/// catalog-wide) restricts tables that cannot be listed without a crawl of the
/// object store, so it makes the preview incomplete (`false`).
async fn sample_tables(
    conn: &mut PgConnection,
    warehouse_id: Uuid,
    catalog_id: Uuid,
) -> Result<(Vec<(Vec<String>, String)>, bool), GateError> {
    let mut out: Vec<(Vec<String>, String)> = Vec::new();
    let mut push = |ns: Vec<String>, t: String| {
        if !ns.is_empty() && !out.iter().any(|(n, x)| *n == ns && *x == t) {
            out.push((ns, t));
        }
    };
    let grants: Vec<(Option<Vec<String>>, Option<String>)> = sqlx::query_as(
        "SELECT g.namespace, g.table_name FROM catalog.catalog_grants g \
           JOIN catalog.catalog_roles r ON r.id = g.catalog_role_id \
          WHERE r.warehouse_id = $1 AND g.securable_type = 'table'",
    )
    .bind(warehouse_id)
    .fetch_all(&mut *conn)
    .await?;
    for (ns, t) in grants {
        if let (Some(ns), Some(t)) = (ns, t) {
            push(ns, t);
        }
    }
    let rules: Vec<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT schema_name, table_name FROM perm.rules WHERE catalog_id = $1")
            .bind(catalog_id)
            .fetch_all(&mut *conn)
            .await?;
    let mut complete = true;
    for (schema, table) in rules {
        match (schema, table) {
            (Some(s), Some(t)) => push(s.split('.').map(str::to_string).collect(), t),
            _ => complete = false,
        }
    }
    let tagged = load_classified_columns_for_catalog_via(conn, catalog_id).await?;
    for c in tagged {
        push(c.schema.split('.').map(str::to_string).collect(), c.table);
    }
    Ok((out, complete))
}

/// **plan**: preview switching rule enforcement on. Changes nothing.
pub async fn plan(
    store: &CatalogStore,
    enforcement: &CatalogEnforcementSnapshotCache,
    catalog_id: Uuid,
) -> Result<IcebergRestPlan, GateError> {
    let mut conn = store.pool().acquire().await?;
    let row: Option<(Option<Uuid>, String)> =
        sqlx::query_as("SELECT warehouse_id, name FROM registry.catalogs WHERE id = $1")
            .bind(catalog_id)
            .fetch_optional(&mut *conn)
            .await?;
    let (warehouse_id, catalog_name) = match row {
        Some((Some(w), n)) => (w, n),
        _ => return Err(GateError::NoRegistryCatalog(catalog_id)),
    };
    let unified = catalog_grant_source(&mut conn, catalog_id).await? == Source::Unified;
    if !unified {
        // Rule enforcement is a no-op on a legacy-RBAC catalog and nothing is
        // proven about its grants: say so, compute nothing.
        return Ok(IcebergRestPlan {
            grant_source_unified: false,
            grants_identical: false,
            grant_divergences: Vec::new(),
            new_denies: Vec::new(),
            preview_complete: false,
        });
    }

    let grant_snapshot = build_catalog_snapshot_via(&mut conn, catalog_id)
        .await?
        .ok_or(GateError::NoRegistryCatalog(catalog_id))?;
    let enforce_snapshot = enforcement
        .get(&mut conn, catalog_id)
        .await?
        .ok_or(GateError::NoRegistryCatalog(catalog_id))?;
    let principals = sample_principals(&mut conn, warehouse_id).await?;
    let (tables, preview_complete) = sample_tables(&mut conn, warehouse_id, catalog_id).await?;
    drop(conn);

    let mut new_denies = Vec::new();
    for principal in &principals {
        for (ns, table) in &tables {
            let schema = ns.join(".");
            if covers(
                &grant_snapshot,
                principal,
                &catalog_name,
                ns,
                Some(table),
                false,
            ) && enforce_snapshot.table_read_restricted(principal, &catalog_name, &schema, table)
            {
                new_denies.push(RuleDeny {
                    principal: principal.clone(),
                    catalog: catalog_name.clone(),
                    schema,
                    table: table.clone(),
                });
            }
        }
    }

    let (grants_identical, grant_divergences) =
        grant_check(store, catalog_id, warehouse_id).await?;
    Ok(IcebergRestPlan {
        grant_source_unified: true,
        grants_identical,
        grant_divergences,
        new_denies,
        preview_complete,
    })
}

/// **switch**: turn rule enforcement on. Refused (unchanged) unless the catalog's
/// grants are on the unified store AND the grant check is clean. Returns the plan
/// it switched on.
pub async fn switch(
    store: &CatalogStore,
    enforcement: &CatalogEnforcementSnapshotCache,
    catalog_id: Uuid,
    actor: &str,
) -> Result<IcebergRestPlan, GateError> {
    let mut conn = store.pool().acquire().await?;
    if catalog_grant_source(&mut conn, catalog_id).await? != Source::Unified {
        return Err(GateError::NotUnified(catalog_id));
    }
    drop(conn);

    let plan = plan(store, enforcement, catalog_id).await?;
    if !plan.grants_identical {
        return Err(GateError::NotEquivalent(Box::new(EquivalenceReport {
            catalog_id,
            principals: 0,
            objects: 0,
            comparisons: 0,
            divergences: plan.grant_divergences,
        })));
    }
    set_enforce(store, catalog_id, true, actor).await?;
    Ok(plan)
}

/// **undo**: turn rule enforcement off. Reversible; nothing was dropped.
pub async fn undo(store: &CatalogStore, catalog_id: Uuid, actor: &str) -> Result<(), GateError> {
    set_enforce(store, catalog_id, false, actor).await?;
    Ok(())
}

async fn set_enforce(
    store: &CatalogStore,
    catalog_id: Uuid,
    enforce: bool,
    actor: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO perm.catalog_rule_enforcement_switch \
           (catalog_id, enforce, switched_at, switched_by) VALUES ($1, $2, now(), $3) \
         ON CONFLICT (catalog_id) DO UPDATE \
            SET enforce = EXCLUDED.enforce, switched_at = now(), switched_by = EXCLUDED.switched_by",
    )
    .bind(catalog_id)
    .bind(enforce)
    .bind(actor)
    .execute(store.pool())
    .await?;
    Ok(())
}
