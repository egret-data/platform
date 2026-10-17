//! DG frontend redesign #1 (spec §2.3 "Lineage"): `GET
//! /governance/lineage/{catalog_id}?schema=&table=` — what egret knows of one
//! object's table-level lineage, for the Governance Data page's Lineage tab.
//! Read-only; gated like browse on `policy:read` OR `tag:read`
//! ([`require_governance_read`]).
//!
//! **Table or view.** In an egret catalog the warehouse says which the object
//! is (and a 404 when it has neither), under [`CpState::browse_timeout`]. In
//! any other catalog egret dials nothing to find out: the object is a view
//! exactly when a lineage is recorded for it (`view_lineage_state`, migration
//! 0187), which only egret's view commits write.
//!
//! **For a table: its readers.**
//! - The views whose recorded lineage (`view_lineage`) lists it as a base,
//!   compared case-insensitively as Trino resolves names. Nested views resolve
//!   to their base tables, so a view over a view over this table is listed too.
//! - The StarRocks materialized views that read it, from the same listing the
//!   classification page uses (`view_exposure::list_all`), on every StarRocks
//!   attached to the catalog that egret runs and that is `running`, all under
//!   ONE [`CpState::browse_timeout`]. An external engine is never dialled
//!   (origin first: its `running` is a placeholder), nor a stopped one.
//!
//! **A reader list is never presented as complete when it is not.** A
//! listing that fails or misses the deadline, and an engine that was not
//! asked, sets `readersIncomplete` with a sentence naming the engine; the
//! engine's own text goes to the log only. So does, fail closed as the
//! DEFINER audit is, every view in ANY of egret's catalogs whose lineage
//! `failed` or is `missing` (listed through [`CpState::view_catalog`] under the same
//! deadline): it may read this table. In a catalog that is not egret's,
//! `readersIncomplete` is always set: views defined there are not tracked. For
//! a VIEW the readers are always incomplete: recorded lineage names base
//! tables only, so no view is ever recorded as reading a view.
//!
//! **For a view: its own lineage** — the recorded bases, the lineage state
//! (`computed`, `failed` with why, or `missing`), the security mode and
//! owner read from the view's stored `trino.run-as-owner` (only in egret's own
//! catalogs; `unknown` elsewhere and whenever the read fails), and whether the
//! DEFINER owner is cleared for every classification the bases carry, as the
//! classification page's DEFINER audit reports it.

use std::collections::BTreeSet;

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_core::http::ErrorBody;
use egret_perm_compile::lineage::{self, RecordedLineage, ViewRef};
use egret_perm_compile::view_exposure::{ClassifiedEngine, ClassifiedTable};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::CpError;
use crate::governance_browse::{
    attached_engines, load_catalog, require_governance_read, seconds, BrowseKind,
};
use crate::view_exposure::{list_all, not_dialled_reason, LineageState, MatchedBy, SecurityMode};
use crate::CpState;

pub fn router() -> Router<CpState> {
    Router::new().route("/governance/lineage/{catalog_id}", get(object_lineage))
}

/// One view or materialized view that reads the object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LineageReader {
    /// The registry catalog name for a view; `default_catalog` for a
    /// StarRocks materialized view, where StarRocks keeps every one.
    pub catalog: String,
    pub schema: String,
    pub name: String,
    /// `view` or `materialized_view`.
    pub kind: BrowseKind,
    /// The StarRocks engine holding a materialized view; `null` for a view.
    pub engine: Option<String>,
    /// `lineage` for a view (its recorded lineage); `baseTable` or `text` for
    /// a materialized view, as the classification page matches them. `text`
    /// means the view has no base-table map and its definition names the
    /// catalog and schema: it may over-report.
    pub matched_by: MatchedBy,
}

/// A recorded base table of a view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct LineageTableRef {
    pub catalog: String,
    pub schema: String,
    pub table: String,
}

/// Why the reader list may be missing some.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ReadersIncomplete {
    /// A sentence for the page. Never an engine's own error text.
    pub detail: String,
}

