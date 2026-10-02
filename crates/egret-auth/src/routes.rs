//! /api/auth routes: login (local), logout, me.

use std::sync::OnceLock;

use axum::extract::{FromRef, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::cookie::{CookieJar, PrivateCookieJar};
use egret_config::Config;
use egret_core::http::ErrorBody;
use egret_core::ratelimit::{client_ip, RateLimiter};
use egret_core::{Ack, Error};
use serde::{Deserialize, Serialize};

use crate::cookie::{
    clear_cookie, csrf_cookie, read_session, secure_cookies, session_cookie, CSRF_COOKIE,
    HOST_CSRF_COOKIE, HOST_SESSION_COOKIE, SESSION_COOKIE,
};
use crate::impersonation::{validate_impersonation_target, ImpersonationRepo, NewImpersonation};
use crate::oidc_cookie::{build_state_cookie, ImpersonateIntent, OidcStateCookie};
use crate::password::verify_password;
use crate::principal::{Auth, AuthError};
use crate::session::SessionService;
use crate::token::generate_token;
use crate::users::UserRepo;
use egret_audit::{AuditEvent, AuditSink};
use egret_core::{UserId, WorkspaceId};

static DUMMY_HASH: OnceLock<String> = OnceLock::new();

#[derive(Clone)]
pub struct AuthState {
    pub users: UserRepo,
    pub sessions: SessionService,
    pub config: Config,
    pub rate_limiter: RateLimiter,
    pub audit: AuditSink,
    pub impersonations: ImpersonationRepo,
    /// The OIDC service, present only when SSO is enabled. Used by the
    /// impersonation start endpoint to build the step-up authorize URL for an
    /// OIDC-provider admin (Task 8). `None` on non-OIDC deploys and in unit tests.
    pub oidc: Option<crate::oidc::OidcService>,
    /// The cookie-encryption key, present only when OIDC is enabled. Needed to
    /// seal the impersonation intent into the same `PrivateCookieJar`-encrypted
    /// state cookie the normal login flow uses. `None` when `oidc` is `None`.
    pub cookie_key: Option<axum_extra::extract::cookie::Key>,
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

/// `409` body returned when an OIDC-provider admin starts an impersonation: the
/// client must redirect the browser to `authorize_url` to complete a fresh IdP
/// re-authentication (`prompt=login`) before the overlay is created (Task 8). The
/// `code` lets the SPA branch on this specific response.
#[derive(Serialize, utoipa::ToSchema)]
pub struct OidcStepUp {
    /// Always `"oidc_step_up"`.
    pub code: String,
    /// The OIDC authorize URL (carries the sealed impersonation intent in `state`).
    pub authorize_url: String,
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
    let ip = client_ip(&headers);
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
    if let Some(c) = read_session(&jar, secure_cookies()) {
        st.sessions.revoke(c.value()).await.map_err(AuthError)?;
    }
    // Both spellings are cleared: a browser that logged in before `session.secure` was
    // switched on still holds the plain pair.
    let jar = jar
        .add(clear_cookie(SESSION_COOKIE))
        .add(clear_cookie(CSRF_COOKIE))
        .add(clear_cookie(HOST_SESSION_COOKIE))
        .add(clear_cookie(HOST_CSRF_COOKIE));
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
    let token = read_session(jar, secure_cookies())
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
        (status = 400, description = "Bad target / empty reason", body = ErrorBody),
        (status = 401, description = "Not authenticated, or wrong step-up password", body = ErrorBody),
        (status = 403, description = "Not a platform admin, already impersonating, or target is an admin", body = ErrorBody),
        (status = 409, description = "OIDC admin: re-authenticate at `authorize_url`, then the callback creates the overlay", body = OidcStepUp),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn impersonate_start(
    State(st): State<AuthState>,
    Auth(p): Auth,
    jar: CookieJar,
    Json(body): Json<StartImpersonation>,
) -> Result<Response, AuthError> {
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
    // 3. Parse the target and run the SHARED target guards (not-self, reason
    //    non-empty, target exists, target-not-admin). Same guard set the OIDC
    //    callback and the ws_admin start (Task 9) reuse.
    let target = UserId(
        body.target_user_id
            .parse::<uuid::Uuid>()
            .map_err(|_| AuthError(Error::InvalidInput("invalid target_user_id".into())))?,
    );
    let reason = body.reason.trim().to_string();
    validate_impersonation_target(&st.users, &st.sessions, p.user_id, target, &reason)
        .await
        .map_err(AuthError)?;
    // 4. Step-up re-authentication of the ADMIN. A local admin re-enters their
    //    password inline; an OIDC admin must re-authenticate with the IdP, so we
    //    return a 409 carrying the authorize URL (`prompt=login`) with the
    //    impersonation intent sealed into the encrypted state cookie. The overlay
    //    is then created by the OIDC callback (see `oidc_routes::oidc_callback`).
    let admin = st
        .users
        .find_by_id(p.user_id)
        .await
        .map_err(AuthError)?
        .ok_or(AuthError(Error::Unauthorized))?;
    if admin.provider != "local" {
        return impersonate_oidc_step_up(&st, p.user_id, target, reason);
    }
    let hash = admin
        .password_hash
        .as_deref()
        .ok_or(AuthError(Error::Unauthorized))?;
    let ok = verify_password(body.password.as_deref().unwrap_or(""), hash).map_err(AuthError)?;
    if !ok {
        return Err(AuthError(Error::Unauthorized));
    }
    // 5. Write the overlay on THIS admin session (step-up already passed, so a
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
    // 6. Audit both parties.
    st.audit.emit(AuditEvent::Mutation {
        actor: Some(p.user_id.0),
        impersonator: Some(p.user_id.0),
        workspace_id: None,
        action: "impersonation.start".into(),
        resource_type: "user".into(),
        resource_id: Some(target.0.to_string()),
        detail: serde_json::json!({ "reason": reason, "scope": "platform" }),
    });
    Ok(Json(serde_json::json!({ "ok": true })).into_response())
}

/// Build the OIDC step-up `409` for an OIDC-provider admin: seal the impersonation
/// intent into the same `PrivateCookieJar`-encrypted state cookie the login flow
/// uses, and hand the client an authorize URL with `prompt=login`. No overlay is
/// written here — that happens in the OIDC callback once the admin has proven a
/// fresh identity AND been confirmed to be the same admin who initiated.
fn impersonate_oidc_step_up(
    st: &AuthState,
    impersonator_id: UserId,
    target: UserId,
    reason: String,
) -> Result<Response, AuthError> {
    let (Some(oidc), Some(key)) = (st.oidc.as_ref(), st.cookie_key.as_ref()) else {
        // OIDC admin on a deploy without OIDC wired: nothing can complete the
        // step-up, so this is a server misconfiguration, not a client error.
        return Err(AuthError(Error::Internal(anyhow::anyhow!(
            "OIDC step-up requested but OIDC is not configured"
        ))));
    };
    Ok(build_impersonation_step_up(
        oidc,
        key,
        st.config.session.secure,
        impersonator_id,
        target,
        reason,
        None,
    ))
}

/// Build the OIDC step-up `409` response carrying a sealed impersonation intent.
/// `workspace_id` is `None` for a platform-scoped step-up and `Some(ws)` for a
/// ws_admin-scoped one (Task 9). No overlay is written here — the OIDC callback
/// creates it only after a fresh IdP re-authentication proves the same admin's
/// identity AND re-checks the required authority (platform_admin, or `ws_admin` on
/// `ws`). The intent rides inside the `PrivateCookieJar`-sealed state cookie, so a
/// client cannot forge a target or a scope without the server's cookie key.
///
/// Shared (`pub`) so the ws_admin start endpoint in egret-workspace can emit the
/// identical 409 without duplicating the sealing logic across crates.
#[allow(clippy::too_many_arguments)]
pub fn build_impersonation_step_up(
    oidc: &crate::oidc::OidcService,
    cookie_key: &axum_extra::extract::cookie::Key,
    secure: bool,
    impersonator_id: UserId,
    target: UserId,
    reason: String,
    workspace_id: Option<WorkspaceId>,
) -> Response {
    let start = oidc.start_step_up();
    let cookie = build_state_cookie(
        &OidcStateCookie {
            state: start.state,
            nonce: start.nonce,
            pkce_verifier: start.pkce_verifier,
            impersonate: Some(ImpersonateIntent {
                impersonator_id: impersonator_id.0,
                target_user_id: target.0,
                reason,
                workspace_id: workspace_id.map(|w| w.0),
            }),
        },
        secure,
    );
    let jar = PrivateCookieJar::new(cookie_key.clone()).add(cookie);
    (
        StatusCode::CONFLICT,
        jar,
        Json(OidcStepUp {
            code: "oidc_step_up".into(),
            authorize_url: start.authorize_url,
        }),
    )
        .into_response()
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
        OidcStepUp,
        Ack,
        ErrorBody
    )),
    tags((name = "auth", description = "Local login/logout, session identity, and the pre-login config probe")),
)]
pub struct AuthApi;
