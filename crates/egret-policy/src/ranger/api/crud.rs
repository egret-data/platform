use crate::ranger::error::RangerError;
use crate::ranger::routes::RangerState;
use crate::AuthzError;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

pub fn crud_routes() -> Router<RangerState> {
    Router::new()
        .route("/plugins/definitions", get(list_defs))
        .route("/plugins/definitions/name/{name}", get(get_def))
        .route("/plugins/services", get(list_services).post(create_service))
        .route(
            "/plugins/services/{id}",
            axum::routing::delete(delete_service),
        )
        .route("/plugins/policies", post(create_policy))
        .route(
            "/plugins/policies/{id}",
            get(get_policy).put(update_policy).delete(delete_policy),
        )
}

async fn list_defs(State(s): State<RangerState>) -> Result<Json<Value>, RangerError> {
    let defs: Vec<Value> = s
        .store
        .list_service_defs()
        .await?
        .into_iter()
        .map(|d| d.def_jsonb)
        .collect();
    Ok(Json(json!({ "serviceDefs": defs })))
}

async fn get_def(
    State(s): State<RangerState>,
    Path(name): Path<String>,
) -> Result<Json<Value>, RangerError> {
    Ok(Json(s.store.get_service_def(&name).await?.def_jsonb))
}

async fn list_services(State(s): State<RangerState>) -> Result<Json<Value>, RangerError> {
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

async fn create_service(
    State(s): State<RangerState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, RangerError> {
    let name = body["name"]
        .as_str()
        .ok_or_else(|| RangerError::from(AuthzError::BadRequest("service.name required".into())))?;
    let typ = body["type"]
        .as_str()
        .ok_or_else(|| RangerError::from(AuthzError::BadRequest("service.type required".into())))?;
    let configs = body.get("configs").cloned().unwrap_or_else(|| json!({}));
    let row = s.store.create_service(name, typ, &configs).await?;
    Ok(Json(json!({
        "id": row.id,
        "name": row.name,
        "type": row.r#type,
        "configs": row.config_jsonb,
        "policyVersion": row.policy_version,
        "isEnabled": true,
    })))
}

async fn delete_service(
    State(s): State<RangerState>,
    Path(id): Path<i64>,
) -> Result<axum::http::StatusCode, RangerError> {
    s.store.delete_service(id).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

async fn create_policy(
    State(s): State<RangerState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, RangerError> {
    let service = body["service"].as_str().ok_or_else(|| {
        RangerError::from(AuthzError::BadRequest("policy.service required".into()))
    })?;
    let name = body["name"]
        .as_str()
        .ok_or_else(|| RangerError::from(AuthzError::BadRequest("policy.name required".into())))?;
    let svc = s.store.get_service(service).await?;
    let id = s.store.create_policy(svc.id, name, &body).await?;
    let mut out = body.clone();
    out["id"] = json!(id);
    Ok(Json(out))
}

async fn get_policy(
    State(s): State<RangerState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, RangerError> {
    let row = s.store.get_policy(id).await?;
    let mut out = row.policy_jsonb;
    out["id"] = json!(row.id);
    Ok(Json(out))
}

async fn update_policy(
    State(s): State<RangerState>,
    Path(id): Path<i64>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, RangerError> {
    let name = body["name"]
        .as_str()
        .ok_or_else(|| RangerError::from(AuthzError::BadRequest("policy.name required".into())))?;
    let enabled = body
        .get("isEnabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    s.store.update_policy(id, name, &body, enabled).await?;
    let mut out = body.clone();
    out["id"] = json!(id);
    Ok(Json(out))
}

async fn delete_policy(
    State(s): State<RangerState>,
    Path(id): Path<i64>,
) -> Result<axum::http::StatusCode, RangerError> {
    s.store.delete_policy(id).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
