//! OIDC login: claims, provisioning, and (Task 4) the OidcService.

use egret_core::{Result, UserId};

use crate::groups::GroupRepo;
use crate::users::UserRepo;

/// Normalized claims extracted from a validated id_token.
#[derive(Debug, Clone)]
pub struct OidcClaims {
    pub subject: String,
    pub email: String,
    pub name: String,
    pub groups: Vec<String>,
}

/// Result of a completed login: normalized claims from the validated id_token.
#[derive(Debug)]
pub struct OidcLogin {
    pub claims: OidcClaims,
}

/// JIT-provision the user (match by `sub`) and sync their OIDC group memberships.
/// Returns the local user id. Roles resolve via the existing group→role path.
///
/// The sync recompiles every engine policy a MOVED group reaches, through
/// `recompiler` (`crate::groups`' module doc) — the per-login writer of
/// `user_groups`, and the reason the recompile is a port.
pub async fn provision_and_sync(
    users: &UserRepo,
    groups: &GroupRepo,
    claims: &OidcClaims,
    recompiler: &dyn crate::groups::GroupMembershipRecompiler,
) -> Result<UserId> {
    let user_id = match users.find_by_external_subject(&claims.subject).await? {
        Some(u) => {
            users
                .update_profile(u.id, &claims.email, &claims.name)
                .await?;
            u.id
        }
        None => {
            users
                .insert_oidc(&claims.subject, &claims.email, &claims.name)
                .await?
        }
    };
    users.touch_last_login(user_id).await?;
    for g in &claims.groups {
        groups.upsert_oidc_group(g).await?;
    }
    // Design R2: a claimed name that names a LOCAL group is ignored by the
    // sync (R1) — `upsert_oidc_group` returned the local id above and left its
    // source alone. Say so, once per name: before R1 such a claim joined the
    // user to the local group, and any row it added is still there,
    // indistinguishable from one an admin added (`already_member`). The login
    // is not refused.
    for (name, member) in groups.claimed_local_groups(user_id, &claims.groups).await? {
        tracing::warn!(
            user_id = %user_id.0,
            group = %name,
            already_member = member,
            "OIDC groups claim names a local group; membership not managed by the IdP; \
             if it was added by a login before this fix, review it"
        );
    }
    groups
        .sync_user_oidc_groups(user_id, &claims.groups, recompiler)
        .await?;
    Ok(user_id)
}

use egret_config::OidcConfig;
use egret_core::Error;
use openidconnect::core::{
    CoreAuthDisplay, CoreAuthPrompt, CoreAuthenticationFlow, CoreErrorResponseType,
    CoreGenderClaim, CoreJsonWebKey, CoreJweContentEncryptionAlgorithm, CoreJwsSigningAlgorithm,
    CoreProviderMetadata, CoreRevocableToken, CoreRevocationErrorResponse,
    CoreTokenIntrospectionResponse, CoreTokenType,
};
use openidconnect::{
    AuthorizationCode, Client, ClientId, ClientSecret, CsrfToken, EmptyExtraTokenFields,
    EndpointMaybeSet, EndpointNotSet, EndpointSet, IdTokenClaims, IdTokenFields, IssuerUrl, Nonce,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, StandardErrorResponse,
    StandardTokenResponse, TokenResponse,
};
use serde::{Deserialize, Serialize};

/// Additional id_token claims beyond the OIDC standard set. We only care about the
/// configurable `groups` array (a string array emitted by the IdP's group mapper).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct GroupsClaims {
    #[serde(default)]
    groups: Option<Vec<String>>,
}

impl openidconnect::AdditionalClaims for GroupsClaims {}

/// Token response carrying our [`GroupsClaims`] inside the id_token fields.
type GroupsTokenResponse = StandardTokenResponse<
    IdTokenFields<
        GroupsClaims,
        EmptyExtraTokenFields,
        CoreGenderClaim,
        CoreJweContentEncryptionAlgorithm,
        CoreJwsSigningAlgorithm,
    >,
    CoreTokenType,
