//! Refuse requests a sibling host made with the viewer's cookies.
//!
//! The apps origin (`apps.<host>`) runs author JavaScript and is **same-site** with
//! egret, so egret's `SameSite=Lax` cookies ride any request that code makes — a
//! `fetch`, an image, a navigation of its own frame. S0 measured it: a `GET` from the
//! widget reached egret's `whoami` as the viewer, and only CORS kept the reply from the
//! script. Any `GET` with a side effect would therefore be a way to act as the viewer.
//!
//! Browsers label every request with `Sec-Fetch-Site`. egret's own SPA is
//! `same-origin`, an IdP redirect is `cross-site` (or `none` for a typed URL); a
//! request from a sibling host is `same-site`, and nothing in egret legitimately makes
//! one. So the layer refuses them all, with one narrow exception: a top-level document
//! navigation to the OIDC callback, because an IdP hosted on a sibling name
//! (`login.<host>`) redirects the browser back with exactly that label.
//!
//! Defence in depth, not the only line: the audit that no `GET` changes state, header
//! based CSRF and the `__Host-` cookie names stay. A client that sends no
//! `Sec-Fetch-Site` (old browsers, `curl`) is not refused — it is not a browser an
//! attacker's page can drive.

use axum::extract::Request;
use axum::http::{header::HeaderName, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

const SITE: HeaderName = HeaderName::from_static("sec-fetch-site");
const MODE: HeaderName = HeaderName::from_static("sec-fetch-mode");
const DEST: HeaderName = HeaderName::from_static("sec-fetch-dest");

/// The only path a same-site top-level navigation may reach.
const OIDC_CALLBACK: &str = "/api/auth/oidc/callback";

fn header_is(request: &Request, name: &HeaderName, wanted: &str) -> bool {
    request
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case(wanted))
}

pub async fn refuse_same_site(request: Request, next: Next) -> Response {
    if header_is(&request, &SITE, "same-site") {
        let top_level_callback = request.uri().path() == OIDC_CALLBACK
            && header_is(&request, &MODE, "navigate")
            && header_is(&request, &DEST, "document");
        if !top_level_callback {
            return (StatusCode::FORBIDDEN, "same-site requests are not accepted").into_response();
        }
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::routing::any;
    use axum::Router;
    use tower::ServiceExt;

    async fn status(method: &str, path: &str, headers: &[(&str, &str)]) -> StatusCode {
        let app = Router::new()
            .route("/{*rest}", any(|| async { "ok" }))
            .layer(axum::middleware::from_fn(refuse_same_site));
        let mut req = Request::builder().method(method).uri(path);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        app.oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn a_same_site_request_is_refused_whatever_its_method() {
        for method in ["GET", "POST", "PUT", "DELETE"] {
            assert_eq!(
                status(method, "/api/auth/me", &[("sec-fetch-site", "same-site")]).await,
                StatusCode::FORBIDDEN,
                "{method}"
            );
        }
        // the value is a token, compared case-insensitively
        assert_eq!(
            status("GET", "/api/auth/me", &[("sec-fetch-site", "Same-Site")]).await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn every_other_origin_relation_and_a_missing_header_pass() {
        for value in ["same-origin", "cross-site", "none"] {
            assert_eq!(
                status("GET", "/api/auth/me", &[("sec-fetch-site", value)]).await,
                StatusCode::OK,
                "{value}"
            );
        }
        assert_eq!(status("GET", "/api/auth/me", &[]).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn a_same_site_top_level_navigation_to_the_oidc_callback_passes() {
        let headers = [
            ("sec-fetch-site", "same-site"),
            ("sec-fetch-mode", "navigate"),
            ("sec-fetch-dest", "document"),
        ];
        assert_eq!(
            status("GET", "/api/auth/oidc/callback?code=x", &headers).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn the_callback_exception_is_not_a_way_in_for_a_frame_or_a_fetch_or_another_path() {
        // The apps iframe navigating ITSELF to egret is mode=navigate too, but its
        // destination is `iframe`, not `document`.
        let frame = [
            ("sec-fetch-site", "same-site"),
            ("sec-fetch-mode", "navigate"),
            ("sec-fetch-dest", "iframe"),
        ];
        assert_eq!(
            status("GET", "/api/auth/oidc/callback", &frame).await,
            StatusCode::FORBIDDEN
        );
        let fetch = [
            ("sec-fetch-site", "same-site"),
            ("sec-fetch-mode", "cors"),
            ("sec-fetch-dest", "empty"),
        ];
        assert_eq!(
            status("GET", "/api/auth/oidc/callback", &fetch).await,
            StatusCode::FORBIDDEN
        );
        let top_level = [
            ("sec-fetch-site", "same-site"),
            ("sec-fetch-mode", "navigate"),
            ("sec-fetch-dest", "document"),
        ];
        assert_eq!(
            status("GET", "/api/auth/me", &top_level).await,
            StatusCode::FORBIDDEN,
            "only the callback path is exempt"
        );
    }
}
