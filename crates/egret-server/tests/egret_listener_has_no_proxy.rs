//! The other half of D17: egret's own router serves neither the ticket route nor the
//! relay, so nothing an app's pod or an app's page says can reach the proxy through egret's
//! host. (`egret-apps/tests/proxy.rs` proves the apps router serves nothing of egret's.)

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_server::{build_router, AppState};
use tower::ServiceExt;

#[tokio::test]
async fn the_egret_listener_has_no_ticket_route_and_no_pod_prefixes() {
    for path in [
        "/_ticket?t=egt_x",
        "/e/abc/",
        "/e/abc/api/kernel/save",
        "/r/abc/ws",
    ] {
        let res = build_router(AppState::without_db())
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{path}");
    }
}
