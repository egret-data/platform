//! Control-plane Polaris connection: the singleton `polaris_connection` row
//! (Plan 4), a [`PolarisClient`] provider built from it, and the platform-admin
//! `/polaris/connection` routes.
//!
//! egret drives one external Apache Polaris deployment. Its admin Keycloak
//! client-credentials secret is stored as a GLOBAL named secret
//! (`polaris-admin-kc-secret`, workspace scope `None`); the connection row keeps
//! only a `kc_secret_ref` pointing at it, never the plaintext. Every handler is
//! gated on `require_platform_admin` and the connection is a singleton
//! (`id = true`).

use std::sync::Arc;

use axum::extract::State;
use axum::routing::{get, put};
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_secrets::{SecretsError, WorkspaceSecretStore};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::require_platform_admin;
use crate::error::CpError;
use crate::CpState;

/// Name of the GLOBAL named secret holding the Polaris admin Keycloak client secret.
const KC_SECRET_NAME: &str = "polaris-admin-kc-secret";

/// Fields required to (re)register the singleton Polaris connection.
pub struct PolarisConnInput {
    pub base_url: String,
    pub kc_token_url: String,
    pub kc_client_id: String,
    pub kc_secret_ref: Uuid,
    pub kc_scope: Option<String>,
}

/// The stored singleton Polaris connection (never carries the secret plaintext).
pub struct PolarisConn {
    pub base_url: String,
    pub kc_token_url: String,
    pub kc_client_id: String,
    pub kc_secret_ref: Uuid,
    pub kc_scope: Option<String>,
}

/// One `polaris_connection` row, as selected from SQL.
type ConnRow = (String, String, String, Uuid, Option<String>);

impl From<ConnRow> for PolarisConn {
    fn from((base_url, kc_token_url, kc_client_id, kc_secret_ref, kc_scope): ConnRow) -> Self {
        PolarisConn {
            base_url,
            kc_token_url,
            kc_client_id,
            kc_secret_ref,
            kc_scope,
        }
    }
}

/// Postgres-backed store for the singleton `polaris_connection` row.
#[derive(Clone)]
pub struct PolarisConnStore {
    db: egret_metastore::Db,
}

impl PolarisConnStore {
    pub fn new(db: egret_metastore::Db) -> Self {
        Self { db }
    }

    /// Insert or update the singleton connection row (`id = true`).
    pub async fn upsert(&self, i: &PolarisConnInput) -> Result<(), CpError> {
        sqlx::query(
            "INSERT INTO polaris_connection \
             (id, base_url, kc_token_url, kc_client_id, kc_secret_ref, kc_scope, updated_at) \
             VALUES (true, $1, $2, $3, $4, $5, now()) \
             ON CONFLICT (id) DO UPDATE SET \
             base_url = $1, kc_token_url = $2, kc_client_id = $3, \
             kc_secret_ref = $4, kc_scope = $5, updated_at = now()",
        )
        .bind(&i.base_url)
        .bind(&i.kc_token_url)
        .bind(&i.kc_client_id)
        .bind(i.kc_secret_ref)
        .bind(&i.kc_scope)
        .execute(&self.db.pool)
        .await
        .map_err(|_| CpError::Internal)?;
        Ok(())
    }

    /// Fetch the singleton connection (`None` if never configured).
    pub async fn get(&self) -> Result<Option<PolarisConn>, CpError> {
        let row: Option<ConnRow> = sqlx::query_as(
            "SELECT base_url, kc_token_url, kc_client_id, kc_secret_ref, kc_scope \
             FROM polaris_connection WHERE id = true",
        )
        .fetch_optional(&self.db.pool)
        .await
        .map_err(|_| CpError::Internal)?;
        Ok(row.map(Into::into))
    }
}

/// Build a [`PolarisClient`] from the registered connection, resolving the admin
/// Keycloak client secret plaintext from the GLOBAL secret store.
pub async fn polaris_client(
    store: &PolarisConnStore,
    secrets: &WorkspaceSecretStore,
) -> Result<egret_polaris::PolarisClient, CpError> {
    let c = store
        .get()
        .await?
        .ok_or_else(|| CpError::BadRequest("polaris connection not configured".into()))?;
    let client_secret = secrets
        .resolve_plaintext(None, c.kc_secret_ref)
        .await
        .map_err(CpError::from)?;
    Ok(egret_polaris::PolarisClient::new(
        egret_polaris::PolarisConfig {
            base_url: c.base_url,
            kc_token_url: c.kc_token_url,
            kc_client_id: c.kc_client_id,
            kc_client_secret: client_secret,
            kc_scope: c.kc_scope,
        },
        Arc::new(egret_polaris::ReqwestTransport {
            http: reqwest::Client::new(),
        }),
    ))
}

