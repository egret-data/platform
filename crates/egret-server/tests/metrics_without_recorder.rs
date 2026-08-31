//! `GET /metrics` in a process where no recorder was ever installed.
//!
//! Its own binary, and it must stay that way: `install_metrics_recorder` sets a
//! process-global, so a single test anywhere in this binary that called it would
//! make the assertion below unreachable — and it would fail by *passing*
//! whenever it happened to run first.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_server::{build_router, AppState};
use http_body_util::BodyExt;
use tower::ServiceExt;

/// 503, not an empty 200.
///
/// The difference is the whole value of the endpoint during a bad deploy. A
/// scrape job alerts on a target that is DOWN; it has no rule for a target that
/// is up and cheerfully reporting that nothing has ever happened — which is
/// exactly what an empty 200 says, and exactly what a process with no recorder
/// would say forever.
#[tokio::test]
async fn a_process_with_no_recorder_answers_503_rather_than_an_empty_200() {
    let res = build_router(AppState::without_db())
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "an uninstrumented process must not look like an idle one"
    );
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        body.contains("no metrics recorder"),
        "the body must say why, so an operator is not left diffing an empty \
         scrape: {body}"
    );
}
