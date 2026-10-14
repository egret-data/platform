//! View governance A4 (revised 2026-10-14): `GET
//! /classifications/{id}/view-exposure` — the materialized views that already
//! exist and read a table of a classification.
//!
//! The A3 deny stops NEW materialized views; it cannot remove one that exists,
//! and an existing one may hold a copy of classified values. egret lists them
//! and does not drop them. Gated on `tag:read`, the permission that reads a
//! classification (`GET /tags`).
//!
//! **Where the views are.** StarRocks creates a materialized view only in
//! `default_catalog`, whatever catalog its base tables live in, and `SHOW
//! MATERIALIZED VIEWS FROM <external catalog>.<db>` answers an empty set
//! (measured on 4.1.6, `docs/en/plans/2026-10-14-starrocks-mv-placement-
//! measurement.md`). So each running engine is asked ONCE for every view it
//! holds, with its base tables, and the views are matched here:
//!
//! - **by base table**: a key of `BASE_TABLE_REFRESH_VERSION_TIMES` names a
//!   classified `catalog.db.table` in a catalog attached to THAT engine;
//! - **by text**, only when the view has no base-table map: its definition
//!   contains the classified catalog and database names. Flagged, because an
//!   unqualified or aliased reference is missed and a mention is not a read.
//!
//! **Null means "could not list", never "none".** `starrocksMaterializedViews`
//! is `null` exactly when at least one running engine's listing failed, and
//! `starrocksListError` then names each failing engine. A partial list is never
//! returned as if it were complete.
//!
//! **Engines that are never dialled.** An EXTERNAL engine (SP4d-4) is stored
//! with the placeholder status `running`, but egret never connects to a
//! cluster it does not own and holds no credential for one, so it is split off
//! on `origin` FIRST (Task 4b review C-1). A provisioned engine that is not
//! `running` is not dialled either. Both are named in `enginesNotChecked` with
//! the reason; their views are unknown, not absent.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_core::http::ErrorBody;
use egret_perm_compile::view_exposure::{ClassifiedEngine, MV_CATALOG};
use serde::Serialize;
use uuid::Uuid;

use crate::error::CpError;
use crate::CpState;

pub fn router() -> Router<CpState> {
    Router::new().route(
        "/classifications/{id}/view-exposure",
        get(get_view_exposure),
    )
}

/// An engine to list on. Carries the id (what the real lister dials) and the
/// name (what the page and the error line show).
#[derive(Debug, Clone)]
pub struct ExposureEngine {
    pub id: Uuid,
    pub name: String,
}

/// One row of the engine's `information_schema.materialized_views`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MvRow {
    /// The `default_catalog` database the view lives in.
    pub database: String,
    pub name: String,
    /// The keys of `BASE_TABLE_REFRESH_VERSION_TIMES` (`catalog.db.table`).
    /// `None` when the map is missing or empty, or not a JSON object.
    pub base_tables: Option<Vec<String>>,
    /// `MATERIALIZED_VIEW_DEFINITION`, as written.
    pub definition: String,
}

/// Lists every materialized view of one StarRocks engine through egret's admin
/// connection. A trait so the route test needs no live StarRocks.
#[async_trait::async_trait]
pub trait MaterializedViewLister: Send + Sync {
    /// Every materialized view on `engine`, or the reason it could not be
    /// asked.
    async fn list(&self, engine: &ExposureEngine) -> Result<Vec<MvRow>, String>;
}

/// How a view was matched to the classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum MatchedBy {
    /// A base table of the view is a classified table.
    BaseTable,
    /// The view has no base-table map, and its definition names the classified
    /// catalog and database.
    Text,
    /// A Trino view: its recorded lineage (spec B1) reaches the table, or
    /// could not be computed (`lineageState`).
    Lineage,
}

/// One view that reads a classified table. Shared by the StarRocks list and the
/// Trino list (filled by a later task).
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ViewExposureItem {
    pub engine: String,
    /// Where the view lives: `default_catalog` for every StarRocks view.
    pub catalog: String,
    pub database: String,
    pub name: String,
    pub matched_by: MatchedBy,
}

