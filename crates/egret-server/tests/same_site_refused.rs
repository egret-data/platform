//! The composed router refuses `Sec-Fetch-Site: same-site` requests (spec §6.1).
//!
//! `egret-auth`'s own tests call the layer on a toy router and so pass whether or not
//! `build_router` mounts it; only the composed router can say it is on. Probes and
//! scrapers send no `Sec-Fetch-*` headers and must be unaffected.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_server::{build_router, AppState};
use tower::ServiceExt;

async fn status(path: &str, headers: &[(&str, &str)]) -> StatusCode {
    let mut req = Request::builder().uri(path);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    build_router(AppState::without_db())
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn a_request_from_a_sibling_host_is_refused_before_any_route_runs() {
    for path in ["/healthz", "/api-docs/openapi.json", "/no/such/route"] {
        assert_eq!(
            status(path, &[("sec-fetch-site", "same-site")]).await,
            StatusCode::FORBIDDEN,
            "{path}"
        );
    }
}

#[tokio::test]
async fn the_spa_probes_and_scrapers_are_not_affected() {
    assert_eq!(
        status("/healthz", &[("sec-fetch-site", "same-origin")]).await,
        StatusCode::OK
    );
    assert_eq!(status("/healthz", &[]).await, StatusCode::OK);
}
