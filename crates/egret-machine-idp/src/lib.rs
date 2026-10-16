//! egret mini machine-IdP — an embedded, egret-owned OAuth2 provider that mints
//! machine tokens for Catalog Clients: `POST /oauth/token` (client_credentials)
//! → egret-signed JWT, plus `/.well-known/{openid-configuration,jwks.json}` so
//! the embedded lakekeeper can validate them. See the egret-embedded-catalog spec.

mod error;
mod keys;
mod token;
mod verified_cache;

pub use error::IdpError;
pub use keys::{generate_rsa_key, KeyMaterial};
pub use token::{sign_claims, verify_issued, verify_with_jwk};
pub use verified_cache::VerifiedCache;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use egret_core::http::ErrorBody;
use egret_core::ratelimit::{client_ip, RateLimiter};
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

/// A verified machine client. The verifier that matched owns the token's shape:
/// which `aud` it carries, what audience-specific claims it adds, and how long
/// it lives. That is what lets one IdP serve the Catalog and the Data API
/// without either knowing about the other.
#[derive(Clone, Debug, PartialEq)]
pub struct VerifiedClient {
    pub client_id: String,
    /// The JWT `aud`.
    pub audience: String,
    /// Merged into the JWT alongside the standard claims. Must not contain
    /// `iss`, `sub`, `aud` or `exp` — those are the IdP's to set.
    pub extra_claims: serde_json::Map<String, serde_json::Value>,
    /// Token lifetime. Catalog: 3600. Data API: 600 — short, because a stateless
    /// token cannot be revoked before it expires.
    pub ttl_secs: u64,
}

/// A client's current minting state, read WITHOUT verifying a secret.
///
/// No `Debug`: `secret_hash` is not the secret, but it is the offline-crackable
/// form of it and has no business in a log line.
#[derive(Clone)]
pub struct ClientState {
    /// The token shape a successful `verify` would mint from now.
    pub verified: VerifiedClient,
    /// The stored secret hash (PHC string) as of this read. From `verify`, it
    /// is the hash the secret was checked against.
    pub secret_hash: String,
}

/// Verifies a client's `client_id`/`client_secret` against one client registry.
/// Implemented by the Catalog Client registry (`egret-catalog-engine`) and, per
/// audience, by any other machine-client registry.
#[async_trait::async_trait]
pub trait ClientVerifier: Send + Sync {
    /// The state the secret was checked against, from the SAME row read the
    /// Argon2 check used — `secret_hash` is the hash that verified. `None` for
    /// every refusal, wrong secret included.
    async fn verify(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> Result<Option<ClientState>, IdpError>;
    /// `None` when the client is unknown or may not mint for ANY reason the
    /// secret path would refuse (revoked, expired, disabled principal, archived
    /// workspace, …). Never runs Argon2.
    async fn current(&self, client_id: &str) -> Result<Option<ClientState>, IdpError>;
}

/// Digest of a stored secret hash, carried in every minted token as `cgen`.
/// First 16 hex chars of SHA-256 over the PHC string.
///
/// Rotating a secret changes the stored hash (a fresh salt every time), so a
/// token's `cgen` names the credential generation that minted it without
/// revealing anything an attacker could crack.
pub fn credential_generation(secret_hash: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(secret_hash.as_bytes());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// A `ClientVerifier` that always rejects — the placeholder used before the real
/// `CatalogClientStore` is wired in (EB-3).
pub struct RejectAllVerifier;

#[async_trait::async_trait]
impl ClientVerifier for RejectAllVerifier {
    async fn verify(&self, _: &str, _: &str) -> Result<Option<ClientState>, IdpError> {
        Ok(None)
    }
    async fn current(&self, _: &str) -> Result<Option<ClientState>, IdpError> {
        Ok(None)
    }
}

#[derive(Clone)]
pub struct IdpState {
    pub keys: Arc<dyn SigningKeyStore>,
    /// The egret public root URL used as the JWT `iss` + OIDC issuer.
    pub issuer: String,
    /// Tried in order until one recognises the `client_id`. Each owns its own
    /// audience; a client belongs to exactly one registry.
    pub verifiers: Vec<Arc<dyn ClientVerifier>>,
    /// Guards `POST /oauth/token`, which is reachable from the internet and runs
    /// one Argon2 verify per request with a known `client_id`. Keyed on
    /// `ip:client_id` — see [`token_endpoint`].
    pub rate_limiter: RateLimiter,
    /// Secrets already proven good for a client whose stored hash has not
    /// changed since. A hit skips the limiter and the Argon2 verify — see
    /// [`token_endpoint`].
    pub verified_cache: VerifiedCache,
}

/// The mini-IdP router: discovery + JWKS + the client_credentials token endpoint.
/// Public (no CSRF / no session auth) — these are machine endpoints.
pub fn idp_router() -> Router<IdpState> {
    Router::new()
        .route("/.well-known/openid-configuration", get(discovery))
        .route("/.well-known/jwks.json", get(jwks))
        .route("/oauth/token", post(token_endpoint))
}

/// OpenAPI fragment for the mini machine-IdP router (discovery + JWKS + the
/// client_credentials token endpoint). These paths are merged un-nested into the
/// root router, so the documented paths are absolute as-is. Merged into
/// `egret_machine_idp::openapi()` (E0·P4b-1).
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(discovery, jwks, token_endpoint),
    components(schemas(TokenForm, TokenResp, DiscoveryDoc, JwksDoc, ErrorBody)),
    tags((name = "idp", description = "egret mini machine-IdP: OIDC discovery, JWKS, and token minting (client_credentials + RFC 8693 token exchange)")),
)]
pub struct IdpApi;

/// The machine-IdP seam's OpenAPI document (E0·P4b-1). `egret-server` merges
/// this into the top-level API doc. This is the per-crate `openapi()` convention
/// every annotated seam exposes.
pub fn openapi() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi as _;
    IdpApi::openapi()
}

#[utoipa::path(
    get,
    path = "/.well-known/openid-configuration",
    tag = "idp",
    responses(
        (status = 200, description = "OIDC discovery metadata for the egret machine-IdP", body = DiscoveryDoc),
    ),
)]
async fn discovery(State(st): State<IdpState>) -> Json<serde_json::Value> {
    let iss = st.issuer.trim_end_matches('/');
    Json(serde_json::json!({
        "issuer": iss,
        "jwks_uri": format!("{iss}/.well-known/jwks.json"),
        "token_endpoint": format!("{iss}/oauth/token"),
        "grant_types_supported": ["client_credentials", TOKEN_EXCHANGE_GRANT],
        "id_token_signing_alg_values_supported": ["RS256"],
    }))
}

