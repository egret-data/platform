//! The Data Apps pod API (`/api/apps/pod/…`) is mounted in the COMPOSED router and is
//! NOT behind the CSRF layer: its callers are marimo processes inside pods holding a
//! pod token and no browser cookie. Same reasoning, and the same outcome table, as
//! `apps_gateway_mounted.rs` (403 = CSRF rejected it, 404 = unmounted, 401 = the handler
//! refused the credential). No database is reached: a token without the `egp_` prefix
//! is answered `None` before the pool is touched.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_core::authz::{Decision, Pdp, Resource, Subject};
use egret_metastore::Db;
use egret_server::{build_router, AppState};
use std::sync::Arc;
use tower::ServiceExt;

const TEST_BUCKET: &str = "test-bucket";
const TEST_PREFIX: &str = "apps/";

fn test_object_store() -> Arc<dyn egret_storage::ObjectStore> {
    Arc::new(egret_storage::MemStore::new())
}

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

fn composed_app_with_pod_api() -> axum::Router {
    let db = Db::connect_lazy("postgres://egret:egret@127.0.0.1:1/egret").expect("lazy pool");
    let state = egret_apps::PodApiState {
        db: db.clone(),
        pdp: Arc::new(DenyPdp),
        pods: egret_apps::podtoken::PodTokenRepo::new(db.clone()),
        store: egret_apps::store::AppStore::new(
            db,
            test_object_store(),
            TEST_BUCKET.to_string(),
            TEST_PREFIX.to_string(),
        ),
    };
    build_router(AppState::without_db().with_apps_pod(state))
}

const APP: &str = "00000000-0000-0000-0000-000000000001";

fn request(method: &str, uri: String) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("authorization", "Bearer not-a-pod-token")
        .body(Body::from(r#"{"source":"x"}"#))
        .unwrap()
}

#[tokio::test]
async fn the_pod_routes_are_mounted_and_need_no_csrf_header() {
    for (method, uri) in [
        ("GET", format!("/api/apps/pod/apps/{APP}/draft")),
        ("PUT", format!("/api/apps/pod/apps/{APP}/draft")),
        (
            "GET",
            format!(
                "/api/apps/pod/apps/{APP}/versions/{}",
                APP.replace('1', "2")
            ),
        ),
    ] {
        let res = composed_app_with_pod_api()
            .oneshot(request(method, uri.clone()))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
}
