//! Encrypted, short-lived cookie carrying OIDC state/nonce/pkce between login and callback.

use axum_extra::extract::cookie::{Cookie, SameSite};
use serde::{Deserialize, Serialize};

pub const OIDC_STATE_COOKIE: &str = "egret_oidc_state";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcStateCookie {
    pub state: String,
    pub nonce: String,
    pub pkce_verifier: String,
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
        };
        let c = build_state_cookie(&v, true);
        let back: OidcStateCookie = serde_json::from_str(c.value()).unwrap();
        assert_eq!(back.nonce, "n");
        assert_eq!(c.http_only(), Some(true));
        assert_eq!(c.path(), Some("/api/auth/oidc"));
    }
}