/// An engine that reaches the classification but was not asked.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct EngineNotChecked {
    pub engine: String,
    /// Why it was not asked: `external` (egret never connects to an engine it
    /// does not run, and holds no credential for it) or `not running: <status>`
    /// (`provisioning`, `failed` or `deleting`).
    pub reason: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ViewExposureDto {
    /// `null` when at least one listing failed; see `starrocks_list_error`.
    pub starrocks_materialized_views: Option<Vec<ViewExposureItem>>,
    /// Set exactly when `starrocks_materialized_views` is `null`; names each
    /// failing engine.
    pub starrocks_list_error: Option<String>,
    /// Engines that reach the classification but were not asked (external, or
    /// not running). Their views are unknown, not absent.
    pub engines_not_checked: Vec<EngineNotChecked>,
    /// The views in egret's catalogs that the DEFINER audit lists (spec B5):
    /// their lineage reaches a table of this classification, or a table a mask
    /// or row-filter rule selects, or is unknown. `null` when egret's catalogs
    /// could not all be listed; see `trino_list_error`.
    pub trino_views: Option<Vec<TrinoViewItem>>,
    /// Set exactly when `trino_views` is `null`; names what could not be
    /// listed.
    pub trino_list_error: Option<String>,
}

/// A Trino view's security mode, read from its stored `trino.run-as-owner`
/// property and never from the statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SecurityMode {
    /// The property is present; the view runs as its owner.
    Definer,
    /// The view's metadata was read and carries no such property.
    Invoker,
    /// The metadata could not be read. Never inferred as `definer`.
    Unknown,
}

/// Why a Trino view is listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrinoViewReason {
    /// The lineage reaches a table of this classification. Also given, fail
    /// closed, to a view whose lineage is `missing` or `failed`: it may.
    Classified,
    /// The lineage reaches a table a mask or row-filter rule selects. Base
    /// masks and row filters apply as a DEFINER view's owner, not its reader.
    MaskOrRowFilter,
}

/// What egret has recorded of a Trino view's lineage (spec B1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LineageState {
    Computed,
    /// Never computed: the view predates the hold, or was committed while
    /// its profile had no classified column and its EXPLAIN did not run.
    Missing,
    /// The last computation failed; `lineage_detail` says why.
    Failed,
}

/// One Trino view in the DEFINER audit (spec B5).
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TrinoViewItem {
    /// The registry catalog name.
    pub catalog: String,
    pub schema: String,
    pub name: String,
    pub security_mode: SecurityMode,
    /// Why the mode is `unknown`; `null` otherwise.
    pub security_mode_error: Option<String>,
    /// The DEFINER's `trino.run-as-owner`; `null` for any other mode.
    pub owner: Option<String>,
    /// Whether the owner is cleared for every classification the lineage
    /// reaches. `null` when it cannot be said: not DEFINER, lineage not
    /// computed, or a reached clearance names only groups.
    pub owner_cleared: Option<bool>,
    pub reason: TrinoViewReason,
    pub lineage_state: LineageState,
    /// Why the lineage failed; `null` otherwise.
    pub lineage_detail: Option<String>,
    /// Always `lineage`.
    pub matched_by: MatchedBy,
}

#[utoipa::path(
    get,
    path = "/api/control-plane/classifications/{id}/view-exposure",
    tag = "control-plane-tags",
    params(("id" = Uuid, Path, description = "Classification (tag) id")),
    responses(
        (status = 200, description = "The materialized views that already exist and read a table of this classification", body = ViewExposureDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing tag:read permission", body = ErrorBody),
        (status = 404, description = "No such classification", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn get_view_exposure(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<ViewExposureDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "tag:read").await?;
    let tag = st.tags.get(id).await?.ok_or(CpError::NotFound)?;

    let engines = {
        let mut conn = st.db.pool.acquire().await.map_err(|e| {
            tracing::error!(error = %e, "view-exposure: acquire connection");
            CpError::Internal
        })?;
        egret_perm_compile::view_exposure::classified_starrocks_engines(&mut conn, &tag.key)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "view-exposure: resolve classified engines");
                CpError::Internal
            })?
    };
    let mut dialled = Vec::new();
    let mut not_checked = Vec::new();
    for e in engines {
        match not_dialled_reason(&e) {
            None => dialled.push(e),
            Some(reason) => not_checked.push(EngineNotChecked {
                engine: e.engine_name,
                reason,
            }),
        }
    }

    let (views, failures) = list_all(&*st.mv_lister, &dialled, st.mv_list_timeout).await;
    let (trino_views, trino_list_error) = trino_views(&st, id).await?;
    Ok(Json(ViewExposureDto {
        starrocks_materialized_views: failures.is_empty().then_some(views),
        starrocks_list_error: (!failures.is_empty()).then(|| failures.join("; ")),
        engines_not_checked: not_checked,
        trino_views,
        trino_list_error,
    }))
}

