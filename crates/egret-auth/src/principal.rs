//! Axum extractor that authenticates a request from the session cookie.

use axum::extract::{FromRef, FromRequestParts};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use axum_extra::extract::cookie::CookieJar;
use egret_core::{Error, Principal};

use crate::cookie::SESSION_COOKIE;
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
        let token = jar
            .get(SESSION_COOKIE)
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
        let (status, msg) = match self.0 {
            Error::NotFound(_) => (StatusCode::NOT_FOUND, "not found"),
            Error::InvalidInput(_) => (StatusCode::BAD_REQUEST, "invalid input"),
            Error::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Error::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            Error::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            Error::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}