>;

/// id_token claims parameterized with our [`GroupsClaims`].
type GroupsIdTokenClaims = IdTokenClaims<GroupsClaims, CoreGenderClaim>;

/// A raw id_token parameterized with our [`GroupsClaims`]. Parsing a token string
/// (e.g. from a ROPC grant) into this type lets us reuse [`OidcService::claims_from`].
type GroupsIdToken = openidconnect::IdToken<
    GroupsClaims,
    CoreGenderClaim,
    CoreJweContentEncryptionAlgorithm,
    CoreJwsSigningAlgorithm,
>;

/// The concrete client type-state produced by [`Client::from_provider_metadata`]:
/// the authorization endpoint is always set, the token and userinfo endpoints are
/// "maybe set" (present iff the discovery document advertised them).
type GroupsClient = Client<
    GroupsClaims,
    CoreAuthDisplay,
    CoreGenderClaim,
    CoreJweContentEncryptionAlgorithm,
    CoreJsonWebKey,
    CoreAuthPrompt,
    StandardErrorResponse<CoreErrorResponseType>,
    GroupsTokenResponse,
    CoreTokenIntrospectionResponse,
    CoreRevocableToken,
    CoreRevocationErrorResponse,
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

/// What `start` returns; the caller persists state/nonce/verifier in the encrypted cookie.
pub struct OidcStart {
    pub authorize_url: String,
    pub state: String,
    pub nonce: String,
    pub pkce_verifier: String,
}

#[derive(Clone)]
pub struct OidcService {
    client: GroupsClient,
    http: reqwest::Client,
    scopes: Vec<String>,
    groups_claim: String,
    issuer_url: String,
    client_id: String,
    /// The provider's JWKS endpoint, captured from discovery metadata so
    /// [`Self::provider_jwks`] can fetch it independently of the `openidconnect`
    /// client (which caches its own copy for id_token verification only).
    jwks_uri: String,
    /// Short-TTL cache of the provider JWKS, shared across clones. Backs
    /// [`Self::logout_jwks`] so a stream of unauthenticated back-channel logout
    /// POSTs can't amplify into a JWKS fetch per request, and so revocation
    /// survives a transient IdP JWKS outage (fetch failure → last good keys).
    jwks_cache: std::sync::Arc<std::sync::RwLock<Option<CachedJwks>>>,
}

/// A provider JWKS with the instant it was fetched, for TTL expiry.
struct CachedJwks {
    jwks: jsonwebtoken::jwk::JwkSet,
    fetched_at: std::time::Instant,
}

/// How long a fetched JWKS is served from cache before a refetch. Keycloak key
/// rotation is infrequent and back-channel delivery is retried, so a few minutes
/// of staleness is safe; the win is bounding unauthenticated fetch amplification.
const JWKS_TTL: std::time::Duration = std::time::Duration::from_secs(300);

fn oidc_err(msg: impl std::fmt::Display) -> Error {
    Error::Internal(anyhow::anyhow!("oidc: {msg}"))
}

impl OidcService {
    /// Discover provider metadata and construct the client.
    pub async fn discover(cfg: &OidcConfig) -> Result<Self> {
        // Redirects are disabled on the discovery/token HTTP client to avoid SSRF.
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(oidc_err)?;
        let issuer = IssuerUrl::new(cfg.issuer_url.clone()).map_err(oidc_err)?;
        let meta = CoreProviderMetadata::discover_async(issuer, &http)
            .await
            .map_err(oidc_err)?;
        let jwks_uri = meta.jwks_uri().url().to_string();
        let client: GroupsClient = Client::from_provider_metadata(
            meta,
            ClientId::new(cfg.client_id.clone()),
            Some(ClientSecret::new(cfg.client_secret.clone())),
        )
        .set_redirect_uri(RedirectUrl::new(cfg.redirect_url.clone()).map_err(oidc_err)?);
        Ok(Self {
            client,
            http,
            scopes: cfg.scopes.clone(),
            groups_claim: cfg.groups_claim.clone(),
            issuer_url: cfg.issuer_url.clone(),
            client_id: cfg.client_id.clone(),
            jwks_uri,
            jwks_cache: std::sync::Arc::new(std::sync::RwLock::new(None)),
        })
    }