/// `engine_mgmt.engines.status` of an engine whose pods all reported ready
/// (`migrations/0145_engine_status_check.sql`).
const RUNNING: &str = "running";

/// `engine_mgmt.engines.origin` of an engine egret does not run
/// (`migrations/0184_engine_external.sql`).
const ORIGIN_EXTERNAL: &str = "external";

/// Why `engine` must not be dialled, or `None` when it may be. Origin is checked
/// BEFORE status: an external engine's `running` is a placeholder.
fn not_dialled_reason(engine: &ClassifiedEngine) -> Option<String> {
    if engine.origin == ORIGIN_EXTERNAL {
        return Some("external".to_string());
    }
    (engine.status != RUNNING).then(|| format!("not running: {}", engine.status))
}

/// How `row` matches one of `engine`'s classified tables, if it does.
///
/// Case-insensitive, as StarRocks resolves catalog, database and table names
/// (and as the service-def's `ignoreCase` matchers do): an over-report here is a
/// warning too many, an under-report hides a copy.
fn match_row(engine: &ClassifiedEngine, row: &MvRow) -> Option<MatchedBy> {
    match &row.base_tables {
        Some(bases) => {
            let reads_classified = bases.iter().any(|base| {
                engine.tables.iter().any(|t| {
                    base.eq_ignore_ascii_case(&format!("{}.{}.{}", t.catalog, t.database, t.table))
                })
            });
            reads_classified.then_some(MatchedBy::BaseTable)
        }
        None => {
            let text = row.definition.to_ascii_lowercase();
            let names_classified = engine.tables.iter().any(|t| {
                text.contains(&t.catalog.to_ascii_lowercase())
                    && text.contains(&t.database.to_ascii_lowercase())
            });
            names_classified.then_some(MatchedBy::Text)
        }
    }
}

/// Ask every engine concurrently, all under one shared deadline; collect what
/// matched and what failed. The caller decides that ANY failure withholds the
/// whole list.
///
/// Each real listing opens its own FE connection (10 s connect, 30 s statement),
/// so asking in turn would cost an unreachable engine that long each. Concurrent
/// asking bounds the request at `timeout`; a listing still pending when it
/// passes is a failure that names its engine.
async fn list_all(
    lister: &dyn MaterializedViewLister,
    engines: &[ClassifiedEngine],
    timeout: std::time::Duration,
) -> (Vec<ViewExposureItem>, Vec<String>) {
    let deadline = tokio::time::Instant::now() + timeout;
    let results = futures_util::future::join_all(engines.iter().map(|e| async move {
        let target = ExposureEngine {
            id: e.engine_id,
            name: e.engine_name.clone(),
        };
        let listed = tokio::time::timeout_at(deadline, lister.list(&target))
            .await
            .unwrap_or_else(|_| Err(format!("no answer within {}s", timeout.as_secs_f32())));
        (e, listed)
    }))
    .await;

    let mut views = Vec::new();
    let mut failures = Vec::new();
    for (e, listed) in results {
        match listed {
            Ok(rows) => views.extend(rows.into_iter().filter_map(|row| {
                match_row(e, &row).map(|matched_by| ViewExposureItem {
                    engine: e.engine_name.clone(),
                    catalog: MV_CATALOG.to_string(),
                    database: row.database,
                    name: row.name,
                    matched_by,
                })
            })),
            Err(err) => failures.push(format!(
                "could not list materialized views on engine {}: {err}",
                e.engine_name
            )),
        }
    }
    (views, failures)
}