#[utoipa::path(
    get,
    path = "/.well-known/jwks.json",
    tag = "idp",
    responses(
        (status = 200, description = "The IdP's active public JWKs (for JWT verification)", body = JwksDoc),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn jwks(State(st): State<IdpState>) -> Result<Json<serde_json::Value>, IdpError> {
    let keys = st.keys.all_public_jwks().await?;
    Ok(Json(serde_json::json!({ "keys": keys })))
}

/// The RFC 8693 grant type: trade a token this IdP issued for a fresh one.
pub const TOKEN_EXCHANGE_GRANT: &str = "urn:ietf:params:oauth:grant-type:token-exchange";
/// The only `subject_token_type` the exchange accepts (RFC 8693 §3).
pub const ACCESS_TOKEN_TYPE: &str = "urn:ietf:params:oauth:token-type:access_token";

/// `POST /oauth/token` form. Which fields are required depends on
/// `grant_type`, so every field but `grant_type` is optional here and checked
/// by the handler — a missing field is a 400 `invalid_request`, never the
/// extractor's 422, which no OAuth client reads as a refusal (I4).
#[derive(Deserialize, utoipa::ToSchema)]
struct TokenForm {
    /// `client_credentials` or `urn:ietf:params:oauth:grant-type:token-exchange`.
    grant_type: String,
    /// Required for `client_credentials`.
    client_id: Option<String>,
    /// Required for `client_credentials`.
    client_secret: Option<String>,
    /// Required for token exchange: an unexpired access token this IdP issued.
    /// A Bearer `Authorization` header alone does not stand in for it.
    subject_token: Option<String>,
    /// Required for token exchange: `urn:ietf:params:oauth:token-type:access_token`.
    subject_token_type: Option<String>,
    /// Accepted and ignored.
    #[allow(dead_code)] // read by nothing: a scope request does not narrow a token here
    scope: Option<String>,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
struct TokenResp {
    access_token: String,
    token_type: String,
    expires_in: u64,
}

/// OIDC discovery document served at `/.well-known/openid-configuration` so the
/// embedded lakekeeper can locate this IdP's JWKS + token endpoint. Doc-only
/// mirror of the `serde_json::Value` the handler builds.
#[derive(serde::Serialize, utoipa::ToSchema)]
struct DiscoveryDoc {
    issuer: String,
    jwks_uri: String,
    token_endpoint: String,
    grant_types_supported: Vec<String>,
    id_token_signing_alg_values_supported: Vec<String>,
}

/// JWK set served at `/.well-known/jwks.json`. Doc-only mirror of the
/// `serde_json::Value` the handler builds; each key is a raw RFC 7517 JWK.
#[derive(serde::Serialize, utoipa::ToSchema)]
struct JwksDoc {
    #[schema(value_type = Vec<Object>)]
    keys: Vec<serde_json::Value>,
}

#[utoipa::path(
    post,
    path = "/oauth/token",
    tag = "idp",
    request_body(content = TokenForm, content_type = "application/x-www-form-urlencoded"),
    responses(
        (status = 200, description = "A signed access token (client_credentials, or an RFC 8693 exchange of an unexpired token this IdP issued for a fresh one with the same client and audience)", body = TokenResp),
        (status = 400, description = "OAuth `{ \"error\": ... }` body: `invalid_request` (a field the grant requires is missing or malformed; carries `error_description`), `invalid_grant` (the subject_token is invalid, expired, foreign, for another audience, or its client was deleted or its secret rotated), or `unsupported_grant_type`", body = ErrorBody),
        (status = 401, description = "Invalid client credentials (OAuth `{ \"error\": ... }` body)", body = ErrorBody),
        (status = 429, description = "Too many token requests from this ip+client_id (or ip+sub for an exchange) (OAuth `{ \"error\": ... }` body)", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn token_endpoint(
    State(st): State<IdpState>,
    headers: HeaderMap,
    Form(f): Form<TokenForm>,
) -> Result<Json<TokenResp>, IdpError> {
    match f.grant_type.as_str() {
        "client_credentials" => {
            let (Some(client_id), Some(client_secret)) = (&f.client_id, &f.client_secret) else {
                return Err(IdpError::InvalidRequest(
                    "client_credentials requires client_id and client_secret",
                ));
            };
            client_credentials(&st, &headers, client_id, client_secret).await
        }
        TOKEN_EXCHANGE_GRANT => token_exchange(&st, &headers, &f).await,
        _ => {
            // Limited as before: an unknown grant costs nothing to answer, but
            // the bucket it counts against is the one its `client_id` names.
            let ip = client_ip(&headers);
            let client_id = f.client_id.as_deref().unwrap_or_default();
            if !st.rate_limiter.check(&format!("{ip}:{client_id}")) {
                return Err(IdpError::RateLimited);
            }
            Err(IdpError::UnsupportedGrant)
        }
    }
}

async fn client_credentials(
    st: &IdpState,
    headers: &HeaderMap,
    client_id: &str,
    client_secret: &str,
) -> Result<Json<TokenResp>, IdpError> {
    // A proven repeat first. Trino mints a token per query session, and with
    // the limiter counting every request a busy engine was throttled at the
    // limit (I3). A secret that already verified for this client, while the
    // registry still stores the hash it verified against, is not the Argon2
    // cost the limiter exists to bound — so it skips both. The registry is
    // still read on every hit (`current`), so a revoke or a rotation refuses
    // the very next request rather than waiting out the cache's TTL.
    if let Some(state) = cached_state(st, client_id, client_secret).await {
        return mint(st, &state).await;
    }

    // Rate-limit ahead of the verifiers, because verifying a known
    // `client_id` costs an Argon2 hash and this endpoint is published at the
    // edge. Keyed on the PAIR `ip:client_id`, mirroring login's `ip:email`: a
    // `client_id` is not a secret, so keying on it alone would let anyone lock a
    // legitimate client out by spamming its id. Keyed on the pair, a flooder
    // only exhausts their own bucket.
    //
    // `client_id` is unauthenticated attacker-supplied input and the limiter's
    // map is never pruned, so the key MUST NOT be stored raw — see the note on
    // `RateLimiter`, which hashes it. How much the `ip:` half is worth depends
    // on the edge; `client_ip` documents which deployments can be trusted.
    let ip = client_ip(headers);
    if !st.rate_limiter.check(&format!("{ip}:{client_id}")) {
        return Err(IdpError::RateLimited);
    }
    // Try every verifier; do NOT short-circuit on the first `Err`. Verifiers own
    // independent audiences, so a transient fault in one (e.g. the catalog's DB)
    // must not 500 a request meant for a different, healthy one. Only once every
    // verifier has either declined or failed do we decide what to return: a
    // match wins outright, otherwise an error means "unknown" cannot honestly be
    // claimed (401 would be a lie), so that beats a clean "no match" (401).
    let mut verified = None;
    let mut verifier_errors = Vec::new();
    for v in &st.verifiers {
        match v.verify(client_id, client_secret).await {
            Ok(Some(state)) => {
                verified = Some(state);
                break;
            }
            Ok(None) => {}
            Err(e) => {
                tracing::error!(client_id = %client_id, error = %e, "client verifier failed");
                verifier_errors.push(e.to_string());
            }
        }
    }
    let state = match verified {
        Some(state) => state,
        None if !verifier_errors.is_empty() => {
            return Err(IdpError::VerificationUnavailable(
                verifier_errors.join("; "),
            ));
        }
        None => return Err(IdpError::InvalidClient),
    };
    // One read, used whole: `state` is the row the secret was checked
    // against, so the token's claims, audience, TTL and `cgen` and the hash
    // cached below are all the ones Argon2 vouched for. A second read here
    // could see a rotation that landed after the check and tie the OLD secret
    // to the NEW hash in the cache, serving it unlimited for a TTL.
    st.verified_cache
        .put(client_id, client_secret, state.secret_hash.clone());
    mint(st, &state).await
}

/// RFC 8693 token exchange: an unexpired token this IdP issued buys a fresh
/// one for the same client and audience. Iceberg's REST client refreshes its
/// catalog token this way; before this grant existed it got a 422 and retried
/// in a storm (I4).
///
/// An exchange is exactly as strong as `client_credentials` (decision D1): it
/// succeeds only while the client still exists, may still mint, and stores
/// the very secret hash the subject token was minted under (`cgen`). There is
/// no cap on the chain's age — a chain ends when the client is revoked or its
/// secret rotated, both of which `client_credentials` honours too.
///
/// The new token comes from ONE `current` read of the client, never from the
/// subject token's claims, so a change to the client's roles or TTL reaches
/// the next refresh. The subject token itself is never logged.
async fn token_exchange(
    st: &IdpState,
    headers: &HeaderMap,
    f: &TokenForm,
) -> Result<Json<TokenResp>, IdpError> {
    if f.subject_token_type.as_deref() != Some(ACCESS_TOKEN_TYPE) {
        return Err(IdpError::InvalidRequest(
            "subject_token_type must be urn:ietf:params:oauth:token-type:access_token",
        ));
    }
    // The form field is the grant. A Bearer header is what the client
    // authenticates its session with, not what it asks to exchange; when both
    // are sent they must name the same token.
    let Some(subject_token) = f.subject_token.as_deref() else {
        return Err(IdpError::InvalidRequest("subject_token is required"));
    };
    if let Some(auth) = headers.get(axum::http::header::AUTHORIZATION) {
        let bearer = auth
            .to_str()
            .ok()
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::trim);
        if bearer != Some(subject_token) {
            return Err(IdpError::InvalidRequest(
                "the Authorization header does not carry the subject_token",
            ));
        }
    }

    // Until a token verifies there is no client to key on, so every failure
    // shares the IP's `exchange` bucket — a junk flood is limited without
    // touching any real client's bucket. A verified token counts against
    // `ip:<sub>` instead.
    let ip = client_ip(headers);
    let jwks = st.keys.all_public_jwks().await?;
    let claims = match token::verify_issued(subject_token, &jwks, st.issuer.trim_end_matches('/')) {
        Ok(claims) => claims,
        Err(e) => {
            if !st.rate_limiter.check(&format!("{ip}:exchange")) {
                return Err(IdpError::RateLimited);
            }
            return Err(e);
        }
    };
    let (Some(sub), Some(aud), Some(cgen)) = (
        claims["sub"].as_str(),
        claims["aud"].as_str(),
        claims["cgen"].as_str(),
    ) else {
        return Err(IdpError::InvalidGrant);
    };
    if !st.rate_limiter.check(&format!("{ip}:{sub}")) {
        return Err(IdpError::RateLimited);
    }

    // The client must still be one that may mint, for the SAME audience: a
    // token never changes audience across an exchange. Same non-short-circuit
    // rule as `client_credentials`: one registry's outage must not refuse a
    // client another registry owns, and an outage is not "no such client".
    let mut found = None;
    let mut verifier_errors = Vec::new();
    for v in &st.verifiers {
        match v.current(sub).await {
            Ok(Some(state)) if state.verified.audience == aud => {
                found = Some(state);
                break;
            }
            Ok(_) => {}
            Err(e) => {
                tracing::error!(client_id = %sub, error = %e, "client state read failed on token exchange");
                verifier_errors.push(e.to_string());
            }
        }
    }
    let state = match found {
        Some(state) => state,
        None if !verifier_errors.is_empty() => {
            return Err(IdpError::VerificationUnavailable(
                verifier_errors.join("; "),
            ));
        }
        None => return Err(IdpError::InvalidGrant),
    };
    // The subject token was minted under the secret the client stores now —
    // a rotation since ends the chain, as it ends `client_credentials`.
    if cgen != credential_generation(&state.secret_hash) {
        return Err(IdpError::InvalidGrant);
    }
    mint(st, &state).await
}

/// The client's current state, if this secret is a proven repeat: it verified
/// before against the hash some registry still stores for this client. Any
/// other outcome — no entry, a changed hash, a client gone, a registry error —
/// is `None`, and the request takes the full path (limiter + verify).
async fn cached_state(st: &IdpState, client_id: &str, secret: &str) -> Option<ClientState> {
    let verified_hash = st.verified_cache.get(client_id, secret)?;
    for v in &st.verifiers {
        match v.current(client_id).await {
            Ok(Some(state)) if state.secret_hash == verified_hash => return Some(state),
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(client_id = %client_id, error = %e, "client state read failed on a cache hit; verifying in full");
            }
        }
    }
    None
}

/// Sign a token for `state` — every claim from this one read.
async fn mint(st: &IdpState, state: &ClientState) -> Result<Json<TokenResp>, IdpError> {
    let vc = &state.verified;
    let km = st.keys.active().await?;
    let exp = chrono::Utc::now().timestamp() as u64 + vc.ttl_secs;
    let mut claims = serde_json::Map::new();
    // Audience-specific claims first, so the standard ones below always win —
    // a verifier must never be able to forge `iss`, `sub`, `aud` or `exp`.
    claims.extend(vc.extra_claims.clone());
    claims.insert("iss".into(), st.issuer.trim_end_matches('/').into());
    claims.insert("sub".into(), vc.client_id.clone().into());
    claims.insert("aud".into(), vc.audience.clone().into());
    claims.insert("exp".into(), exp.into());
    claims.insert(
        "cgen".into(),
        credential_generation(&state.secret_hash).into(),
    );

    let jwt = sign_claims(&km, &serde_json::Value::Object(claims))?;
    Ok(Json(TokenResp {
        access_token: jwt,
        token_type: "Bearer".into(),
        expires_in: vc.ttl_secs,
    }))
}

impl IntoResponse for IdpError {
    fn into_response(self) -> Response {
        use axum::http::StatusCode;
        if let IdpError::InvalidRequest(why) = self {
            // RFC 6749 §5.2 `error_description`: a fixed string naming the
            // missing or malformed field, never request input.
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "invalid_request", "error_description": why })),
            )
                .into_response();
        }
        let (status, err) = match self {
            IdpError::InvalidClient => (StatusCode::UNAUTHORIZED, "invalid_client"),
            IdpError::UnsupportedGrant => (StatusCode::BAD_REQUEST, "unsupported_grant_type"),
            IdpError::InvalidGrant => (StatusCode::BAD_REQUEST, "invalid_grant"),
            // `slow_down` is the registered OAuth error code for "you are asking
            // too often" (RFC 8628 §3.5); RFC 6749 defines none of its own. The
            // body shape is the same `{ "error": ... }` every other rejection
            // here returns, so an existing client parses it unchanged.
            IdpError::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "slow_down"),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, "server_error"),
        };
        (status, Json(serde_json::json!({ "error": err }))).into_response()
    }
}

