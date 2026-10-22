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
use crate::impersonation::{
    validate_impersonation_target, validate_ws_impersonation_target_membership, ImpersonationRepo,
    NewImpersonation,
};
use crate::oidc::{provision_and_sync, OidcService};
use crate::oidc_cookie::{
    build_state_cookie, clear_state_cookie, ImpersonateIntent, OidcStateCookie, OIDC_STATE_COOKIE,
};
use crate::session::SessionService;
use crate::token::generate_token;
use crate::users::UserRepo;
use egret_audit::{AuditEvent, AuditSink};
use egret_core::{UserId, WorkspaceId};

#[derive(Clone)]
pub struct OidcRoutesState {
    pub oidc: OidcService,
    pub users: UserRepo,
    pub groups: GroupRepo,
    pub sessions: SessionService,
    pub config: Config,
    pub cookie_key: Key,
    /// Overlay store — the OIDC callback creates the impersonation here once a
    /// step-up re-auth completes (Task 8).
    pub impersonations: ImpersonationRepo,
    /// Audit sink for the `impersonation.start` event emitted on a completed
    /// step-up.
    pub audit: AuditSink,
    /// Recompiles the engine policies a login's group sync changes (J-F3 Task
    /// 1b) — `egret_perm_compile::PermGroupRecompiler`, wired by `egret-bin`.
    /// Not optional: see `crate::groups`' module doc.
    pub group_recompiler: std::sync::Arc<dyn crate::groups::GroupMembershipRecompiler>,
    /// Turns the workspace pre-registrations for a verified email into
    /// memberships at sign-in (KI-185) — `egret_workspace::members::MemberRepo`,
    /// wired by `egret-bin`. See `crate::preregistration`.
    pub invites: std::sync::Arc<dyn crate::preregistration::PendingMembershipRedeemer>,
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
            impersonate: None,
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
    let user_id =
        match provision_and_sync(&st.users, &st.groups, &login.claims, &*st.group_recompiler).await
        {
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

    // Impersonation step-up (Task 8): this authorize round-trip carried an intent
    // sealed in the (authenticated-encryption) state cookie. Complete the overlay
    // here rather than performing an ordinary login.
    if let Some(intent) = state.impersonate {
        return complete_impersonation_step_up(&st, jar, user_id, intent).await;
    }

    // KI-185: memberships a workspace admin set up for this email before the
    // first sign-in. A failure here must not refuse the sign-in: the
    // pre-registrations stay and the next sign-in redeems them.
    match crate::preregistration::redeem_verified(&*st.invites, user_id, &login.claims).await {
        Ok(redeemed) => {
            for r in redeemed {
                // The admin who added the email made the grant; it takes
                // effect now.
                st.audit.emit(AuditEvent::Mutation {
                    actor: Some(r.invited_by),
                    impersonator: None,
                    workspace_id: Some(r.workspace_id),
                    action: "member:add".into(),
                    resource_type: "grant".into(),
                    resource_id: None,
                    detail: serde_json::json!({
                        "principal_type": "user",
                        "principal_id": user_id.0,
                        "role": r.role,
                        "via": "pre-registration",
                    }),
                });
            }
        }
        Err(e) => tracing::warn!(
            user_id = %user_id.0,
            error = %e,
            "could not redeem workspace pre-registrations; sign-in continues"
        ),
    }

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

/// Complete an OIDC impersonation step-up (Task 8). `authenticated` is the user id
/// `provision_and_sync` resolved from the freshly-verified id_token's `sub`. We:
///
/// 1. require it to equal the admin who INITIATED the step-up (sealed in
///    `intent.impersonator_id`) — a *different* IdP login must not complete
///    someone else's impersonation intent;
/// 2. re-verify the authenticated admin STILL holds the authority the scope
///    requires — the start-time check is baked into the sealed `impersonator_id`,
///    but the role could have been revoked between start and this callback (TOCTOU);
/// 3. re-run the shared target guards (not-self / target-exists / target-not-admin),
///    identical to the platform start endpoint;
/// 4. mint a fresh session for the admin and attach the overlay to it (binding the
///    overlay to a session we KNOW belongs to the just-authenticated admin, rather
///    than trusting a pre-existing session cookie);
/// 5. audit; and
/// 6. redirect into the app carrying that (now-overlaid) session.
///
/// Any failure clears the state cookie and writes NO overlay (and no session).
///
/// `pub` so the callback security paths (same-admin + authority re-check) can be
/// tested at this boundary without a full mock token-exchange; the HTTP callback
/// calls it with the user id `provision_and_sync` resolved from the verified token.
pub async fn complete_impersonation_step_up(
    st: &OidcRoutesState,
    jar: PrivateCookieJar,
    authenticated: UserId,
    intent: ImpersonateIntent,
) -> Response {
    // (1) Same-admin check: the fresh authentication must be the initiator.
    if authenticated.0 != intent.impersonator_id {
        return (
            jar.remove(clear_state_cookie()),
            (StatusCode::FORBIDDEN, "step-up identity mismatch"),
        )
            .into_response();
    }
    // (2) TOCTOU authority re-check: the authority proven at START time is baked
    //     into the sealed `impersonator_id`, but the admin's grant could have been
    //     revoked in the seconds between start and this callback. Re-verify the
    //     authenticated admin STILL holds the authority the scope requires. Fails
    //     closed, identical to the same-admin-mismatch path above.
    let authority_ok = match intent.workspace_id {
        // Platform scope: must still be a platform_admin.
        None => match st.sessions.platform_roles_of(authenticated).await {
            Ok(roles) => roles.iter().any(|r| r == "platform_admin"),
            Err(_) => false,
        },
        // Workspace scope (Task 9's path, which also completes through this
        // callback): the authenticated admin must STILL be a `ws_admin` of the
        // intent's workspace. Re-checked live against `access_grants` (direct or via
        // group), so a ws_admin demoted in the seconds between start and callback
        // fails closed here — no overlay — exactly as the platform branch does.
        Some(ws) => st
            .sessions
            .ws_admin_of(authenticated, WorkspaceId(ws))
            .await
            .unwrap_or(false),
    };
    if !authority_ok {
        return (
            jar.remove(clear_state_cookie()),
            (StatusCode::FORBIDDEN, "step-up authority revoked"),
        )
            .into_response();
    }
    let target = UserId(intent.target_user_id);
    // (3) Re-run the shared target guards (not-self / target-exists / platform
    //     authority, by the intent's own scope — see below).
    if let Err(e) = validate_impersonation_target(
        &st.users,
        &st.sessions,
        authenticated,
        target,
        &intent.reason,
        // The sealed intent's own scope: this one callback completes BOTH the
        // platform step-up (`None`) and the ws_admin one (`Some(ws)`), so the
        // target rule it applies must follow the intent rather than the route.
        intent.workspace_id.map(WorkspaceId),
    )
    .await
    {
        return (
            jar.remove(clear_state_cookie()),
            (e.status_code(), "step-up target rejected"),
        )
            .into_response();
    }
    // (3b) For a WS-SCOPED intent, also re-run the workspace-specific target pair:
    //      the target must be a MEMBER of that workspace and must not be a `ws_admin`
    //      of it. `ws_impersonate_start` runs this as its step 4 before writing the
    //      overlay inline; an OIDC caller never reaches that write (it gets a 409 and
    //      the overlay is minted HERE), and this callback omitted the guard — so
    //      promoting or removing the target during the IdP round trip minted the
    //      overlay anyway (whole-branch review, M-1). Same function as the start
    //      endpoint calls, never a copy of it; nothing is written on refusal.
    if let Some(ws) = intent.workspace_id {
        if let Err(e) =
            validate_ws_impersonation_target_membership(&st.sessions, target, WorkspaceId(ws)).await
        {
            return (
                jar.remove(clear_state_cookie()),
                (e.status_code(), "step-up target rejected"),
            )
                .into_response();
        }
    }
    // (4) Mint a fresh admin session and attach the overlay to it.
    let issued = match st.sessions.create(authenticated, "", "").await {
        Ok(i) => i,
        Err(_) => {
            return (
                jar.remove(clear_state_cookie()),
                (StatusCode::INTERNAL_SERVER_ERROR, "session failed"),
            )
                .into_response()
        }
    };
    let sid = match st.sessions.session_id_for_token(&issued.token).await {
        Ok(Some(id)) => id,
        _ => {
            return (
                jar.remove(clear_state_cookie()),
                (StatusCode::INTERNAL_SERVER_ERROR, "session failed"),
            )
                .into_response()
        }
    };
    let workspace_scope = intent.workspace_id.map(WorkspaceId);
    if let Err(e) = st
        .impersonations
        .start(NewImpersonation {
            admin_session_id: sid,
            impersonator_id: authenticated,
            target_user_id: target,
            workspace_scope,
            reason: intent.reason.clone(),
            ttl_minutes: 30,
        })
        .await
    {
        tracing::warn!(error = %e, "impersonation step-up: overlay insert failed");
        return (
            jar.remove(clear_state_cookie()),
            (StatusCode::INTERNAL_SERVER_ERROR, "overlay failed"),
        )
            .into_response();
    }
    // (5) Audit.
    st.audit.emit(AuditEvent::Mutation {
        actor: Some(authenticated.0),
        impersonator: Some(authenticated.0),
        workspace_id: intent.workspace_id,
        action: "impersonation.start".into(),
        resource_type: "user".into(),
        resource_id: Some(target.0.to_string()),
        detail: serde_json::json!({
            "reason": intent.reason,
            "scope": intent
                .workspace_id
                .map(|w| w.to_string())
                .unwrap_or_else(|| "platform".to_string()),
            "step_up": "oidc",
        }),
    });
    // (6) Redirect into the app carrying the new admin session (now overlaid).
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
    let jwks = match st.oidc.logout_jwks().await {
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
