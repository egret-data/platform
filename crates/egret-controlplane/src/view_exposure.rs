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
//! returned as if it were complete. An engine that is not `running` is not
//! dialled; it is named in `enginesNotChecked` with its status instead.

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

/// An engine that reaches the classification but was not asked, because it is
/// not running.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct EngineNotChecked {
    pub engine: String,
    /// `engine_mgmt.engines.status`: `provisioning`, `failed` or `deleting`.
    pub status: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ViewExposureDto {
    /// `null` when at least one listing failed; see `starrocks_list_error`.
    pub starrocks_materialized_views: Option<Vec<ViewExposureItem>>,
    /// Set exactly when `starrocks_materialized_views` is `null`; names each
    /// failing engine.
    pub starrocks_list_error: Option<String>,
    /// Engines that reach the classification but are not running, so were not
    /// asked. Their views are unknown, not absent.
    pub engines_not_checked: Vec<EngineNotChecked>,
    /// Always empty until Trino views are reported.
    pub trino_views: Vec<ViewExposureItem>,
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
    let (running, not_running): (Vec<_>, Vec<_>) =
        engines.into_iter().partition(|e| e.status == RUNNING);

    let (views, failures) = list_all(&*st.mv_lister, &running, st.mv_list_timeout).await;
    Ok(Json(ViewExposureDto {
        starrocks_materialized_views: failures.is_empty().then_some(views),
        starrocks_list_error: (!failures.is_empty()).then(|| failures.join("; ")),
        engines_not_checked: not_running
            .into_iter()
            .map(|e| EngineNotChecked {
                engine: e.engine_name,
                status: e.status,
            })
            .collect(),
        trino_views: Vec::new(),
    }))
}

/// `engine_mgmt.engines.status` of an engine whose pods all reported ready
/// (`migrations/0145_engine_status_check.sql`).
const RUNNING: &str = "running";

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

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(get_view_exposure),
    components(schemas(ViewExposureDto, ViewExposureItem, MatchedBy, EngineNotChecked, ErrorBody)),
    tags((name = "control-plane-tags", description = "Platform classification-tag catalog (Access P3.1) + access policies (P3.3)")),
)]
pub struct ControlPlaneViewExposureApi;
