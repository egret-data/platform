//! Control-plane Secrets router (`/secrets`), nested under `/api/control-plane`.
//!
//! Global (control-plane) named secrets: every call scopes the
//! `WorkspaceSecretStore` with `None` (rows with a NULL `workspace_id`), gated
//! on `egret_core::authz::require(state.pdp, &p, "secret:<action>")` — the
//! PDP-backed policy enforcement point (platform_admin bypasses).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use egret_auth::principal::Auth;
use egret_secrets::WorkspaceSecretMeta;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use egret_core::http::ErrorBody;
use egret_core::Ack;
use std::collections::BTreeMap;

use crate::error::CpError;
use crate::CpState;

/// Request body for creating a global named secret. Either a typed `data` map
/// (with `type`, default `Opaque`) or the legacy single `value` (→ Opaque{value}).
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateSecretInput {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, rename = "type")]
    pub r#type: Option<String>,
    #[serde(default)]
    pub data: Option<BTreeMap<String, String>>,
    /// Back-compat: a single value stored as `Opaque{value}`.
    #[serde(default)]
    pub value: Option<String>,
}

/// Request body for updating a global named secret. Any subset may be present:
/// a `name` triggers a rename (re-describing with `description`); a `type`+`data`
/// (or legacy `value`) rotates the underlying vault row.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateSecretInput {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, rename = "type")]
    pub r#type: Option<String>,
    #[serde(default)]
    pub data: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub value: Option<String>,
}

/// Response DTO mirroring `WorkspaceSecretMeta` (never exposes the plaintext).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SecretDto {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub used_by: i64,
    #[serde(rename = "type")]
    pub r#type: String,
    pub keys: Vec<String>,
    /// Minted and managed by egret for an engine or a catalog: it cannot be
    /// renamed, rotated or deleted here, and pickers leave it out (KI-207).
    pub system_managed: bool,
}

impl From<WorkspaceSecretMeta> for SecretDto {
    fn from(m: WorkspaceSecretMeta) -> Self {
        SecretDto {
            id: m.id,
            name: m.name,
            description: m.description,
            created_at: m.created_at,
            used_by: m.used_by,
            r#type: m.r#type,
            keys: m.keys,
            system_managed: m.system_managed,
        }
    }
}

/// Resolve the (type, data) to write from the typed fields or the legacy `value`.
fn resolve_type_data(
    r#type: &Option<String>,
    data: &Option<BTreeMap<String, String>>,
    value: &Option<String>,
) -> Result<(String, BTreeMap<String, String>), CpError> {
    match (data, value) {
        (Some(d), None) => Ok((r#type.clone().unwrap_or_else(|| "Opaque".into()), d.clone())),
        (None, Some(v)) => {
            let mut m = BTreeMap::new();
            m.insert("value".to_string(), v.clone());
            Ok(("Opaque".to_string(), m))
        }
        (None, None) => Err(CpError::BadRequest("secret needs `data` or `value`".into())),
        (Some(_), Some(_)) => Err(CpError::BadRequest(
            "give `data` or `value`, not both".into(),
        )),
    }
}

/// Refuse a rename, rotation or delete of a secret egret manages (KI-207):
/// its engine or catalog renders it into running pods and rotates or deletes
/// it with them, so a hand edit here would break that engine or catalog.
async fn refuse_system_managed(st: &CpState, id: Uuid) -> Result<(), CpError> {
    let meta = st.secrets.get_meta(None, id).await?;
    if meta.system_managed {
        return Err(CpError::Conflict(format!(
            "{} is managed by egret: it is rotated and deleted with the engine or catalog \
             that created it, not here",
            meta.name
        )));
    }
    Ok(())
}

pub fn router() -> Router<CpState> {
    Router::new()
        .route("/secrets", get(list).post(create))
        .route("/secrets/{id}", axum::routing::put(update).delete(remove))
}

#[utoipa::path(
    get,
    path = "/api/control-plane/secrets",
    tag = "control-plane-secrets",
    responses(
        (status = 200, description = "Global named secrets (plaintext never exposed)", body = Vec<SecretDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing secret:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list(
    State(st): State<CpState>,
    Auth(principal): Auth,
) -> Result<Json<Vec<SecretDto>>, CpError> {
    egret_core::authz::require(&*st.pdp, &principal, "secret:read").await?;
    let metas = st.secrets.list(None).await?;
    Ok(Json(metas.into_iter().map(Into::into).collect()))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/secrets",
    tag = "control-plane-secrets",
    request_body = CreateSecretInput,
    responses(
        (status = 201, description = "Secret created", body = SecretDto),
        (status = 400, description = "Invalid input", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing secret:create permission", body = ErrorBody),
        (status = 409, description = "A secret with this name already exists", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn create(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Json(body): Json<CreateSecretInput>,
) -> Result<(StatusCode, Json<SecretDto>), CpError> {
    egret_core::authz::require(&*st.pdp, &principal, "secret:create").await?;
    let (r#type, data) = resolve_type_data(&body.r#type, &body.data, &body.value)?;
    let id = st
        .secrets
        .create_typed(
            None,
            &body.name,
            body.description.as_deref(),
            &r#type,
            &data,
        )
        .await?;
    let meta = st.secrets.get_meta(None, id).await?;
    Ok((StatusCode::CREATED, Json(meta.into())))
}

#[utoipa::path(
    put,
    path = "/api/control-plane/secrets/{id}",
    tag = "control-plane-secrets",
    params(("id" = Uuid, Path, description = "Secret id")),
    request_body = UpdateSecretInput,
    responses(
        (status = 200, description = "Secret updated (renamed and/or rotated)", body = SecretDto),
        (status = 400, description = "Invalid input", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing secret:update permission", body = ErrorBody),
        (status = 404, description = "No such secret", body = ErrorBody),
        (status = 409, description = "The new name clashes with an existing secret, or the secret is managed by egret", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn update(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateSecretInput>,
) -> Result<Json<SecretDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &principal, "secret:update").await?;
    refuse_system_managed(&st, id).await?;
    if let Some(name) = &body.name {
        st.secrets
            .rename(None, id, name, body.description.as_deref())
            .await?;
    }
    if body.data.is_some() || body.value.is_some() {
        let (r#type, data) = resolve_type_data(&body.r#type, &body.data, &body.value)?;
        st.secrets.update_typed(None, id, &r#type, &data).await?;
    }
    let meta = st.secrets.get_meta(None, id).await?;
    Ok(Json(meta.into()))
}

#[utoipa::path(
    delete,
    path = "/api/control-plane/secrets/{id}",
    tag = "control-plane-secrets",
    params(("id" = Uuid, Path, description = "Secret id")),
    responses(
        (status = 200, description = "Secret deleted", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing secret:delete permission", body = ErrorBody),
        (status = 404, description = "No such secret", body = ErrorBody),
        (status = 409, description = "Secret is still referenced by one or more connections, or is managed by egret", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn remove(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, CpError> {
    egret_core::authz::require(&*st.pdp, &principal, "secret:delete").await?;
    refuse_system_managed(&st, id).await?;
    st.secrets.delete(None, id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// OpenAPI fragment for the control-plane Secrets router. Merged into
/// `egret_controlplane::openapi()`.
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(list, create, update, remove),
    components(schemas(CreateSecretInput, UpdateSecretInput, SecretDto, Ack, ErrorBody)),
    tags((name = "control-plane-secrets", description = "Global named secrets (platform control plane)")),
)]
pub struct ControlPlaneSecretsApi;
