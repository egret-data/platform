//! SP5b Task 6: the equivalence gate that proves a catalog may switch from the
//! legacy `catalog.catalog_grants` store to the unified `perm.grants` store
//! safely, and the reversible/terminal admin operations built on it (plan →
//! switch → undo / finalize).
//!
//! # Why this lives in `egret-catalog-engine`, not `egret-perm-compile`
//!
//! The gate compares the LEGACY decision
//! ([`crate::grant_match::catalog_grant_covers_by`] over
//! [`crate::store::CatalogStore::grants_for_role`]) against the UNIFIED decision
//! ([`crate::unified_decide::covers`] over a catalog `Snapshot`). Those are
//! `egret-catalog-engine` items. `egret-catalog-engine` already depends on
//! `egret-perm-compile` (for the fold and the snapshot build), so the gate can
//! call `egret_perm_compile::catalog_rbac_migrate::{fold_catalog, undo_fold}` and
//! `egret_perm_compile::opa::build::build_catalog_snapshot_via` from here; the
//! reverse dependency (gate in `egret-perm-compile`) would be a crate cycle.
//! The HTTP endpoints live in [`crate::warehouse_admin`] (the catalog admin
//! router, nested at `/api/control-plane`) and keep the brief's URL
//! `/api/control-plane/perm/catalog-migrate/{catalog}/…`.
//!
//! # The equivalence check ([`equivalence_report`])
//!
//! For one catalog we sample PRINCIPALS × OBJECTS × {read, write} and compare the
//! two decisions. A clean report (zero divergences) is the switch's precondition.
//!
//! - **Principals** are ONLY the authenticatable ones: the service accounts of
//!   the catalog's VALID clients — active owning user and workspace, not revoked,
//!   not expired — the exact filter [`crate::store::embedded`]'s `client_row` and
//!   the fold both use. Sampling a dead client's SA would compare a legacy role
//!   nobody can present against a folded principal, inventing a divergence.
//!   A principal is a `(service-account username, role name)` pair: legacy decides
//!   by the role's grants (what a token carrying that role gets), unified decides
//!   by the service-account principal the fold wrote. One SA bound (through two
//!   clients) to two roles is sampled as two principals, and if unified's union of
//!   those roles exceeds either role alone the gate reports it — correctly
//!   refusing to switch a catalog where unified would over-grant.
//! - **Objects** are every namespace/table NAMED in the warehouse's grants (a
//!   table grant also contributes its enclosing namespace, the listing-reach
//!   object), PLUS a sibling namespace and a sibling table that NO grant covers —
//!   the negative probe that catches a unified over-grant. Each object is probed
//!   read AND write. The catalog root (empty namespace) is deliberately NOT
//!   probed here: the guard decides it by a separate rule (`covers_catalog_root`),
//!   not the per-request `covers` this gate mirrors.
//!
//! Name equality uses byte equality, as the Task 5 equivalence corpus does: the
//! sampled names are the grants' OWN spellings (and clearly-distinct siblings), so
//! legacy and unified agree on them under any collation. The one residual
//! name-matching divergence — a non-ASCII name the ICU collation folds but
//! `to_lowercase` does not — is handled by [`finalize`]'s
//! [`crate::unified_decide::unified_name_matching_is_exact`] refusal, which keeps
//! a finalized catalog exactly equivalent to legacy.

use serde::Serialize;
use sqlx::PgConnection;
use uuid::Uuid;

use egret_perm_compile::catalog_rbac_migrate::{fold_catalog, undo_fold, FoldError};
use egret_perm_compile::opa::build::build_catalog_snapshot_via;

use crate::grant_match::catalog_grant_covers_by;
use crate::store::{CatalogGrant, CatalogStore};
use crate::unified_decide::{covers, unified_name_matching_is_exact};

/// One sampled decision where the legacy and unified stores disagree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Divergence {
    /// The service-account username evaluated.
    pub principal: String,
    /// The catalog role whose legacy grants decided the legacy side.
    pub role: String,
    /// The namespace probed (empty only never happens here).
    pub namespace: Vec<String>,
    /// The table probed, if any.
    pub table: Option<String>,
    /// `true` for a write probe, `false` for a read.
    pub write: bool,
    /// What the legacy store (`catalog_grant_covers_by`) decided.
    pub legacy: bool,
    /// What the unified store (`unified_decide::covers`) decided.
    pub unified: bool,
}

