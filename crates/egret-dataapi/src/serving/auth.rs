//! Bearer token → [`ServingPrincipal`]. The authentication half of the serving
//! surface.
//!
//! **Read this before changing anything below.**
//! [`egret_machine_idp::verify_with_jwk`] validates the SIGNATURE and nothing
//! else. It sets `validate_aud = false`, `validate_exp = false`, and clears
//! `required_spec_claims` — so on its own it will happily accept
//!
//! - a `catalog` token, because the machine-IdP signs several audiences with the
//!   ONE key, and since P2 a `ws_admin` can self-issue a `data-api` token from
//!   that same key;
//! - a token that expired an hour ago;
//! - a token carrying no `exp` claim at all — an immortal credential, which is
//!   the worst of the three because the 600s TTL that makes a stateless,
//!   unrevokable Data API token acceptable simply stops existing.
//!
//! So this module asserts all three itself: `exp` present and future, `aud` is
//! ours, signature against the IdP's active key. `EgretCatalogGuard`
//! (`crates/egret-bin/src/main.rs`) does exactly this for the catalog audience
//! and carries the same reasoning — it is the house pattern, not a local habit.

use super::ServingError;
use egret_core::UserId;
use egret_machine_idp::SigningKeyStore;
use uuid::Uuid;

/// The `aud` a Data API token carries — set by P2's `ClientStore::verify`, which
/// is the only verifier registered for it.
pub const DATA_API_AUDIENCE: &str = "data-api";

/// Who a served request runs as.
///
/// `user_id` is the service account's `users` row: the principal every
/// downstream authorization and delegated query is evaluated against, so Ranger
/// filters the served rows for the service account rather than for whoever
/// published the endpoint. `workspace_id` comes from the TOKEN, never from the
/// request path — a caller must not be able to name a workspace their credential
/// was not minted for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServingPrincipal {
    pub user_id: UserId,
    pub workspace_id: Uuid,
    pub client_id: String,
}

/// Verify a serving bearer token and return its principal.
///
/// Order is deliberate — signature, then `exp`, then `aud`, then identity — but
/// invisible to the caller: **every** failure is [`ServingError::Unauthorized`],
/// so no probe can tell a forged signature from a stale token from a catalog
/// token. Only an egret-side fault ([`ServingError::Internal`]) is distinct, and
/// that one is about us, not about the credential.
pub async fn verify_serving_token(
    keys: &dyn SigningKeyStore,
    bearer: &str,
) -> Result<ServingPrincipal, ServingError> {
    // A key-store failure is OUR outage, not a bad credential. Reporting it as
    // 401 would send an operator to rotate a client secret that is perfectly
    // fine while the real fault goes unlogged.
    let jwk = keys
        .active()
        .await
        .map_err(|e| ServingError::Internal(anyhow::anyhow!("idp signing key: {e}")))?
        .public_jwk;

    let claims =
        egret_machine_idp::verify_with_jwk(bearer, &jwk).map_err(|_| ServingError::Unauthorized)?;

    // `exp` MUST be present. `verify_with_jwk` cleared `required_spec_claims`,
    // so a token with no `exp` gets this far; absent is treated as expired, and
    // never as "never expires".
    let exp = claims
        .get("exp")
        .and_then(|v| v.as_i64())
        .ok_or(ServingError::Unauthorized)?;
    if exp <= chrono::Utc::now().timestamp() {
        return Err(ServingError::Unauthorized);
    }

    // The signature proves the token came from egret. It does not prove the
    // token was meant for the Data API. See the module comment.
    if claims.get("aud").and_then(|v| v.as_str()) != Some(DATA_API_AUDIENCE) {
        return Err(ServingError::Unauthorized);
    }

    // Strict, from here down. A missing or unparseable claim is a refusal — a
    // `unwrap_or_default()` would give every malformed token `Uuid::nil()` as its
    // workspace and principal, i.e. one shared tenant that no real row belongs
    // to but that every such token agrees on.
    let workspace_id = claims
        .get("ws")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or(ServingError::Unauthorized)?;
    let user_id = claims
        .get("egret_user_id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
        .map(UserId)
        .ok_or(ServingError::Unauthorized)?;
    let client_id = claims
        .get("sub")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or(ServingError::Unauthorized)?
        .to_string();

    Ok(ServingPrincipal {
        user_id,
        workspace_id,
        client_id,
    })
}
