//! OIDC login/callback routes. Mounted only when OIDC is enabled.

use axum::extract::{FromRef, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use axum_extra::extract::cookie::{CookieJar, Key, PrivateCookieJar};
use egret_config::Config;
use serde::Deserialize;

use crate::cookie::{csrf_cookie, session_cookie};
use crate::groups::GroupRepo;
use crate::oidc::{provision_and_sync, OidcService};
use crate::oidc_cookie::{
    build_state_cookie, clear_state_cookie, OidcStateCookie, OIDC_STATE_COOKIE,
};
use crate::session::SessionService;
use crate::token::generate_token;
use crate::users::UserRepo;

#[derive(Clone)]
pub struct OidcRoutesState {
    pub oidc: OidcService,
    pub users: UserRepo,
    pub groups: GroupRepo,
    pub sessions: SessionService,
    pub config: Config,
    pub cookie_key: Key,
}

impl FromRef<OidcRoutesState> for Key {
    fn from_ref(s: &OidcRoutesState) -> Self {
        s.cookie_key.clone()
    }
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    pub code: String,
    pub state: String,
}

pub fn oidc_router() -> Router<OidcRoutesState> {
    Router::new()
        .route("/login", get(oidc_login))
        .route("/callback", get(oidc_callback))
}

async fn oidc_login(State(st): State<OidcRoutesState>, jar: PrivateCookieJar) -> Response {
    let start = st.oidc.start();
    let cookie = build_state_cookie(
        &OidcStateCookie {
            state: start.state,
            nonce: start.nonce,
            pkce_verifier: start.pkce_verifier,
        },
        st.config.session.secure,
    );
    let jar = jar.add(cookie);
    (jar, Redirect::to(&start.authorize_url)).into_response()
}

/// Build the successful OIDC login response.
///
/// - `private_jar`: removes the short-lived state cookie (it was encrypted in `PrivateCookieJar`).
/// - A plain `CookieJar` carries `egret_session` and `egret_csrf` as **plaintext** so that
///   `SessionService::validate()` (which sha-256-hashes the raw cookie value) can find the
///   session row. The local-login route sets these the same way.
pub(crate) fn login_response(
    private_jar: PrivateCookieJar,
    session_token: String,
    csrf: String,
    secure: bool,
    max_age_secs: i64,
    redirect: &str,
) -> Response {
    let private_jar = private_jar.remove(clear_state_cookie());
    let plain_jar = CookieJar::new()
        .add(session_cookie(session_token, secure, max_age_secs))
        .add(csrf_cookie(csrf, secure, max_age_secs));
    (private_jar, plain_jar, Redirect::to(redirect)).into_response()
}

async fn oidc_callback(
    State(st): State<OidcRoutesState>,
    jar: PrivateCookieJar,
    Query(q): Query<CallbackQuery>,
) -> Response {
    let Some(raw) = jar.get(OIDC_STATE_COOKIE) else {
        return (
            jar.remove(clear_state_cookie()),
            (StatusCode::BAD_REQUEST, "missing oidc state"),
        )
            .into_response();
    };
    let Ok(state): Result<OidcStateCookie, _> = serde_json::from_str(raw.value()) else {
        return (
            jar.remove(clear_state_cookie()),
            (StatusCode::BAD_REQUEST, "bad oidc state"),
        )
            .into_response();
    };
    if state.state != q.state {
        return (
            jar.remove(clear_state_cookie()),
            (StatusCode::BAD_REQUEST, "state mismatch"),
        )
            .into_response();
    }

    let claims = match st
        .oidc
        .finish(&q.code, &state.pkce_verifier, &state.nonce)
        .await
    {
        Ok(c) => c,
        Err(_) => {
            return (
                jar.remove(clear_state_cookie()),
                (StatusCode::UNAUTHORIZED, "oidc exchange failed"),
            )
                .into_response()
        }
    };
    let user_id = match provision_and_sync(&st.users, &st.groups, &claims).await {
        Ok(id) => id,
        Err(egret_core::Error::Conflict(_)) => {
            return (
                jar.remove(clear_state_cookie()),
                (
                    StatusCode::CONFLICT,
                    "email already used by another account",
                ),
            )
                .into_response()
        }
        Err(_) => {
            return (
                jar.remove(clear_state_cookie()),
                (StatusCode::INTERNAL_SERVER_ERROR, "provisioning failed"),
            )
                .into_response()
        }
    };
    let issued = match st.sessions.create(user_id, "", "").await {
        Ok(i) => i,
        Err(_) => {
            return (
                jar.remove(clear_state_cookie()),
                (StatusCode::INTERNAL_SERVER_ERROR, "session failed"),
            )
                .into_response()
        }
    };

    let secure = st.config.session.secure;
    let max_age = st.config.session.absolute_ttl_hours as i64 * 3600;
    let csrf = generate_token();
    login_response(
        jar,
        issued.token,
        csrf,
        secure,
        max_age,
        &st.config.oidc.post_login_redirect,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cookie::SESSION_COOKIE;

    /// Regression test: after OIDC login the `egret_session` cookie must be **plaintext**,
    /// not an encrypted value produced by `PrivateCookieJar`.
    ///
    /// Strategy: build a response via `login_response`, pull raw `Set-Cookie` headers, find
    /// the `egret_session` one, and assert its value equals the known plaintext token.
    /// A `PrivateCookieJar` would encrypt the value (base64-encoded ciphertext ≠ token),
    /// causing this assertion to fail.
    #[test]
    fn oidc_session_cookie_is_plaintext_not_encrypted() {
        let key = Key::generate();
        let private_jar = PrivateCookieJar::new(key);

        let session_token = "known-session-token-abc123".to_string();
        let csrf_token = "known-csrf-xyz789".to_string();

        let response = login_response(
            private_jar,
            session_token.clone(),
            csrf_token.clone(),
            false,
            3600,
            "/app",
        );

        // Collect all Set-Cookie headers from the response.
        let set_cookies: Vec<String> = response
            .headers()
            .get_all("set-cookie")
            .iter()
            .map(|v| v.to_str().expect("ascii header").to_string())
            .collect();

        // Find the egret_session cookie header.
        let session_header = set_cookies
            .iter()
            .find(|h| h.starts_with(SESSION_COOKIE))
            .unwrap_or_else(|| {
                panic!(
                    "No set-cookie header starting with '{}' found in: {:?}",
                    SESSION_COOKIE, set_cookies
                )
            });

        // The value portion is between '=' and the first ';'.
        let value_part = session_header
            .trim_start_matches(SESSION_COOKIE)
            .trim_start_matches('=');
        let cookie_value = value_part.split(';').next().unwrap_or("").trim();

        assert_eq!(
            cookie_value, session_token,
            "egret_session cookie must be plaintext (not encrypted). \
             Got '{cookie_value}', expected '{session_token}'. \
             This would happen if session_cookie() was added to PrivateCookieJar instead of CookieJar."
        );
    }
}
