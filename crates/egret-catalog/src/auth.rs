//! Bearer-token auth middleware for the Iceberg REST catalog.
//!
//! Authentication (AUTHN) lives here: it resolves the Bearer token to an
//! [`AuthnPrincipal`] and inserts it into request extensions. The per-prefix
//! ownership check (AUTHZ) is enforced downstream in
//! [`crate::api::resolve::resolve_catalog`].

use crate::api::resolve::AuthnPrincipal;
use crate::routes::CatalogState;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

/// Authenticate the Bearer token and stamp an [`AuthnPrincipal`] onto the request.
///
/// - Token equal to the static `state.auth_token` → [`AuthnPrincipal::Admin`]
///   (a documented ops/test/discovery bypass that skips ownership checks).
/// - Otherwise look the token up in `catalog_tokens`; a hit yields
///   [`AuthnPrincipal::Workspace`], a miss is `401 UNAUTHORIZED`.
pub async fn require_bearer(
    State(state): State<CatalogState>,
    mut req: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let token = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let principal = if constant_time_eq(token.as_bytes(), state.auth_token.as_bytes()) {
        AuthnPrincipal::Admin
    } else {
        // Hash + look up the per-workspace catalog token; any miss → 401.
        match state.service.store.workspace_for_token(token).await {
            Ok(ws) => AuthnPrincipal::Workspace(ws),
            Err(_) => return Err(StatusCode::UNAUTHORIZED),
        }
    };

    req.extensions_mut().insert(principal);
    Ok(next.run(req).await)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}
