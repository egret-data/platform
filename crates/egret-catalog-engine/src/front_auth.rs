//! Front-of-router auth middleware that fronts the embedded lakekeeper Iceberg
//! REST engine with egret's per-workspace token ownership (CE-2 T5, A-3) and
//! per-token read/write permissions (CE-3 T3).
//!
//! The bare lakekeeper router (`mount::catalog_engine_router`) carries NO authn
//! middleware and runs an `AllowAllAuthorizer` inside — every handler simply
//! reads `Extension<RequestMetadata>` and trusts it. So egret's tenant boundary
//! is enforced HERE, in front, before the request reaches lakekeeper:
//!
//! 1. **AUTHN** — extract the `Bearer` token and resolve it to an egret
//!    `TokenAuth { workspace_id, permissions }` via
//!    [`CatalogStore::authenticate_token`]. A missing / unknown / revoked token
//!    is `401 UNAUTHORIZED`.
//! 2. **AUTHZ (A-3 ownership)** — the warehouse UUID lives in the request path
//!    as lakekeeper's `{prefix}` segment. We look up the workspace's own
//!    warehouse via [`CatalogStore::warehouse_for_workspace`] and require
//!    `prefix == expected_warehouse`; any mismatch is `403 FORBIDDEN`. This is
//!    the cross-tenant guard: token A may only touch workspace A's warehouse.
//! 3. **AUTHZ (CE-3 scope)** — [`required_permission`] classifies the request
//!    by `(method, path)` and checks that the resolved token's `permissions`
//!    slice contains the required `"catalog:read"` or `"catalog:write"` value.
//!    A token that lacks the needed permission is `403 FORBIDDEN`.
//! 4. **Principal injection** — on success we mirror lakekeeper's own authn
//!    layer by inserting a `RequestMetadata` into the request extensions (the
//!    handlers' `Extension<_>` extractor 500s without it). For CE-2/CE-3 we use
//!    the in-process `new_lakekeeper_internal` actor (AllowAll inside approves
//!    it); mapping the egret token to a real per-token lakekeeper principal is a
//!    future refinement.
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
//! crosses the tenant boundary.
//!
//! The permission check still applies to `/config`: it is classified as
//! `"catalog:read"`, so a token that has at least `catalog:read` passes. A
//! write-only token (unusual in practice) would 403 on config; that is
//! acceptable and not special-cased.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
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

/// The permission a request requires, by (method, lakekeeper-relative path).
///
/// Classification rules:
/// - `GET` / `HEAD` → always `"catalog:read"`.
/// - `DELETE` → always `"catalog:write"` (destructive).
/// - `POST` → read-only for the discovery / reporting / signing endpoints
///   (`/config`, paths ending with `/metrics` or `/aws/s3/sign`); everything
///   else (create namespace, create/register/rename table, commit, set
///   properties, …) → `"catalog:write"`.
/// - Any other method (`PUT`, `PATCH`, …) → `"catalog:write"` (default-deny).
///
/// The path is the lakekeeper-relative path (after the nest strip), so `/config`
/// arrives here as either `"config"` or `/config` depending on the trim applied
/// upstream. We match both forms for safety.
pub fn required_permission(method: &Method, path: &str) -> &'static str {
    match *method {
        Method::GET | Method::HEAD => "catalog:read",
        Method::DELETE => "catalog:write",
        Method::POST => {
            // Read-only POST shapes: discovery endpoint and metric/signing paths.
            let trimmed = path.trim_start_matches('/');
            if trimmed == "config"
                || trimmed.ends_with("/metrics")
                || trimmed.ends_with("/aws/s3/sign")
            {
                "catalog:read"
            } else {
                // All mutating POSTs: create namespace/table/view, commit,
                // properties, register, rename, etc.
                "catalog:write"
            }
        }
        // Default-deny for unknown/future methods (PUT, PATCH, …).
        _ => "catalog:write",
    }
}

/// Front-auth + ownership + permission-scope middleware (see module docs).
///
/// Wire with [`axum::middleware::from_fn_with_state`] over a [`FrontAuthState`],
/// layered onto the (nested) lakekeeper catalog router so that `req.uri().path()`
/// is the lakekeeper-relative path (`/{prefix}/...` or `/config`).
pub async fn front_auth(
    axum::extract::State(state): axum::extract::State<FrontAuthState>,
    mut req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    // --- AUTHN: Bearer token → egret TokenAuth (workspace_id + permissions) ----
    let token = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let auth = state
        .store
        .authenticate_token(token)
        .await
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    let path = req.uri().path().to_owned();

    // --- AUTHZ (ownership): enforce {prefix} (warehouse UUID) belongs to -------
    // --- the caller's workspace, unless this is the prefix-less /config call. --
    if let Some(prefix) = first_path_segment(&path) {
        if prefix != "config" {
            // The `{prefix}` segment is the warehouse UUID per lakekeeper's
            // default `prefix_template = "{warehouse_id}"`.
            let requested_wh = Uuid::parse_str(prefix).map_err(|_| StatusCode::FORBIDDEN)?;
            let expected_wh = state
                .store
                .warehouse_for_workspace(auth.workspace_id)
                .await
                .map_err(|_| StatusCode::FORBIDDEN)?
                .ok_or(StatusCode::FORBIDDEN)?;
            if requested_wh != expected_wh {
                return Err(StatusCode::FORBIDDEN);
            }
        }
    }

    // --- AUTHZ (scope): check the token holds the required permission ----------
    let need = required_permission(req.method(), &path);
    if !auth.permissions.iter().any(|p| p == need) {
        return Err(StatusCode::FORBIDDEN);
    }

    // --- Principal injection: mirror lakekeeper's authn layer ------------------
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

    #[test]
    fn required_permission_classifier() {
        // GET / HEAD → always read
        assert_eq!(
            required_permission(&Method::GET, "/abc123/namespaces/ns/tables/t"),
            "catalog:read"
        );
        assert_eq!(
            required_permission(&Method::HEAD, "/abc123/namespaces"),
            "catalog:read"
        );

        // DELETE → always write
        assert_eq!(
            required_permission(&Method::DELETE, "/abc123/namespaces/ns/tables/t"),
            "catalog:write"
        );

        // POST /config (discovery) → read
        assert_eq!(
            required_permission(&Method::POST, "/config"),
            "catalog:read"
        );
        assert_eq!(required_permission(&Method::POST, "config"), "catalog:read");

        // POST …/metrics → read
        assert_eq!(
            required_permission(&Method::POST, "/abc123/metrics"),
            "catalog:read"
        );

        // POST …/aws/s3/sign → read
        assert_eq!(
            required_permission(&Method::POST, "/abc123/namespaces/ns/tables/t/aws/s3/sign"),
            "catalog:read"
        );

        // POST create namespace → write
        assert_eq!(
            required_permission(&Method::POST, "/abc123/namespaces"),
            "catalog:write"
        );

        // POST create table → write
        assert_eq!(
            required_permission(&Method::POST, "/abc123/namespaces/ns/tables"),
            "catalog:write"
        );

        // POST commit (table updates) → write
        assert_eq!(
            required_permission(&Method::POST, "/abc123/namespaces/ns/tables/t"),
            "catalog:write"
        );

        // Default-deny for other methods
        assert_eq!(
            required_permission(&Method::PUT, "/abc123/namespaces/ns"),
            "catalog:write"
        );
        assert_eq!(
            required_permission(&Method::PATCH, "/abc123/namespaces/ns"),
            "catalog:write"
        );
    }
}
