//! Read-only Ranger Admin REST surface (service-defs / services / policies GET).
//!
//! E2e: the write routes (`POST`/`PUT`/`DELETE` on `/plugins/services` and
//! `/plugins/policies`) were removed. Nothing in egret drove them — the admin
//! policy editor writes through the authenticated control-plane path
//! (`egret-policy`'s `admin` router), and stock engine plugins only ever GET the
//! download endpoint. Keeping unauthenticated write routes on the anonymous
//! `/service` surface (open when `ranger.auth_token` is empty) was pure liability;
//! only these read-only endpoints remain, so an empty token exposes reads, never
//! writes. Re-add behind auth if external Ranger-Admin write compat is ever needed.
use crate::ranger::error::RangerError;
use crate::ranger::routes::{RangerErrorBody, RangerServiceDefs, RangerServiceRef, RangerState};
use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

pub fn crud_routes() -> Router<RangerState> {
    Router::new()
        .route("/plugins/definitions", get(list_defs))
        .route("/plugins/definitions/name/{name}", get(get_def))
        .route("/plugins/services", get(list_services))
        .route("/plugins/policies/{id}", get(get_policy))
}

#[utoipa::path(
    get,
    path = "/ranger/service/plugins/definitions",
    tag = "ranger",
    responses(
        (status = 200, description = "All stored Ranger service-definitions", body = RangerServiceDefs),
        (status = 401, description = "Missing or invalid bearer token (when a ranger auth token is configured)"),
        (status = 500, description = "Internal error", body = RangerErrorBody),
    ),
)]
pub async fn list_defs(State(s): State<RangerState>) -> Result<Json<Value>, RangerError> {
    let defs: Vec<Value> = s
        .store
        .list_service_defs()
        .await?
        .into_iter()
        .map(|d| d.def_jsonb)
        .collect();
    Ok(Json(json!({ "serviceDefs": defs })))
}

#[utoipa::path(
    get,
    path = "/ranger/service/plugins/definitions/name/{name}",
    tag = "ranger",
    params(("name" = String, Path, description = "Service-definition name (e.g. `trino`)")),
    responses(
        (status = 200, description = "The raw Ranger service-definition", body = Object),
        (status = 401, description = "Missing or invalid bearer token (when a ranger auth token is configured)"),
        (status = 404, description = "No such service-definition", body = RangerErrorBody),
        (status = 500, description = "Internal error", body = RangerErrorBody),
    ),
)]
pub async fn get_def(
    State(s): State<RangerState>,
    Path(name): Path<String>,
) -> Result<Json<Value>, RangerError> {
    Ok(Json(s.store.get_service_def(&name).await?.def_jsonb))
}

#[utoipa::path(
    get,
    path = "/ranger/service/plugins/services",
    tag = "ranger",
    responses(
        (status = 200, description = "All policy profiles (Ranger services)", body = Vec<RangerServiceRef>),
        (status = 401, description = "Missing or invalid bearer token (when a ranger auth token is configured)"),
        (status = 500, description = "Internal error", body = RangerErrorBody),
    ),
)]
pub async fn list_services(State(s): State<RangerState>) -> Result<Json<Value>, RangerError> {
    let svcs: Vec<Value> = s
        .store
        .list_services()
        .await?
        .into_iter()
        .map(|r| {
            json!({
                "id": r.id,
                "name": r.name,
                "type": r.r#type,
                "configs": r.config_jsonb,
                "policyVersion": r.policy_version,
                "isEnabled": true,
            })
        })
        .collect();
    Ok(Json(json!(svcs)))
}

#[utoipa::path(
    get,
    path = "/ranger/service/plugins/policies/{id}",
    tag = "ranger",
    params(("id" = i64, Path, description = "Policy id")),
    responses(
        (status = 200, description = "The raw stored Ranger policy body (with id)", body = Object),
        (status = 401, description = "Missing or invalid bearer token (when a ranger auth token is configured)"),
        (status = 404, description = "No such policy", body = RangerErrorBody),
        (status = 500, description = "Internal error", body = RangerErrorBody),
    ),
)]
pub async fn get_policy(
    State(s): State<RangerState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, RangerError> {
    let row = s.store.get_policy(id).await?;
    let mut out = row.policy_jsonb;
    out["id"] = json!(row.id);
    Ok(Json(out))
}
