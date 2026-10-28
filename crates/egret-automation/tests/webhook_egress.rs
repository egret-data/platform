//! Webhook delivery goes through `egret_egress::EgressClient` (package #4,
//! C1 concern 1): an environment proxy cannot take the request off the pinned
//! address, and every vetted address is pinned, not only the last one.
//!
//! Its own test binary because one test sets `HTTP_PROXY` for the process.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::routing::post;
use axum::Router;
use egret_automation::deliver::destination::{Resolver, SsrfPolicy};
use egret_automation::deliver::webhook::{deliver, WebhookError, WebhookPayload};

/// `hook.test` answers with `addrs`; nothing else resolves.
struct Fixed(Vec<SocketAddr>);

#[async_trait::async_trait]
impl Resolver for Fixed {
    async fn resolve(&self, host: &str, _port: u16) -> Result<Vec<SocketAddr>, std::io::Error> {
        if host == "hook.test" {
            Ok(self.0.clone())
        } else {
            Err(std::io::Error::other("unknown host"))
        }
    }
}

/// A receiver on 127.0.0.1 that counts deliveries and answers `status`.
async fn receiver(status: u16) -> (u16, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let app = Router::new().route(
        "/hook",
        post(move || {
            let h = h.clone();
            async move {
                h.fetch_add(1, Ordering::SeqCst);
                axum::http::StatusCode::from_u16(status).unwrap()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (port, hits)
}

fn payload() -> WebhookPayload<'static> {
    WebhookPayload {
        schedule: "s",
        schedule_id: "1",
        ty: "alert",
        state: "success",
        value: None,
        triggered_at: chrono::Utc::now(),
        rows: serde_json::json!([]),
    }
}

fn allow() -> SsrfPolicy {
    SsrfPolicy {
        allow_hosts: vec!["hook.test".into()],
    }
}

fn loopback(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

#[tokio::test]
async fn webhook_regressions() {
    // One test, run in order: the proxy case sets process env vars, and the
    // other cases must not run while they are set.

    // 1. Every vetted address is pinned. `::1` refuses (the receiver listens
    //    on 127.0.0.1 only); before the move only the LAST address was kept,
    //    so this delivery failed.
    let (port, hits) = receiver(200).await;
    let r = Fixed(vec![
        loopback(port),
        SocketAddr::from(([0u16, 0, 0, 0, 0, 0, 0, 1], port)),
    ]);
    deliver(
        &format!("http://hook.test:{port}/hook"),
        None,
        &payload(),
        &allow(),
        &r,
    )
    .await
    .expect("the first pinned address is used");
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    // 2. A non-2xx answer (a redirect included: it is not followed) is
    //    `Status`, with the text it always had.
    let (port, _) = receiver(302).await;
    let err = deliver(
        &format!("http://hook.test:{port}/hook"),
        None,
        &payload(),
        &allow(),
        &Fixed(vec![loopback(port)]),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, WebhookError::Status(302)), "{err:?}");
    assert_eq!(err.to_string(), "webhook returned 302");

    // 3. An environment proxy is ignored. Honoured, the request would go to
    //    the dead proxy on port 9 and fail; pinned, it reaches the receiver.
    let (port, hits) = receiver(200).await;
    for k in ["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"] {
        std::env::set_var(k, "http://127.0.0.1:9");
    }
    let res = deliver(
        &format!("http://hook.test:{port}/hook"),
        None,
        &payload(),
        &allow(),
        &Fixed(vec![loopback(port)]),
    )
    .await;
    for k in ["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"] {
        std::env::remove_var(k);
    }
    res.expect("the environment proxy must not carry the delivery");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}