/// The Trino half of the report (spec B5): every view in egret's catalogs
/// whose recorded lineage reaches a table of the classification `tag_id` or a
/// table with a mask or row-filter rule, with its security mode, owner and
/// whether the owner is cleared.
///
/// **A view whose lineage is missing or failed is listed, fail closed**, with
/// `reason: classified` and its `lineage_state`: egret cannot say it reads
/// nothing classified. **A listing that fails withholds the whole list**
/// (`None` and the error), as the StarRocks half does.
async fn trino_views(
    st: &CpState,
    tag_id: Uuid,
) -> Result<(Option<Vec<TrinoViewItem>>, Option<String>), CpError> {
    use egret_perm_compile::lineage::{self, RecordedLineage, ViewRef};
    use egret_perm_compile::view_audit;

    let listing = match st.view_catalog.list_views().await {
        Ok(l) => l,
        Err(e) => return Ok((None, Some(format!("could not list egret's views: {e}")))),
    };
    if !listing.failures.is_empty() {
        return Ok((None, Some(listing.failures.join("; "))));
    }
    let internal = |what: &'static str| {
        move |e: &dyn std::fmt::Display| {
            tracing::error!(error = %e, "view-exposure: {what}");
            CpError::Internal
        }
    };
    let mut conn = st
        .db
        .pool
        .acquire()
        .await
        .map_err(|e| internal("acquire connection")(&e))?;

    let mut out = Vec::new();
    for view in listing.views {
        let at = ViewRef {
            catalog: view.catalog.clone(),
            schema: view.schema.clone(),
            name: view.name.clone(),
        };
        let recorded = lineage::recorded_lineage(&mut conn, &at)
            .await
            .map_err(|e| internal("recorded lineage")(&e))?;
        let (reason, lineage_state, lineage_detail, tables) = match recorded {
            RecordedLineage::Computed(tables) => {
                let reason = if view_audit::tables_carry_tag(&mut conn, &tables, tag_id)
                    .await
                    .map_err(|e| internal("classified tables")(&e))?
                {
                    TrinoViewReason::Classified
                } else if view_audit::tables_have_rule(&mut conn, &tables)
                    .await
                    .map_err(|e| internal("mask and row-filter rules")(&e))?
                {
                    TrinoViewReason::MaskOrRowFilter
                } else {
                    continue;
                };
                (reason, LineageState::Computed, None, Some(tables))
            }
            RecordedLineage::Missing => (
                TrinoViewReason::Classified,
                LineageState::Missing,
                None,
                None,
            ),
            RecordedLineage::Failed { detail } => (
                TrinoViewReason::Classified,
                LineageState::Failed,
                Some(detail),
                None,
            ),
        };
        let (security_mode, owner, security_mode_error) =
            match st.view_catalog.run_as_owner(&view).await {
                Ok(Some(owner)) => (SecurityMode::Definer, Some(owner), None),
                Ok(None) => (SecurityMode::Invoker, None, None),
                Err(e) => (SecurityMode::Unknown, None, Some(e)),
            };
        let owner_cleared = match (&owner, &tables) {
            (Some(owner), Some(tables)) => {
                let reached = lineage::view_tag_set(&mut conn, tables)
                    .await
                    .map_err(|e| internal("view tag set")(&e))?;
                view_audit::owner_cleared(&mut conn, owner, &reached)
                    .await
                    .map_err(|e| internal("owner clearance")(&e))?
            }
            _ => None,
        };
        out.push(TrinoViewItem {
            catalog: view.catalog,
            schema: view.schema,
            name: view.name,
            security_mode,
            security_mode_error,
            owner,
            owner_cleared,
            reason,
            lineage_state,
            lineage_detail,
            matched_by: MatchedBy::Lineage,
        });
    }
    Ok((Some(out), None))
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(get_view_exposure),
    components(schemas(
        ViewExposureDto,
        ViewExposureItem,
        MatchedBy,
        EngineNotChecked,
        TrinoViewItem,
        SecurityMode,
        TrinoViewReason,
        LineageState,
        ErrorBody
    )),
    tags((name = "control-plane-tags", description = "Platform classification-tag catalog (Access P3.1) + access policies (P3.3)")),
)]
pub struct ControlPlaneViewExposureApi;