/// Postgres-backed [`SigningKeyStore`] over the `oidc_signing_keys` table.
#[derive(Clone)]
pub struct PgSigningKeyStore {
    db: egret_metastore::Db,
}

impl PgSigningKeyStore {
    pub fn new(db: egret_metastore::Db) -> Self {
        Self { db }
    }
}

/// Serialises first-key creation across egret replicas and concurrent tasks:
/// without it two callers that both see "no active key" each insert one.
const SIGNING_KEY_CREATE_LOCK: i64 = 0x4547_5245_5453_4b31; // "EGRETSK1"

/// The newest non-retired key, read on `conn`.
async fn newest_active(conn: &mut sqlx::PgConnection) -> Result<Option<KeyMaterial>, IdpError> {
    // jsonb is read as ::text to avoid needing sqlx's `json` feature.
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT kid, private_pem, public_jwk::text FROM catalog.oidc_signing_keys \
         WHERE retired_at IS NULL ORDER BY created_at DESC LIMIT 1",
    )
    .fetch_optional(conn)
    .await?;
    row.map(|(kid, private_pem, jwk_text)| {
        let public_jwk =
            serde_json::from_str(&jwk_text).map_err(|e| IdpError::Crypto(e.to_string()))?;
        Ok(KeyMaterial {
            kid,
            private_pem,
            public_jwk,
        })
    })
    .transpose()
}

#[async_trait::async_trait]
impl SigningKeyStore for PgSigningKeyStore {
    async fn active(&self) -> Result<KeyMaterial, IdpError> {
        let mut conn = self.db.pool.acquire().await?;
        if let Some(km) = newest_active(&mut conn).await? {
            return Ok(km);
        }
        // No active key — create one under an advisory lock, re-checking inside
        // it so the loser of a race adopts the winner's key.
        let mut tx = sqlx::Connection::begin(&mut *conn).await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(SIGNING_KEY_CREATE_LOCK)
            .execute(&mut *tx)
            .await?;
        if let Some(km) = newest_active(&mut tx).await? {
            return Ok(km);
        }
        let kid = uuid::Uuid::new_v4().to_string();
        let km = generate_rsa_key(&kid)?;
        let jwk_text =
            serde_json::to_string(&km.public_jwk).map_err(|e| IdpError::Crypto(e.to_string()))?;
        sqlx::query(
            "INSERT INTO catalog.oidc_signing_keys (kid, algorithm, private_pem, public_jwk) \
             VALUES ($1, 'RS256', $2, $3::jsonb) ON CONFLICT (kid) DO NOTHING",
        )
        .bind(&km.kid)
        .bind(&km.private_pem)
        .bind(&jwk_text)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(km)
    }

