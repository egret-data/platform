//! The process's metrics recorder and the `GET /metrics` scrape endpoint.
//!
//! # Why the handle is a process global and not a field on `AppState`
//!
//! The thing it is a handle to — `metrics`' recorder — already is one. The
//! facade's `counter!`/`histogram!` macros resolve through a single global
//! recorder installed once per process, which is what lets a library crate
//! (`egret-dataapi`) instrument itself without being handed anything. Putting
//! the READ side in application state while the WRITE side is a global would
//! create two sources of truth about whether metrics exist: a `build_router`
//! called with `metrics: None` after a recorder was installed would answer 503
//! on a scrape while happily counting into it.
//!
//! It also keeps this package from touching [`crate::AppState`], which every
//! composition site and several tests construct by hand.
//!
//! # Why the route is not in the OpenAPI document
//!
//! `/healthz` and `/readyz` carry no `#[utoipa::path]` and so appear in no
//! merged seam document; the committed `openapi.json` — byte-asserted by
//! `committed_openapi_json_is_current` in this crate — does not describe them.
//! `/metrics` follows that precedent, and it is the right precedent rather than
//! a convenient one: the generated document is the contract the frontend's types
//! are generated from and the surface an API consumer codes against. A
//! Prometheus scrape endpoint is neither. It emits `text/plain` in an exposition
//! format OpenAPI cannot describe usefully, its consumer is a scrape job that
//! reads no schema, and adding it would put a route in every generated client in
//! the tree.
//!
//! So: no annotation, no regeneration, and the byte assertion stays green
//! without being touched.
//!
//! # Exposure
//!
//! Mounted unauthenticated beside the health routes, which is what a Prometheus
//! scrape job in the same cluster expects. That is only defensible because of
//! the label discipline one layer down: `egret_dataapi::serving::metrics` carries
//! no workspace, no endpoint id, no path and no client id, so a scrape reveals
//! deployment-wide request rates and latency and nothing that belongs to a
//! tenant. **A label added later that names a tenant would silently turn this
//! into a cross-tenant disclosure** — read that module's cardinality section
//! before adding one, because the same rule that keeps the series count finite
//! is the one that keeps this endpoint safe to leave open.

use std::sync::{Mutex, OnceLock};

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use metrics_exporter_prometheus::{BuildError, PrometheusBuilder, PrometheusHandle};

/// The Prometheus text exposition format's content type. Written out rather
/// than left to default to `text/plain`, because a scrape that negotiates
/// nothing still records the version it parsed.
const EXPOSITION_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// Histogram bucket boundaries, in seconds.
///
/// One global set, because `metrics-exporter-prometheus` matches buckets by
/// metric-name pattern and every histogram this process records today is a
/// request latency.
///
/// The range runs to a minute on purpose. The Data API's own latency is
/// dominated by an engine round trip whose ceiling is the endpoint's published
/// `timeout_seconds`, clamped by `TIMEOUT_CEILING_SECS` — so a bucket set that
/// stopped at 10s would put every slow query in `+Inf` and make the p95 of a
/// degraded endpoint unreadable exactly when it is being read.
const LATENCY_BUCKETS_SECONDS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0,
];

/// Set once by [`install_recorder`]; read by the scrape handler.
static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// Serialises the install so two threads cannot both pass the "not installed
/// yet" check and race into `install_recorder`, where the second would fail
/// because the global recorder was already set.
///
/// Only ever contended at startup — this is not on any request path.
static INSTALL_LOCK: Mutex<()> = Mutex::new(());

/// Install the process-wide Prometheus recorder and register the metric
/// descriptions.
///
/// Idempotent: a second call is a no-op that returns `Ok`. That matters because
/// a test binary runs many tests against one process, and "already installed" is
/// success, not a failure worth propagating into a boot sequence.
///
/// Call it before serving traffic. Until it runs, every `counter!`/`histogram!`
/// in the tree records into the no-op recorder the facade defaults to, and
/// `GET /metrics` answers 503 rather than an empty 200 — an empty scrape is
/// indistinguishable from "nothing has happened yet", which is the reading an
/// operator must not be given while the recorder is missing.
pub fn install_recorder() -> Result<(), BuildError> {
    let _guard = INSTALL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if HANDLE.get().is_some() {
        return Ok(());
    }
    let handle = PrometheusBuilder::new()
        .set_buckets(LATENCY_BUCKETS_SECONDS)?
        .install_recorder()?;
    // Names, units and help text. Registered here rather than in each crate's
    // startup because "describe once, at the point the recorder exists" is the
    // only ordering that works: a description registered before the recorder is
    // installed goes to the no-op recorder and is lost.
    egret_dataapi::serving::metrics::describe();
    let _ = HANDLE.set(handle);
    Ok(())
}

/// `GET /metrics` — the Prometheus text exposition of everything recorded so far.
pub(crate) async fn metrics_scrape() -> Response {
    let Some(handle) = HANDLE.get() else {
        // 503 and not 200: see [`install_recorder`]. A scrape job alerts on a
        // failing target; it cannot alert on a target that is up and reporting
        // that nothing has ever happened.
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::CONTENT_TYPE, EXPOSITION_CONTENT_TYPE)],
            "# no metrics recorder is installed in this process\n",
        )
            .into_response();
    };
    // Cheap, and the only place it can honestly go. `install_recorder` builds no
    // background task (that is `PrometheusBuilder::install`, which also binds its
    // own HTTP listener — we mount the route on the existing server instead), so
    // upkeep has no other driver. Doing it per scrape ties the work to the only
    // reader there is.
    handle.run_upkeep();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, EXPOSITION_CONTENT_TYPE)],
        handle.render(),
    )
        .into_response()
}
