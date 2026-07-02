//! Front-of-router auth middleware that fronts the embedded lakekeeper Iceberg
//! REST engine with egret's per-workspace token ownership (CE-2 T5, A-3) and
//! per-token read/write permissions (CE-3 T3).
//!
//! The bare lakekeeper router (`mount::catalog_engine_router`) carries NO authn
//! middleware and runs egret's [`EgretAuthorizer`] inside — every handler reads
//! `Extension<RequestMetadata>` and the authorizer gates data actions off its
//! `"egret"` claim. So egret's tenant boundary is enforced in two layers: HERE
//! in front (workspace ownership + coarse read/write permission), and AGAIN
//! inside the engine (the authorizer, which evaluates the per-token resource
//! `grants` with OR semantics):
//!
//! 1. **AUTHN** — extract the `Bearer` token and resolve it to an egret
//!    `TokenAuth { workspace_id, warehouse_id, grants, permissions }` (the
//!    `permissions` are the deduped union of the grants' perms, for the coarse
//!    gate) via [`CatalogStore::authenticate_token`]. A missing / unknown /
//!    revoked token is `401 UNAUTHORIZED`.
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
//!    `"egret"` authentication claim carries this token's resource `grants`.
//!    [`EgretAuthorizer`] reads that claim to re-enforce the per-resource grant
//!    decision (namespace / table coverage + per-grant read/write) inside the
//!    engine — that boundary is enforced ONLY here-plus-authorizer, since the
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
    /// Optional OIDC-JWT authenticator (F2-P2). When present, a bearer token that
    /// *looks like* a JWT (three dot-separated segments) is verified via this
    /// authenticator instead of the opaque `catalog_tokens` store. `None` when
    /// OIDC is disabled (or its verifier failed to initialise at boot): JWTs are
    /// then rejected and only opaque tokens are accepted.
    pub jwt: Option<Arc<crate::jwt_auth::CatalogJwtAuth>>,
    /// The externally-reachable catalog base URL (e.g. `http://localhost:8080/catalog`).
    /// The embedded lakekeeper reports its own internal bind address
    /// (`http://localhost:8181/catalog`) in the `/config` response's
    /// `overrides.uri` — which clients then follow for table operations and cannot
    /// reach (lakekeeper is only exposed through egret's `/catalog` proxy). When
    /// set, we rewrite that field on the `/config` response to this public URL so
    /// Iceberg clients (pyiceberg/Daft) talk to the proxy. `None` leaves the
    /// response untouched.
    pub catalog_public_url: Option<String>,
}

impl FrontAuthState {
    /// Build the front-auth state over an egret-side [`CatalogStore`], optionally
    /// with an OIDC-JWT authenticator for the hybrid (JWT-or-opaque) path and the
    /// public catalog URL used to rewrite lakekeeper's `/config` `overrides.uri`.
    pub fn new(
        store: CatalogStore,
        jwt: Option<Arc<crate::jwt_auth::CatalogJwtAuth>>,
        catalog_public_url: Option<String>,
    ) -> Self {
        Self {
            store: Arc::new(store),
            jwt,
            catalog_public_url,
        }
    }
}

/// The base URL the embedded lakekeeper reports for itself in self-referential
/// response fields (`overrides.uri` in `/config`, `s3.signer.uri` in loadTable,
/// etc.). It comes from `RequestMetadata`'s hardcoded default at this lakekeeper
/// rev (`new_lakekeeper_internal` → `base_url = "http://localhost:8181"`, then
/// `+ "/catalog"`), and there is no public setter to correct it. Clients follow
/// these URLs and cannot reach `:8181` (lakekeeper is only exposed through
/// egret's `/catalog` proxy), so we rewrite this prefix to the public catalog
/// base on the way out.
const LAKEKEEPER_INTERNAL_CATALOG_BASE: &str = "http://localhost:8181/catalog";

