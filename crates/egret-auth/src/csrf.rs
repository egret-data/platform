//! Double-submit CSRF: mutating requests must echo the CSRF cookie in a header.

use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum_extra::extract::cookie::CookieJar;

use crate::cookie::{read_csrf, secure_cookies, CSRF_HEADER};

/// Reject mutating requests whose `X-Egret-CSRF` header is missing or ≠ the CSRF cookie.
/// Safe methods pass through. Pre-auth POSTs (login/oidc) are mounted on a router without
/// this layer, so they are not affected.
pub async fn csrf_layer(jar: CookieJar, request: Request, next: Next) -> Response {
    let method = request.method();
    let is_mutating = matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    );
    if !is_mutating {
        return next.run(request).await;
    }

    let cookie_val = read_csrf(&jar, secure_cookies()).map(|c| c.value().to_string());
    let header_val = request
        .headers()
        .get(CSRF_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    match (cookie_val, header_val) {
        (Some(c), Some(h)) if c == h && !c.is_empty() => next.run(request).await,
        _ => (StatusCode::FORBIDDEN, "csrf check failed").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use axum::routing::post;
    use axum::Router;
    use tower::ServiceExt;

    fn app() -> Router {
        Router::new()
            .route("/m", post(|| async { "ok" }))
            .layer(axum::middleware::from_fn(csrf_layer))
    }

    #[tokio::test]
    async fn rejects_post_without_matching_header() {
        let resp = app()
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/m")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn allows_post_with_matching_cookie_and_header() {
        let resp = app()
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/m")
                    .header("cookie", "egret_csrf=abc")
                    .header("x-egret-csrf", "abc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn allows_get_without_csrf() {
        let app = Router::new()
            .route("/g", axum::routing::get(|| async { "ok" }))
            .layer(axum::middleware::from_fn(csrf_layer));
        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/g")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
