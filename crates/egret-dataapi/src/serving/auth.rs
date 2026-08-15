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
//!
//! **One deliberate departure from the house pattern, for whoever copies it
//! next: copy THIS one.** `EgretCatalogGuard` reads `exp` with `unwrap_or(0)`.
//! That is safe — `0 <= now`, so a token with no `exp` is refused — but it
//! refuses it by pretending it expired in 1970, which means the guard cannot
//! tell a genuinely stale credential from a malformed one, and neither can the
//! operator reading its log. Here the two are separate arms with separate log
//! lines: absent `exp` is *malformed*, past `exp` is *expired*. The two
//! rejections that P4 most needs to tell apart should not share a code path.

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

/// Every rejection below logs its reason and then returns the SAME
/// [`ServingError::Unauthorized`]. Written once here so no arm can quietly stop
/// logging: the caller learns only that authentication failed, the operator
/// learns why.
///
/// `debug`, not `warn`: on a public, key-authenticated surface a failed
/// authentication is routine traffic, and a `warn` per probe is a log-flood
/// lever anyone on the internet can pull. It is server-side either way, so it
/// costs nothing on the oracle side.
fn refuse(reason: &'static str) -> ServingError {
    tracing::debug!(reason, "data-api serving token refused");
    ServingError::Unauthorized
}

/// Verify a serving bearer token and return its principal.
///
/// Order is deliberate — signature, then `exp`, then `aud`, then identity — but
/// invisible to the caller: **every** failure is [`ServingError::Unauthorized`],
/// so no probe can tell a forged signature from a stale token from a catalog
/// token. Only an egret-side fault ([`ServingError::Internal`]) is distinct, and
/// that one is about us, not about the credential.
///
/// The reason is not lost, it is just not *returned*: each arm logs through
/// [`refuse`] before collapsing into the unit variant. That split is the point —
/// an operator staring at a wall of 401s can tell "their clock is wrong" from
/// "someone is replaying a catalog token" without the prober learning the same
/// thing. Task 5's access log records the CODE; this is where the reason lives.
///
/// # It verifies against the ACTIVE key only, so a rotation invalidates every
/// live token
///
/// Stated here, beside the three checks this function does make, rather than
/// only in a ledger somewhere: `keys.active()` below is the single JWK this
/// verifies against, and both minting and verification follow `active()`
/// forward together. So the instant the signing key rotates, **every
/// already-issued `data-api` token stops verifying** and its holder gets 401s
/// until it re-exchanges — up to the full 600s TTL of blanket failure across
/// every client at once, indistinguishable from a bad credential and looking to
/// the operator like a mass outage.
///
/// Not fireable today: no rotation tooling exists in-tree, so `active()` never
/// changes in a running deployment. The fix when it does is to verify against
/// every JWK from `all_public_jwks()` — which the [`SigningKeyStore`] trait
/// already exposes, and which is exactly why it exposes it — accepting any that
/// validates. **Whoever adds rotation must do that first**, in the same change
/// and not after; a rotation shipped ahead of it is a scheduled outage.
///
/// This note lives in the doc rather than only in a design ledger deliberately.
/// The reason `verify_with_jwk` checks neither `aud` nor `exp` — the thing this
/// function exists to compensate for — survived three phases as a ledger entry
/// nobody opened while reading this code.
pub async fn verify_serving_token(
    keys: &dyn SigningKeyStore,
    bearer: &str,
) -> Result<ServingPrincipal, ServingError> {
    // Checked before the key store so an empty Authorization header — the most
    // common way to arrive here — costs no database read.
    if bearer.trim().is_empty() {
        return Err(refuse("empty bearer"));
    }

    // A key-store failure is OUR outage, not a bad credential. Reporting it as
    // 401 would send an operator to rotate a client secret that is perfectly
    // fine while the real fault goes unlogged.
    let jwk = keys
        .active()
        .await
        .map_err(|e| ServingError::Internal(anyhow::anyhow!("idp signing key: {e}")))?
        .public_jwk;

    // The error is logged HERE and nowhere else: it is the only point at which
    // "malformed JWT" and "signature does not verify" are distinguishable at
    // all, and jsonwebtoken's message is the only description of which.
    let claims = egret_machine_idp::verify_with_jwk(bearer, &jwk).map_err(|e| {
        tracing::debug!(reason = "signature", error = %e, "data-api serving token refused");
        ServingError::Unauthorized
    })?;

    // `exp` MUST be present. `verify_with_jwk` cleared `required_spec_claims`,
    // so a token with no `exp` gets this far; absent is treated as expired, and
    // never as "never expires". Absent and stale are separate arms so the log
    // can tell them apart — see the module comment.
    let exp = claims
        .get("exp")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| refuse("no exp claim"))?;
    let now = chrono::Utc::now().timestamp();
    if exp <= now {
        tracing::debug!(
            reason = "expired",
            exp,
            now,
            "data-api serving token refused"
        );
        return Err(ServingError::Unauthorized);
    }

    // The signature proves the token came from egret. It does not prove the
    // token was meant for the Data API. See the module comment.
    //
    // The `aud` is logged because it is the one field that says WHICH egret
    // surface the token was minted for, and it is signature-verified by this
    // point, so it is egret's own string rather than caller-controlled input.
    let aud = claims.get("aud").and_then(|v| v.as_str());
    if aud != Some(DATA_API_AUDIENCE) {
        tracing::debug!(
            reason = "wrong audience",
            aud = ?aud,
            "data-api serving token refused"
        );
        return Err(ServingError::Unauthorized);
    }

    // Strict, from here down. A missing or unparseable claim is a refusal — a
    // `unwrap_or_default()` would give every malformed token `Uuid::nil()` as its
    // workspace and principal, i.e. one shared tenant that no real row belongs
    // to but that every such token agrees on.
    //
    // The claim NAME is logged, never its value: the name is what an operator
    // needs to find the mis-minting verifier, and the value is a bearer token's
    // contents.
    let workspace_id = claims
        .get("ws")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| refuse("ws claim missing or not a uuid"))?;
    let user_id = claims
        .get("egret_user_id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
        .map(UserId)
        .ok_or_else(|| refuse("egret_user_id claim missing or not a uuid"))?;
    let client_id = claims
        .get("sub")
        .and_then(|v| v.as_str())
        // `trim`, not just `is_empty`: a whitespace-only `sub` is not a client
        // id, and it would otherwise become one in the access log.
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| refuse("sub claim missing or blank"))?
        .to_string();

    Ok(ServingPrincipal {
        user_id,
        workspace_id,
        client_id,
    })
}
