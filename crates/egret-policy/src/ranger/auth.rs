//! Bearer-token auth middleware for the Ranger REST surface.

use super::routes::RangerState;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

/// Constant-time bearer check against the configured ranger token.
///
/// Anonymous mode: when no token is configured (empty), `/service` is open. The native
/// Ranger plugins (e.g. Trino's `RangerAdminRESTClient`) authenticate out-of-band
/// (Kerberos / trusted network) and do not send a bearer token, so an empty token lets
/// stock plugins download policies. Set a token to require bearer auth.
pub async fn require_bearer(
    State(state): State<RangerState>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    if state.auth_token.is_empty() {
        return Ok(next.run(req).await);
    }
    let token = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    match token {
        Some(t) if ct_eq(t.as_bytes(), state.auth_token.as_bytes()) => Ok(next.run(req).await),
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut d = 0u8;
    for (x, y) in a.iter().zip(b) {
        d |= x ^ y;
    }
    d == 0
}
