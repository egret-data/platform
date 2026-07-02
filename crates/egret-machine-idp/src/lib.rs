//! egret mini machine-IdP — an embedded, egret-owned OAuth2 provider that mints
//! machine tokens for Catalog Clients: `POST /oauth/token` (client_credentials)
//! → egret-signed JWT, plus `/.well-known/{openid-configuration,jwks.json}` so
//! the embedded lakekeeper can validate them. See the egret-embedded-catalog spec.

mod error;
mod keys;
mod token;

pub use error::IdpError;
pub use keys::{generate_rsa_key, KeyMaterial};
pub use token::{sign_claims, verify_with_jwk};

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use serde::Deserialize;
use std::sync::Arc;

/// Persistence + lifecycle for the IdP's signing keys.
#[async_trait::async_trait]
pub trait SigningKeyStore: Send + Sync {
    /// The active signing key (creating + persisting one if none exists).
    async fn active(&self) -> Result<KeyMaterial, IdpError>;
    /// All public JWKs a verifier may need (active + not-yet-retired).
    async fn all_public_jwks(&self) -> Result<Vec<serde_json::Value>, IdpError>;
}

/// A verified Catalog Client: its id + the warehouse/role its token conveys.
pub struct VerifiedClient {
    pub client_id: String,
    pub warehouse: String,
    pub catalog_role: String,
}

/// Verifies a client's `client_id`/`client_secret` against the client registry.
/// Implemented by `egret-controlplane`'s `CatalogClientStore` (EB-3).
#[async_trait::async_trait]
pub trait ClientVerifier: Send + Sync {
    async fn verify(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> Result<Option<VerifiedClient>, IdpError>;
}

/// A `ClientVerifier` that always rejects — the placeholder used before the real
/// `CatalogClientStore` is wired in (EB-3).
pub struct RejectAllVerifier;

#[async_trait::async_trait]
impl ClientVerifier for RejectAllVerifier {
    async fn verify(&self, _: &str, _: &str) -> Result<Option<VerifiedClient>, IdpError> {
        Ok(None)
    }
}

#[derive(Clone)]
pub struct IdpState {
    pub keys: Arc<dyn SigningKeyStore>,
    /// The egret public root URL used as the JWT `iss` + OIDC issuer.
    pub issuer: String,
    pub clients: Arc<dyn ClientVerifier>,
}

/// The mini-IdP router: discovery + JWKS + the client_credentials token endpoint.
/// Public (no CSRF / no session auth) — these are machine endpoints.
pub fn idp_router() -> Router<IdpState> {
    Router::new()
        .route("/.well-known/openid-configuration", get(discovery))
        .route("/.well-known/jwks.json", get(jwks))
        .route("/oauth/token", post(token_endpoint))
}

async fn discovery(State(st): State<IdpState>) -> Json<serde_json::Value> {
    let iss = st.issuer.trim_end_matches('/');
    Json(serde_json::json!({
        "issuer": iss,
        "jwks_uri": format!("{iss}/.well-known/jwks.json"),
        "token_endpoint": format!("{iss}/oauth/token"),
        "grant_types_supported": ["client_credentials"],
        "id_token_signing_alg_values_supported": ["RS256"],
    }))
}

async fn jwks(State(st): State<IdpState>) -> Result<Json<serde_json::Value>, IdpError> {
    let keys = st.keys.all_public_jwks().await?;
    Ok(Json(serde_json::json!({ "keys": keys })))
}

#[derive(Deserialize)]
struct TokenForm {
    grant_type: String,
    client_id: String,
    client_secret: String,
}

#[derive(Debug, serde::Serialize)]
struct TokenResp {
    access_token: String,
    token_type: String,
    expires_in: u64,
}

async fn token_endpoint(
    State(st): State<IdpState>,
    Form(f): Form<TokenForm>,
) -> Result<Json<TokenResp>, IdpError> {
    if f.grant_type != "client_credentials" {
        return Err(IdpError::UnsupportedGrant);
    }
    let vc = st
        .clients
        .verify(&f.client_id, &f.client_secret)
        .await?
        .ok_or(IdpError::InvalidClient)?;
    let km = st.keys.active().await?;
    let exp = chrono::Utc::now().timestamp() as u64 + 3600;
    let claims = serde_json::json!({
        "iss": st.issuer.trim_end_matches('/'),
        "sub": vc.client_id,
        "aud": "catalog",
        "warehouse": vc.warehouse,
        "roles": [vc.catalog_role],
        "exp": exp,
    });
    let jwt = sign_claims(&km, &claims)?;
    Ok(Json(TokenResp {
        access_token: jwt,
        token_type: "Bearer".into(),
        expires_in: 3600,
    }))
}

impl IntoResponse for IdpError {
    fn into_response(self) -> Response {
        use axum::http::StatusCode;
        let (status, err) = match self {
            IdpError::InvalidClient => (StatusCode::UNAUTHORIZED, "invalid_client"),
            IdpError::UnsupportedGrant => (StatusCode::BAD_REQUEST, "unsupported_grant_type"),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
        };
        (status, Json(serde_json::json!({ "error": err }))).into_response()
    }
}

/// Postgres-backed [`SigningKeyStore`] over the `oidc_signing_keys` table.
pub struct PgSigningKeyStore {
    db: egret_metastore::Db,
}

impl PgSigningKeyStore {
    pub fn new(db: egret_metastore::Db) -> Self {
        Self { db }
    }
}

#[async_trait::async_trait]
impl SigningKeyStore for PgSigningKeyStore {
    async fn active(&self) -> Result<KeyMaterial, IdpError> {
        // jsonb is read as ::text to avoid needing sqlx's `json` feature.
        let row: Option<(String, String, String)> = sqlx::query_as(
            "SELECT kid, private_pem, public_jwk::text FROM oidc_signing_keys \
             WHERE retired_at IS NULL ORDER BY created_at DESC LIMIT 1",
        )
        .fetch_optional(&self.db.pool)
        .await?;
        if let Some((kid, private_pem, jwk_text)) = row {
            let public_jwk =
                serde_json::from_str(&jwk_text).map_err(|e| IdpError::Crypto(e.to_string()))?;
            return Ok(KeyMaterial {
                kid,
                private_pem,
                public_jwk,
            });
        }
        // No active key — mint one and persist it.
        let kid = uuid::Uuid::new_v4().to_string();
        let km = generate_rsa_key(&kid)?;
        let jwk_text =
            serde_json::to_string(&km.public_jwk).map_err(|e| IdpError::Crypto(e.to_string()))?;
        sqlx::query(
            "INSERT INTO oidc_signing_keys (kid, algorithm, private_pem, public_jwk) \
             VALUES ($1, 'RS256', $2, $3::jsonb) ON CONFLICT (kid) DO NOTHING",
        )
        .bind(&km.kid)
        .bind(&km.private_pem)
        .bind(&jwk_text)
        .execute(&self.db.pool)
        .await?;
        Ok(km)
    }

