//! The Data API serving group must NOT carry the CSRF layer — asserted through
//! the **composed** application router.
//!
//! # Why this file exists at all, and why it is here rather than in `egret-dataapi`
//!
//! `egret-dataapi`'s own `serving_routing.rs` calls `serving_router()` directly.
//! A test written that way **cannot see a CSRF failure**, because it never
//! mounts the layer: it passes identically whether or not `build_router` wraps
//! the group, which makes it worthless for exactly the property it would claim
//! to check. That is not hypothetical. `render_router` carries the scar in its
//! own doc comment (`crates/egret-automation/src/routes.rs:115-126`): *"the
//! integration tests call this router directly, so the layer they never mount
//! rejected every POST in production"*.
//!
//! `AppState::without_db()` sets `dataapi: None`, so before this file no test
//! had ever built the composed router with this group merged at all. That gap is
//! why `AppState::with_dataapi` was added — the alternative was to skip the one
//! test that can catch this class.
//!
//! # No database, and it still bites
//!
//! The state is built with `Db::connect_lazy`, and no test here reaches a query.
//! The three outcomes a POST to the serving path can produce are already
//! distinguishable without one:
//!
//! * `403 csrf check failed` — the middleware rejected it and the handler never
//!   ran. This is the failure the file exists to catch.
//! * `404` — nothing is mounted at that path at all, which would make a
//!   "not 403" assertion pass vacuously.
//! * `401` — the handler ran and answered with its own first step,
//!   `verify_serving_token`. That is the evidence: it can only be produced by
//!   code downstream of where the layer would have rejected the request.
//!
//! So the POST carries a deliberately unverifiable bearer token. The full
//! POST → 200 path needs a database and is covered by `egret-dataapi`'s
//! `a_post_endpoint_is_served_on_post`; what only the composed router can prove
//! is the layering, and 401-from-the-handler proves it. The file therefore
//! asserts on every run, with no `DATABASE_URL` and no skip.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_core::authz::{Decision, Pdp, Resource, Subject};
use egret_dataapi::{ClientStore, DataApiState, EndpointStore};
use egret_machine_idp::{generate_rsa_key, IdpError, KeyMaterial, SigningKeyStore};
use egret_metastore::Db;
use egret_server::{build_router, AppState};
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;
use uuid::Uuid;

/// A DENY-everything PDP.
///
/// The serving path DOES consult the PDP now — spec §6.2 step 4 checks
/// `data_api_endpoint:invoke` on the endpoint object (this comment used to say
/// the check "is not implemented", and used the short `dataapi:invoke` spelling
/// the spec has since reconciled to the catalog key). Deny is still the right
/// stub, and now for a stronger reason than "it is never called": every test in
/// this file is about MOUNTING — the route exists, the CSRF layer is or is not
/// there, the security scheme resolves — and none of them needs a call to
/// succeed. An allow-all stub would let a mount test start depending on the
/// authorization outcome without anyone noticing.
struct DenyPdp;

#[async_trait::async_trait]
impl Pdp for DenyPdp {
    async fn authorize(
        &self,
        _s: &Subject,
        _a: &str,
        _r: &Resource,
    ) -> egret_core::Result<Decision> {
        Ok(Decision {
            allow: false,
            reasons: vec![],
        })
    }
}

struct UnreachablePort;

#[async_trait::async_trait]
impl egret_query_api::QueryExecPort for UnreachablePort {
    async fn describe_as(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        _principal: egret_core::UserId,
        _sql: &str,
    ) -> Result<Vec<egret_core::ColumnMeta>, egret_core::Error> {
        unreachable!("no request in this file gets past the workspace lookup")
    }
    async fn execute_as(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        _principal: egret_core::UserId,
        _sql: &str,
        _limits: egret_query_api::Limits,
    ) -> Result<egret_core::TabularResult, egret_core::Error> {
        unreachable!("no request in this file gets past the workspace lookup")
    }
}