/// A view's own lineage.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ViewLineageInfo {
    pub security_mode: SecurityMode,
    /// Why the mode is `unknown`; `null` otherwise.
    pub security_mode_error: Option<String>,
    /// The DEFINER's `trino.run-as-owner`; `null` for any other mode.
    pub owner: Option<String>,
    /// Whether the owner is cleared for every classification the bases carry.
    /// `null` when it cannot be said: not DEFINER, lineage not computed, or a
    /// reached clearance names only groups. Read from current role membership.
    pub owner_cleared: Option<bool>,
    pub lineage_state: LineageState,
    /// Why the lineage failed; `null` otherwise.
    pub lineage_detail: Option<String>,
    /// The recorded base tables, sorted; empty unless `computed`.
    pub bases: Vec<LineageTableRef>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ObjectLineage {
    /// The views and materialized views that read this table. See
    /// `readers_incomplete`.
    pub readers: Vec<LineageReader>,
    /// Set when `readers` may be missing some: a view in any of egret's
    /// catalogs has unknown lineage (failed or not yet computed), egret's views or an
    /// engine's materialized views could not be listed or were not asked, the
    /// catalog is not egret's (its own views are not tracked), or the object
    /// is a view.
    pub readers_incomplete: Option<ReadersIncomplete>,
    /// Set exactly when the object is a view.
    pub view: Option<ViewLineageInfo>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct LineageQuery {
    /// The object's schema (dotted for a nested egret namespace).
    pub schema: String,
    /// The table or view.
    pub table: String,
}

#[utoipa::path(
    get,
    path = "/api/control-plane/governance/lineage/{catalog_id}",
    tag = "control-plane-governance",
    params(
        ("catalog_id" = Uuid, Path, description = "Registry catalog id"),
        LineageQuery,
    ),
    responses(
        (status = 200, description = "The object's readers, and for a view its own lineage", body = ObjectLineage),
        (status = 400, description = "A missing or empty `schema` or `table`", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read and tag:read permission", body = ErrorBody),
        (status = 404, description = "No such catalog, or (egret catalog) no such table or view", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
        (status = 503, description = "(egret catalog) egret's warehouse could not be read to tell a table from a view", body = ErrorBody),
        (status = 504, description = "(egret catalog) egret's warehouse did not answer within the deadline", body = ErrorBody),
    ),
)]
async fn object_lineage(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(catalog_id): Path<Uuid>,
    Query(q): Query<LineageQuery>,
) -> Result<Json<ObjectLineage>, CpError> {
    require_governance_read(&st, &p).await?;
    if q.schema.is_empty() || q.table.is_empty() {
        return Err(CpError::BadRequest(
            "`schema` and `table` must not be empty".to_string(),
        ));
    }
    let catalog = load_catalog(&st, catalog_id)
        .await?
        .ok_or(CpError::NotFound)?;
    let object = ViewRef {
        catalog: catalog.name.clone(),
        schema: q.schema.clone(),
        name: q.table.clone(),
    };
    let recorded = {
        let mut conn = acquire(&st).await?;
        lineage::recorded_lineage(&mut conn, &object)
            .await
            .map_err(db("recorded lineage"))?
    };

    let lakekeeper = match catalog.warehouse_id {
        Some(warehouse_id) => {
            let lk = lakekeeper_id(&st, warehouse_id).await?;
            let is_view = egret_object_is_view(&st, &lk, &q.schema, &q.table).await?;
            Some((lk, is_view))
        }
        None => None,
    };
    let is_view = match &lakekeeper {
        Some((_, is_view)) => *is_view,
        None => recorded != RecordedLineage::Missing,
    };

    if is_view {
        let lk = lakekeeper.map(|(lk, _)| lk);
        let view = view_info(&st, &object, lk, recorded).await?;
        return Ok(Json(ObjectLineage {
            readers: Vec::new(),
            readers_incomplete: Some(ReadersIncomplete {
                detail: VIEW_READERS_UNKNOWN.to_string(),
            }),
            view: Some(view),
        }));
    }

    // A view in any of egret's catalogs may read a table of any catalog.
    let mut incomplete = unknown_lineage(&st).await?;
    if lakekeeper.is_none() {
        // Only egret's view commits record lineage, so a view defined in
        // this catalog is never seen, and this object may itself be one.
        incomplete.push(VIEWS_NOT_TRACKED.to_string());
    }
    let mut readers = view_readers(&st, &object).await?;
    let (mvs, mv_incomplete) = mv_readers(&st, catalog_id, &object).await?;
    readers.extend(mvs);
    incomplete.extend(mv_incomplete);
    Ok(Json(ObjectLineage {
        readers,
        readers_incomplete: (!incomplete.is_empty()).then(|| ReadersIncomplete {
            detail: incomplete.join("; "),
        }),
        view: None,
    }))
}

/// `readersIncomplete` for a view.
const VIEW_READERS_UNKNOWN: &str = "egret records a view's lineage down to its base tables, \
                                    so the views and materialized views that read this view \
                                    are listed on those tables, not here";

/// `readersIncomplete` for a table in a catalog that is not egret's.
const VIEWS_NOT_TRACKED: &str = "views defined in this catalog are not tracked by egret, so \
                                 they are not listed here";

/// `readersIncomplete` when egret's views could not all be listed.
const VIEWS_UNLISTED: &str = "egret's views could not all be listed, so a view whose lineage \
                              is not yet computed may read this table; the error is in the \
                              server log";

/// `securityModeError` when the view's metadata could not be read.
const METADATA_UNREADABLE: &str =
    "the view's metadata could not be read; the error is in the server log";

/// `securityModeError` for a view outside egret's catalogs.
const NOT_EGRET_CATALOG: &str = "egret reads a view's security mode only in its own catalogs";

fn db(what: &'static str) -> impl FnOnce(sqlx::Error) -> CpError {
    move |e| {
        tracing::error!(error = %e, "governance lineage: {what}");
        CpError::Internal
    }
}

async fn acquire(st: &CpState) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>, CpError> {
    st.db.pool.acquire().await.map_err(db("acquire connection"))
}

/// The lakekeeper warehouse of an egret catalog. A warehouse not provisioned
/// in egret's catalog holds no object, so the object is not found.
async fn lakekeeper_id(st: &CpState, warehouse_id: Uuid) -> Result<String, CpError> {
    let warehouse = st.storage.get_warehouse(warehouse_id).await?;
    warehouse
        .and_then(|w| w.lakekeeper_warehouse_id)
        .ok_or(CpError::NotFound)
}

/// Whether `schema.table` in the lakekeeper warehouse `lk` is a view, read
/// under [`CpState::browse_timeout`]. Neither a table nor a view: 404. The
/// warehouse failing is a 503, missing the deadline a 504, each with a
/// sentence; its own text goes to the log.
async fn egret_object_is_view(
    st: &CpState,
    lk: &str,
    schema: &str,
    table: &str,
) -> Result<bool, CpError> {
    let Some(browser) = st.browser.as_ref() else {
        tracing::error!("governance lineage: no warehouse browser is wired");
        return Err(CpError::Internal);
    };
    // A schema is a namespace, its levels joined with `.` (as browse does).
    let ns: Vec<String> = schema.split('.').map(String::from).collect();
    match tokio::time::timeout(st.browse_timeout, browser.tabulars(lk, &ns)).await {
        Err(_) => {
            tracing::warn!(warehouse = %lk, timeout = ?st.browse_timeout,
                "governance lineage: the warehouse did not answer before the deadline");
            Err(CpError::Timeout(format!(
                "egret's warehouse did not answer within {} s, so egret cannot tell whether \
                 this is a table or a view",
                seconds(st.browse_timeout)
            )))
        }
        Ok(Err(egret_catalog_engine::CatalogEngineError::NotFound(_))) => Err(CpError::NotFound),
        Ok(Err(e)) => {
            tracing::warn!(warehouse = %lk, error = %e,
                "governance lineage: the warehouse listing failed");
            Err(CpError::Unavailable(
                "egret's warehouse could not be read, so egret cannot tell whether this is a \
                 table or a view; the error is in the server log"
                    .to_string(),
            ))
        }
        Ok(Ok(tabulars)) => tabulars
            .into_iter()
            .find(|t| t.name == table)
            .map(|t| t.kind == "view")
            .ok_or(CpError::NotFound),
    }
}

/// A view's own lineage. `lk` is its lakekeeper warehouse in an egret
/// catalog, `None` elsewhere.
async fn view_info(
    st: &CpState,
    view: &ViewRef,
    lk: Option<String>,
    recorded: RecordedLineage,
) -> Result<ViewLineageInfo, CpError> {
    let (lineage_state, lineage_detail, tables) = match recorded {
        RecordedLineage::Computed(tables) if !tables.is_empty() => {
            (LineageState::Computed, None, Some(tables))
        }
        // `write_lineage` never records an empty computed set; one read here
        // is unknown, as the classification page treats it.
        RecordedLineage::Computed(_) | RecordedLineage::Missing => {
            (LineageState::Missing, None, None)
        }
        RecordedLineage::Failed { detail } => (LineageState::Failed, Some(detail), None),
    };
    let (security_mode, owner, security_mode_error) = match lk {
        Some(prefix) => security_mode(st, view, prefix).await,
        None => (
            SecurityMode::Unknown,
            None,
            Some(NOT_EGRET_CATALOG.to_string()),
        ),
    };
    let owner_cleared = match (&owner, &tables) {
        (Some(owner), Some(tables)) => {
            let mut conn = acquire(st).await?;
            let reached = lineage::view_tag_set(&mut conn, tables)
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "governance lineage: view tag set");
                    CpError::Internal
                })?;
            egret_perm_compile::view_audit::owner_cleared(&mut conn, owner, &reached)
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "governance lineage: owner clearance");
                    CpError::Internal
                })?
        }
        _ => None,
    };
    Ok(ViewLineageInfo {
        security_mode,
        security_mode_error,
        owner,
        owner_cleared,
        lineage_state,
        lineage_detail,
        bases: tables
            .unwrap_or_default()
            .into_iter()
            .map(|(catalog, schema, table)| LineageTableRef {
                catalog,
                schema,
                table,
            })
            .collect(),
    })
}