/// The result of comparing the legacy and unified stores for one catalog over a
/// sample of principals × objects × accesses. Clean iff [`Self::divergences`] is
/// empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EquivalenceReport {
    pub catalog_id: Uuid,
    /// Authenticatable principals sampled (valid clients' `(sa, role)` pairs).
    pub principals: usize,
    /// Distinct objects sampled (named namespaces/tables + sibling probes).
    pub objects: usize,
    /// Total (principal, object, access) comparisons made.
    pub comparisons: usize,
    /// Every disagreement found. Empty ⇒ the catalog may switch.
    pub divergences: Vec<Divergence>,
}

impl EquivalenceReport {
    /// No divergences: the unified store decides every sampled request exactly as
    /// the legacy store does.
    pub fn clean(&self) -> bool {
        self.divergences.is_empty()
    }
}

/// What [`finalize`] removed when it closed a catalog onto the unified store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FinalizeSummary {
    pub catalog_id: Uuid,
    /// Clients whose `catalog_role_id` was NULLed (the clients themselves survive).
    pub clients_detached: u64,
    /// `catalog.catalog_grants` rows dropped for the warehouse.
    pub grants_dropped: u64,
    /// `catalog.catalog_roles` rows dropped for the warehouse.
    pub roles_dropped: u64,
}

/// Why a catalog-migrate operation could not complete.
#[derive(Debug, thiserror::Error)]
pub enum GateError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("catalog store error: {0}")]
    Store(#[from] crate::store::CatalogError),
    #[error("fold error: {0}")]
    Fold(#[from] FoldError),
    #[error("compile error: {0}")]
    Compile(#[from] egret_perm::CompileError),
    /// The catalog has no registry row / no perm snapshot could be built.
    #[error("catalog {0} has no registry catalog to evaluate")]
    NoRegistryCatalog(Uuid),
    /// `switch`/`finalize` before the catalog was folded (`plan`).
    #[error("catalog {0} has not been folded; run plan first")]
    NotFolded(Uuid),
    /// `switch` refused: the unified store is not equivalent to the legacy one.
    #[error("catalog {} is not equivalent under the unified store ({} divergence(s))", .0.catalog_id, .0.divergences.len())]
    NotEquivalent(Box<EquivalenceReport>),
    /// `finalize` before the switch is `unified`.
    #[error("catalog {0} is not switched to the unified store")]
    NotUnified(Uuid),
    /// `finalize` refused: a non-ASCII folded name would under-grant under
    /// unified, so legacy must not be dropped.
    #[error("catalog {0} has a non-ASCII folded name; unified would under-grant, so legacy is not dropped")]
    NonAsciiNames(Uuid),
}

/// The catalog's warehouse id + the catalog's name, or `None` when there is no
/// registry row.
async fn catalog_warehouse_and_name(
    conn: &mut PgConnection,
    catalog_id: Uuid,
) -> Result<Option<(Uuid, String)>, sqlx::Error> {
    let row: Option<(Option<Uuid>, String)> =
        sqlx::query_as("SELECT warehouse_id, name FROM registry.catalogs WHERE id = $1")
            .bind(catalog_id)
            .fetch_optional(&mut *conn)
            .await?;
    Ok(row.and_then(|(wh, name)| wh.map(|w| (w, name))))
}

/// One authenticatable principal: a valid client's service-account username and
/// the role that client carries.
struct Principal {
    username: String,
    role: String,
}

/// The authenticatable principals of a warehouse: the service accounts of its
/// VALID clients (active owner + workspace, not revoked, not expired — the same
/// filter as `client_row` and the fold), paired with the client's role. Distinct
/// `(username, role)`.
async fn valid_principals(
    conn: &mut PgConnection,
    warehouse_id: Uuid,
) -> Result<Vec<Principal>, sqlx::Error> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT DISTINCT u.username::text, r.name \
           FROM catalog.catalog_clients c \
           JOIN catalog.catalog_roles r ON r.id = c.catalog_role_id \
           JOIN service_accounts sa ON sa.user_id = c.service_account_id \
           JOIN users u ON u.id = sa.user_id \
           JOIN workspaces ws ON ws.id = c.workspace_id \
          WHERE c.warehouse_id = $1 \
            AND u.status = 'active' AND ws.status = 'active' \
            AND c.revoked_at IS NULL \
            AND (c.expires_at IS NULL OR c.expires_at > now()) \
          ORDER BY 1, 2",
    )
    .bind(warehouse_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(username, role)| Principal { username, role })
        .collect())
}

