//! View governance A4: `GET /classifications/{id}/view-exposure` — the
//! materialized views that already exist in the databases holding a
//! classification's columns.
//!
//! The A3 deny stops NEW materialized views there; it cannot remove one that
//! exists, and an existing one may hold a copy of classified values. egret lists
//! them and does not drop them. Gated on `tag:read`, the permission that reads a
//! classification (`GET /tags`).
//!
//! **Null means "could not list", never "none".** `starrocksMaterializedViews`
//! is `null` exactly when at least one listing failed, and
//! `starrocksListError` then names the failing engine and database. A partial
//! list is never returned as if it were complete.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_core::http::ErrorBody;
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

/// Lists the materialized views of one StarRocks database through egret's admin
/// connection. A trait so the route test needs no live StarRocks.
#[async_trait::async_trait]
pub trait MaterializedViewLister: Send + Sync {
    /// The names of the materialized views in `catalog`.`database`, or the
    /// reason the engine could not be asked.
    async fn list(
        &self,
        engine: &ExposureEngine,
        catalog: &str,
        database: &str,
    ) -> Result<Vec<String>, String>;
}

/// One view that exists in a classified database. Shared by the StarRocks list
/// and the Trino list (filled by a later task).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ViewExposureItem {
    pub engine: String,
    pub catalog: String,
    pub database: String,
    pub name: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ViewExposureDto {
    /// `null` when at least one listing failed; see `starrocks_list_error`.
    pub starrocks_materialized_views: Option<Vec<ViewExposureItem>>,
    /// Set exactly when `starrocks_materialized_views` is `null`; names each
    /// failing engine and database.
    pub starrocks_list_error: Option<String>,
    /// Always empty until Trino views are reported.
    pub trino_views: Vec<ViewExposureItem>,
}

#[utoipa::path(
    get,
    path = "/api/control-plane/classifications/{id}/view-exposure",
    tag = "control-plane-tags",
    params(("id" = Uuid, Path, description = "Classification (tag) id")),
    responses(
        (status = 200, description = "The materialized views already present in the databases that hold this classification's columns", body = ViewExposureDto),
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

    let databases = {
        let mut conn = st.db.pool.acquire().await.map_err(|e| {
            tracing::error!(error = %e, "view-exposure: acquire connection");
            CpError::Internal
        })?;
        egret_perm_compile::view_exposure::classified_starrocks_databases(&mut conn, &tag.key)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "view-exposure: resolve classified databases");
                CpError::Internal
            })?
    };

    let (views, failures) = list_all(&*st.mv_lister, &databases).await;
    Ok(Json(ViewExposureDto {
        starrocks_materialized_views: failures.is_empty().then_some(views),
        starrocks_list_error: (!failures.is_empty()).then(|| failures.join("; ")),
        trino_views: Vec::new(),
    }))
}

/// Ask every database; collect what listed and what failed. The caller decides
/// that ANY failure withholds the whole list.
async fn list_all(
    lister: &dyn MaterializedViewLister,
    databases: &[egret_perm_compile::view_exposure::ClassifiedDatabase],
) -> (Vec<ViewExposureItem>, Vec<String>) {
    let mut views = Vec::new();
    let mut failures = Vec::new();
    for d in databases {
        let engine = ExposureEngine {
            id: d.engine_id,
            name: d.engine_name.clone(),
        };
        match lister.list(&engine, &d.catalog, &d.database).await {
            Ok(names) => views.extend(names.into_iter().map(|name| ViewExposureItem {
                engine: d.engine_name.clone(),
                catalog: d.catalog.clone(),
                database: d.database.clone(),
                name,
            })),
            Err(e) => failures.push(format!(
                "could not list materialized views on engine {} in {}.{}: {e}",
                d.engine_name, d.catalog, d.database
            )),
        }
    }
    (views, failures)
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(get_view_exposure),
    components(schemas(ViewExposureDto, ViewExposureItem, ErrorBody)),
    tags((name = "control-plane-tags", description = "Platform classification-tag catalog (Access P3.1) + access policies (P3.3)")),
)]
pub struct ControlPlaneViewExposureApi;