/// The view's stored `trino.run-as-owner`, read under
/// [`CpState::browse_timeout`]. A failed or late read is `unknown`, never
/// inferred as either mode.
async fn security_mode(
    st: &CpState,
    view: &ViewRef,
    prefix: String,
) -> (SecurityMode, Option<String>, Option<String>) {
    let target = egret_catalog_engine::CatalogView {
        catalog: view.catalog.clone(),
        schema: view.schema.clone(),
        name: view.name.clone(),
        prefix,
    };
    match tokio::time::timeout(st.browse_timeout, st.view_catalog.run_as_owner(&target)).await {
        Ok(Ok(Some(owner))) => (SecurityMode::Definer, Some(owner), None),
        Ok(Ok(None)) => (SecurityMode::Invoker, None, None),
        Ok(Err(e)) => {
            tracing::warn!(catalog = %view.catalog, schema = %view.schema, view = %view.name,
                error = %e, "governance lineage: the view's metadata could not be read");
            (
                SecurityMode::Unknown,
                None,
                Some(METADATA_UNREADABLE.to_string()),
            )
        }
        Err(_) => {
            tracing::warn!(catalog = %view.catalog, schema = %view.schema, view = %view.name,
                timeout = ?st.browse_timeout,
                "governance lineage: the view's metadata was not read before the deadline");
            (
                SecurityMode::Unknown,
                None,
                Some(format!(
                    "the view's metadata was not read within {} s",
                    seconds(st.browse_timeout)
                )),
            )
        }
    }
}

