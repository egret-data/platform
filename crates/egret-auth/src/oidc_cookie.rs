//! Encrypted, short-lived cookie carrying OIDC state/nonce/pkce between login and callback.

use axum_extra::extract::cookie::{Cookie, SameSite};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const OIDC_STATE_COOKIE: &str = "egret_oidc_state";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcStateCookie {
    pub state: String,
    pub nonce: String,
    pub pkce_verifier: String,
    /// Present only when this authorize round-trip is an **impersonation step-up**
    /// (Task 8): the admin re-authenticates with the IdP (`prompt=login`) to prove
    /// a fresh identity before an overlay is created. The intent rides inside this
    /// cookie, which `PrivateCookieJar` seals with authenticated encryption — so it
    /// is exactly as tamper-proof as the `state`/`nonce`/PKCE it travels with. An
    /// attacker cannot forge an intent (e.g. a target of their choosing) without the
    /// server's cookie key. `None` for an ordinary SSO login.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impersonate: Option<ImpersonateIntent>,
}

/// The impersonation intent carried across the OIDC step-up redirect (spec §8).
/// Sealed inside [`OidcStateCookie`]'s encrypted cookie; the callback trusts it
/// only after (a) verifying the freshly-authenticated user equals
/// [`Self::impersonator_id`] and (b) re-running the target guards.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpersonateIntent {
    /// The admin (local user id) who initiated the step-up. The callback requires
    /// the freshly-authenticated user to match this exactly, so a *different* IdP
    /// login cannot complete someone else's impersonation.
    pub impersonator_id: Uuid,
    /// Who the admin intends to act as.
    pub target_user_id: Uuid,
    /// The audit reason captured at initiation.
    pub reason: String,
    /// `None` = platform scope; `Some(ws)` = workspace-scoped (used by the Task 9
    /// ws_admin entry point). Encodes the spec's `scope` discriminant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<Uuid>,
}

/// Build the (plaintext) cookie to be added to a `PrivateCookieJar` (which encrypts it).
pub fn build_state_cookie(value: &OidcStateCookie, secure: bool) -> Cookie<'static> {
    let json = serde_json::to_string(value).expect("serialize oidc state");
    Cookie::build((OIDC_STATE_COOKIE, json))
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(secure)
        .path("/api/auth/oidc")
        .max_age(time::Duration::seconds(600))
        .build()
}

pub fn clear_state_cookie() -> Cookie<'static> {
    Cookie::build((OIDC_STATE_COOKIE, ""))
        .path("/api/auth/oidc")
        .max_age(time::Duration::seconds(0))
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_roundtrips_through_json() {
        let v = OidcStateCookie {
            state: "s".into(),
            nonce: "n".into(),
            pkce_verifier: "p".into(),
            impersonate: None,
        };
        let c = build_state_cookie(&v, true);
        let back: OidcStateCookie = serde_json::from_str(c.value()).unwrap();
        assert_eq!(back.nonce, "n");
        assert!(back.impersonate.is_none());
        assert_eq!(c.http_only(), Some(true));
        assert_eq!(c.path(), Some("/api/auth/oidc"));
    }
}