    /// The configured issuer URL (used to validate back-channel logout tokens).
    pub fn issuer(&self) -> &str {
        &self.issuer_url
    }

    /// The configured client id (the expected `aud` on back-channel logout tokens).
    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// Provider JWKS for verifying back-channel logout tokens, served from a
    /// short-TTL in-memory cache (see [`JWKS_TTL`]). Callers on the unauthenticated
    /// `/backchannel-logout` path must use this, not [`Self::provider_jwks`]:
    /// caching stops a stream of logout POSTs (including garbage ones) from
    /// amplifying into one IdP fetch each, and a fetch failure falls back to the
    /// last good keys so a transient JWKS outage doesn't silently drop a revoke.
    pub async fn logout_jwks(&self) -> Result<jsonwebtoken::jwk::JwkSet> {
        // Fast path: a cached copy still within its TTL.
        if let Some(jwks) = self.cached_jwks(true) {
            return Ok(jwks);
        }
        // Cold or stale: refetch. On failure, fall back to any cached copy so
        // revocation keeps working through a transient IdP JWKS outage.
        match self.provider_jwks().await {
            Ok(jwks) => {
                if let Ok(mut guard) = self.jwks_cache.write() {
                    *guard = Some(CachedJwks {
                        jwks: jwks.clone(),
                        fetched_at: std::time::Instant::now(),
                    });
                }
                Ok(jwks)
            }
            Err(e) => match self.cached_jwks(false) {
                Some(jwks) => {
                    tracing::warn!(error = %e, "provider JWKS refetch failed; using cached keys");
                    Ok(jwks)
                }
                None => Err(e),
            },
        }
    }

    /// Read the cached JWKS. When `fresh_only`, returns `None` unless the cached
    /// copy is still within [`JWKS_TTL`]; otherwise returns any cached copy
    /// (used as the stale fallback when a refetch fails).
    fn cached_jwks(&self, fresh_only: bool) -> Option<jsonwebtoken::jwk::JwkSet> {
        let guard = self.jwks_cache.read().ok()?;
        let c = guard.as_ref()?;
        if fresh_only && c.fetched_at.elapsed() >= JWKS_TTL {
            return None;
        }
        Some(c.jwks.clone())
    }

    /// Fetch the provider's current JWKS directly (no cache). Prefer
    /// [`Self::logout_jwks`] on the unauthenticated back-channel path.
    pub async fn provider_jwks(&self) -> Result<jsonwebtoken::jwk::JwkSet> {
        let resp = self
            .http
            .get(&self.jwks_uri)
            .send()
            .await
            .map_err(oidc_err)?
            .error_for_status()
            .map_err(oidc_err)?;
        let jwks = resp
            .json::<jsonwebtoken::jwk::JwkSet>()
            .await
            .map_err(oidc_err)?;
        Ok(jwks)
    }

    /// Build the authorize URL with a fresh PKCE challenge, nonce, and state.
    pub fn start(&self) -> OidcStart {
        self.start_inner(false)
    }

    /// Like [`Self::start`] but forces the IdP to **re-prompt for credentials**
    /// (`prompt=login`) rather than silently reusing an existing IdP session. This
    /// is the OIDC step-up used to gate an impersonation start (Task 8): it proves
    /// a *fresh* proof of identity, the SSO analogue of re-entering the password.
    pub fn start_step_up(&self) -> OidcStart {
        self.start_inner(true)
    }