/// Rewrite every occurrence of lakekeeper's internal catalog base
/// ([`LAKEKEEPER_INTERNAL_CATALOG_BASE`]) to `public_catalog_base` (the
/// externally-reachable catalog URL, e.g. `http://localhost:8080/catalog`) in a
/// catalog response body. Covers `overrides.uri` (`/config`) and the per-table
/// `s3.signer.uri` (loadTable) with one prefix swap. Returns `None` when the body
/// contains no occurrence, so callers pass the original body through unchanged.
pub(crate) fn rewrite_lakekeeper_base(body: &[u8], public_catalog_base: &str) -> Option<Vec<u8>> {
    let needle = LAKEKEEPER_INTERNAL_CATALOG_BASE.as_bytes();
    if !body.windows(needle.len()).any(|w| w == needle) {
        return None;
    }
    let public = public_catalog_base.trim_end_matches('/');
    let text = std::str::from_utf8(body).ok()?;
    Some(
        text.replace(LAKEKEEPER_INTERNAL_CATALOG_BASE, public)
            .into_bytes(),
    )
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
/// carries the token's resource `grants` in the `"egret"` authentication claim —
/// exactly the shape [`EgretAuthorizer`] reads to gate data actions
/// (`{ "egret": { "grants": [ { "ns", "table", "perms" } ] } }`). The front-auth
/// ownership + permission checks above are belt-and-braces; the authorizer is
/// the in-engine, defense-in-depth enforcer of the same scope.
fn token_request_metadata(auth: &TokenAuth) -> RequestMetadata {
    let uid = TOKEN_PRINCIPAL_ID.clone();
    // Serialize each grant under the short claim keys the authorizer's
    // `egret_grants` parser expects.
    let grants: Vec<_> = auth
        .grants
        .iter()
        .map(|g| {
            serde_json::json!({
                "ns": g.namespace,
                "table": g.table,
                "perms": g.permissions,
            })
        })
        .collect();
    let authentication = Authentication::builder()
        .token_header(None)
        .claims(serde_json::json!({ "egret": { "grants": grants } }))
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

    let path = req.uri().path().to_owned();

    // Hybrid authn: a bearer with three dot-separated segments is a JWT; an
    // opaque `catalog_token` is random hex (no dots). JWTs go through the OIDC
    // authenticator (when configured), opaque tokens through the token store.
    // Either path yields the SAME `TokenAuth` shape, so everything below is
    // identical regardless of which credential was presented.
    let looks_like_jwt = token.split('.').count() == 3;
    let auth = if looks_like_jwt {
        match &state.jwt {
            Some(j) => {
                // The warehouse UUID to resolve the JWT's grant against: the path
                // prefix (or the signer path's second segment). `/config` has none
                // (→ None) and opaque tokens handle discovery.
                let prefix = warehouse_prefix(&path).ok_or(StatusCode::UNAUTHORIZED)?;
                let wh = Uuid::parse_str(prefix).map_err(|_| StatusCode::UNAUTHORIZED)?;
                j.authenticate(token, wh).await.map_err(|e| match e {
                    crate::jwt_auth::JwtAuthError::Unauthenticated => StatusCode::UNAUTHORIZED,
                    crate::jwt_auth::JwtAuthError::Forbidden => StatusCode::FORBIDDEN,
                    crate::jwt_auth::JwtAuthError::Internal => StatusCode::INTERNAL_SERVER_ERROR,
                })?
            }
            // A JWT was presented but OIDC is not configured: reject it (opaque
            // tokens still work; we do not silently fall through to the store).
            None => return Err(StatusCode::UNAUTHORIZED),
        }
    } else {
        state
            .store
            .authenticate_token(token)
            .await
            .map_err(|_| StatusCode::UNAUTHORIZED)?
    };

    // --- AUTHZ (ownership): enforce the warehouse UUID in the path belongs to ---
    // --- the caller's workspace (skipped for the prefix-less /config call). ----
    if let Some(prefix) = warehouse_prefix(&path) {
        // The warehouse UUID is the first path segment per lakekeeper's default
        // `prefix_template = "{warehouse_id}"`, or the second on S3-signer paths
        // (`signer/{warehouse_id}/...`) — `warehouse_prefix` returns the right one.
        let requested_wh = Uuid::parse_str(prefix).map_err(|_| StatusCode::FORBIDDEN)?;
        // Resolve the lakekeeper UUID of the warehouse this token was minted for
        // (workspace-scoped, so a token can never name another tenant's warehouse
        // row). A missing/cross-workspace row → 403.
        let expected_wh = state
            .store
            .lakekeeper_id(auth.workspace_id, auth.warehouse_id)
            .await
            .map_err(|_| StatusCode::FORBIDDEN)?;
        if requested_wh != expected_wh {
            return Err(StatusCode::FORBIDDEN);
        }
    }

    // --- AUTHZ (coarse scope): the token holds the required permission ---------
    // This is the COARSE union gate (does the token carry ANY read / ANY write).
    // The per-resource decision (which namespace/table each grant covers) is
    // re-evaluated inside the engine by `EgretAuthorizer` over the grants claim.
    let need = required_permission(req.method(), &path);
    if !auth.permissions.iter().any(|p| p == need) {
        return Err(StatusCode::FORBIDDEN);
    }

    // --- Principal injection: a real token principal carrying the egret scope --
    // Inject a per-token lakekeeper principal whose `"egret"` authentication
    // claim carries this token's resource `grants`, so `EgretAuthorizer`
    // re-enforces the per-resource grant decision INSIDE the engine (defense in
    // depth, and the sole enforcer of the per-grant namespace/table boundary).
    req.extensions_mut().insert(token_request_metadata(&auth));

    let resp = next.run(req).await;

    // Rewrite lakekeeper's internal self-URLs (`:8181`) → the public catalog base
    // in JSON responses, so Iceberg clients follow the reachable egret proxy for
    // discovery (`/config` `overrides.uri`) and per-table S3 signing
    // (`s3.signer.uri`). Catalog responses are small JSON, so buffering is cheap;
    // only JSON is touched, and a body with no occurrence passes through unchanged.
    let Some(public) = state.catalog_public_url.as_deref() else {
        return Ok(resp);
    };
    let is_json = resp
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("application/json"));
    if !is_json {
        return Ok(resp);
    }
    let (mut parts, body) = resp.into_parts();
    let bytes = axum::body::to_bytes(body, 4 * 1024 * 1024)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    match rewrite_lakekeeper_base(&bytes, public) {
        Some(out) => {
            parts.headers.remove(axum::http::header::CONTENT_LENGTH);
            Ok(Response::from_parts(parts, Body::from(out)))
        }
        None => Ok(Response::from_parts(parts, Body::from(bytes))),
    }
}