/// Why egret's views may hold readers that
/// [`view_readers`] cannot see: views whose lineage `failed` (from
/// `view_lineage_state`), and views egret's catalog lists that have no lineage
/// recorded at all (`missing`). Fail closed, as the classification page's
/// DEFINER audit lists such views: either may read this table. A listing that
/// fails or misses [`CpState::browse_timeout`] is said too.
///
/// Across ALL of egret's catalogs: a view in one catalog may read a table of
/// another (`view_lineage` keeps the two catalogs apart, migration 0187), as
/// [`view_readers`] already reports for computed lineage.
async fn unknown_lineage(st: &CpState) -> Result<Vec<String>, CpError> {
    let mut unknown: BTreeSet<ViewRef> = {
        let mut conn = acquire(st).await?;
        sqlx::query_as::<_, (String, String, String)>(
            "SELECT view_catalog, view_schema, view_name FROM view_lineage_state \
              WHERE state <> 'computed'",
        )
        .fetch_all(&mut *conn)
        .await
        .map_err(db("failed lineages"))?
        .into_iter()
        .map(|(catalog, schema, name)| ViewRef {
            catalog,
            schema,
            name,
        })
        .collect()
    };
    let mut out = Vec::new();
    match tokio::time::timeout(st.browse_timeout, st.view_catalog.list_views()).await {
        Err(_) => {
            tracing::warn!(timeout = ?st.browse_timeout,
                "governance lineage: egret's views were not listed before the deadline");
            out.push(VIEWS_UNLISTED.to_string());
        }
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "governance lineage: listing egret's views");
            out.push(VIEWS_UNLISTED.to_string());
        }
        Ok(Ok(listing)) => {
            if !listing.failures.is_empty() {
                tracing::warn!(failures = ?listing.failures,
                    "governance lineage: listing egret's views");
                out.push(VIEWS_UNLISTED.to_string());
            }
            let refs: Vec<ViewRef> = listing
                .views
                .into_iter()
                .map(|v| ViewRef {
                    catalog: v.catalog,
                    schema: v.schema,
                    name: v.name,
                })
                .collect();
            let mut conn = acquire(st).await?;
            let recorded = lineage::recorded_lineages(&mut conn, &refs)
                .await
                .map_err(db("recorded lineages"))?;
            for (v, r) in refs.into_iter().zip(recorded) {
                if !matches!(&r, RecordedLineage::Computed(t) if !t.is_empty()) {
                    unknown.insert(v);
                }
            }
        }
    }
    if !unknown.is_empty() {
        let n = unknown.len();
        out.insert(
            0,
            format!(
                "{n} {} in egret's catalogs {} lineage egret does not know (failed or not yet \
                 computed); {} may read this table",
                if n == 1 { "view" } else { "views" },
                if n == 1 { "has" } else { "have" },
                if n == 1 { "it" } else { "they" },
            ),
        );
    }
    Ok(out)
}

