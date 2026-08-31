//! `GET /metrics` through the COMPOSED router.
//!
//! Its own test binary because the thing under test is process-global: the
//! `metrics` recorder is installed once per process, so "before it is installed"
//! and "after it is installed" cannot be two tests in one binary — they would
//! race, and whichever ran second would assert the other's state. The 503 case
//! lives in `metrics_without_recorder.rs` for exactly that reason.
//!
//! No database: `AppState::without_db()` mounts the health group, which is where
//! `/metrics` lives.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_server::{build_router, AppState};
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn scrape() -> (StatusCode, String, String) {
    let res = build_router(AppState::without_db())
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let content_type = res
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        content_type,
        String::from_utf8(bytes.to_vec()).unwrap(),
    )
}

/// The endpoint renders what was actually recorded, and the number MOVES.
///
/// The increment is deliberately made between two scrapes rather than asserted
/// once: a test that only checks the series is present passes against a recorder
/// that was installed and never written to, which is the failure mode this whole
/// package exists to rule out.
#[tokio::test]
async fn the_endpoint_renders_a_counter_whose_value_moved() {
    egret_server::install_metrics_recorder().expect("install the recorder");

    // A metric of this test's own, so the assertion does not depend on which
    // other test in this binary ran first.
    metrics::counter!("egret_server_metrics_endpoint_probe").increment(3);
    let (status, content_type, before) = scrape().await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        content_type.starts_with("text/plain"),
        "a Prometheus scrape must be text/plain exposition, got {content_type:?}"
    );
    assert!(
        before.contains("egret_server_metrics_endpoint_probe 3"),
        "the scrape does not carry the recorded value:\n{before}"
    );

    metrics::counter!("egret_server_metrics_endpoint_probe").increment(4);
    let (_, _, after) = scrape().await;
    assert!(
        after.contains("egret_server_metrics_endpoint_probe 7"),
        "the endpoint served a stale render — the value moved to 7:\n{after}"
    );
}

/// Installing twice is success, not an error.
///
/// `install_recorder` on the underlying builder fails when a global recorder
/// already exists. A boot sequence that propagated that would turn "this process
/// already has metrics" into a startup failure, and a test binary that calls it
/// per test would fail on the second test.
#[tokio::test]
async fn installing_the_recorder_twice_is_a_no_op() {
    egret_server::install_metrics_recorder().expect("first install");
    egret_server::install_metrics_recorder().expect("second install must be a no-op, not an error");
}

/// The Data API's metric descriptions are registered by the installer.
///
/// `describe_*` is what produces `# HELP` and `# TYPE`, and it only works if it
/// runs AFTER the recorder is installed — a description registered against the
/// no-op recorder is silently discarded. That ordering is invisible in the
/// source (both lines are in one function, in the right order) and this is what
/// pins it.
#[tokio::test]
async fn the_installer_registers_the_data_api_metric_descriptions() {
    egret_server::install_metrics_recorder().expect("install the recorder");
    // A description alone renders nothing; the series has to exist for the
    // exporter to emit its HELP line beside it.
    metrics::counter!("egret_dataapi_preauth_rejections_total").increment(0);
    let (_, _, text) = scrape().await;
    assert!(
        text.contains("# HELP egret_dataapi_preauth_rejections_total"),
        "the pre-auth counter has no help text, so `describe()` did not run \
         after the recorder was installed:\n{text}"
    );
}