    fn start_inner(&self, prompt_login: bool) -> OidcStart {
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let mut req = self.client.authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        );
        for s in &self.scopes {
            req = req.add_scope(Scope::new(s.clone()));
        }
        if prompt_login {
            req = req.add_prompt(CoreAuthPrompt::Login);
        }
        let (url, state, nonce) = req.set_pkce_challenge(challenge).url();
        OidcStart {
            authorize_url: url.to_string(),
            state: state.secret().clone(),
            nonce: nonce.secret().clone(),
            pkce_verifier: verifier.secret().clone(),
        }
    }

    /// Exchange the code and validate the id_token; return normalized claims.
    pub async fn finish(&self, code: &str, pkce_verifier: &str, nonce: &str) -> Result<OidcLogin> {
        let token_response = self
            .client
            .exchange_code(AuthorizationCode::new(code.to_string()))
            .map_err(oidc_err)?
            .set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier.to_string()))
            .request_async(&self.http)
            .await
            .map_err(oidc_err)?;
        let claims = self.verify_from_response(&token_response, nonce)?;
        Ok(OidcLogin { claims })
    }

    /// Validate a raw id_token string (used by tests / non-auth-code flows such as
    /// ROPC). The `nonce` is checked only when `Some`; when `None`, the id_token must
    /// not carry a `nonce` claim (matching the OIDC "no nonce requested" case). Runs
    /// the same signature/iss/aud/exp validation as [`Self::finish`]. Returns
    /// normalized claims.
    pub fn verify_id_token(&self, id_token_str: &str, nonce: Option<&str>) -> Result<OidcClaims> {
        use std::str::FromStr;
        let id_token = GroupsIdToken::from_str(id_token_str).map_err(oidc_err)?;
        let verifier = self.client.id_token_verifier();
        // `NonceVerifier` for a closure is `FnOnce(Option<&Nonce>) -> Result<(), String>`.
        let nonce_check = |got: Option<&Nonce>| match (nonce, got) {
            // No nonce expected: accept tokens without a nonce claim (ROPC).
            (None, None) => Ok(()),
            (None, Some(_)) => Err("unexpected nonce claim".to_string()),
            (Some(_), None) => Err("missing nonce claim".to_string()),
            (Some(expected), Some(got)) if got.secret() == expected => Ok(()),
            (Some(_), Some(_)) => Err("nonce mismatch".to_string()),
        };
        let claims = id_token.claims(&verifier, nonce_check).map_err(oidc_err)?;
        self.claims_from(claims)
    }

    /// Verify the id_token in a token response (signature/iss/aud/exp + nonce) and
    /// return normalized claims.
    fn verify_from_response(
        &self,
        token_response: &GroupsTokenResponse,
        nonce: &str,
    ) -> Result<OidcClaims> {
        let id_token = token_response
            .id_token()
            .ok_or_else(|| oidc_err("no id_token"))?;
        let verifier = self.client.id_token_verifier();
        let claims = id_token
            .claims(&verifier, &Nonce::new(nonce.to_string()))
            .map_err(oidc_err)?;
        self.claims_from(claims)
    }

    fn claims_from(&self, claims: &GroupsIdTokenClaims) -> Result<OidcClaims> {
        let subject = claims.subject().to_string();
        let email = claims
            .email()
            .map(|e| e.to_string())
            .ok_or_else(|| oidc_err("no email claim"))?;
        let name = claims
            .name()
            .and_then(|n| n.get(None))
            .map(|n| n.to_string())
            .unwrap_or_else(|| email.clone());
        let groups = extract_groups(claims, &self.groups_claim);
        Ok(OidcClaims {
            subject,
            email,
            name,
            groups,
        })
    }
}

/// Read the configured string-array groups claim from the id_token.
///
/// The common case is the `groups` additional claim captured directly by
/// [`GroupsClaims`]. When the deployment configures a differently-named claim,
/// fall back to scanning the serialized claims JSON for that field.
fn extract_groups(claims: &GroupsIdTokenClaims, claim: &str) -> Vec<String> {
    if claim == "groups" {
        if let Some(groups) = claims.additional_claims().groups.clone() {
            return groups;
        }
    }
    let val = serde_json::to_value(claims).unwrap_or(serde_json::Value::Null);
    match val.get(claim) {
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        _ => Vec::new(),
    }
}

