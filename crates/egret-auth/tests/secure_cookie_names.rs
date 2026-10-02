//! With `session.secure` on, CSRF is checked against `__Host-egret_csrf` ONLY.
//!
//! This is its own test binary because the setting is process-wide: the unit tests in
//! `src/` run in plain mode, and flipping it there would race them.
//!
//! The attack it closes (spec §6.1, measured in S0): author JavaScript on the apps
//! origin sets `egret_csrf=evil; Domain=<parent>`, then POSTs to egret with
//! `X-Egret-CSRF: evil`. A double-submit check that accepts the plain cookie compares
//! `evil == evil` and passes.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::Router;
use egret_auth::cookie::set_secure_cookies;
use egret_auth::csrf::csrf_layer;
use tower::ServiceExt;

fn app() -> Router {
    Router::new()
        .route("/m", post(|| async { "ok" }))
        .layer(axum::middleware::from_fn(csrf_layer))
}

async fn post_with(cookie: &str, header: &str) -> StatusCode {
    app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/m")
                .header("cookie", cookie)
                .header("x-egret-csrf", header)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn a_tossed_plain_csrf_cookie_does_not_pass_in_a_secure_deployment() {
    set_secure_cookies(true);
    assert_eq!(
        post_with("egret_csrf=evil", "evil").await,
        StatusCode::FORBIDDEN,
        "the plain name must not satisfy the check"
    );
    // Both present: the host-prefixed one is the only one consulted, so a header that
    // matches the tossed value still fails.
    assert_eq!(
        post_with("egret_csrf=evil; __Host-egret_csrf=real", "evil").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post_with("egret_csrf=evil; __Host-egret_csrf=real", "real").await,
        StatusCode::OK
    );
}