/// The warehouse-UUID segment of a lakekeeper-relative `path` to enforce
/// ownership against, or `None` when the path carries no warehouse (the
/// prefix-less `config` discovery endpoint, or the router root).
///
/// The warehouse UUID is normally the first path segment (lakekeeper's default
/// `prefix_template = "{warehouse_id}"`), but S3 remote-signing paths are
/// `signer/{warehouse_id}/tabular-id/.../aws/s3/sign` — there the UUID is the
/// second segment. Both shapes resolve here.
fn warehouse_prefix(path: &str) -> Option<&str> {
    let mut segs = path
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty());
    match segs.next()? {
        "config" => None,
        "signer" => segs.next(),
        wh => Some(wh),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warehouse_prefix_extracts_the_warehouse_uuid() {
        // Prefix-less discovery endpoint and root → no warehouse.
        assert_eq!(warehouse_prefix("/config"), None);
        assert_eq!(warehouse_prefix("/"), None);
        assert_eq!(warehouse_prefix(""), None);
        // Normal catalog paths: warehouse is the first segment.
        assert_eq!(warehouse_prefix("/0190abcd/namespaces"), Some("0190abcd"));
        assert_eq!(
            warehouse_prefix("/0190abcd/namespaces/ns/tables/t"),
            Some("0190abcd")
        );
        // S3-signer paths: warehouse is the second segment.
        assert_eq!(
            warehouse_prefix("/signer/0190abcd/tabular-id/019f/v1/aws/s3/sign"),
            Some("0190abcd")
        );
        // Malformed signer path with no warehouse → None (→ 403 upstream).
        assert_eq!(warehouse_prefix("/signer"), None);
    }

    #[test]
    fn rewrites_config_overrides_uri() {
        let body = br#"{"overrides":{"uri":"http://localhost:8181/catalog","k":"v"},"defaults":{"prefix":"p"}}"#;
        let out = rewrite_lakekeeper_base(body, "http://localhost:8080/catalog").unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["overrides"]["uri"], "http://localhost:8080/catalog");
        assert_eq!(v["overrides"]["k"], "v"); // sibling field preserved
        assert_eq!(v["defaults"]["prefix"], "p"); // defaults untouched
    }

    #[test]
    fn rewrites_s3_signer_uri_in_loadtable_response() {
        // A per-table signer URL (loadTable) shares the same internal-base prefix.
        let body = br#"{"config":{"s3.signer.uri":"http://localhost:8181/catalog/v1/signer/wh/tabular-id/t"}}"#;
        let out = rewrite_lakekeeper_base(body, "http://localhost:8080/catalog/").unwrap(); // trailing slash trimmed
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            v["config"]["s3.signer.uri"],
            "http://localhost:8080/catalog/v1/signer/wh/tabular-id/t"
        );
    }

    #[test]
    fn rewrite_returns_none_when_no_internal_base_present() {
        assert!(rewrite_lakekeeper_base(b"not json but also no needle", "http://x").is_none());
        assert!(rewrite_lakekeeper_base(br#"{"defaults":{"prefix":"p"}}"#, "http://x").is_none());
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
