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
use crate::impersonation::{ImpersonationRepo, NewImpersonation};
use crate::password::verify_password;
use crate::principal::{Auth, AuthError};
use crate::ratelimit::RateLimiter;
use crate::session::SessionService;
use crate::token::generate_token;
use crate::users::UserRepo;
use egret_audit::{AuditEvent, AuditSink};
use egret_core::UserId;

static DUMMY_HASH: OnceLock<String> = OnceLock::new();

#[derive(Clone)]
pub struct AuthState {
    pub users: UserRepo,
    pub sessions: SessionService,
    pub config: Config,
    pub rate_limiter: RateLimiter,
    pub audit: AuditSink,
    pub impersonations: ImpersonationRepo,
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
    /// Present when the caller is currently impersonating another user (an active
    /// admin overlay resolved to the target). `null` for an ordinary session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub impersonating: Option<ImpersonationDto>,
}

/// A minimal user reference (id + email) for the impersonation state on `/me`.
#[derive(Serialize, utoipa::ToSchema)]
pub struct UserRef {
    pub user_id: String,
    pub email: String,
}

/// The active impersonation as surfaced by `/me`: who is being acted-as
/// (`target`), the admin behind it (`impersonator`), and the overlay's bounds.
#[derive(Serialize, utoipa::ToSchema)]
pub struct ImpersonationDto {
    pub target: UserRef,
    pub impersonator: UserRef,
    pub workspace_scope: Option<String>,
    pub expires_at: String,
}

/// Body for `POST /api/admin/impersonate`.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct StartImpersonation {
    pub target_user_id: String,
    pub reason: String,
    /// Required for a local-auth admin (step-up). OIDC admins re-auth via OIDC (Task 8).
    pub password: Option<String>,
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
        .route("/impersonation/exit", post(impersonate_exit))
        .layer(axum::middleware::from_fn(crate::csrf::csrf_layer));
    Router::new()
        .route("/login", post(login)) // CSRF-exempt
        .route("/config", get(auth_config)) // CSRF-exempt, pre-login
        .merge(protected)
}

/// Platform-admin impersonation start endpoint, mounted at `/api/admin` so the
/// full path is `POST /api/admin/impersonate`. Behind CSRF (mutation) + `Auth`.
pub fn admin_impersonation_router() -> Router<AuthState> {
    Router::new()
        .route("/impersonate", post(impersonate_start))
        .layer(axum::middleware::from_fn(crate::csrf::csrf_layer))
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
            impersonating: None,
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
    let impersonating = principal.impersonator.as_ref().map(|i| ImpersonationDto {
        target: UserRef {
            user_id: principal.user_id.to_string(),
            email: principal.email.clone(),
        },
        impersonator: UserRef {
            user_id: i.user_id.to_string(),
            email: i.email.clone(),
        },
        workspace_scope: i.workspace_scope.map(|w| w.0.to_string()),
        expires_at: i.expires_at.to_rfc3339(),
    });
    Json(MeResponse {
        user_id: principal.user_id.to_string(),
        email: principal.email,
        platform_roles: principal.platform_roles,
        permissions: principal.permissions,
        impersonating,
    })
}

/// Resolve THIS request's `sessions.id` from the session cookie. The overlay is
/// keyed on the admin's own session row, so start/exit must operate on it.
async fn session_id_from_jar(st: &AuthState, jar: &CookieJar) -> Result<uuid::Uuid, AuthError> {
    let token = jar
        .get(SESSION_COOKIE)
        .map(|c| c.value().to_string())
        .ok_or(AuthError(Error::Unauthorized))?;
    st.sessions
        .session_id_for_token(&token)
        .await
        .map_err(AuthError)?
        .ok_or(AuthError(Error::Unauthorized))
}

