//! View governance B3: `POST /classifications/lineage-sweep` — run the lineage
//! sweep now, from the classification page, and report the views whose
//! lineage could not be computed (`egret_catalog_engine::lineage_sweep`).
//!
//! Gated on `tag:manage`, the permission of the classification write routes
//! (`PUT`/`DELETE /tags/{id}/access-policy`): the sweep writes lineage and
//! view tags, and bumps the tag versions of the profiles it changes.

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_catalog_engine::{SweepError, SweepFailure, SweepReport};
use egret_core::http::ErrorBody;

use crate::error::CpError;
use crate::CpState;

pub fn router() -> Router<CpState> {
    Router::new().route("/classifications/lineage-sweep", post(run_lineage_sweep))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/classifications/lineage-sweep",
    tag = "control-plane-tags",
    responses(
        (status = 200, description = "The sweep ran. `failures` names every view (or listing) whose lineage could not be computed; those views keep their tags", body = SweepReport),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing tag:manage permission", body = ErrorBody),
        (status = 409, description = "A sweep is already running, or the embedded catalog is not enabled", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn run_lineage_sweep(
    State(st): State<CpState>,
    Auth(p): Auth,
) -> Result<Json<SweepReport>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "tag:manage").await?;
    match st.lineage_sweeper.sweep().await {
        Ok(report) => Ok(Json(report)),
        Err(e @ (SweepError::Busy | SweepError::Unavailable(_))) => {
            Err(CpError::Conflict(e.to_string()))
        }
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(run_lineage_sweep),
    components(schemas(SweepReport, SweepFailure, ErrorBody)),
    tags((name = "control-plane-tags", description = "Platform classification-tag catalog (Access P3.1) + access policies (P3.3)")),
)]
pub struct ControlPlaneLineageSweepApi;