/// One object to probe: a namespace, optionally a table within it.
#[derive(Clone, PartialEq, Eq, Hash)]
struct ObjectProbe {
    namespace: Vec<String>,
    table: Option<String>,
}

/// The objects to sample for a warehouse: every namespace/table NAMED in its
/// grants (a table grant also yields its enclosing namespace, for listing reach),
/// plus a sibling namespace and a sibling table that no grant covers.
async fn sample_objects(
    conn: &mut PgConnection,
    warehouse_id: Uuid,
    catalog_id: Uuid,
) -> Result<Vec<ObjectProbe>, sqlx::Error> {
    let grants: Vec<(String, Option<Vec<String>>, Option<String>)> = sqlx::query_as(
        "SELECT g.securable_type, g.namespace, g.table_name \
           FROM catalog.catalog_grants g \
           JOIN catalog.catalog_roles r ON r.id = g.catalog_role_id \
          WHERE r.warehouse_id = $1",
    )
    .bind(warehouse_id)
    .fetch_all(&mut *conn)
    .await?;

    let mut objects: Vec<ObjectProbe> = Vec::new();
    let mut push = |o: ObjectProbe| {
        if !objects.contains(&o) {
            objects.push(o);
        }
    };
    let mut a_named_namespace: Option<Vec<String>> = None;
    for (securable, namespace, table) in &grants {
        match securable.as_str() {
            "namespace" => {
                if let Some(ns) = namespace {
                    if !ns.is_empty() {
                        a_named_namespace.get_or_insert_with(|| ns.clone());
                        push(ObjectProbe {
                            namespace: ns.clone(),
                            table: None,
                        });
                    }
                }
            }
            "table" => {
                if let (Some(ns), Some(t)) = (namespace, table) {
                    if !ns.is_empty() {
                        a_named_namespace.get_or_insert_with(|| ns.clone());
                        push(ObjectProbe {
                            namespace: ns.clone(),
                            table: Some(t.clone()),
                        });
                        // The enclosing namespace: a table-grant holder may list
                        // it (reach), which the gate must prove equivalent too.
                        push(ObjectProbe {
                            namespace: ns.clone(),
                            table: None,
                        });
                    }
                }
            }
            // A `catalog` grant names no namespace/table; its root object is
            // decided by a separate guard rule and is not sampled here.
            _ => {}
        }
    }

    // Negative probes: names no grant covers, so legacy denies — the gate asserts
    // unified denies too (no over-grant). The nonce keys off the catalog id so two
    // catalogs sharing a database never collide.
    let nonce = catalog_id.simple().to_string();
    push(ObjectProbe {
        namespace: vec![format!("egret_gate_sibling_ns_{nonce}")],
        table: None,
    });
    if let Some(ns) = a_named_namespace {
        push(ObjectProbe {
            namespace: ns,
            table: Some(format!("egret_gate_sibling_tbl_{nonce}")),
        });
    }
    Ok(objects)
}