#[utoipa::path(
    post,
    path = "/api/admin/impersonate",
    tag = "auth",
    request_body = StartImpersonation,
    responses(
        (status = 200, description = "Impersonation started; the session now resolves to the target"),
        (status = 400, description = "Bad target / empty reason / OIDC step-up required", body = ErrorBody),
        (status = 401, description = "Not authenticated, or wrong step-up password", body = ErrorBody),
        (status = 403, description = "Not a platform admin, already impersonating, or target is an admin", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn impersonate_start(
    State(st): State<AuthState>,
    Auth(p): Auth,
    jar: CookieJar,
    Json(body): Json<StartImpersonation>,
) -> Result<impl IntoResponse, AuthError> {
    // 1. No nesting: an active overlay session cannot start another. This is
    //    checked BEFORE anything reads the target, so a non-admin (or a caller
    //    already impersonating) learns nothing about the target.
    if p.impersonator.is_some() {
        return Err(AuthError(Error::Forbidden));
    }
    // 2. Must be a platform admin.
    if !p.platform_roles.iter().any(|r| r == "platform_admin") {
        return Err(AuthError(Error::Forbidden));
    }
    // 3. Parse + validate inputs.
    let target = UserId(
        body.target_user_id
            .parse::<uuid::Uuid>()
            .map_err(|_| AuthError(Error::InvalidInput("invalid target_user_id".into())))?,
    );
    if target == p.user_id {
        return Err(AuthError(Error::InvalidInput(
            "cannot impersonate yourself".into(),
        )));
    }
    let reason = body.reason.trim().to_string();
    if reason.is_empty() {
        return Err(AuthError(Error::InvalidInput("reason is required".into())));
    }
    // 4. Step-up re-authentication of the ADMIN. Local admins re-enter their
    //    password; OIDC admins are rejected here (Task 8 adds OIDC step-up).
    let admin = st
        .users
        .find_by_id(p.user_id)
        .await
        .map_err(AuthError)?
        .ok_or(AuthError(Error::Unauthorized))?;
    if admin.provider == "local" {
        let hash = admin
            .password_hash
            .as_deref()
            .ok_or(AuthError(Error::Unauthorized))?;
        let ok =
            verify_password(body.password.as_deref().unwrap_or(""), hash).map_err(AuthError)?;
        if !ok {
            return Err(AuthError(Error::Unauthorized));
        }
    } else {
        return Err(AuthError(Error::InvalidInput(
            "OIDC step-up required".into(),
        )));
    }
    // 5. Target must exist and must NOT itself be a platform admin.
    let _target_user = st
        .users
        .find_by_id(target)
        .await
        .map_err(AuthError)?
        .ok_or(AuthError(Error::InvalidInput("unknown target".into())))?;
    let target_roles = st
        .sessions
        .platform_roles_of(target)
        .await
        .map_err(AuthError)?;
    if target_roles.iter().any(|r| r == "platform_admin") {
        return Err(AuthError(Error::Forbidden));
    }
    // 6. Write the overlay on THIS admin session (step-up already passed, so a
    //    wrong password left zero rows). Map a slipped-through nesting Conflict
    //    to Forbidden.
    let sid = session_id_from_jar(&st, &jar).await?;
    st.impersonations
        .start(NewImpersonation {
            admin_session_id: sid,
            impersonator_id: p.user_id,
            target_user_id: target,
            workspace_scope: None,
            reason: reason.clone(),
            ttl_minutes: 30,
        })
        .await
        .map_err(|e| match e {
            Error::Conflict(_) => AuthError(Error::Forbidden),
            other => AuthError(other),
        })?;
    // 7. Audit both parties.
    st.audit.emit(AuditEvent::Mutation {
        actor: Some(p.user_id.0),
        impersonator: Some(p.user_id.0),
        workspace_id: None,
        action: "impersonation.start".into(),
        resource_type: "user".into(),
        resource_id: Some(target.0.to_string()),
        detail: serde_json::json!({ "reason": reason, "scope": "platform" }),
    });
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    post,
    path = "/api/auth/impersonation/exit",
    tag = "auth",
    responses(
        (status = 200, description = "Impersonation ended; the session returns to the admin", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn impersonate_exit(
    State(st): State<AuthState>,
    Auth(p): Auth,
    jar: CookieJar,
) -> Result<impl IntoResponse, AuthError> {
    let sid = session_id_from_jar(&st, &jar).await?;
    st.impersonations.exit(sid).await.map_err(AuthError)?;
    // At exit time the principal is still the TARGET carrying the admin as
    // `impersonator`; record the admin as the actor for the end event.
    st.audit.emit(AuditEvent::Mutation {
        actor: p.impersonator.as_ref().map(|i| i.user_id.0),
        impersonator: p.impersonator.as_ref().map(|i| i.user_id.0),
        workspace_id: None,
        action: "impersonation.end".into(),
        resource_type: "user".into(),
        resource_id: Some(p.user_id.0.to_string()),
        detail: serde_json::json!({ "scope": "platform" }),
    });
    Ok(Json(serde_json::json!({ "ok": true })))
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
    paths(login, auth_config, logout, me, impersonate_start, impersonate_exit),
    components(schemas(
        LoginRequest,
        MeResponse,
        AuthConfig,
        UserRef,
        ImpersonationDto,
        StartImpersonation,
        Ack,
        ErrorBody
    )),
    tags((name = "auth", description = "Local login/logout, session identity, and the pre-login config probe")),
)]
pub struct AuthApi;