/// The views whose computed lineage lists `table` as a base, sorted.
async fn view_readers(st: &CpState, table: &ViewRef) -> Result<Vec<LineageReader>, CpError> {
    let mut conn = acquire(st).await?;
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT DISTINCT l.view_catalog, l.view_schema, l.view_name \
           FROM view_lineage l \
           JOIN view_lineage_state st \
             ON st.view_catalog = l.view_catalog AND st.view_schema = l.view_schema \
            AND st.view_name = l.view_name AND st.state = 'computed' \
          WHERE lower(l.base_catalog) = lower($1) AND lower(l.base_schema) = lower($2) \
            AND lower(l.base_table) = lower($3) \
          ORDER BY 1, 2, 3",
    )
    .bind(&table.catalog)
    .bind(&table.schema)
    .bind(&table.name)
    .fetch_all(&mut *conn)
    .await
    .map_err(db("view readers"))?;
    Ok(rows
        .into_iter()
        .map(|(catalog, schema, name)| LineageReader {
            catalog,
            schema,
            name,
            kind: BrowseKind::View,
            engine: None,
            matched_by: MatchedBy::Lineage,
        })
        .collect())
}

/// `engine_mgmt.engines.type` of a StarRocks engine.
const STARROCKS: &str = "starrocks";

/// The materialized views that read `table` on the StarRocks engines attached
/// to the catalog, and one sentence per engine whose views are unknown (not
/// asked, failed, or late).
async fn mv_readers(
    st: &CpState,
    catalog_id: Uuid,
    table: &ViewRef,
) -> Result<(Vec<LineageReader>, Vec<String>), CpError> {
    let mut incomplete = Vec::new();
    let mut dialled = Vec::new();
    for e in attached_engines(st, catalog_id).await? {
        if e.engine_type != STARROCKS {
            continue;
        }
        let engine = ClassifiedEngine {
            engine_id: e.id,
            engine_name: e.name,
            status: e.status,
            origin: e.origin,
            tables: vec![ClassifiedTable {
                catalog: table.catalog.clone(),
                database: table.schema.clone(),
                table: table.name.clone(),
            }],
        };
        match not_dialled_reason(&engine) {
            None => dialled.push(engine),
            Some(reason) => incomplete.push(format!(
                "the materialized views on engine {} were not checked ({reason})",
                engine.engine_name
            )),
        }
    }
    let (views, failures) = list_all(&*st.mv_lister, &dialled, st.browse_timeout).await;
    for f in failures {
        tracing::warn!(engine = %f.engine, error = %f.error,
            "governance lineage: the materialized-view listing failed");
        incomplete.push(format!(
            "egret could not list the materialized views on engine {}; the error is in the \
             server log",
            f.engine
        ));
    }
    let mut readers: Vec<LineageReader> = views
        .into_iter()
        .map(|v| LineageReader {
            catalog: v.catalog,
            schema: v.database,
            name: v.name,
            kind: BrowseKind::MaterializedView,
            engine: Some(v.engine),
            matched_by: v.matched_by,
        })
        .collect();
    readers.sort_by(|a, b| (&a.engine, &a.schema, &a.name).cmp(&(&b.engine, &b.schema, &b.name)));
    Ok((readers, incomplete))
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(object_lineage),
    components(schemas(
        ObjectLineage,
        LineageReader,
        LineageTableRef,
        ReadersIncomplete,
        ViewLineageInfo,
        BrowseKind,
        MatchedBy,
        SecurityMode,
        LineageState,
        ErrorBody
    )),
    tags((name = "control-plane-governance", description = "Governance Data: browse a catalog and read an object's lineage (DG frontend redesign #1)")),
)]
pub struct ControlPlaneGovernanceLineageApi;
