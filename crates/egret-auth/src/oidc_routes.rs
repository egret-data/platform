//! OIDC login/callback routes. Mounted only when OIDC is enabled.

use axum::extract::{FromRef, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
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
        .route("/backchannel-logout", post(backchannel_logout))
}

/// OpenAPI fragment for the OIDC login/callback/back-channel-logout router
/// (`/api/auth/oidc`). The login/callback endpoints are browser-facing redirects
/// (303) whose error paths return `text/plain` bodies — not the JSON `ErrorBody`
/// envelope — so no response schemas are declared. Merged into
/// `egret_auth::openapi()` (E0·P4b-1).
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(oidc_login, oidc_callback, backchannel_logout),
    tags((name = "oidc", description = "OIDC single-sign-on login initiation and provider callback")),
)]
pub struct OidcApi;

#[utoipa::path(
    get,
    path = "/api/auth/oidc/login",
    tag = "oidc",
    responses(
        (status = 303, description = "Redirect to the OIDC provider's authorize endpoint (sets the encrypted state cookie)"),
    ),
)]
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

#[utoipa::path(
    get,
    path = "/api/auth/oidc/callback",
    tag = "oidc",
    params(
        ("code" = String, Query, description = "Authorization code from the OIDC provider"),
        ("state" = String, Query, description = "Opaque state value echoed back for CSRF protection"),
    ),
    responses(
        (status = 303, description = "Login succeeded; sets session + CSRF cookies and redirects to the post-login page"),
        (status = 400, description = "Missing, malformed, or mismatched OIDC state cookie (text/plain body)"),
        (status = 401, description = "OIDC code exchange failed (text/plain body)"),
        (status = 409, description = "Email already used by another account (text/plain body)"),
        (status = 500, description = "Provisioning or session failure (text/plain body)"),
    ),
)]
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

    let login = match st
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
    let user_id = match provision_and_sync(&st.users, &st.groups, &login.claims).await {
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

#[derive(serde::Deserialize, utoipa::ToSchema)]
struct BackchannelLogoutForm {
    logout_token: String,
}

/// `POST /api/auth/oidc/backchannel-logout` — OIDC Back-Channel Logout 1.0.
///
/// Keycloak (or any conformant IdP) calls this server-to-server when a user logs
/// out or their session ends upstream. There is no cookie and no CSRF token on
/// this request — it's secured entirely by the logout token's JWS signature
/// (verified against the provider's JWKS, `iss`/`aud`/`exp` enforced) — see
/// [`crate::oidc::verify_logout_token`]. On success, every egret session for the
/// token's `sub` is revoked. An unrecognized `sub` (e.g. a user who never
/// completed OIDC login) is a no-op 200, matching the spec's guidance that the
/// RP should not leak whether it knows a given subject. Always sets
/// `Cache-Control: no-store` per the Back-Channel Logout spec.
#[utoipa::path(
    post,
    path = "/api/auth/oidc/backchannel-logout",
    tag = "oidc",
    request_body(content = BackchannelLogoutForm, content_type = "application/x-www-form-urlencoded"),
    responses(
        (status = 200, description = "Logout token accepted; matching sessions (if any) revoked"),
        (status = 400, description = "Missing/invalid logout token, or JWKS unavailable"),
    ),
)]
async fn backchannel_logout(
    State(st): State<OidcRoutesState>,
    axum::Form(form): axum::Form<BackchannelLogoutForm>,
) -> Response {
    use axum::http::header;
    let no_store = [(header::CACHE_CONTROL, "no-store")];
    let jwks = match st.oidc.provider_jwks().await {
        Ok(j) => j,
        Err(_) => return (StatusCode::BAD_REQUEST, no_store).into_response(),
    };
    let sub = match crate::oidc::verify_logout_token(
        &jwks,
        st.oidc.issuer(),
        st.oidc.client_id(),
        &form.logout_token,
    ) {
        Ok(s) => s,
        Err(_) => return (StatusCode::BAD_REQUEST, no_store).into_response(),
    };
    // Best-effort revoke: a lookup/revoke failure must not leak (still 200), but
    // a real DB error would otherwise be invisible, so log it at warn.
    match st.users.find_by_external_subject(&sub).await {
        Ok(Some(user)) => {
            if let Err(e) = st.sessions.revoke_all_for_user(user.id).await {
                tracing::warn!(error = %e, user_id = %user.id, "backchannel-logout: session revoke failed");
            }
        }
        Ok(None) => {}
        Err(e) => {
            tracing::warn!(error = %e, "backchannel-logout: user lookup failed");
        }
    }
    (StatusCode::OK, no_store).into_response()
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