    async fn all_public_jwks(&self) -> Result<Vec<serde_json::Value>, IdpError> {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT public_jwk::text FROM oidc_signing_keys \
             WHERE retired_at IS NULL ORDER BY created_at DESC",
        )
        .fetch_all(&self.db.pool)
        .await?;
        rows.iter()
            .map(|s| serde_json::from_str(s).map_err(|e| IdpError::Crypto(e.to_string())))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeKeys(KeyMaterial);
    #[async_trait::async_trait]
    impl SigningKeyStore for FakeKeys {
        async fn active(&self) -> Result<KeyMaterial, IdpError> {
            Ok(KeyMaterial {
                kid: self.0.kid.clone(),
                private_pem: self.0.private_pem.clone(),
                public_jwk: self.0.public_jwk.clone(),
            })
        }
        async fn all_public_jwks(&self) -> Result<Vec<serde_json::Value>, IdpError> {
            Ok(vec![self.0.public_jwk.clone()])
        }
    }

    struct FakeVerifier;
    #[async_trait::async_trait]
    impl ClientVerifier for FakeVerifier {
        async fn verify(
            &self,
            client_id: &str,
            client_secret: &str,
        ) -> Result<Option<VerifiedClient>, IdpError> {
            if client_id == "cid" && client_secret == "sec" {
                Ok(Some(VerifiedClient {
                    client_id: "cid".into(),
                    warehouse: "eng-wh".into(),
                    catalog_role: "engine".into(),
                }))
            } else {
                Ok(None)
            }
        }
    }

    fn state() -> IdpState {
        let km = generate_rsa_key("kid-t").unwrap();
        IdpState {
            keys: Arc::new(FakeKeys(km)),
            issuer: "http://localhost:8080".into(),
            clients: Arc::new(FakeVerifier),
        }
    }

    #[tokio::test]
    async fn token_endpoint_mints_jwt() {
        let st = state();
        let resp = token_endpoint(
            State(st.clone()),
            Form(TokenForm {
                grant_type: "client_credentials".into(),
                client_id: "cid".into(),
                client_secret: "sec".into(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(resp.0.token_type, "Bearer");
        let jwk = st.keys.all_public_jwks().await.unwrap()[0].clone();
        let claims = verify_with_jwk(&resp.0.access_token, &jwk).unwrap();
        assert_eq!(claims["sub"], "cid");
        assert_eq!(claims["aud"], "catalog");
        assert_eq!(claims["warehouse"], "eng-wh");
        assert_eq!(claims["roles"][0], "engine");
    }

    #[tokio::test]
    async fn token_endpoint_rejects_bad_secret() {
        let st = state();
        let err = token_endpoint(
            State(st),
            Form(TokenForm {
                grant_type: "client_credentials".into(),
                client_id: "cid".into(),
                client_secret: "WRONG".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, IdpError::InvalidClient));
    }
}
