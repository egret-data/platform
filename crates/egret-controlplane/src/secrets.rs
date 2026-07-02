//! Control-plane Secrets router (`/secrets`), nested under `/api/control-plane`.
//!
//! Global (control-plane) named secrets: every call scopes the
//! `WorkspaceSecretStore` with `None` (rows with a NULL `workspace_id`), gated
//! on `require_platform_admin`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use egret_auth::principal::Auth;
use egret_secrets::WorkspaceSecretMeta;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::require_platform_admin;
use crate::error::CpError;
use crate::CpState;

/// Request body for creating a global named secret.
#[derive(Debug, Deserialize)]
pub struct CreateSecretInput {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub value: String,
}

/// Request body for updating a global named secret. Any subset may be present:
/// a `name` triggers a rename (re-describing with `description`); a `value`
/// rotates the underlying vault row.
#[derive(Debug, Deserialize)]
pub struct UpdateSecretInput {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub value: Option<String>,
}

/// Response DTO mirroring `WorkspaceSecretMeta` (never exposes the plaintext).
#[derive(Debug, Serialize)]
pub struct SecretDto {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub used_by: i64,
}

impl From<WorkspaceSecretMeta> for SecretDto {
    fn from(m: WorkspaceSecretMeta) -> Self {
        SecretDto {
            id: m.id,
            name: m.name,
            description: m.description,
            created_at: m.created_at,
            used_by: m.used_by,
        }
    }
}

pub fn router() -> Router<CpState> {
    Router::new()
        .route("/secrets", get(list).post(create))
        .route("/secrets/{id}", axum::routing::put(update).delete(remove))
}

async fn list(
    State(st): State<CpState>,
    Auth(principal): Auth,
) -> Result<Json<Vec<SecretDto>>, CpError> {
    require_platform_admin(&principal)?;
    let metas = st.secrets.list(None).await?;
    Ok(Json(metas.into_iter().map(Into::into).collect()))
}

async fn create(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Json(body): Json<CreateSecretInput>,
) -> Result<(StatusCode, Json<SecretDto>), CpError> {
    require_platform_admin(&principal)?;
    let id = st
        .secrets
        .create(None, &body.name, body.description.as_deref(), &body.value)
        .await?;
    let meta = st.secrets.get_meta(None, id).await?;
    Ok((StatusCode::CREATED, Json(meta.into())))
}

async fn update(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateSecretInput>,
) -> Result<Json<SecretDto>, CpError> {
    require_platform_admin(&principal)?;
    if let Some(name) = &body.name {
        st.secrets
            .rename(None, id, name, body.description.as_deref())
            .await?;
    }
    if let Some(value) = &body.value {
        st.secrets.update_value(None, id, value).await?;
    }
    let meta = st.secrets.get_meta(None, id).await?;
    Ok(Json(meta.into()))
}

async fn remove(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, CpError> {
    require_platform_admin(&principal)?;
    st.secrets.delete(None, id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}
