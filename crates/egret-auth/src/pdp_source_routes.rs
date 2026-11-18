//! EO3: platform-admin endpoints controlling the PDP policy source switch.
//! Nested at `/api/control-plane` (routes carry `/perm/pdp-source`). The switch
//! logic lives in `egret-auth` (the only crate that sees both assemblers); these
//! handlers are thin. Every mutation publishes `RolesChanged` so the PDP reloads
//! and the new source applies promptly; the state is re-read every reload, so a
//! missed nudge is harmless.
use axum::extract::{FromRef, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use egret_events::EventBus;
use egret_metastore::Db;

use crate::events::RolesChanged;
use crate::pdp::source_switch::{
    read_state, write_source, Divergence, DivergenceKind, PolicySource, SourceState,
};
use crate::pdp::CedarPdp;
use crate::principal::PlatformAuth;
use crate::session::SessionService;

#[derive(Clone)]
pub struct PdpSourceState {
    pub db: Db,
    pub sessions: SessionService,
    pub bus: EventBus,
}

impl FromRef<PdpSourceState> for SessionService {
    fn from_ref(s: &PdpSourceState) -> Self {
        s.sessions.clone()
    }
}

pub fn pdp_source_router() -> Router<PdpSourceState> {
    Router::new()
        .route("/perm/pdp-source", get(get_source))
        .route("/perm/pdp-source/report", post(post_report))
        .route("/perm/pdp-source/shadow", post(post_shadow))
        .route("/perm/pdp-source/switch", post(post_switch))
        .route("/perm/pdp-source/undo", post(post_undo))
}

/// OpenAPI fragment for the EO3 PDP-source operator endpoints. Merged into
/// `egret_auth::openapi()`.
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(get_source, post_report, post_shadow, post_switch, post_undo),
    components(schemas(SourceState, Divergence, DivergenceKind, PolicySource))
)]
pub struct PdpSourceApi;

#[utoipa::path(
    get, path = "/api/control-plane/perm/pdp-source", tag = "control-plane-perm",
    responses(
        (status = 200, description = "Current PDP policy source + audit", body = SourceState),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Not a platform admin", body = egret_core::http::ErrorBody),
    ),
)]
async fn get_source(
    State(st): State<PdpSourceState>,
    _admin: PlatformAuth,
) -> Result<Json<SourceState>, egret_core::Error> {
    Ok(Json(read_state(&st.db).await?))
}

#[utoipa::path(
    post, path = "/api/control-plane/perm/pdp-source/report", tag = "control-plane-perm",
    responses(
        (status = 200, description = "Live equivalence report (empty ⇒ safe to switch)", body = [Divergence]),
        (status = 403, description = "Not a platform admin", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn post_report(
    State(st): State<PdpSourceState>,
    _admin: PlatformAuth,
) -> Result<Json<Vec<Divergence>>, egret_core::Error> {
    Ok(Json(CedarPdp::report(&st.db).await?))
}

#[utoipa::path(post, path = "/api/control-plane/perm/pdp-source/shadow", tag = "control-plane-perm",
    responses((status = 200, body = SourceState), (status = 403, body = egret_core::http::ErrorBody)))]
async fn post_shadow(
    State(st): State<PdpSourceState>,
    PlatformAuth(p): PlatformAuth,
) -> Result<Json<SourceState>, egret_core::Error> {
    write_source(&st.db, PolicySource::Shadow, &p.email).await?;
    st.bus.publish(RolesChanged);
    Ok(Json(read_state(&st.db).await?))
}

#[utoipa::path(post, path = "/api/control-plane/perm/pdp-source/switch", tag = "control-plane-perm",
    responses(
        (status = 200, description = "Switched to compiled (report was clean)", body = SourceState),
        (status = 403, body = egret_core::http::ErrorBody),
        (status = 409, description = "Not in shadow, or the report is not clean", body = egret_core::http::ErrorBody),
    ))]
async fn post_switch(
    State(st): State<PdpSourceState>,
    PlatformAuth(p): PlatformAuth,
) -> Result<Json<SourceState>, egret_core::Error> {
    // Gate 1: must be in shadow (soak before cutover).
    if read_state(&st.db).await?.source != PolicySource::Shadow {
        return Err(egret_core::Error::Conflict(
            "switch to compiled requires the current source to be shadow".into(),
        ));
    }
    // Gate 2: a freshly-run report must be clean.
    let report = CedarPdp::report(&st.db).await?;
    if !report.is_empty() {
        return Err(egret_core::Error::Conflict(format!(
            "refusing to switch: {} divergence(s) between legacy and compiled",
            report.len()
        )));
    }
    write_source(&st.db, PolicySource::Compiled, &p.email).await?;
    st.bus.publish(RolesChanged);
    Ok(Json(read_state(&st.db).await?))
}

#[utoipa::path(post, path = "/api/control-plane/perm/pdp-source/undo", tag = "control-plane-perm",
    responses((status = 200, description = "Stepped back one state", body = SourceState),
        (status = 403, body = egret_core::http::ErrorBody)))]
async fn post_undo(
    State(st): State<PdpSourceState>,
    PlatformAuth(p): PlatformAuth,
) -> Result<Json<SourceState>, egret_core::Error> {
    // compiled -> shadow -> legacy. Idempotent at legacy.
    let next = match read_state(&st.db).await?.source {
        PolicySource::Compiled => PolicySource::Shadow,
        PolicySource::Shadow => PolicySource::Legacy,
        PolicySource::Legacy => PolicySource::Legacy,
    };
    write_source(&st.db, next, &p.email).await?;
    st.bus.publish(RolesChanged);
    Ok(Json(read_state(&st.db).await?))
}