/// Compare the legacy and unified stores for one catalog over the sampled
/// principals × objects × {read, write}. The catalog must be FOLDED already
/// (`plan`): the legacy side still reads `catalog.catalog_grants` (the fold does
/// not delete it), the unified side the folded `perm.grants`.
pub async fn equivalence_report(
    store: &CatalogStore,
    catalog_id: Uuid,
) -> Result<EquivalenceReport, GateError> {
    let mut conn = store.pool().acquire().await?;
    let (warehouse_id, catalog_name) = catalog_warehouse_and_name(&mut conn, catalog_id)
        .await?
        .ok_or(GateError::NoRegistryCatalog(catalog_id))?;
    let warehouse_name: String =
        sqlx::query_scalar("SELECT name FROM catalog.warehouses WHERE id = $1")
            .bind(warehouse_id)
            .fetch_one(&mut *conn)
            .await?;

    let snapshot = build_catalog_snapshot_via(&mut conn, catalog_id)
        .await?
        .ok_or(GateError::NoRegistryCatalog(catalog_id))?;

    let principals = valid_principals(&mut conn, warehouse_id).await?;
    let objects = sample_objects(&mut conn, warehouse_id, catalog_id).await?;
    drop(conn);

    // The legacy grant set per distinct role, loaded once.
    let mut by_role: std::collections::HashMap<String, Vec<CatalogGrant>> =
        std::collections::HashMap::new();
    for p in &principals {
        if !by_role.contains_key(&p.role) {
            let grants = store.grants_for_role(&warehouse_name, &p.role).await?;
            by_role.insert(p.role.clone(), grants);
        }
    }

    let byte_eq = |a: &str, b: &str| a == b;
    let mut divergences = Vec::new();
    let mut comparisons = 0usize;
    for p in &principals {
        let legacy_grants = &by_role[&p.role];
        for obj in &objects {
            for write in [false, true] {
                comparisons += 1;
                let legacy = legacy_grants.iter().any(|g| {
                    catalog_grant_covers_by(
                        g,
                        Some(&obj.namespace),
                        obj.table.as_deref(),
                        write,
                        &byte_eq,
                    )
                });
                let unified = covers(
                    &snapshot,
                    &p.username,
                    &catalog_name,
                    &obj.namespace,
                    obj.table.as_deref(),
                    write,
                );
                if legacy != unified {
                    divergences.push(Divergence {
                        principal: p.username.clone(),
                        role: p.role.clone(),
                        namespace: obj.namespace.clone(),
                        table: obj.table.clone(),
                        write,
                        legacy,
                        unified,
                    });
                }
            }
        }
    }

    Ok(EquivalenceReport {
        catalog_id,
        principals: principals.len(),
        objects: objects.len(),
        comparisons,
        divergences,
    })
}

/// Whether the catalog has been folded into the unified store.
async fn is_folded(conn: &mut PgConnection, catalog_id: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM perm.catalog_rbac_migrated WHERE catalog_id = $1)",
    )
    .bind(catalog_id)
    .fetch_one(&mut *conn)
    .await
}

/// The catalog's current switch source.
async fn is_unified(conn: &mut PgConnection, catalog_id: Uuid) -> Result<bool, sqlx::Error> {
    let source: Option<String> =
        sqlx::query_scalar("SELECT source FROM perm.catalog_grant_switch WHERE catalog_id = $1")
            .bind(catalog_id)
            .fetch_optional(&mut *conn)
            .await?;
    Ok(source.as_deref() == Some("unified"))
}

/// **plan**: fold the catalog's RBAC into the unified store WITHOUT switching
/// (idempotent — a re-fold is a no-op), then return the equivalence report so an
/// operator can preview it. Changes nothing a caller may do.
pub async fn plan(
    store: &CatalogStore,
    catalog_id: Uuid,
    actor: &str,
) -> Result<EquivalenceReport, GateError> {
    let mut conn = store.pool().acquire().await?;
    fold_catalog(&mut conn, catalog_id, actor).await?;
    drop(conn);
    equivalence_report(store, catalog_id).await
}

