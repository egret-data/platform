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
pub async fn provision_and_sync(
    users: &UserRepo,
    groups: &GroupRepo,
    claims: &OidcClaims,
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
    groups
        .sync_user_oidc_groups(user_id, &claims.groups)
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
}

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
        })
    }

    /// Build the authorize URL with a fresh PKCE challenge, nonce, and state.
    pub fn start(&self) -> OidcStart {
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let mut req = self.client.authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        );
        for s in &self.scopes {
            req = req.add_scope(Scope::new(s.clone()));
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
