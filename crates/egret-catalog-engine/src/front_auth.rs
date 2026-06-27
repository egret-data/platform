//! Front-of-router auth middleware that fronts the embedded lakekeeper Iceberg
//! REST engine with egret's per-workspace token ownership (CE-2 T5, A-3).
//!
//! The bare lakekeeper router (`mount::catalog_engine_router`) carries NO authn
//! middleware and runs an `AllowAllAuthorizer` inside — every handler simply
//! reads `Extension<RequestMetadata>` and trusts it. So egret's tenant boundary
//! is enforced HERE, in front, before the request reaches lakekeeper:
//!
//! 1. **AUTHN** — extract the `Bearer` token and resolve it to an egret
//!    `workspace_id` via [`CatalogStore::workspace_for_token`]. A missing /
//!    unknown / revoked token is `401 UNAUTHORIZED`.
//! 2. **AUTHZ (A-3 ownership)** — the warehouse UUID lives in the request path
//!    as lakekeeper's `{prefix}` segment. We look up the workspace's own
//!    warehouse via [`CatalogStore::warehouse_for_workspace`] and require
//!    `prefix == expected_warehouse`; any mismatch is `403 FORBIDDEN`. This is
//!    the cross-tenant guard: token A may only touch workspace A's warehouse.
//! 3. **Principal injection** — on success we mirror lakekeeper's own authn
//!    layer by inserting a `RequestMetadata` into the request extensions (the
//!    handlers' `Extension<_>` extractor 500s without it). For CE-2 we use the
//!    in-process `new_lakekeeper_internal` actor (AllowAll inside approves it);
//!    mapping the egret token to a real per-token lakekeeper principal is CE-3.
//!
//! ## `{prefix}` extraction
//!
//! The router is mounted (nested) so that, by the time this middleware runs, the
//! request path is the lakekeeper-relative path: `/{prefix}/...` for catalog ops
//! and `/config` for warehouse discovery. We parse `req.uri().path()` directly
//! (rather than an axum `Path` extractor) because this middleware sits ABOVE the
//! method routers — the matched-path params aren't populated yet — and because
//! `/config` has no `{prefix}` at all.
//!
//! ## `/config` (no prefix)
//!
//! `GET /config?warehouse=<slug>` is the discovery call: it has no `{prefix}`
//! path segment, so there is nothing to ownership-check against the path. Any
//! VALID token is allowed through (401 still applies to bad tokens). The
//! `?warehouse=<slug>` query names a warehouse by *name* (the workspace slug);
//! lakekeeper resolves it within the default project and hands back that
//! warehouse's UUID as `defaults.prefix`. A token could in principle ask config
//! for another workspace's slug and learn its prefix UUID, but every subsequent
//! op against that prefix is blocked by the step-2 ownership check, so no data
//! crosses the tenant boundary. (Tightening config to the caller's own slug is a
//! CE-3 refinement once the token→principal bridge lands.)

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use egret_catalog::service::CatalogStore;
use lakekeeper::api::RequestMetadata;
use uuid::Uuid;

/// State carried by the front-auth middleware: the egret-side store used to
/// resolve `token → workspace` and `workspace → warehouse`.
#[derive(Clone)]
pub struct FrontAuthState {
    pub store: Arc<CatalogStore>,
}

impl FrontAuthState {
    /// Build the front-auth state over an egret-side [`CatalogStore`].
    pub fn new(store: CatalogStore) -> Self {
        Self {
            store: Arc::new(store),
        }
    }
}

/// Front-auth + ownership middleware (see module docs).
///
/// Wire with [`axum::middleware::from_fn_with_state`] over a [`FrontAuthState`],
/// layered onto the (nested) lakekeeper catalog router so that `req.uri().path()`
/// is the lakekeeper-relative path (`/{prefix}/...` or `/config`).
pub async fn front_auth(
    axum::extract::State(state): axum::extract::State<FrontAuthState>,
    mut req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    // --- AUTHN: Bearer token → egret workspace_id (401 on any miss) ----------
    let token = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let workspace_id = state
        .store
        .workspace_for_token(token)
        .await
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    // --- AUTHZ: enforce the path {prefix} (warehouse UUID) belongs to the ----
    // --- caller's workspace, unless this is the prefix-less /config call. -----
    if let Some(prefix) = first_path_segment(req.uri().path()) {
        if prefix != "config" {
            // The `{prefix}` segment is the warehouse UUID per lakekeeper's
            // default `prefix_template = "{warehouse_id}"`.
            let requested_wh = Uuid::parse_str(prefix).map_err(|_| StatusCode::FORBIDDEN)?;
            let expected_wh = state
                .store
                .warehouse_for_workspace(workspace_id)
                .await
                .map_err(|_| StatusCode::FORBIDDEN)?
                .ok_or(StatusCode::FORBIDDEN)?;
            if requested_wh != expected_wh {
                return Err(StatusCode::FORBIDDEN);
            }
        }
    }

    // --- Principal injection: mirror lakekeeper's authn layer (CE-3 seam) -----
    req.extensions_mut()
        .insert(RequestMetadata::new_lakekeeper_internal(Uuid::now_v7()));

    Ok(next.run(req).await)
}

/// Return the first non-empty path segment of `path` (the lakekeeper `{prefix}`
/// or the literal `config`), or `None` for the router root.
fn first_path_segment(path: &str) -> Option<&str> {
    path.trim_start_matches('/')
        .split('/')
        .find(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_path_segment_extracts_prefix() {
        assert_eq!(first_path_segment("/config"), Some("config"));
        assert_eq!(first_path_segment("/0190abcd/namespaces"), Some("0190abcd"));
        assert_eq!(
            first_path_segment("/0190abcd/namespaces/ns/tables/t"),
            Some("0190abcd")
        );
        assert_eq!(first_path_segment("/"), None);
        assert_eq!(first_path_segment(""), None);
    }
}