/// **switch**: flip the catalog to the unified store — but only after proving, at
/// this instant, that the two stores are equivalent. Refuses (unchanged) if the
/// catalog was not folded, or if the live equivalence has any divergence. Returns
/// the (clean) report it switched on.
pub async fn switch(
    store: &CatalogStore,
    catalog_id: Uuid,
    actor: &str,
) -> Result<EquivalenceReport, GateError> {
    let mut conn = store.pool().acquire().await?;
    if !is_folded(&mut conn, catalog_id).await? {
        return Err(GateError::NotFolded(catalog_id));
    }
    drop(conn);

    let report = equivalence_report(store, catalog_id).await?;
    if !report.clean() {
        return Err(GateError::NotEquivalent(Box::new(report)));
    }

    sqlx::query(
        "INSERT INTO perm.catalog_grant_switch (catalog_id, source, switched_at, switched_by) \
         VALUES ($1, 'unified', now(), $2) \
         ON CONFLICT (catalog_id) DO UPDATE \
            SET source = 'unified', switched_at = now(), switched_by = EXCLUDED.switched_by",
    )
    .bind(catalog_id)
    .bind(actor)
    .execute(store.pool())
    .await?;
    Ok(report)
}

/// **undo**: return the catalog to the legacy store and remove the folded rows,
/// atomically. Sets the switch back to `catalog_rbac` and calls the hash-guarded
/// [`undo_fold`]; if the legacy RBAC changed since the fold — or was dropped by
/// [`finalize`] — `undo_fold` refuses ([`FoldError::SourceChanged`]) and the whole
/// operation rolls back, so the switch is NOT flipped. Reversible only while the
/// migration is still open.
pub async fn undo(store: &CatalogStore, catalog_id: Uuid, actor: &str) -> Result<(), GateError> {
    let mut tx = store.pool().begin().await?;
    sqlx::query(
        "UPDATE perm.catalog_grant_switch \
            SET source = 'catalog_rbac', switched_at = now(), switched_by = $2 \
          WHERE catalog_id = $1",
    )
    .bind(catalog_id)
    .bind(actor)
    .execute(&mut *tx)
    .await?;
    // Nested in the outer transaction (sqlx issues a SAVEPOINT), so a refusal here
    // rolls back the switch flip above too.
    undo_fold(&mut tx, catalog_id).await?;
    tx.commit().await?;
    Ok(())
}

/// **finalize**: close the catalog onto the unified store, irreversibly. Refuses
/// (unchanged) unless the switch is `unified`, and refuses a catalog with any
/// non-ASCII folded name (where unified would under-grant relative to
/// lakekeeper's collation — [`unified_name_matching_is_exact`]). Then, in one
/// transaction: NULLs the warehouse's clients' `catalog_role_id` (keeping the
/// clients and their credentials), drops the warehouse's `catalog.catalog_grants`
/// and `catalog.catalog_roles`, and so retires the legacy store. Dropping the
/// source makes [`undo_fold`]'s hash guard fail ever after, which is what makes
/// finalize terminal: a later `undo` is refused.
pub async fn finalize(
    store: &CatalogStore,
    catalog_id: Uuid,
) -> Result<FinalizeSummary, GateError> {
    let mut tx = store.pool().begin().await?;
    if !is_unified(&mut tx, catalog_id).await? {
        return Err(GateError::NotUnified(catalog_id));
    }
    if !unified_name_matching_is_exact(&mut tx, catalog_id).await? {
        return Err(GateError::NonAsciiNames(catalog_id));
    }
    let warehouse_id: Uuid = catalog_warehouse_and_name(&mut tx, catalog_id)
        .await?
        .ok_or(GateError::NoRegistryCatalog(catalog_id))?
        .0;

    // Keep the clients; drop only their now-dead role binding so the roles can go.
    let clients_detached = sqlx::query(
        "UPDATE catalog.catalog_clients SET catalog_role_id = NULL WHERE warehouse_id = $1",
    )
    .bind(warehouse_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    let grants_dropped = sqlx::query(
        "DELETE FROM catalog.catalog_grants g \
          USING catalog.catalog_roles r \
          WHERE g.catalog_role_id = r.id AND r.warehouse_id = $1",
    )
    .bind(warehouse_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    let roles_dropped = sqlx::query("DELETE FROM catalog.catalog_roles WHERE warehouse_id = $1")
        .bind(warehouse_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();

    tx.commit().await?;
    Ok(FinalizeSummary {
        catalog_id,
        clients_detached,
        grants_dropped,
        roles_dropped,
    })
}
