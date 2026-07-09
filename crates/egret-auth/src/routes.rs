//! /api/auth routes: login (local), logout, me.

use std::sync::OnceLock;

use axum::extract::{FromRef, State};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::cookie::CookieJar;
use egret_config::Config;
use egret_core::http::ErrorBody;
use egret_core::{Ack, Error};
use serde::{Deserialize, Serialize};

use crate::cookie::{clear_cookie, csrf_cookie, session_cookie, CSRF_COOKIE, SESSION_COOKIE};
use crate::password::verify_password;
use crate::principal::{Auth, AuthError};
use crate::ratelimit::RateLimiter;
use crate::session::SessionService;
use crate::token::generate_token;
use crate::users::UserRepo;

static DUMMY_HASH: OnceLock<String> = OnceLock::new();

#[derive(Clone)]
pub struct AuthState {
    pub users: UserRepo,
    pub sessions: SessionService,
    pub config: Config,
    pub rate_limiter: RateLimiter,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct MeResponse {
    pub user_id: String,
    pub email: String,
    pub platform_roles: Vec<String>,
    pub permissions: Vec<String>,
}

/// Pre-login capability probe returned by `GET /api/auth/config`: tells the
/// front-end whether the OIDC "Sign in with SSO" button should be shown.
#[derive(Serialize, utoipa::ToSchema)]
pub struct AuthConfig {
    pub oidc_enabled: bool,
}

/// Build the auth router. `login` and `config` are CSRF-exempt (client has no cookie yet);
/// `logout` and `me` are behind `csrf_layer` (axum layers routes added before them).
pub fn auth_router() -> Router<AuthState> {
    let protected = Router::new()
        .route("/logout", post(logout))
        .route("/me", get(me))
        .layer(axum::middleware::from_fn(crate::csrf::csrf_layer));
    Router::new()
        .route("/login", post(login)) // CSRF-exempt
        .route("/config", get(auth_config)) // CSRF-exempt, pre-login
        .merge(protected)
}

#[utoipa::path(
    get,
    path = "/api/auth/config",
    tag = "auth",
    responses(
        (status = 200, description = "Pre-login capability probe (is OIDC enabled?)", body = AuthConfig),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn auth_config(State(st): State<AuthState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "oidc_enabled": st.config.oidc.enabled }))
}

#[utoipa::path(
    post,
    path = "/api/auth/login",
    tag = "auth",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Logged in; sets session + CSRF cookies", body = MeResponse),
        (status = 401, description = "Invalid email or password", body = ErrorBody),
        (status = 429, description = "Too many login attempts", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn login(
    State(st): State<AuthState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(body): Json<LoginRequest>,
) -> Result<impl IntoResponse, AuthError> {
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .unwrap_or("")
        .trim()
        .to_string();
    let rl_key = format!("{ip}:{}", body.email.to_lowercase());
    if !st.rate_limiter.check(&rl_key) {
        return Err(AuthError(Error::TooManyRequests));
    }

    let dummy =
        DUMMY_HASH.get_or_init(|| crate::password::hash_password("not-a-real-password").unwrap());
    let user = st
        .users
        .find_by_email(&body.email)
        .await
        .map_err(AuthError)?;
    let user = match user {
        Some(u) if u.status == "active" && u.password_hash.is_some() => u,
        _ => {
            // Equalize timing for unknown/disabled/no-password users.
            let _ = crate::password::verify_password(&body.password, dummy);
            return Err(AuthError(Error::Unauthorized));
        }
    };
    let hash = user.password_hash.as_deref().unwrap();
    if !verify_password(&body.password, hash).map_err(AuthError)? {
        return Err(AuthError(Error::Unauthorized));
    }

    let issued = st
        .sessions
        .create(user.id, "", &ip)
        .await
        .map_err(AuthError)?;
    st.users
        .touch_last_login(user.id)
        .await
        .map_err(AuthError)?;

    let secure = st.config.session.secure;
    let max_age = st.config.session.absolute_ttl_hours as i64 * 3600;
    let csrf = generate_token();
    let jar = jar
        .add(session_cookie(issued.token, secure, max_age))
        .add(csrf_cookie(csrf, secure, max_age));

    Ok((
        jar,
        Json(MeResponse {
            user_id: user.id.to_string(),
            email: user.email,
            platform_roles: vec![],
            permissions: vec![],
        }),
    ))
}

#[utoipa::path(
    post,
    path = "/api/auth/logout",
    tag = "auth",
    responses(
        (status = 200, description = "Session revoked; clears session + CSRF cookies", body = Ack),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn logout(
    State(st): State<AuthState>,
    jar: CookieJar,
) -> Result<impl IntoResponse, AuthError> {
    if let Some(c) = jar.get(SESSION_COOKIE) {
        st.sessions.revoke(c.value()).await.map_err(AuthError)?;
    }
    let jar = jar
        .add(clear_cookie(SESSION_COOKIE))
        .add(clear_cookie(CSRF_COOKIE));
    Ok((jar, Json(serde_json::json!({ "ok": true }))))
}

#[utoipa::path(
    get,
    path = "/api/auth/me",
    tag = "auth",
    responses(
        (status = 200, description = "The authenticated caller's identity + permissions", body = MeResponse),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn me(Auth(principal): Auth) -> impl IntoResponse {
    Json(MeResponse {
        user_id: principal.user_id.to_string(),
        email: principal.email,
        platform_roles: principal.platform_roles,
        permissions: principal.permissions,
    })
}

// Let the Principal extractor pull SessionService from AuthState.
impl FromRef<AuthState> for SessionService {
    fn from_ref(st: &AuthState) -> SessionService {
        st.sessions.clone()
    }
}

/// OpenAPI fragment for the local-auth router (`/api/auth`). Merged into the
/// Auth seam's `egret_auth::openapi()` (E0·P4b-1).
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(login, auth_config, logout, me),
    components(schemas(LoginRequest, MeResponse, AuthConfig, Ack, ErrorBody)),
    tags((name = "auth", description = "Local login/logout, session identity, and the pre-login config probe")),
)]
pub struct AuthApi;
