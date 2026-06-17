//! OIDC login/callback routes. Mounted only when OIDC is enabled.

use axum::extract::{FromRef, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use axum_extra::extract::cookie::{Key, PrivateCookieJar};
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

async fn oidc_callback(
    State(st): State<OidcRoutesState>,
    jar: PrivateCookieJar,
    Query(q): Query<CallbackQuery>,
) -> Response {
    let Some(raw) = jar.get(OIDC_STATE_COOKIE) else {
        return (StatusCode::BAD_REQUEST, "missing oidc state").into_response();
    };
    let Ok(state): Result<OidcStateCookie, _> = serde_json::from_str(raw.value()) else {
        return (StatusCode::BAD_REQUEST, "bad oidc state").into_response();
    };
    if state.state != q.state {
        return (StatusCode::BAD_REQUEST, "state mismatch").into_response();
    }

    let claims = match st
        .oidc
        .finish(&q.code, &state.pkce_verifier, &state.nonce)
        .await
    {
        Ok(c) => c,
        Err(_) => return (StatusCode::UNAUTHORIZED, "oidc exchange failed").into_response(),
    };
    let user_id = match provision_and_sync(&st.users, &st.groups, &claims).await {
        Ok(id) => id,
        Err(egret_core::Error::Conflict(_)) => {
            return (
                StatusCode::CONFLICT,
                "email already used by another account",
            )
                .into_response()
        }
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "provisioning failed").into_response()
        }
    };
    let issued = match st.sessions.create(user_id, "", "").await {
        Ok(i) => i,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "session failed").into_response(),
    };

    let secure = st.config.session.secure;
    let max_age = st.config.session.absolute_ttl_hours as i64 * 3600;
    let csrf = generate_token();
    let jar = jar
        .remove(clear_state_cookie())
        .add(session_cookie(issued.token, secure, max_age))
        .add(csrf_cookie(csrf, secure, max_age));
    (jar, Redirect::to(&st.config.oidc.post_login_redirect)).into_response()
}