fn active_key() -> &'static KeyMaterial {
    static KEY: OnceLock<KeyMaterial> = OnceLock::new();
    KEY.get_or_init(|| generate_rsa_key("egret-active").expect("generate key"))
}

struct FakeKeys;

#[async_trait::async_trait]
impl SigningKeyStore for FakeKeys {
    async fn active(&self) -> Result<KeyMaterial, IdpError> {
        let km = active_key();
        Ok(KeyMaterial {
            kid: km.kid.clone(),
            private_pem: km.private_pem.clone(),
            public_jwk: km.public_jwk.clone(),
        })
    }
    async fn all_public_jwks(&self) -> Result<Vec<serde_json::Value>, IdpError> {
        Ok(vec![active_key().public_jwk.clone()])
    }
}

/// The composed application router, built through `egret_server::build_router`
/// — NOT `serving_router()`. That distinction is the entire point of the file.
fn composed_app_with_dataapi() -> axum::Router {
    // Port 1 so the lazy pool has somewhere to fail fast; it is never connected
    // to. `connect_lazy` parses the URL and touches no network.
    let db = Db::connect_lazy("postgres://egret:egret@127.0.0.1:1/egret").expect("lazy pool");
    let state = DataApiState {
        db: db.clone(),
        clients: ClientStore::new(db.clone()),
        endpoints: EndpointStore::new(db.clone()),
        users: egret_auth::users::UserRepo::new(db.clone()),
        sessions: egret_auth::session::SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        workspaces: egret_workspace::workspaces::WorkspaceRepo::new(db.clone()),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        pdp: Arc::new(DenyPdp),
        query: Arc::new(UnreachablePort),
        keys: Arc::new(FakeKeys),
        issuer: "https://egret.test".into(),
        // Same as `issuer`: the fallback, i.e. `data_api.public_url` unset,
        // which is what every deployment produces today.
        consumer_base_url: "https://egret.test".into(),
        audit: egret_audit::AuditSink::new(1).0,
        cache: std::sync::Arc::new(egret_dataapi::serving::ResponseCache::new()),
        rate_limits: std::sync::Arc::new(egret_dataapi::serving::TokenBucket::new()),
        // Same lazy, never-connected pool as `db` above — this file never
        // exercises Activity, so a fixed retention is enough to satisfy the
        // field.
        activity: egret_dataapi::ActivityStore::new(db.clone(), Some(90)),
        group_recompiler: std::sync::Arc::new(egret_perm_compile::PermGroupRecompiler),
    };
    build_router(AppState::without_db().with_dataapi(state))
}

async fn body_text(res: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[tokio::test]
async fn a_post_to_a_live_endpoint_needs_no_csrf_cookie() {
    // Built through egret_server::build_router, NOT serving_router() — a direct
    // call never mounts the CSRF layer and so cannot fail this.
    let app = composed_app_with_dataapi();

    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/data-api/v1/default/reports/daily")
                // Unverifiable on purpose — see the module comment. The handler's
                // 401 is what proves it ran; a valid token would only move the
                // failure to the (absent) database.
                .header("authorization", "Bearer not-a-real-token")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"params":{}}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = res.status();
    let body = body_text(res).await;
    assert_ne!(
        status,
        StatusCode::FORBIDDEN,
        "the serving group must not carry the CSRF layer; got {status} {body}"
    );
    assert!(
        !body.contains("csrf"),
        "the CSRF layer rejected the request: {body}"
    );
    // The positive half. Without it, both assertions above would also pass on a
    // 404 from a router that never mounted the serving group at all.
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "expected the serving handler itself to answer, got {status} {body}"
    );
}

#[tokio::test]
async fn the_management_group_still_carries_the_csrf_layer() {
    // The other half of the split, and the reason the test above is not simply
    // "CSRF was removed". `build_router` mounts two groups from one
    // `DataApiState`; only one of them is exempt.
    let app = composed_app_with_dataapi();

    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/ws/default/data-api/service-accounts")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = res.status();
    let body = body_text(res).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "got {status} {body}");
    assert_eq!(body, "csrf check failed");
}

