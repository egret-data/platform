//! Axum extractor that authenticates a request from the session cookie.

use axum::extract::{FromRef, FromRequestParts};
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use axum_extra::extract::cookie::CookieJar;
use egret_core::{Error, Principal};

use crate::cookie::{read_session, secure_cookies};
use crate::session::SessionService;

/// Wrapper so we can implement `FromRequestParts` for a foreign type (`Principal`).
pub struct Auth(pub Principal);

impl<S> FromRequestParts<S> for Auth
where
    SessionService: axum::extract::FromRef<S>,
    S: Send + Sync,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let jar = CookieJar::from_headers(&parts.headers);
        let token = read_session(&jar, secure_cookies())
            .map(|c| c.value().to_string())
            .ok_or(AuthError(Error::Unauthorized))?;
        let svc = SessionService::from_ref(state);
        match svc.validate(&token).await.map_err(AuthError)? {
            Some(principal) => Ok(Auth(principal)),
            None => Err(AuthError(Error::Unauthorized)),
        }
    }
}

/// Newtype to satisfy orphan rules for `IntoResponse`.
pub struct AuthError(pub Error);

impl From<Error> for AuthError {
    fn from(e: Error) -> Self {
        AuthError(e)
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        self.0.into_response()
    }
}