/// OIDC Back-Channel Logout 1.0 token validation. Verifies the JWS against `jwks`
/// (RS256, by `kid`), enforces `iss`/`aud`/`exp`, requires the back-channel-logout
/// `events` key, and rejects any token carrying a `nonce` (that's an ID token, not
/// a logout token). Returns the `sub` on success.
pub fn verify_logout_token(
    jwks: &jsonwebtoken::jwk::JwkSet,
    issuer: &str,
    audience: &str,
    token: &str,
) -> Result<String> {
    use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
    let kid = decode_header(token)
        .map_err(oidc_err)?
        .kid
        .ok_or(Error::Unauthorized)?;
    let jwk = jwks.find(&kid).ok_or(Error::Unauthorized)?;
    let key = DecodingKey::from_jwk(jwk).map_err(oidc_err)?;
    let mut v = Validation::new(Algorithm::RS256);
    v.set_issuer(&[issuer]);
    v.set_audience(&[audience]);
    // jsonwebtoken only checks iss/aud *if the claim is present*; require them
    // (plus exp/sub) so a token that omits iss/aud can't slip past the match.
    v.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
    let c = decode::<serde_json::Value>(token, &key, &v)
        .map_err(oidc_err)?
        .claims;
    let has_event = c
        .get("events")
        .and_then(|e| e.as_object())
        .map(|o| o.contains_key("http://schemas.openid.net/event/backchannel-logout"))
        .unwrap_or(false);
    if !has_event {
        return Err(Error::Unauthorized);
    }
    if c.get("nonce").is_some() {
        return Err(Error::Unauthorized);
    }
    c.get("sub")
        .and_then(|s| s.as_str())
        .map(String::from)
        .ok_or(Error::Unauthorized)
}

#[cfg(test)]
mod logout_token_tests {
    use super::verify_logout_token;
    use egret_machine_idp::{generate_rsa_key, KeyMaterial};
    use jsonwebtoken::{encode, jwk::JwkSet, EncodingKey, Header};
    use serde_json::json;

    const ISSUER: &str = "https://keycloak.example.com/realms/egret";
    const AUDIENCE: &str = "egret-backend";

    /// Build a fresh RSA keypair + the JWKS an issuer would publish for it.
    fn keypair() -> (KeyMaterial, JwkSet) {
        let km = generate_rsa_key("test-kid").expect("generate rsa key");
        let jwks: JwkSet =
            serde_json::from_value(json!({ "keys": [km.public_jwk] })).expect("parse jwks");
        (km, jwks)
    }

    /// Sign `claims` (already containing header-relevant fields like `kid` via
    /// the encoding key) with `km`'s private key.
    fn sign(km: &KeyMaterial, claims: &serde_json::Value) -> String {
        let mut header = Header::new(jsonwebtoken::Algorithm::RS256);
        header.kid = Some(km.kid.clone());
        let key = EncodingKey::from_rsa_pem(km.private_pem.as_bytes()).expect("encoding key");
        encode(&header, claims, &key).expect("sign token")
    }

    fn base_claims(sub: &str, exp: i64) -> serde_json::Value {
        json!({
            "iss": ISSUER,
            "aud": AUDIENCE,
            "sub": sub,
            "exp": exp,
            "iat": chrono::Utc::now().timestamp(),
            "jti": "logout-jti-1",
            "events": {
                "http://schemas.openid.net/event/backchannel-logout": {}
            }
        })
    }

    fn future_exp() -> i64 {
        (chrono::Utc::now() + chrono::Duration::minutes(5)).timestamp()
    }

    #[test]
    fn accepts_valid_logout_token() {
        let (km, jwks) = keypair();
        let claims = base_claims("user-123", future_exp());
        let token = sign(&km, &claims);
        let sub = verify_logout_token(&jwks, ISSUER, AUDIENCE, &token).unwrap();
        assert_eq!(sub, "user-123");
    }