#[tokio::test]
async fn the_serving_route_is_mounted_where_the_spec_says() {
    // A wrong prefix would make every assertion above pass vacuously: an
    // unmounted path is a 404, which is neither 403 nor "csrf check failed".
    // `/data-api/v1/{ws}/{*path}` is a published contract with external clients.
    let app = composed_app_with_dataapi();

    let res = app
        .oneshot(
            Request::builder()
                .uri("/data-api/v1/default/reports/daily")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // No Authorization header: the handler's own first step answers 401. A 404
    // here would mean nothing is mounted at this path.
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

/// **The two-segment contract URL survives composition too.**
///
/// Package 8 mounts `GET /data-api/v1/{ws}` beside `/{ws}/{*path}`, and its
/// whole safety argument is that no published endpoint can occupy that URL —
/// `normalize_path` refuses an empty path, so every published URL has a segment
/// after the slug. That argument is about `serving_router()`'s own route table.
/// It says nothing about what `build_router` merges AFTER it, and a two-segment
/// URL is exactly the shape a redirect layer, a trailing-slash normaliser or an
/// SPA fallback added later would claim. All 15 of `egret-dataapi`'s tests for
/// this route drive `serving_router()` directly, so every one of them would stay
/// green while the composed application answered something else — which is the
/// distinction this file exists for.
///
/// There is no such layer today, so this is prevention: the assertion is here to
/// fail on the day one is added, in the file whose subject is composition.
///
/// No `Authorization` header, so the answer is `verify_serving_token`'s own 401
/// and no database is needed — the same evidence the sibling test above uses,
/// for the same reason: a 401 in the SERVING envelope can only have come from
/// this handler.
#[tokio::test]
async fn the_contract_route_is_mounted_where_the_spec_says() {
    let app = composed_app_with_dataapi();

    let res = app
        .oneshot(
            Request::builder()
                .uri("/data-api/v1/default")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let status = res.status();
    let body = body_text(res).await;
    // 404 would mean nothing is mounted at the two-segment URL; 405 would mean
    // something is, but not a GET; a redirect would mean a layer claimed it.
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "the contract handler must answer at /data-api/v1/{{ws}} through the \
         COMPOSED router, got {status} {body}"
    );
    // …and the 401 is the serving surface's own envelope, not some other
    // handler's. Without this the assertion above would also pass if a later
    // layer intercepted the URL and happened to answer 401 itself.
    assert!(
        body.contains("key_invalid"),
        "the serving handler's own refusal envelope is the evidence it ran: {body}"
    );
}

#[test]
fn the_serving_operations_security_requirement_resolves_to_a_declared_scheme() {
    // `security(("data_api_bearer" = []))` on the two operations is the ONLY
    // security requirement in the whole merged document, so nothing else would
    // have declared the scheme. A requirement naming a scheme that
    // `components.securitySchemes` does not define is an invalid document — it
    // reads as documented auth while telling the FE codegen that consumes
    // openapi.json nothing it can act on.
    //
    // Asserted through `openapi_doc()` — the merged document — because the
    // scheme is contributed by a `Modify` on ONE of two docs merged from the
    // same crate, and whether `merge` carries it across is the part that could
    // silently regress.
    let doc = egret_server::openapi_doc();
    let json = serde_json::to_value(&doc).unwrap();

    let op = &json["paths"]["/data-api/v1/{ws}/{path}"]["get"];
    assert!(
        !op.is_null(),
        "the serving operation is missing from the doc"
    );

    let required: Vec<String> = op["security"]
        .as_array()
        .expect("the serving operation must carry a security requirement")
        .iter()
        .flat_map(|r| r.as_object().unwrap().keys().cloned())
        .collect();
    assert!(!required.is_empty(), "no scheme named");

    let declared = &json["components"]["securitySchemes"];
    for name in &required {
        assert!(
            !declared[name].is_null(),
            "operation requires `{name}` but components.securitySchemes declares only {:?}",
            declared.as_object().map(|o| o.keys().collect::<Vec<_>>())
        );
    }
}
