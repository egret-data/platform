//! The Data Apps management API (`/api/ws/{workspace}/apps…`) is mounted in the
//! COMPOSED router and IS behind the CSRF layer — the opposite of the gateway
//! (`apps_gateway_mounted.rs`).
//!
//! `egret-apps`' own HTTP tests mount `apps_router()` with the CSRF layer they
//! add themselves, so they pass whether or not `build_router` mounts the group
//! at all, or mounts it without the layer. Only the composed router can answer.
//!
//! No database: the state uses `Db::connect_lazy` against port 1 and nothing
//! here reaches a query. The outcomes are distinguishable without one:
//!
//! * `404` — nothing is mounted at the path;
//! * `401` — the session extractor ran and found no cookie (mounted, and the
//!   session is required);
//! * `403` with the CSRF middleware's body — the CSRF layer rejected the call
//!   before the handler ran.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_auth::session::SessionService;
use egret_core::authz::{Decision, Pdp, Resource, Subject};
use egret_metastore::Db;
use egret_server::{build_router, AppState};
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use std::sync::Arc;
use tower::ServiceExt;

const TEST_BUCKET: &str = "test-bucket";
const TEST_PREFIX: &str = "apps/";

fn test_object_store() -> Arc<dyn egret_storage::ObjectStore> {
    Arc::new(egret_storage::MemStore::new())
}

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

/// Built through `egret_server::build_router`, NOT `apps_router()`.
fn composed_app_with_apps_api() -> axum::Router {
    let db = Db::connect_lazy("postgres://egret:egret@127.0.0.1:1/egret").expect("lazy pool");
    let state = egret_apps::AppsApiState {
        store: egret_apps::store::AppStore::new(
            db.clone(),
            test_object_store(),
            TEST_BUCKET.to_string(),
            TEST_PREFIX.to_string(),
        ),
        db: db.clone(),
        sessions: SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        workspaces: WorkspaceRepo::new(db.clone()),
        authz: Authorizer::new(db.clone()),
        pdp: Arc::new(DenyPdp),
        audit: egret_audit::AuditSink::new(1).0,
        grants: egret_apps::grant::GrantRepo::new(db.clone()),
        apps_origin: String::new(),
        ticket_ttl: std::time::Duration::from_secs(30),
    };
    build_router(AppState::without_db().with_apps_api(state))
}

async fn body_text(res: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[tokio::test]
async fn list_without_a_session_cookie_is_401() {
    // Arrange
    let app = composed_app_with_apps_api();
    let req = Request::builder()
        .method("GET")
        .uri("/api/ws/x/apps")
        .body(Body::empty())
        .unwrap();

    // Act
    let res = app.oneshot(req).await.unwrap();

    // Assert: 401 from the session extractor — not 404 (unmounted).
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn create_with_a_cookie_but_no_csrf_header_is_403_from_the_csrf_layer() {
    // Arrange
    let app = composed_app_with_apps_api();
    let req = Request::builder()
        .method("POST")
        .uri("/api/ws/x/apps")
        .header("cookie", "egret_session=s; egret_csrf=c1")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"name":"a"}"#))
        .unwrap();

    // Act
    let res = app.oneshot(req).await.unwrap();

    // Assert: the CSRF middleware's own 403, before any handler or extractor.
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert_eq!(body_text(res).await, "csrf check failed");
}

#[tokio::test]
async fn without_the_apps_api_nothing_is_mounted() {
    // Arrange: the control — the 401 above must come from THIS group.
    let app = build_router(AppState::without_db());
    let req = Request::builder()
        .method("GET")
        .uri("/api/ws/x/apps")
        .body(Body::empty())
        .unwrap();

    // Act
    let res = app.oneshot(req).await.unwrap();

    // Assert
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}
