//! Front-of-router auth middleware that fronts the embedded lakekeeper Iceberg
//! REST engine with egret's per-workspace token ownership (CE-2 T5, A-3) and
//! per-token read/write permissions (CE-3 T3).
//!
//! The bare lakekeeper router (`mount::catalog_engine_router`) carries NO authn
//! middleware and runs egret's [`EgretAuthorizer`] inside — every handler reads
//! `Extension<RequestMetadata>` and the authorizer gates data actions off its
//! `"egret"` claim. So egret's tenant boundary is enforced in two layers: HERE
//! in front (workspace ownership + coarse read/write permission), and AGAIN
//! inside the engine (the authorizer, which also enforces `namespace_scope`):
//!
//! 1. **AUTHN** — extract the `Bearer` token and resolve it to an egret
//!    `TokenAuth { workspace_id, warehouse_id, permissions, namespace_scope }`
//!    via [`CatalogStore::authenticate_token`]. A missing / unknown / revoked
//!    token is `401 UNAUTHORIZED`.
//! 2. **AUTHZ (A-3 ownership)** — the warehouse UUID lives in the request path
//!    as lakekeeper's `{prefix}` segment. A token is now minted FOR a specific
//!    warehouse (`TokenAuth::warehouse_id`, an egret-side warehouse row id), so
//!    we resolve that row's lakekeeper UUID via
//!    [`CatalogStore::lakekeeper_id`] (workspace-scoped) and require
//!    `prefix == that lakekeeper UUID`; any mismatch is `403 FORBIDDEN`. This is
//!    the cross-tenant guard: a token may only touch the one warehouse it was
//!    minted for (and that warehouse belongs to the token's workspace).
//! 3. **AUTHZ (CE-3 scope)** — [`required_permission`] classifies the request
//!    by `(method, path)` and checks that the resolved token's `permissions`
//!    slice contains the required `"catalog:read"` or `"catalog:write"` value.
//!    A token that lacks the needed permission is `403 FORBIDDEN`.
//! 4. **Principal injection (CE-4)** — on success we insert a `RequestMetadata`
//!    carrying a real per-token lakekeeper principal (`Actor::Principal`, a fixed
//!    synthetic [`token_principal_id`] distinct from the system principal) whose
//!    `"egret"` authentication claim carries this token's `perms` +
//!    `namespace_scope`. [`EgretAuthorizer`] reads that claim to re-enforce the
//!    permission scope AND the per-namespace `namespace_scope` boundary inside
//!    the engine — the latter is enforced ONLY here-plus-authorizer, since the
//!    front layer is namespace-agnostic. (The handlers' `Extension<_>` extractor
//!    500s without the metadata, so injection is also load-bearing for routing.)
//!
//! [`EgretAuthorizer`]: crate::authorizer::EgretAuthorizer
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
use egret_catalog::service::{CatalogStore, TokenAuth};
use lakekeeper::api::RequestMetadata;
use lakekeeper::limes::Authentication;
use lakekeeper::service::authn::{Actor, UserId};
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

/// IdP id for the synthetic per-token lakekeeper principal. Mirrors the system
/// principal's `oidc` IdP (see [`crate::bootstrap`]).
const TOKEN_USER_IDP: &str = "oidc";

/// Fixed namespace UUID for deriving the **token** principal's subject. It is
/// DISTINCT from `EGRET_SYSTEM_USER_NAMESPACE` (in [`crate::bootstrap`]) and the
/// subject is a fixed string, so the derived token-principal `UserId` can NEVER
/// collide with the system principal — guaranteeing per-token requests never hit
/// [`EgretAuthorizer`]'s system-principal bypass.
///
/// Generated ONCE for egret (RFC-4122 random v4), verified distinct from
/// `EGRET_SYSTEM_USER_NAMESPACE`. It MUST NEVER change after first deployment:
/// changing it re-derives the token principal's id (harmless for the token path,
/// which keys off the claim not the id, but it would silently void the
/// distinct-from-system invariant if edited to collide).
const EGRET_TOKEN_USER_NAMESPACE: Uuid = Uuid::from_u128(0xb54e_1e3e_2ab4_4686_9bff_b2ee_f0e3_d7ab);

/// The stable lakekeeper [`UserId`] for the synthetic token principal, computed
/// once (the v5 derivation + `UserId` validation are on the per-request hot path
/// otherwise).
///
/// A single, fixed subject (NOT per-workspace): the egret scope claim — not the
/// principal identity — carries the per-token permissions + namespace scope that
/// [`EgretAuthorizer`] enforces. This principal exists only so lakekeeper's
/// handlers have an `Actor::Principal` to read, and so the authorizer's claim
/// path (not its system bypass) is exercised.
static TOKEN_PRINCIPAL_ID: std::sync::LazyLock<UserId> = std::sync::LazyLock::new(|| {
    let subject = Uuid::new_v5(&EGRET_TOKEN_USER_NAMESPACE, b"egret-catalog-token");
    UserId::try_from(format!("{TOKEN_USER_IDP}~{subject}").as_str())
        .expect("static token principal id is valid")
});

/// Build the per-request [`RequestMetadata`] for a successfully authenticated
/// egret catalog token.
///
/// It installs the synthetic [`token_principal_id`] as `Actor::Principal` and
/// carries the token's `permissions` + `namespace_scope` + `table_scope` in the
/// `"egret"` authentication claim — exactly the shape [`EgretAuthorizer`] reads
/// to gate data actions
/// (`{ "egret": { "perms": [...], "ns": [...], "tables": [...] } }`). The front-auth
/// ownership + permission checks above are belt-and-braces; the authorizer is
/// the in-engine, defense-in-depth enforcer of the same scope.
fn token_request_metadata(auth: &TokenAuth) -> RequestMetadata {
    let uid = TOKEN_PRINCIPAL_ID.clone();
    let authentication = Authentication::builder()
        .token_header(None)
        .claims(serde_json::json!({
            "egret": {
                "perms": auth.permissions,
                "ns": auth.namespace_scope,
                "tables": auth.table_scope
            }
        }))
        .subject(uid.clone().into())
        .name(Some("Egret Catalog Token".to_string()))
        .email(None)
        .principal_type(None)
        .build();

    let mut md = RequestMetadata::new_lakekeeper_internal(Uuid::now_v7());
    md.set_authentication(Actor::Principal(uid), authentication);
    md
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
            // Resolve the lakekeeper UUID of the warehouse this token was minted
            // for (workspace-scoped, so a token can never name another tenant's
            // warehouse row). A missing/cross-workspace row → 403.
            let expected_wh = state
                .store
                .lakekeeper_id(auth.workspace_id, auth.warehouse_id)
                .await
                .map_err(|_| StatusCode::FORBIDDEN)?;
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

    // --- Principal injection: a real token principal carrying the egret scope --
    // Inject a per-token lakekeeper principal whose `"egret"` authentication
    // claim carries this token's perms + namespace_scope, so `EgretAuthorizer`
    // re-enforces the same scope INSIDE the engine (defense in depth, and the
    // sole enforcer of the per-namespace `namespace_scope` boundary).
    req.extensions_mut().insert(token_request_metadata(&auth));

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
