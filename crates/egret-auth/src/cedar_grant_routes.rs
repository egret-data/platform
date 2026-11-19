//! EO4: platform-admin endpoints controlling the grant-source switch (legacy
//! `access_grants` vs the unified `perm.cedar_grants` mirror). Nested at
//! `/api/control-plane` (routes carry `/perm/cedar-grants`). Handlers are thin;
//! every mutation publishes `RolesChanged` so the PDP reloads promptly. The state
//! is re-read on every reload, so a missed nudge is harmless.
use axum::extract::{FromRef, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use egret_events::EventBus;
use egret_metastore::Db;

use crate::events::RolesChanged;
use crate::grants::{AccessGrantStore, GrantDivergence, GrantDivergenceKind};
use crate::pdp::grant_source_switch::{read_state, write_source, GrantSource, GrantSourceState};
use crate::principal::PlatformAuth;
use crate::session::SessionService;

#[derive(Clone)]
pub struct CedarGrantState {
    pub db: Db,
    pub sessions: SessionService,
    pub bus: EventBus,
}

impl FromRef<CedarGrantState> for SessionService {
    fn from_ref(s: &CedarGrantState) -> Self {
        s.sessions.clone()
    }
}

pub fn cedar_grant_router() -> Router<CedarGrantState> {
    Router::new()
        .route("/perm/cedar-grants", get(get_state))
        .route("/perm/cedar-grants/report", post(post_report))
        .route("/perm/cedar-grants/fold", post(post_fold))
        .route("/perm/cedar-grants/switch", post(post_switch))
        .route("/perm/cedar-grants/undo", post(post_undo))
}

/// OpenAPI fragment for the EO4 grant-source operator endpoints. Merged into
/// `egret_auth::openapi()`.
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(get_state, post_report, post_fold, post_switch, post_undo),
    components(schemas(GrantSourceState, GrantSource, GrantDivergence, GrantDivergenceKind))
)]
pub struct CedarGrantApi;

#[utoipa::path(
    get, path = "/api/control-plane/perm/cedar-grants", tag = "control-plane-perm",
    operation_id = "cedar_grants_state",
    responses(
        (status = 200, description = "Current grant source + audit", body = GrantSourceState),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Not a platform admin", body = egret_core::http::ErrorBody),
    ),
)]
async fn get_state(
    State(st): State<CedarGrantState>,
    _admin: PlatformAuth,
) -> Result<Json<GrantSourceState>, egret_core::Error> {
    Ok(Json(read_state(&st.db).await?))
}

#[utoipa::path(
    post, path = "/api/control-plane/perm/cedar-grants/report", tag = "control-plane-perm",
    operation_id = "cedar_grants_report",
    responses(
        (status = 200, description = "Live equivalence report (empty => safe to switch)", body = [GrantDivergence]),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Not a platform admin", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn post_report(
    State(st): State<CedarGrantState>,
    _admin: PlatformAuth,
) -> Result<Json<Vec<GrantDivergence>>, egret_core::Error> {
    Ok(Json(AccessGrantStore::new(st.db.clone()).report().await?))
}

#[utoipa::path(post, path = "/api/control-plane/perm/cedar-grants/fold", tag = "control-plane-perm",
    operation_id = "cedar_grants_fold",
    responses((status = 200, description = "Mirror backfilled; source unchanged", body = GrantSourceState),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, body = egret_core::http::ErrorBody)))]
async fn post_fold(
    State(st): State<CedarGrantState>,
    _admin: PlatformAuth,
) -> Result<Json<GrantSourceState>, egret_core::Error> {
    AccessGrantStore::new(st.db.clone()).fold().await?;
    st.bus.publish(RolesChanged);
    Ok(Json(read_state(&st.db).await?))
}

#[utoipa::path(post, path = "/api/control-plane/perm/cedar-grants/switch", tag = "control-plane-perm",
    operation_id = "cedar_grants_switch",
    responses(
        (status = 200, description = "Switched to unified (report was clean)", body = GrantSourceState),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, body = egret_core::http::ErrorBody),
        (status = 409, description = "The report is not clean", body = egret_core::http::ErrorBody),
    ))]
async fn post_switch(
    State(st): State<CedarGrantState>,
    PlatformAuth(p): PlatformAuth,
) -> Result<Json<GrantSourceState>, egret_core::Error> {
    // The clean report is the gate (EO4 has no shadow state).
    let report = AccessGrantStore::new(st.db.clone()).report().await?;
    if !report.is_empty() {
        return Err(egret_core::Error::Conflict(format!(
            "refusing to switch: {} grant divergence(s) between access_grants and the unified mirror",
            report.len()
        )));
    }
    write_source(&st.db, GrantSource::Unified, &p.email).await?;
    st.bus.publish(RolesChanged);
    Ok(Json(read_state(&st.db).await?))
}

#[utoipa::path(post, path = "/api/control-plane/perm/cedar-grants/undo", tag = "control-plane-perm",
    operation_id = "cedar_grants_undo",
    responses((status = 200, description = "Back to legacy", body = GrantSourceState),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, body = egret_core::http::ErrorBody)))]
async fn post_undo(
    State(st): State<CedarGrantState>,
    PlatformAuth(p): PlatformAuth,
) -> Result<Json<GrantSourceState>, egret_core::Error> {
    write_source(&st.db, GrantSource::Legacy, &p.email).await?;
    st.bus.publish(RolesChanged);
    Ok(Json(read_state(&st.db).await?))
}
