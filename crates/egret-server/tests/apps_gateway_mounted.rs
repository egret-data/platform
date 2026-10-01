//! The Data Apps gateway (`POST /api/apps/query`) is mounted in the COMPOSED
//! router and is NOT behind the CSRF layer.
//!
//! Its callers are marimo processes inside app pods: they hold a per-session
//! bearer token and no browser cookie, so a CSRF layer would reject every call.
//! `egret-apps`' own tests call `gateway_router()` directly and therefore never
//! mount that layer — they would pass whether or not `build_router` wraps the
//! group. Only the composed router can answer the question (the same reasoning
//! as `dataapi_serving_mount.rs`).
//!
//! No database: the state uses `Db::connect_lazy` against port 1 and nothing
//! here reaches a query. The outcomes are distinguishable without one:
//!
//! * `403` — the CSRF middleware rejected the call before the handler ran;
//! * `404` — nothing is mounted at the path, which would make "not 403" vacuous;
//! * `401` — the handler ran and refused the credential. A garbage token lacks
//!   the app-token prefix, so `AppSessionRepo::resolve` answers `None` without
//!   touching the pool.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_core::authz::{Decision, Pdp, Resource, Subject};
use egret_metastore::Db;
use egret_server::{build_router, AppState};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

/// DENY-everything: every test here is about mounting, so none may depend on
/// an authorization outcome.
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
        unreachable!("no request in this file gets past the bearer check")
    }
    async fn execute_as(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        _principal: egret_core::UserId,
        _sql: &str,
        _limits: egret_query_api::Limits,
    ) -> Result<egret_core::TabularResult, egret_core::Error> {
        unreachable!("no request in this file gets past the bearer check")
    }
}

/// Built through `egret_server::build_router`, NOT `gateway_router()`.
fn composed_app_with_apps() -> axum::Router {
    let db = Db::connect_lazy("postgres://egret:egret@127.0.0.1:1/egret").expect("lazy pool");
    let cfg = egret_config::AppsConfig::default();
    let state = egret_apps::AppsState {
        db: db.clone(),
        pdp: Arc::new(DenyPdp),
        query: Arc::new(UnreachablePort),
        audit: egret_audit::AuditSink::new(1).0,
        sessions: egret_apps::session::AppSessionRepo::new(db.clone()),
        limiter: egret_core::ratelimit::RateLimiter::new(
            cfg.rate_max,
            cfg.rate_window_secs,
            cfg.rate_window_secs,
        ),
        cfg,
    };
    build_router(AppState::without_db().with_apps(state))
}

fn query_request(authorization: Option<&str>) -> Request<Body> {
    let mut b = Request::builder()
        .method("POST")
        .uri("/api/apps/query")
        .header("content-type", "application/json");
    if let Some(a) = authorization {
        b = b.header("authorization", a);
    }
    b.body(Body::from(r#"{"database":"db","sql":"SELECT 1"}"#))
        .unwrap()
}

#[tokio::test]
async fn gateway_without_csrf_header_or_cookie_reaches_the_handler() {
    // Arrange
    let app = composed_app_with_apps();

    // Act: no X-Egret-CSRF header, no cookie, no bearer.
    let res = app.oneshot(query_request(None)).await.unwrap();

    // Assert: 401 from the handler — not 403 (CSRF) and not 404 (unmounted).
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn gateway_with_a_garbage_bearer_is_unauthorized() {
    // Arrange
    let app = composed_app_with_apps();

    // Act
    let res = app
        .oneshot(query_request(Some("Bearer not-an-app-token")))
        .await
        .unwrap();

    // Assert
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}