    #[test]
    fn rejects_missing_events() {
        let (km, jwks) = keypair();
        let mut claims = base_claims("user-123", future_exp());
        claims.as_object_mut().unwrap().remove("events");
        let token = sign(&km, &claims);
        assert!(verify_logout_token(&jwks, ISSUER, AUDIENCE, &token).is_err());
    }

    #[test]
    fn rejects_nonce_present() {
        let (km, jwks) = keypair();
        let mut claims = base_claims("user-123", future_exp());
        claims["nonce"] = json!("some-nonce");
        let token = sign(&km, &claims);
        assert!(verify_logout_token(&jwks, ISSUER, AUDIENCE, &token).is_err());
    }

    #[test]
    fn rejects_wrong_aud() {
        let (km, jwks) = keypair();
        let claims = base_claims("user-123", future_exp());
        let token = sign(&km, &claims);
        assert!(verify_logout_token(&jwks, ISSUER, "some-other-aud", &token).is_err());
    }

    #[test]
    fn rejects_wrong_iss() {
        let (km, jwks) = keypair();
        let claims = base_claims("user-123", future_exp());
        let token = sign(&km, &claims);
        assert!(verify_logout_token(
            &jwks,
            "https://not-the-issuer.example.com",
            AUDIENCE,
            &token
        )
        .is_err());
    }

    #[test]
    fn rejects_expired() {
        let (km, jwks) = keypair();
        let expired = (chrono::Utc::now() - chrono::Duration::minutes(5)).timestamp();
        let claims = base_claims("user-123", expired);
        let token = sign(&km, &claims);
        assert!(verify_logout_token(&jwks, ISSUER, AUDIENCE, &token).is_err());
    }

    #[test]
    fn rejects_bad_signature() {
        let (_km, jwks) = keypair();
        let (other_km, _other_jwks) = keypair();
        let claims = base_claims("user-123", future_exp());
        // Signed by a key that is NOT in `jwks`, but whose `kid` happens to not
        // match — exercise the "signed by a different key than published" path
        // by reusing the same kid so lookup succeeds but the signature check fails.
        let mut header = Header::new(jsonwebtoken::Algorithm::RS256);
        header.kid = Some("test-kid".to_string());
        let key = EncodingKey::from_rsa_pem(other_km.private_pem.as_bytes()).unwrap();
        let token = encode(&header, &claims, &key).unwrap();
        assert!(verify_logout_token(&jwks, ISSUER, AUDIENCE, &token).is_err());
    }

    #[test]
    fn rejects_missing_aud() {
        // A token that OMITS `aud` must be rejected — jsonwebtoken only checks
        // aud when the claim is present, so `required_spec_claims` must force it.
        let (km, jwks) = keypair();
        let mut claims = base_claims("user-123", future_exp());
        claims.as_object_mut().unwrap().remove("aud");
        let token = sign(&km, &claims);
        assert!(verify_logout_token(&jwks, ISSUER, AUDIENCE, &token).is_err());
    }

    #[test]
    fn rejects_missing_iss() {
        // A token that OMITS `iss` must likewise be rejected.
        let (km, jwks) = keypair();
        let mut claims = base_claims("user-123", future_exp());
        claims.as_object_mut().unwrap().remove("iss");
        let token = sign(&km, &claims);
        assert!(verify_logout_token(&jwks, ISSUER, AUDIENCE, &token).is_err());
    }

    #[test]
    fn rejects_hs256_or_alg_none() {
        // The verifier pins RS256; a token signed with a symmetric HS256 key
        // (the classic alg-confusion attack) must not verify.
        let (_km, jwks) = keypair();
        let claims = base_claims("user-123", future_exp());
        let mut header = Header::new(jsonwebtoken::Algorithm::HS256);
        header.kid = Some("test-kid".to_string());
        let key = EncodingKey::from_secret(b"attacker-secret");
        let token = encode(&header, &claims, &key).unwrap();
        assert!(verify_logout_token(&jwks, ISSUER, AUDIENCE, &token).is_err());
    }
}