/// Request body for `PUT /polaris/connection`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolarisConnPut {
    pub base_url: String,
    pub kc_token_url: String,
    pub kc_client_id: String,
    pub kc_client_secret: String,
    #[serde(default)]
    pub kc_scope: Option<String>,
}

/// Response DTO for the stored connection (never carries the secret plaintext).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolarisConnDto {
    pub base_url: String,
    pub kc_token_url: String,
    pub kc_client_id: String,
    pub kc_secret_ref: Uuid,
    pub kc_scope: Option<String>,
}

impl From<PolarisConn> for PolarisConnDto {
    fn from(c: PolarisConn) -> Self {
        PolarisConnDto {
            base_url: c.base_url,
            kc_token_url: c.kc_token_url,
            kc_client_id: c.kc_client_id,
            kc_secret_ref: c.kc_secret_ref,
            kc_scope: c.kc_scope,
        }
    }
}

pub fn router() -> Router<CpState> {
    Router::new()
        .route(
            "/polaris/connection",
            put(put_connection).get(get_connection),
        )
        .route("/polaris/connection/health", get(health))
}

/// Store the admin Keycloak client secret as a GLOBAL named secret, reusing (and
/// rotating) the existing `polaris-admin-kc-secret` row on a name conflict.
async fn resolve_kc_secret_ref(st: &CpState, plaintext: &str) -> Result<Uuid, CpError> {
    match st
        .secrets
        .create(
            None,
            KC_SECRET_NAME,
            Some("Polaris admin Keycloak client secret"),
            plaintext,
        )
        .await
    {
        Ok(id) => Ok(id),
        Err(SecretsError::AlreadyExists(_)) => {
            // Reuse the existing global secret by name and rotate its value.
            let metas = st.secrets.list(None).await?;
            let meta = metas
                .into_iter()
                .find(|m| m.name == KC_SECRET_NAME)
                .ok_or(CpError::Internal)?;
            st.secrets.update_value(None, meta.id, plaintext).await?;
            Ok(meta.id)
        }
        Err(e) => Err(e.into()),
    }
}

async fn put_connection(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Json(body): Json<PolarisConnPut>,
) -> Result<Json<PolarisConnDto>, CpError> {
    require_platform_admin(&principal)?;
    let kc_secret_ref = resolve_kc_secret_ref(&st, &body.kc_client_secret).await?;
    let input = PolarisConnInput {
        base_url: body.base_url,
        kc_token_url: body.kc_token_url,
        kc_client_id: body.kc_client_id,
        kc_secret_ref,
        kc_scope: body.kc_scope,
    };
    st.polaris_conn.upsert(&input).await?;
    let conn = st.polaris_conn.get().await?.ok_or(CpError::Internal)?;
    Ok(Json(conn.into()))
}

async fn get_connection(
    State(st): State<CpState>,
    Auth(principal): Auth,
) -> Result<Json<Option<PolarisConnDto>>, CpError> {
    require_platform_admin(&principal)?;
    let conn = st.polaris_conn.get().await?;
    Ok(Json(conn.map(Into::into)))
}

async fn health(
    State(st): State<CpState>,
    Auth(principal): Auth,
) -> Result<Json<serde_json::Value>, CpError> {
    require_platform_admin(&principal)?;
    // A missing connection is still a 4xx (BadRequest); a reachable-or-not
    // Polaris returns 200 with `ok` true/false so the UI can render red without
    // treating an unhealthy catalog as a hard error.
    let client = polaris_client(&st.polaris_conn, &st.secrets).await?;
    match client.health().await {
        Ok(()) => Ok(Json(serde_json::json!({ "ok": true }))),
        Err(e) => Ok(Json(
            serde_json::json!({ "ok": false, "error": e.to_string() }),
        )),
    }
}
