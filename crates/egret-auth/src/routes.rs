//! /api/auth routes: login (local), logout, me.

use axum::extract::{FromRef, State};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::cookie::CookieJar;
use egret_config::Config;
use egret_core::Error;
use serde::{Deserialize, Serialize};

use crate::cookie::{clear_cookie, csrf_cookie, session_cookie, CSRF_COOKIE, SESSION_COOKIE};
use crate::password::verify_password;
use crate::principal::{Auth, AuthError};
use crate::session::SessionService;
use crate::token::generate_token;
use crate::users::UserRepo;

#[derive(Clone)]
pub struct AuthState {
    pub users: UserRepo,
    pub sessions: SessionService,
    pub config: Config,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct MeResponse {
    pub user_id: String,
    pub email: String,
    pub platform_roles: Vec<String>,
}

/// Build the auth router. `login` is CSRF-exempt (client has no cookie yet);
/// `logout` and `me` are behind `csrf_layer` (axum layers routes added before them).
pub fn auth_router() -> Router<AuthState> {
    let protected = Router::new()
        .route("/logout", post(logout))
        .route("/me", get(me))
        .layer(axum::middleware::from_fn(crate::csrf::csrf_layer));
    Router::new()
        .route("/login", post(login)) // CSRF-exempt
        .merge(protected)
}

async fn login(
    State(st): State<AuthState>,
    jar: CookieJar,
    Json(body): Json<LoginRequest>,
) -> Result<impl IntoResponse, AuthError> {
    // Uniform error for unknown user vs bad password (no enumeration).
    let user = st
        .users
        .find_by_email(&body.email)
        .await
        .map_err(AuthError)?;
    let user = user.ok_or(AuthError(Error::Unauthorized))?;
    if user.status != "active" {
        return Err(AuthError(Error::Unauthorized));
    }
    let hash = user
        .password_hash
        .as_deref()
        .ok_or(AuthError(Error::Unauthorized))?;
    if !verify_password(&body.password, hash).map_err(AuthError)? {
        return Err(AuthError(Error::Unauthorized));
    }

    let issued = st
        .sessions
        .create(user.id, "", "")
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
        }),
    ))
}

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

async fn me(Auth(principal): Auth) -> impl IntoResponse {
    Json(MeResponse {
        user_id: principal.user_id.to_string(),
        email: principal.email,
        platform_roles: principal.platform_roles,
    })
}

// Let the Principal extractor pull SessionService from AuthState.
impl FromRef<AuthState> for SessionService {
    fn from_ref(st: &AuthState) -> SessionService {
        st.sessions.clone()
    }
}