    async fn all_public_jwks(&self) -> Result<Vec<serde_json::Value>, IdpError> {
        // Never publish an empty set: an engine that reads it first would cache
        // it and reject every delegated query until its TTL. `active()` creates
        // and persists the key when none exists, so reuse that path.
        self.active().await?;
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT public_jwk::text FROM catalog.oidc_signing_keys \
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
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    /// A verifier that recognises exactly one client and hands back a token
    /// shape the test chooses. `new()` behaves like a Catalog Client
    /// (`cid`/`sec`); `with_extra_claims` behaves like a second-audience client
    /// (`api1`/`s`) whose `extra_claims` the test controls.
    struct FakeVerifier {
        client_id: String,
        client_secret: String,
        audience: String,
        extra_claims: serde_json::Map<String, serde_json::Value>,
        ttl_secs: u64,
    }

    impl FakeVerifier {
        fn new() -> Self {
            Self {
                client_id: "cid".into(),
                client_secret: "sec".into(),
                audience: "catalog".into(),
                extra_claims: serde_json::json!({ "warehouse": "eng-wh", "roles": ["engine"] })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ttl_secs: 3600,
            }
        }

        /// A second-audience verifier whose `extra_claims` the caller supplies —
        /// used to prove a verifier cannot smuggle standard claims into a token.
        fn with_extra_claims(extra: serde_json::Value) -> Self {
            Self {
                client_id: "api1".into(),
                client_secret: "s".into(),
                audience: "data-api".into(),
                extra_claims: extra.as_object().cloned().unwrap_or_default(),
                ttl_secs: 600,
            }
        }
    }

    /// A verifier that always errors — stands in for a transient infra fault
    /// (e.g. the catalog's or the Data API's DB being briefly unreachable).
    /// Used to prove the `token_endpoint` loop does not short-circuit on the
    /// first `Err`: independent audiences must not fail each other.
    struct AlwaysErrorsVerifier;

    #[async_trait::async_trait]
    impl ClientVerifier for AlwaysErrorsVerifier {
        async fn verify(&self, _: &str, _: &str) -> Result<Option<ClientState>, IdpError> {
            Err(IdpError::Db("simulated outage".into()))
        }
        async fn current(&self, _: &str) -> Result<Option<ClientState>, IdpError> {
            Err(IdpError::Db("simulated outage".into()))
        }
    }

    /// The stored-hash stand-in every `FakeVerifier` client reports.
    const FAKE_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$ZmFrZQ$ZmFrZS1oYXNo";
    /// The stored-hash stand-in `CatalogLikeVerifier` reports.
    const CATALOG_LIKE_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$Y2F0$Y2F0LWhhc2g";

    #[async_trait::async_trait]
    impl ClientVerifier for FakeVerifier {
        async fn verify(
            &self,
            client_id: &str,
            client_secret: &str,
        ) -> Result<Option<ClientState>, IdpError> {
            if client_secret == self.client_secret {
                self.current(client_id).await
            } else {
                Ok(None)
            }
        }
        async fn current(&self, client_id: &str) -> Result<Option<ClientState>, IdpError> {
            if client_id != self.client_id {
                return Ok(None);
            }
            Ok(Some(ClientState {
                verified: VerifiedClient {
                    client_id: client_id.to_string(),
                    audience: self.audience.clone(),
                    extra_claims: self.extra_claims.clone(),
                    ttl_secs: self.ttl_secs,
                },
                secret_hash: FAKE_HASH.into(),
            }))
        }
    }

    #[test]
    fn database_url_is_exported_rather_than_read_from_dotenv() {
        egret_test_db::assert_database_url_is_exported();
    }

    /// J-F1: a fresh deployment has no key row until the first mint, and an
    /// engine that read the JWKS first cached an empty set.
    #[sqlx::test(migrations = "../../migrations")]
    async fn jwks_on_a_fresh_store_is_never_empty(pool: sqlx::PgPool) {
        let store = PgSigningKeyStore::new(egret_metastore::Db { pool });
        let keys = store.all_public_jwks().await.unwrap();
        assert_eq!(keys.len(), 1, "a fresh store must publish one key");
        let active = store.active().await.unwrap();
        assert_eq!(keys[0]["kid"], active.kid.as_str(), "no second key");
        assert_eq!(store.all_public_jwks().await.unwrap().len(), 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn concurrent_first_calls_end_with_one_active_key(pool: sqlx::PgPool) {
        let store = PgSigningKeyStore::new(egret_metastore::Db { pool: pool.clone() });
        let (a, b, c, d) = tokio::join!(
            store.active(),
            store.all_public_jwks(),
            store.active(),
            store.all_public_jwks()
        );
        let (a, c) = (a.unwrap(), c.unwrap());
        assert_eq!(a.kid, c.kid);
        assert_eq!(b.unwrap().len(), 1);
        assert_eq!(d.unwrap().len(), 1);
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM catalog.oidc_signing_keys WHERE retired_at IS NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(n, 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn jwks_endpoint_on_a_fresh_store_answers_one_key(pool: sqlx::PgPool) {
        let mut st = state();
        st.keys = Arc::new(PgSigningKeyStore::new(egret_metastore::Db { pool }));
        let Json(body) = jwks(State(st)).await.unwrap();
        assert_eq!(body["keys"].as_array().unwrap().len(), 1);
    }

    fn state() -> IdpState {
        let km = generate_rsa_key("kid-t").unwrap();
        IdpState {
            keys: Arc::new(FakeKeys(km)),
            issuer: "http://localhost:8080".into(),
            verifiers: vec![Arc::new(FakeVerifier::new())],
            rate_limiter: permissive_limiter(),
            verified_cache: VerifiedCache::default(),
        }
    }

    /// A limiter that never fires, for the tests that are about token SHAPE
    /// rather than about rate limiting. The window is long enough that a whole
    /// test file's worth of requests cannot age out of it.
    fn permissive_limiter() -> RateLimiter {
        RateLimiter::new(1_000, 3_600, 3_600)
    }

    /// Stands in for `egret-catalog-engine`'s `ClientVerifier` impl. That crate
    /// depends on this one, so its impl cannot be used here — this mirrors it
    /// and must be kept in lockstep with
    /// `crates/egret-catalog-engine/src/store/embedded.rs`.
    struct CatalogLikeVerifier;

    #[async_trait::async_trait]
    impl ClientVerifier for CatalogLikeVerifier {
        async fn verify(
            &self,
            client_id: &str,
            client_secret: &str,
        ) -> Result<Option<ClientState>, IdpError> {
            if client_secret == "s" {
                self.current(client_id).await
            } else {
                Ok(None)
            }
        }
        async fn current(&self, client_id: &str) -> Result<Option<ClientState>, IdpError> {
            if client_id != "cat1" {
                return Ok(None);
            }
            Ok(Some(ClientState {
                verified: VerifiedClient {
                    client_id: client_id.to_string(),
                    audience: "catalog".into(),
                    extra_claims: serde_json::json!({ "warehouse": "wh1", "roles": ["role1"] })
                        .as_object()
                        .cloned()
                        .unwrap(),
                    ttl_secs: 3600,
                },
                secret_hash: CATALOG_LIKE_HASH.into(),
            }))
        }
    }

    /// An `IdpState` over an explicit verifier list.
    async fn test_idp_state_with(verifiers: Vec<Arc<dyn ClientVerifier>>) -> IdpState {
        let km = generate_rsa_key("kid-t").unwrap();
        IdpState {
            keys: Arc::new(FakeKeys(km)),
            issuer: "http://localhost:8080".into(),
            verifiers,
            rate_limiter: permissive_limiter(),
            verified_cache: VerifiedCache::default(),
        }
    }

    /// An `IdpState` serving only the catalog's client registry — the exact
    /// deployment shape before the Data API existed.
    async fn test_idp_state_with_catalog_verifier() -> IdpState {
        test_idp_state_with(vec![Arc::new(CatalogLikeVerifier)]).await
    }

    /// Catalog + a second audience, registered side by side.
    async fn test_idp_state_with_two_verifiers() -> IdpState {
        test_idp_state_with(vec![
            Arc::new(CatalogLikeVerifier),
            Arc::new(FakeVerifier::with_extra_claims(serde_json::json!({
                "ws": "11111111-1111-1111-1111-111111111111",
            }))),
        ])
        .await
    }

    /// Parse an `application/x-www-form-urlencoded` token request body the way
    /// axum's `Form` extractor would (fixtures use no percent-escapes).
    fn parse_token_form(body: &str) -> TokenForm {
        let mut f = TokenForm {
            grant_type: String::new(),
            client_id: None,
            client_secret: None,
            subject_token: None,
            subject_token_type: None,
            scope: None,
        };
        for pair in body.split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let v = v.to_string();
            match k {
                "grant_type" => f.grant_type = v,
                "client_id" => f.client_id = Some(v),
                "client_secret" => f.client_secret = Some(v),
                "subject_token" => f.subject_token = Some(v),
                "subject_token_type" => f.subject_token_type = Some(v),
                "scope" => f.scope = Some(v),
                other => panic!("unexpected form field {other}"),
            }
        }
        f
    }

    /// A `client_credentials` form.
    fn cc_form(client_id: &str, client_secret: &str) -> TokenForm {
        parse_token_form(&format!(
            "grant_type=client_credentials&client_id={client_id}&client_secret={client_secret}"
        ))
    }

    /// Drive `POST /oauth/token` with a form body; panics if the IdP rejects it.
    async fn post_token(st: &IdpState, body: &str) -> TokenResp {
        token_endpoint(
            State(st.clone()),
            HeaderMap::new(),
            Form(parse_token_form(body)),
        )
        .await
        .expect("token endpoint rejected the request")
        .0
    }

    struct TokenErr {
        status: u16,
        body: serde_json::Value,
    }

    /// Drive `POST /oauth/token` expecting a rejection, rendered through
    /// `IntoResponse` so the test sees the real HTTP status + OAuth error body.
    async fn post_token_expecting_error(st: &IdpState, body: &str) -> TokenErr {
        let err = token_endpoint(
            State(st.clone()),
            HeaderMap::new(),
            Form(parse_token_form(body)),
        )
        .await
        .expect_err("token endpoint unexpectedly succeeded");
        let resp = err.into_response();
        let status = resp.status().as_u16();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        TokenErr {
            status,
            body: serde_json::from_slice(&bytes).unwrap(),
        }
    }

    /// Read a JWT's claims without checking its signature — these tests assert
    /// on the payload the IdP built, which is independent of signing.
    fn decode_claims_without_verifying(jwt: &str) -> serde_json::Value {
        use base64::Engine as _;
        let payload = jwt.split('.').nth(1).expect("jwt has no payload segment");
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .expect("jwt payload is not base64url");
        serde_json::from_slice(&raw).expect("jwt payload is not json")
    }

    /// The catalog's token shape is a published contract with every Iceberg
    /// client already deployed. This test exists to fail loudly if the
    /// multi-audience refactor changes it.
    #[tokio::test]
    async fn catalog_token_claims_are_unchanged() {
        let st = test_idp_state_with_catalog_verifier().await;
        let resp = post_token(
            &st,
            "grant_type=client_credentials&client_id=cat1&client_secret=s",
        )
        .await;

        let claims = decode_claims_without_verifying(&resp.access_token);
        assert_eq!(claims["aud"], "catalog");
        assert_eq!(claims["sub"], "cat1");
        assert_eq!(claims["warehouse"], "wh1");
        assert_eq!(claims["roles"], serde_json::json!(["role1"]));
        assert_eq!(resp.expires_in, 3600, "catalog TTL must stay 3600");
    }

    #[tokio::test]
    async fn a_second_verifier_serves_its_own_audience_and_ttl() {
        // Two verifiers registered; each client resolves through exactly one.
        let st = test_idp_state_with_two_verifiers().await;

        let cat = post_token(
            &st,
            "grant_type=client_credentials&client_id=cat1&client_secret=s",
        )
        .await;
        assert_eq!(
            decode_claims_without_verifying(&cat.access_token)["aud"],
            "catalog"
        );
        assert_eq!(cat.expires_in, 3600);

        let api = post_token(
            &st,
            "grant_type=client_credentials&client_id=api1&client_secret=s",
        )
        .await;
        let c = decode_claims_without_verifying(&api.access_token);
        assert_eq!(c["aud"], "data-api");
        assert_eq!(c["ws"], "11111111-1111-1111-1111-111111111111");
        assert_eq!(api.expires_in, 600, "Data API TTL is short by design");
        assert!(
            c.get("warehouse").is_none(),
            "catalog claims must not leak in"
        );
    }

    #[tokio::test]
    async fn an_unknown_client_is_rejected_by_every_verifier() {
        let st = test_idp_state_with_two_verifiers().await;
        let err = post_token_expecting_error(
            &st,
            "grant_type=client_credentials&client_id=nope&client_secret=s",
        )
        .await;
        assert_eq!(err.status, 401);
        assert_eq!(err.body["error"], "invalid_client");
    }

    /// The verifier loop must not short-circuit on the first `Err`: with a
    /// failing first verifier and a matching second one, the request must
    /// still succeed. Before this test existed, `v.verify(...).await?` aborted
    /// the whole request on the FIRST verifier that errored — harmless with one
    /// verifier, but once a second (independent) audience is registered, a
    /// transient fault in one registry would wrongly 500 every request meant
    /// for the other.
    #[tokio::test]
    async fn a_failing_verifier_does_not_block_a_later_match() {
        let st = test_idp_state_with(vec![
            Arc::new(AlwaysErrorsVerifier),
            Arc::new(FakeVerifier::with_extra_claims(serde_json::json!({
                "ws": "22222222-2222-2222-2222-222222222222",
            }))),
        ])
        .await;

        let resp = post_token(
            &st,
            "grant_type=client_credentials&client_id=api1&client_secret=s",
        )
        .await;
        let c = decode_claims_without_verifying(&resp.access_token);
        assert_eq!(c["aud"], "data-api");
        assert_eq!(c["ws"], "22222222-2222-2222-2222-222222222222");
    }

    /// When every verifier errors (none positively found the client valid OR
    /// invalid), the endpoint must not claim `401 invalid_client` — that would
    /// assert something it never actually checked. It must surface as a server
    /// error instead.
    #[tokio::test]
    async fn when_every_verifier_errors_the_response_is_a_server_error_not_401() {
        let st = test_idp_state_with(vec![Arc::new(AlwaysErrorsVerifier)]).await;
        let err = post_token_expecting_error(
            &st,
            "grant_type=client_credentials&client_id=whoever&client_secret=x",
        )
        .await;
        assert_eq!(err.status, 500);
        assert_eq!(err.body["error"], "server_error");
    }

    /// A verifier supplies `extra_claims`, so it must not be able to overwrite
    /// the claims that establish WHO the token is for. This is why the standard
    /// claims are inserted after the verifier's, and it is a security property,
    /// not a style choice — so it gets a test rather than a comment.
    #[tokio::test]
    async fn a_verifier_cannot_forge_the_standard_claims() {
        let hostile = FakeVerifier::with_extra_claims(serde_json::json!({
            "sub": "attacker",
            "aud": "catalog",
            "iss": "https://evil.example",
            "exp": 9_999_999_999u64,
        }));
        let st = test_idp_state_with(vec![Arc::new(hostile)]).await;

        let resp = post_token(
            &st,
            "grant_type=client_credentials&client_id=api1&client_secret=s",
        )
        .await;
        let c = decode_claims_without_verifying(&resp.access_token);

        assert_eq!(
            c["sub"], "api1",
            "sub must come from the verified client id"
        );
        assert_eq!(
            c["aud"], "data-api",
            "aud must come from the verifier's audience field"
        );
        assert_eq!(c["iss"], st.issuer.trim_end_matches('/'));
        assert_ne!(
            c["exp"], 9_999_999_999u64,
            "exp must be derived from ttl_secs"
        );
    }

    #[tokio::test]
    async fn token_endpoint_mints_jwt() {
        let st = state();
        let resp = token_endpoint(
            State(st.clone()),
            HeaderMap::new(),
            Form(cc_form("cid", "sec")),
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

    /// A `ClientVerifier` that counts how many times it was consulted. The real
    /// registries run one Argon2 verify per call with a known `client_id`
    /// (`egret_dataapi::ClientStore::verify` / `CatalogStore::verify`), which is
    /// the cost the limiter exists to bound — this counter stands in for it, so
    /// a test can assert the limiter answered BEFORE that cost was paid rather
    /// than merely that it answered.
    ///
    /// `stored` is the client's stored secret hash as the registry sees it now;
    /// `None` means the client is gone (revoked, disabled …). A test flips it to
    /// simulate a rotation or a revoke between requests.
    struct CountingVerifier {
        calls: Arc<AtomicUsize>,
        stored: Arc<std::sync::Mutex<Option<String>>>,
        /// When set, `current` errors (a registry outage) while `verify` keeps
        /// working off its own read.
        current_fails: Arc<std::sync::atomic::AtomicBool>,
    }

    #[async_trait::async_trait]
    impl ClientVerifier for CountingVerifier {
        async fn verify(
            &self,
            client_id: &str,
            client_secret: &str,
        ) -> Result<Option<ClientState>, IdpError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if client_secret == "sec" {
                Ok(self.read(client_id))
            } else {
                Ok(None)
            }
        }
        /// Not counted: `calls` stands in for the Argon2 cost, which `current`
        /// never pays.
        async fn current(&self, client_id: &str) -> Result<Option<ClientState>, IdpError> {
            if self.current_fails.load(Ordering::SeqCst) {
                return Err(IdpError::Db("simulated outage".into()));
            }
            Ok(self.read(client_id))
        }
    }

    impl CountingVerifier {
        fn read(&self, client_id: &str) -> Option<ClientState> {
            let stored = self.stored.lock().unwrap().clone();
            stored
                .filter(|_| client_id == "cid")
                .map(|hash| ClientState {
                    verified: VerifiedClient {
                        client_id: client_id.to_string(),
                        audience: "data-api".into(),
                        extra_claims: serde_json::Map::new(),
                        ttl_secs: 600,
                    },
                    secret_hash: hash,
                })
        }
    }

    /// The knobs a `CountingVerifier` test turns: how many verifies ran, and
    /// what the registry currently stores for `cid`.
    struct Counting {
        calls: Arc<AtomicUsize>,
        stored: Arc<std::sync::Mutex<Option<String>>>,
        current_fails: Arc<std::sync::atomic::AtomicBool>,
    }

    impl Counting {
        fn verifies(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
        fn set_stored(&self, hash: Option<&str>) {
            *self.stored.lock().unwrap() = hash.map(str::to_string);
        }
        fn fail_current(&self) {
            self.current_fails.store(true, Ordering::SeqCst);
        }
    }

    /// An `IdpState` whose token endpoint allows `max` requests per key, over a
    /// window long enough that no test can outrun it — so these tests never
    /// sleep and never race the clock.
    fn counting_state(max: u32) -> (IdpState, Counting) {
        let calls = Arc::new(AtomicUsize::new(0));
        let stored = Arc::new(std::sync::Mutex::new(Some(FAKE_HASH.to_string())));
        let current_fails = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let km = generate_rsa_key("kid-t").unwrap();
        let st = IdpState {
            keys: Arc::new(FakeKeys(km)),
            issuer: "http://localhost:8080".into(),
            verifiers: vec![Arc::new(CountingVerifier {
                calls: calls.clone(),
                stored: stored.clone(),
                current_fails: current_fails.clone(),
            })],
            rate_limiter: RateLimiter::new(max, 3_600, 3_600),
            verified_cache: VerifiedCache::default(),
        };
        (
            st,
            Counting {
                calls,
                stored,
                current_fails,
            },
        )
    }

    fn rate_limited_state(max: u32) -> (IdpState, Arc<AtomicUsize>) {
        let (st, counting) = counting_state(max);
        (st, counting.calls)
    }

    struct RawResp {
        status: u16,
        body: serde_json::Value,
    }

    /// Drive `POST /oauth/token` as if it arrived through the ingress from
    /// `ip`, rendering whichever way it goes through `IntoResponse` so the test
    /// reads the real HTTP status and body.
    async fn post_token_from(st: &IdpState, ip: &str, client_id: &str, secret: &str) -> RawResp {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", ip.parse().unwrap());
        let form = cc_form(client_id, secret);
        let resp = match token_endpoint(State(st.clone()), headers, Form(form)).await {
            Ok(ok) => ok.into_response(),
            Err(e) => e.into_response(),
        };
        let status = resp.status().as_u16();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        RawResp {
            status,
            body: serde_json::from_slice(&bytes).unwrap(),
        }
    }

    /// Under the limit nothing changes: a bad secret still reaches the verifier
    /// and comes back as the normal OAuth rejection. Guards against a limiter
    /// that "works" by rejecting everything.
    #[tokio::test]
    async fn under_the_limit_a_bad_secret_still_reaches_the_verifier() {
        let (st, calls) = rate_limited_state(3);

        let r = post_token_from(&st, "203.0.113.7", "cid", "WRONG").await;

        assert_eq!(r.status, 401);
        assert_eq!(r.body["error"], "invalid_client");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "a request under the limit must still be verified"
        );
    }

    /// Over the limit the endpoint answers 429 **without consulting the
    /// verifier** — the whole point, since the verify is the Argon2 hash this
    /// bounds. The counter is what makes that observable; the status alone
    /// would not distinguish it from a limiter placed after the verify.
    #[tokio::test]
    async fn over_the_limit_the_request_is_rejected_before_the_verifier_runs() {
        let (st, calls) = rate_limited_state(2);

        for _ in 0..2 {
            assert_eq!(
                post_token_from(&st, "203.0.113.7", "cid", "WRONG")
                    .await
                    .status,
                401
            );
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        let r = post_token_from(&st, "203.0.113.7", "cid", "WRONG").await;

        assert_eq!(r.status, 429);
        assert_eq!(
            r.body["error"], "slow_down",
            "the 429 body must be the same OAuth `{{ \"error\": ... }}` shape as every other rejection"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "the rejected request must not have reached the verifier"
        );
    }

    /// **The test that pins the KEY.** The two above pass for any limiter that
    /// limits *something* — one keyed on the whole request, or on `client_id`
    /// alone, satisfies both. This one fails for either.
    ///
    /// A `client_id` is not a secret. Keyed on `client_id` alone, anyone could
    /// lock a legitimate machine client out of minting tokens by spamming its
    /// id from anywhere; keyed on `ip` alone, one noisy client would take out
    /// every other client sharing an ingress or a NAT. Keyed on the pair — as
    /// local login keys on `ip:email` — a flooder only exhausts their own
    /// bucket.
    #[tokio::test]
    async fn the_limit_is_keyed_on_ip_and_client_id_together() {
        let (st, _calls) = rate_limited_state(1);

        // Exhaust exactly one ip+client_id pair.
        assert_eq!(
            post_token_from(&st, "203.0.113.7", "cid", "WRONG")
                .await
                .status,
            401
        );
        assert_eq!(
            post_token_from(&st, "203.0.113.7", "cid", "WRONG")
                .await
                .status,
            429
        );

        // Same client_id, different IP → unaffected. Fails if keyed on client_id.
        assert_eq!(
            post_token_from(&st, "198.51.100.9", "cid", "WRONG")
                .await
                .status,
            401,
            "a different IP must not inherit another caller\'s lockout for the same client_id"
        );

        // Same IP, different client_id → unaffected. Fails if keyed on IP.
        assert_eq!(
            post_token_from(&st, "203.0.113.7", "other", "WRONG")
                .await
                .status,
            401,
            "a different client_id from the same IP must have its own budget"
        );
    }

    #[test]
    fn credential_generation_is_16_lowercase_hex_stable_and_input_sensitive() {
        let g = credential_generation("abc");
        assert_eq!(g.len(), 16);
        assert!(
            g.chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "not lowercase hex: {g}"
        );
        assert_eq!(g, credential_generation("abc"), "must be stable");
        assert_ne!(g, credential_generation("abd"));
        // First 16 hex chars of SHA-256("abc") — pins the algorithm, not just
        // the shape, so a later change of digest cannot pass silently.
        assert_eq!(g, "ba7816bf8f01cfea");
    }

    /// Every token carries `cgen`, the digest of the hash the client's state
    /// had when it was minted — read through `current` on the verifier that
    /// matched, not through any other.
    #[tokio::test]
    async fn a_minted_token_carries_the_matching_verifiers_cgen() {
        let st = test_idp_state_with(vec![
            Arc::new(CatalogLikeVerifier),
            Arc::new(FakeVerifier::with_extra_claims(serde_json::json!({}))),
        ])
        .await;

        let cat = post_token(
            &st,
            "grant_type=client_credentials&client_id=cat1&client_secret=s",
        )
        .await;
        assert_eq!(
            decode_claims_without_verifying(&cat.access_token)["cgen"],
            credential_generation(CATALOG_LIKE_HASH)
        );

        let api = post_token(
            &st,
            "grant_type=client_credentials&client_id=api1&client_secret=s",
        )
        .await;
        assert_eq!(
            decode_claims_without_verifying(&api.access_token)["cgen"],
            credential_generation(FAKE_HASH)
        );
    }

    #[tokio::test]
    async fn reject_all_has_no_current_state() {
        assert!(RejectAllVerifier
            .current("anything")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn token_endpoint_rejects_bad_secret() {
        let st = state();
        let err = token_endpoint(State(st), HeaderMap::new(), Form(cc_form("cid", "WRONG")))
            .await
            .unwrap_err();
        assert!(matches!(err, IdpError::InvalidClient));
    }

    // ---- I3: a proven repeat skips Argon2 and the limiter -------------------

    /// Trino mints a token per query session. With the limiter counting every
    /// request, a busy engine hit 429 at 60/min. A repeat of a secret that
    /// already verified, against a client whose stored hash is unchanged, is
    /// not the Argon2 cost the limiter bounds — it must pass both.
    #[tokio::test]
    async fn a_repeat_of_a_good_secret_skips_verify_and_the_limiter() {
        let (st, counting) = counting_state(2);

        for i in 0..10 {
            let r = post_token_from(&st, "203.0.113.7", "cid", "sec").await;
            assert_eq!(r.status, 200, "request {i} was refused: {}", r.body);
            assert_eq!(
                decode_claims_without_verifying(r.body["access_token"].as_str().unwrap())["cgen"],
                credential_generation(FAKE_HASH),
                "a cache-hit token must carry the current state's cgen"
            );
        }
        assert_eq!(counting.verifies(), 1, "only the first request may verify");
    }

    /// The cache only ever answers for a secret that verified. Anything else
    /// takes the old path, limiter included.
    #[tokio::test]
    async fn a_wrong_secret_is_still_limited() {
        let (st, counting) = counting_state(2);
        // A good secret in the cache must not open the door for a wrong one.
        assert_eq!(
            post_token_from(&st, "198.51.100.9", "cid", "sec")
                .await
                .status,
            200
        );

        let statuses = [
            post_token_from(&st, "203.0.113.7", "cid", "WRONG")
                .await
                .status,
            post_token_from(&st, "203.0.113.7", "cid", "WRONG")
                .await
                .status,
            post_token_from(&st, "203.0.113.7", "cid", "WRONG")
                .await
                .status,
        ];

        assert_eq!(statuses, [401, 401, 429]);
        assert_eq!(
            counting.verifies(),
            3,
            "1 good + 2 wrong; the 429 never verified"
        );
    }

    /// A rotation writes a new stored hash. The cached entry names the old one,
    /// so it must stop answering and the request must verify again.
    #[tokio::test]
    async fn a_rotated_secret_misses_the_cache() {
        let (st, counting) = counting_state(100);
        assert_eq!(
            post_token_from(&st, "203.0.113.7", "cid", "sec")
                .await
                .status,
            200
        );
        assert_eq!(counting.verifies(), 1);

        counting.set_stored(Some(CATALOG_LIKE_HASH));
        let r = post_token_from(&st, "203.0.113.7", "cid", "sec").await;

        assert_eq!(
            counting.verifies(),
            2,
            "a rotated hash must not be served from cache"
        );
        assert_eq!(r.status, 200);
        assert_eq!(
            decode_claims_without_verifying(r.body["access_token"].as_str().unwrap())["cgen"],
            credential_generation(CATALOG_LIKE_HASH)
        );
    }

    /// A revoked client is refused on its next request, cached secret or not —
    /// the cache never outlives the registry's say-so.
    #[tokio::test]
    async fn a_client_gone_is_refused_at_once() {
        let (st, counting) = counting_state(100);
        assert_eq!(
            post_token_from(&st, "203.0.113.7", "cid", "sec")
                .await
                .status,
            200
        );

        counting.set_stored(None);
        let r = post_token_from(&st, "203.0.113.7", "cid", "sec").await;

        assert_eq!(r.status, 401);
        assert_eq!(r.body["error"], "invalid_client");
        assert_eq!(
            counting.verifies(),
            2,
            "the miss falls through to the normal path, which verifies once"
        );
    }

    /// `verify` and `current` disagree (a role and the hash changed right
    /// after the Argon2 check). A freshly verified token must be built from
    /// ONE read — the state `verify` checked the secret against, which also
    /// supplies `cgen` — never a mix with a later read.
    struct ShapeChangesAfterVerify;

    fn shape(aud: &str, role: &str, ttl_secs: u64, hash: &str) -> ClientState {
        ClientState {
            verified: VerifiedClient {
                client_id: "x".into(),
                audience: aud.into(),
                extra_claims: serde_json::json!({ "roles": [role] })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ttl_secs,
            },
            secret_hash: hash.into(),
        }
    }

    #[async_trait::async_trait]
    impl ClientVerifier for ShapeChangesAfterVerify {
        async fn verify(&self, _: &str, _: &str) -> Result<Option<ClientState>, IdpError> {
            Ok(Some(shape("checked-aud", "checked", 600, FAKE_HASH)))
        }
        async fn current(&self, _: &str) -> Result<Option<ClientState>, IdpError> {
            Ok(Some(shape("later-aud", "later", 3600, CATALOG_LIKE_HASH)))
        }
    }

    #[tokio::test]
    async fn a_token_is_minted_from_one_state_read() {
        let st = test_idp_state_with(vec![Arc::new(ShapeChangesAfterVerify)]).await;
        let resp = post_token(
            &st,
            "grant_type=client_credentials&client_id=x&client_secret=y",
        )
        .await;
        let c = decode_claims_without_verifying(&resp.access_token);
        assert_eq!(c["aud"], "checked-aud");
        assert_eq!(c["roles"], serde_json::json!(["checked"]));
        assert_eq!(resp.expires_in, 600);
        assert_eq!(c["cgen"], credential_generation(FAKE_HASH));
    }

    /// A rotation lands between the Argon2 check and any later read: `verify`
    /// checked the OLD secret against H1, while `current` already sees H2.
    /// The cache must hold H1 — the hash Argon2 actually checked — or the old
    /// secret would be tied to the new hash and served unlimited for a TTL.
    struct RotatesBetweenReads {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl ClientVerifier for RotatesBetweenReads {
        async fn verify(
            &self,
            client_id: &str,
            client_secret: &str,
        ) -> Result<Option<ClientState>, IdpError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            // Checked against FAKE_HASH (H1), the hash before the rotation.
            Ok((client_secret == "old").then(|| ClientState {
                verified: VerifiedClient {
                    client_id: client_id.to_string(),
                    audience: "data-api".into(),
                    extra_claims: serde_json::Map::new(),
                    ttl_secs: 600,
                },
                secret_hash: FAKE_HASH.into(),
            }))
        }
        async fn current(&self, client_id: &str) -> Result<Option<ClientState>, IdpError> {
            // The rotation has landed: the registry now stores H2.
            Ok(Some(ClientState {
                verified: VerifiedClient {
                    client_id: client_id.to_string(),
                    audience: "data-api".into(),
                    extra_claims: serde_json::Map::new(),
                    ttl_secs: 600,
                },
                secret_hash: CATALOG_LIKE_HASH.into(),
            }))
        }
    }

    #[tokio::test]
    async fn the_cache_holds_the_hash_argon2_checked_not_a_later_read() {
        let calls = Arc::new(AtomicUsize::new(0));
        let st = test_idp_state_with(vec![Arc::new(RotatesBetweenReads {
            calls: calls.clone(),
        })])
        .await;

        let _ = post_token_from(&st, "203.0.113.7", "cid", "old").await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let _ = post_token_from(&st, "203.0.113.7", "cid", "old").await;

        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "the old secret must not be served from cache against the rotated hash"
        );
    }

    /// A registry error on the hit path is not a refusal and not a pass: the
    /// request takes the full path and verifies.
    #[tokio::test]
    async fn a_registry_error_on_a_hit_falls_through_to_verify() {
        let (st, counting) = counting_state(100);
        assert_eq!(
            post_token_from(&st, "203.0.113.7", "cid", "sec")
                .await
                .status,
            200
        );

        counting.fail_current();
        let r = post_token_from(&st, "203.0.113.7", "cid", "sec").await;

        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(counting.verifies(), 2);
    }

    // ---- I4: RFC 8693 token exchange ---------------------------------------

    /// One client, `m1`/`sec`, whose registry row a test can rewrite between
    /// requests: rotate its hash, delete it, change its claims or audience.
    struct MutableVerifier {
        row: Arc<std::sync::Mutex<Option<ClientState>>>,
    }

    #[async_trait::async_trait]
    impl ClientVerifier for MutableVerifier {
        async fn verify(
            &self,
            client_id: &str,
            client_secret: &str,
        ) -> Result<Option<ClientState>, IdpError> {
            if client_secret == "sec" {
                self.current(client_id).await
            } else {
                Ok(None)
            }
        }
        async fn current(&self, client_id: &str) -> Result<Option<ClientState>, IdpError> {
            Ok(self
                .row
                .lock()
                .unwrap()
                .clone()
                .filter(|s| s.verified.client_id == client_id))
        }
    }

    type Row = Arc<std::sync::Mutex<Option<ClientState>>>;

    fn m1_row(roles: serde_json::Value, hash: &str) -> ClientState {
        ClientState {
            verified: VerifiedClient {
                client_id: "m1".into(),
                audience: "catalog".into(),
                extra_claims: serde_json::json!({ "warehouse": "wh1", "roles": roles })
                    .as_object()
                    .cloned()
                    .unwrap(),
                ttl_secs: 3600,
            },
            secret_hash: hash.into(),
        }
    }

    fn exchange_state(max: u32) -> (IdpState, Row) {
        let row: Row = Arc::new(std::sync::Mutex::new(Some(m1_row(
            serde_json::json!(["r1"]),
            FAKE_HASH,
        ))));
        let km = generate_rsa_key("kid-t").unwrap();
        let st = IdpState {
            keys: Arc::new(FakeKeys(km)),
            issuer: "http://localhost:8080/".into(),
            verifiers: vec![Arc::new(MutableVerifier { row: row.clone() })],
            rate_limiter: RateLimiter::new(max, 3_600, 3_600),
            verified_cache: VerifiedCache::default(),
        };
        (st, row)
    }

    fn exchange_form(subject_token: &str) -> TokenForm {
        parse_token_form(&format!(
            "grant_type={TOKEN_EXCHANGE_GRANT}&subject_token={subject_token}\
             &subject_token_type={ACCESS_TOKEN_TYPE}&scope=catalog"
        ))
    }

    /// Drive the endpoint with explicit headers and form, rendering either
    /// outcome through `IntoResponse`.
    async fn post_form(st: &IdpState, headers: HeaderMap, form: TokenForm) -> RawResp {
        let resp = match token_endpoint(State(st.clone()), headers, Form(form)).await {
            Ok(ok) => ok.into_response(),
            Err(e) => e.into_response(),
        };
        let status = resp.status().as_u16();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        RawResp {
            status,
            body: serde_json::from_slice(&bytes).unwrap(),
        }
    }

    fn bearer(token: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        h
    }

    fn now_secs() -> i64 {
        chrono::Utc::now().timestamp()
    }

    /// A real subject token, minted by `client_credentials` through the endpoint.
    async fn mint_m1(st: &IdpState) -> String {
        let r = post_form(st, HeaderMap::new(), cc_form("m1", "sec")).await;
        assert_eq!(r.status, 200, "{}", r.body);
        r.body["access_token"].as_str().unwrap().to_string()
    }

    /// Sign `claims` with the IdP's own active key.
    async fn sign_as_idp(st: &IdpState, claims: serde_json::Value) -> String {
        sign_claims(&st.keys.active().await.unwrap(), &claims).unwrap()
    }

    fn m1_claims(aud: &str, iss: &str, exp: i64) -> serde_json::Value {
        serde_json::json!({
            "warehouse": "wh1",
            "roles": ["r1"],
            "iss": iss,
            "sub": "m1",
            "aud": aud,
            "exp": exp,
            "cgen": credential_generation(FAKE_HASH),
        })
    }

    #[tokio::test]
    async fn a_live_token_exchanges_for_a_fresh_one() {
        let (st, row) = exchange_state(100);
        let subject = mint_m1(&st).await;
        let subject_claims = decode_claims_without_verifying(&subject);
        // `exp` has one-second resolution; wait one out so "later" is decidable.
        tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
        // The registry's view changes after the subject token was minted.
        *row.lock().unwrap() = Some(m1_row(serde_json::json!(["r2"]), FAKE_HASH));

        // Iceberg's refresh sends the same token in the header and the form.
        let r = post_form(&st, bearer(&subject), exchange_form(&subject)).await;

        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(r.body["token_type"], "Bearer");
        assert_eq!(r.body["expires_in"], 3600);
        let fresh = r.body["access_token"].as_str().unwrap();
        let jwk = st.keys.all_public_jwks().await.unwrap()[0].clone();
        let c = verify_with_jwk(fresh, &jwk).unwrap();
        assert_eq!(c["sub"], "m1");
        assert_eq!(c["aud"], "catalog");
        assert_eq!(c["iss"], "http://localhost:8080");
        assert_eq!(
            c["roles"],
            serde_json::json!(["r2"]),
            "claims come from the client's current state, not the subject token"
        );
        assert_eq!(c["cgen"], credential_generation(FAKE_HASH));
        assert!(
            c["exp"].as_i64().unwrap() > subject_claims["exp"].as_i64().unwrap(),
            "fresh exp {} must be later than the subject's {}",
            c["exp"],
            subject_claims["exp"]
        );
    }

    #[tokio::test]
    async fn the_bearer_header_alone_is_not_enough() {
        let (st, _row) = exchange_state(100);
        let subject = mint_m1(&st).await;

        let mut no_field = exchange_form(&subject);
        no_field.subject_token = None;
        let r = post_form(&st, bearer(&subject), no_field).await;
        assert_eq!(r.status, 400, "{}", r.body);
        assert_eq!(r.body["error"], "invalid_request");

        // A header naming a different token than the form field is refused too.
        let other = sign_as_idp(
            &st,
            m1_claims("catalog", "http://localhost:8080", now_secs() + 600),
        )
        .await;
        let r = post_form(&st, bearer(&other), exchange_form(&subject)).await;
        assert_eq!(r.status, 400, "{}", r.body);
        assert_eq!(r.body["error"], "invalid_request");
    }

    #[tokio::test]
    async fn an_expired_subject_token_is_invalid_grant() {
        let (st, _row) = exchange_state(100);
        let expired = sign_as_idp(
            &st,
            m1_claims("catalog", "http://localhost:8080", now_secs() - 3_600),
        )
        .await;

        let r = post_form(&st, HeaderMap::new(), exchange_form(&expired)).await;

        assert_eq!(r.status, 400, "{}", r.body);
        assert_eq!(r.body["error"], "invalid_grant");
    }

    #[tokio::test]
    async fn a_foreign_signature_is_invalid_grant() {
        let (st, _row) = exchange_state(100);
        // Same `kid` as the IdP's key, different key: only the signature differs.
        let foreign = generate_rsa_key("kid-t").unwrap();
        let tok = sign_claims(
            &foreign,
            &m1_claims("catalog", "http://localhost:8080", now_secs() + 600),
        )
        .unwrap();

        let r = post_form(&st, HeaderMap::new(), exchange_form(&tok)).await;

        assert_eq!(r.status, 400, "{}", r.body);
        assert_eq!(r.body["error"], "invalid_grant");
    }

    #[tokio::test]
    async fn a_wrong_issuer_is_invalid_grant() {
        let (st, _row) = exchange_state(100);
        let tok = sign_as_idp(
            &st,
            m1_claims("catalog", "https://evil.example", now_secs() + 600),
        )
        .await;

        let r = post_form(&st, HeaderMap::new(), exchange_form(&tok)).await;

        assert_eq!(r.status, 400, "{}", r.body);
        assert_eq!(r.body["error"], "invalid_grant");
    }

    #[tokio::test]
    async fn a_rotated_secret_ends_the_chain() {
        let (st, row) = exchange_state(100);
        let subject = mint_m1(&st).await;
        *row.lock().unwrap() = Some(m1_row(serde_json::json!(["r1"]), CATALOG_LIKE_HASH));

        let r = post_form(&st, HeaderMap::new(), exchange_form(&subject)).await;

        assert_eq!(r.status, 400, "{}", r.body);
        assert_eq!(r.body["error"], "invalid_grant");
    }

    #[tokio::test]
    async fn a_deleted_client_ends_the_chain() {
        let (st, row) = exchange_state(100);
        let subject = mint_m1(&st).await;
        *row.lock().unwrap() = None;

        let r = post_form(&st, HeaderMap::new(), exchange_form(&subject)).await;

        assert_eq!(r.status, 400, "{}", r.body);
        assert_eq!(r.body["error"], "invalid_grant");
    }

    #[tokio::test]
    async fn the_audience_cannot_change() {
        let (st, row) = exchange_state(100);
        let mut data_api = m1_row(serde_json::json!(["r1"]), FAKE_HASH);
        data_api.verified.audience = "data-api".into();
        *row.lock().unwrap() = Some(data_api);
        let exp = now_secs() + 600;

        let as_catalog = sign_as_idp(&st, m1_claims("catalog", "http://localhost:8080", exp)).await;
        let r = post_form(&st, HeaderMap::new(), exchange_form(&as_catalog)).await;
        assert_eq!(r.status, 400, "{}", r.body);
        assert_eq!(r.body["error"], "invalid_grant");

        // Control: the same token for the audience the client has is good.
        let as_data_api =
            sign_as_idp(&st, m1_claims("data-api", "http://localhost:8080", exp)).await;
        let r = post_form(&st, HeaderMap::new(), exchange_form(&as_data_api)).await;
        assert_eq!(r.status, 200, "{}", r.body);
    }

    #[tokio::test]
    async fn a_wrong_subject_token_type_is_invalid_request() {
        let (st, _row) = exchange_state(100);
        let subject = mint_m1(&st).await;

        let mut wrong = exchange_form(&subject);
        wrong.subject_token_type = Some("urn:ietf:params:oauth:token-type:id_token".into());
        let r = post_form(&st, HeaderMap::new(), wrong).await;
        assert_eq!(r.status, 400, "{}", r.body);
        assert_eq!(r.body["error"], "invalid_request");

        let mut missing = exchange_form(&subject);
        missing.subject_token_type = None;
        let r = post_form(&st, HeaderMap::new(), missing).await;
        assert_eq!(r.status, 400, "{}", r.body);
        assert_eq!(r.body["error"], "invalid_request");
    }

    /// Junk subject tokens share one `ip:exchange` bucket, so a flood of them
    /// is limited — and a real exchange, keyed on its `sub`, is not caught in it.
    #[tokio::test]
    async fn junk_subject_tokens_are_limited_per_ip() {
        let (st, _row) = exchange_state(2);
        let subject = mint_m1(&st).await;

        let statuses = [
            post_form(&st, HeaderMap::new(), exchange_form("junk1"))
                .await
                .status,
            post_form(&st, HeaderMap::new(), exchange_form("junk2"))
                .await
                .status,
            post_form(&st, HeaderMap::new(), exchange_form("junk3"))
                .await
                .status,
        ];
        assert_eq!(statuses, [400, 400, 429]);

        let r = post_form(&st, HeaderMap::new(), exchange_form(&subject)).await;
        assert_eq!(r.status, 200, "{}", r.body);
    }

    #[tokio::test]
    async fn discovery_lists_both_grants() {
        let (st, _row) = exchange_state(100);
        let doc = discovery(State(st)).await.0;
        assert_eq!(
            doc["grant_types_supported"],
            serde_json::json!(["client_credentials", TOKEN_EXCHANGE_GRANT])
        );
    }

    /// Through axum's real `Form` extractor: a missing field used to be a 422
    /// from the extractor (I4), which no OAuth client reads as a refusal.
    #[tokio::test]
    async fn client_credentials_without_client_id_is_invalid_request_not_422() {
        use axum::extract::FromRequest as _;
        let (st, counting) = counting_state(100);
        for body in [
            "grant_type=client_credentials&client_secret=sec",
            "grant_type=client_credentials&client_id=cid",
        ] {
            let req = axum::http::Request::builder()
                .method("POST")
                .uri("/oauth/token")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(axum::body::Body::from(body))
                .unwrap();
            let form = match Form::<TokenForm>::from_request(req, &()).await {
                Ok(f) => f,
                Err(rej) => panic!(
                    "the extractor refused `{body}` with {}",
                    rej.into_response().status()
                ),
            };

            let r = post_form(&st, HeaderMap::new(), form.0).await;

            assert_eq!(r.status, 400, "{body}: {}", r.body);
            assert_eq!(r.body["error"], "invalid_request", "{body}");
        }
        assert_eq!(counting.verifies(), 0);
    }

    #[tokio::test]
    async fn any_other_grant_is_unsupported() {
        let (st, _row) = exchange_state(100);
        let r = post_form(
            &st,
            HeaderMap::new(),
            parse_token_form("grant_type=password&client_id=m1&client_secret=sec"),
        )
        .await;
        assert_eq!(r.status, 400, "{}", r.body);
        assert_eq!(r.body["error"], "unsupported_grant_type");
    }
}
