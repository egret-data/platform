//! What the three serving bounds put in a log line — asserted by reading it back.
//!
//! # Why this is a whole binary for one test
//!
//! It lived in `serving::guard`'s unit tests first and **flaked there**, in a way
//! worth recording because it is not obvious and it is not this test's fault.
//! `tracing` caches a callsite's `Interest` process-wide. `set_default` is
//! thread-local, so in the 365-test lib binary another test's subscriber can be
//! the one that first evaluates `serving_deadline`'s `warn!` callsite; if it is
//! not interested, the callsite is cached disabled and this test's subscriber
//! never sees the event. Observed exactly once, on the deadline case only, with
//! the shed and the body cap in the same test passing — which is the signature of
//! a race on a per-callsite cache rather than a broken assertion.
//!
//! A binary containing one test has one subscriber, so the cache cannot be
//! poisoned. Nothing about the property changes; only what else is running.
//!
//! # The property
//!
//! On this surface a URI's query string **is the caller's parameter values**.
//! `dataapi.access_log.request_path` stores the path alone, no other serving log
//! line records a URI, and `serving::error`'s taxonomy is deliberately built so
//! no parameter name, bound or constraint kind escapes to a caller. A WARN
//! printing `?customer_email=…` undoes that from the side an operator reads —
//! shipped to a log store, indexed, and pasted into tickets.

use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::routing::post;
use axum::Router;
use egret_dataapi::serving::guard::{
    serving_admit, serving_body_limit, serving_bulkhead, serving_deadline,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tower::ServiceExt;

/// Collects `tracing` output so the test can assert on what a bound wrote.
///
/// Same device as `tests/serving_auth.rs`'s, and for the same reason: a promise
/// about a log line is prose until a subscriber reads it back.
#[derive(Clone, Default)]
struct LogCapture(Arc<Mutex<Vec<u8>>>);

struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log lock").extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogCapture {
    type Writer = LogWriter;
    fn make_writer(&'a self) -> Self::Writer {
        LogWriter(self.0.clone())
    }
}

/// A query string whose values are the sort of thing a Data API caller really
/// sends, and which cannot appear in a log line by coincidence.
const SECRET_QS: &str = "customer_email=ada%40example.com&account=ACC-90210";

const PATH: &str = "/svc/reports/daily";

/// Drive `app` at a URI carrying [`SECRET_QS`] with a DEBUG subscriber
/// installed, and return `(status, everything logged)`.
///
/// DEBUG, not WARN: two of the three bounds log at DEBUG on purpose (see
/// `serving_admit`'s doc), and a capture that could not see them would report
/// "no parameter leaked" about lines it never read.
async fn drive_and_capture(app: Router) -> (StatusCode, String) {
    let cap = LogCapture::default();
    let sub = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(cap.clone())
        .without_time()
        .finish();
    let uri = format!("{PATH}?{SECRET_QS}");
    let res = {
        let _guard = tracing::subscriber::set_default(sub);
        app.oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri(&uri)
                // Declared, not sent: the body cap reads the header and never
                // the body, which is the whole point of that constant's doc.
                .header(header::CONTENT_LENGTH, "1048576")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
    };
    let status = res.status();
    let bytes = cap.0.lock().expect("log lock").clone();
    (
        status,
        String::from_utf8(bytes).expect("log output is utf-8"),
    )
}

/// The control matters as much as the assertion: a bound that logged *nothing*
/// would satisfy "the query string is absent" perfectly.
fn assert_query_absent_and_path_present(log: &str, what: &str) {
    assert!(
        log.contains(PATH),
        "{what} must log the path, or this test proves nothing by finding no \
         query string in an empty log:\n{log}"
    );
    for leaked in ["customer_email", "ada%40example.com", "ACC-90210", "?"] {
        assert!(
            !log.contains(leaked),
            "{what} logged `{leaked}` — that is a caller's parameter value, and \
             no other instrument on this surface records one:\n{log}"
        );
    }
}

/// Each of the three bounds, driven for real, with its own output read back.
#[tokio::test]
async fn a_bound_logs_the_path_and_never_the_query_string() {
    // The shed: a semaphore with no permits refuses immediately.
    let full = serving_bulkhead(0);
    let shedding = Router::new()
        .route(PATH, post(|| async { "unreachable" }))
        .layer(axum::middleware::from_fn(move |req, next| {
            serving_admit(full.clone(), req, next)
        }));
    let (status, log) = drive_and_capture(shedding).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_query_absent_and_path_present(&log, "the bulkhead shed");

    // The body cap: a `Content-Length` above the cap, declared honestly.
    let capped = Router::new()
        .route(PATH, post(|| async { "unreachable" }))
        .layer(axum::middleware::from_fn(move |req, next| {
            serving_body_limit(4, req, next)
        }));
    let (status, log) = drive_and_capture(capped).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_query_absent_and_path_present(&log, "the body cap");

    // The deadline: a handler that never returns, bounded at zero.
    let slow = Router::new()
        .route(
            PATH,
            post(|| async {
                std::future::pending::<()>().await;
                "unreachable"
            }),
        )
        .layer(axum::middleware::from_fn(move |req, next| {
            serving_deadline(Duration::ZERO, req, next)
        }));
    let (status, log) = drive_and_capture(slow).await;
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
    assert_query_absent_and_path_present(&log, "the deadline");
}