/// A latency metric renders as a HISTOGRAM, not as a summary.
///
/// `metrics-exporter-prometheus` emits a summary with pre-computed quantiles
/// unless the builder gave that metric buckets — and that difference is
/// invisible until someone tries to use the number. A summary's quantiles cannot
/// be aggregated across replicas: "the p95 across three egret pods" is not
/// computable from three summaries and has no correct answer, whereas cumulative
/// buckets add. The install path is the only place that choice is made, and this
/// is the only thing that checks it was made.
///
/// The probe's name ends in `_seconds` on purpose: that suffix is the whole
/// selector now, so a probe named anything else would be asserting the fallback
/// rather than the configuration.
#[tokio::test]
async fn the_latency_histogram_renders_as_buckets_and_not_as_quantiles() {
    egret_server::install_metrics_recorder().expect("install the recorder");
    metrics::histogram!("egret_server_metrics_endpoint_latency_probe_seconds").record(0.42);
    let (_, _, text) = scrape().await;
    assert!(
        text.contains("egret_server_metrics_endpoint_latency_probe_seconds_bucket{le="),
        "no buckets — the exporter fell back to a summary, whose quantiles \
         cannot be aggregated across replicas:\n{text}"
    );
    assert!(
        !text.contains("egret_server_metrics_endpoint_latency_probe_seconds{quantile="),
        "a summary was emitted beside the buckets:\n{text}"
    );
    // The topmost bucket must be above the endpoint timeout ceiling, or every
    // slow query lands in `+Inf` and a degraded endpoint has an unreadable p95.
    assert!(
        text.contains("egret_server_metrics_endpoint_latency_probe_seconds_bucket{le=\"60\"}"),
        "the bucket set no longer reaches a minute:\n{text}"
    );
}

/// The second-shaped buckets reach the latency histogram and NOTHING ELSE.
///
/// Both names below are the real registered ones, written out as literals rather
/// than derived, because the property is about the exact strings the recorder
/// sees:
///
/// * `egret_dataapi_request_duration_seconds` —
///   `egret_dataapi::serving::metrics::REQUEST_DURATION_SECONDS`, a latency.
/// * `lakekeeper_role_membership_edge_fanout_users` — the embedded lakekeeper's
///   histogram of how many users a single role-membership edge change
///   invalidates (`crates/lakekeeper/src/service/cache_metrics.rs:22` at the
///   pinned rev). It goes through the same `metrics` facade into the same
///   process-global recorder this crate installs, and it counts USERS.
///
/// A global `set_buckets` — which is what the installer used to do — would
/// bucket that count into boundaries topping out at 60, so a fan-out of 500
/// would land in `+Inf` beside a fan-out of 5000 and the histogram would be a
/// constant. The last assertion is that the 500 is still READABLE, which a
/// "no buckets were applied" check on its own would not establish.
#[tokio::test]
async fn only_second_shaped_histograms_get_the_second_shaped_buckets() {
    egret_server::install_metrics_recorder().expect("install the recorder");
    metrics::histogram!(egret_dataapi::serving::metrics::REQUEST_DURATION_SECONDS).record(0.42);
    metrics::histogram!("lakekeeper_role_membership_edge_fanout_users").record(500.0);
    let (_, _, text) = scrape().await;

    assert!(
        text.contains("egret_dataapi_request_duration_seconds_bucket{le="),
        "the suffix matcher no longer covers the Data API's latency histogram, \
         so it silently became a summary:\n{text}"
    );
    assert!(
        !text.contains("lakekeeper_role_membership_edge_fanout_users_bucket"),
        "a COUNT histogram was given second-shaped buckets; every fan-out above \
         60 now lands in +Inf and the distribution is a constant:\n{text}"
    );
    assert!(
        text.contains("lakekeeper_role_membership_edge_fanout_users{quantile=\"0.99\"} 500"),
        "the recorded fan-out of 500 is not readable from the scrape:\n{text}"
    );
}

/// `/metrics` is not part of the API contract.
///
/// The committed `openapi.json` is byte-asserted by this crate's own
/// `committed_openapi_json_is_current`, so a route that leaked into the
/// generated document would already fail there. This states the intent
/// positively rather than relying on a diff nobody re-reads: a Prometheus scrape
/// is `text/plain` exposition consumed by a scrape job that reads no schema, and
/// putting it in the document would add a route to every generated client in the
/// tree.
#[test]
fn the_scrape_endpoint_is_not_in_the_openapi_document() {
    let doc = egret_server::openapi_doc();
    assert!(
        !doc.paths.paths.contains_key("/metrics"),
        "/metrics must stay out of the generated document, like /healthz and /readyz"
    );
    assert!(!doc.paths.paths.contains_key("/healthz"), "precondition");
}
