//! The chart half of the deployment bounds, asserted against the templates.
//!
//! # Why a Rust test reads YAML
//!
//! Three of this package's numbers live on both sides of a seam — the drain
//! deadline in `egret-server` and `terminationGracePeriodSeconds` in the chart;
//! `/readyz` as a handler and `/readyz` as a probe target. Each pair is correct
//! only together, and nothing else in the tree can fail when one half moves.
//! That is the seam class this repo keeps shipping defects through, so the
//! relation is asserted rather than commented.
//!
//! The templates are not parseable YAML (`replicas: {{ … }}` is a Go template,
//! not a mapping), so these read text. That is a weaker instrument than a parse
//! and it is enough for the one question asked: did the probe stop being a
//! `tcpSocket`.

use std::time::Duration;

fn read(rel: &str) -> String {
    let path = format!("{}/../../{rel}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
}

fn deployment() -> String {
    read("deploy/helm/egret/templates/backend-deployment.yaml")
}

fn values() -> String {
    read("deploy/helm/egret/values.yaml")
}

/// The value of the first `key: <integer>` line in `yaml`.
fn scalar_int(yaml: &str, key: &str) -> u64 {
    yaml.lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            (k.trim() == key).then(|| v.trim())
        })
        .unwrap_or_else(|| panic!("no `{key}:` line"))
        .parse()
        .unwrap_or_else(|e| panic!("`{key}` is not an integer: {e}"))
}

/// The finding this exists to close: `/readyz` is a handler that pings the
/// database, and the probe was a `tcpSocket`, so it had never once been called.
/// A pod with a dead metastore still had a listening socket and therefore still
/// received traffic.
#[test]
fn the_readiness_probe_actually_calls_readyz() {
    let d = deployment();
    let readiness = d
        .split("readinessProbe:")
        .nth(1)
        .expect("no readinessProbe in the backend deployment");
    assert!(
        readiness.contains("httpGet"),
        "the readiness probe must be an HTTP GET; a tcpSocket probe passes on a \
         pod whose database is gone"
    );
    assert!(
        readiness.contains("path: /readyz"),
        "the readiness probe must call /readyz — the only handler that checks a \
         dependency"
    );
}

/// The liveness probe must NOT check the database.
///
/// This is the tempting simplification and it is a trap: pointing liveness at
/// `/readyz` turns a Postgres blip into a simultaneous restart of every replica,
/// which is strictly worse than the outage it reacts to. The distinction is only
/// visible in the chart, so it is pinned here.
#[test]
fn the_liveness_probe_checks_the_process_and_not_the_database() {
    let d = deployment();
    let liveness = d
        .split("livenessProbe:")
        .nth(1)
        .expect("no livenessProbe in the backend deployment");
    assert!(
        liveness.contains("path: /healthz"),
        "liveness must call /healthz"
    );
    assert!(
        !liveness.contains("/readyz"),
        "liveness must not depend on the database: a blip would restart every \
         replica at once"
    );
}

/// BestEffort QoS was the finding; a `resources:` block that the template never
/// reads would be the silent way to keep it.
#[test]
fn the_backend_container_declares_requests_and_limits() {
    let d = deployment();
    assert!(
        d.contains(".Values.backend.resources"),
        "the deployment must render backend.resources, or the values below are \
         decoration and the container stays BestEffort"
    );
    let v = values();
    let backend = v
        .split("\nfrontend:")
        .next()
        .expect("values.yaml must start with the backend block");
    for needle in ["resources:", "requests:", "limits:", "cpu:", "memory:"] {
        assert!(
            backend.contains(needle),
            "backend.resources must declare `{needle}`: {backend}"
        );
    }
}

/// The seam that makes the graceful shutdown reachable at all.
///
/// Kubernetes' default grace period is 30 s — below even the per-request
/// deadline — so leaving it unset would have meant SIGKILL arriving in the
/// middle of every drain and the whole of this package's item 3 doing nothing.
#[test]
fn the_drain_deadline_fits_inside_the_pods_termination_grace_period() {
    let d = deployment();
    assert!(
        d.contains(".Values.backend.terminationGracePeriodSeconds"),
        "the deployment must set terminationGracePeriodSeconds; the default 30 s \
         is shorter than the drain"
    );
    let grace = Duration::from_secs(scalar_int(&values(), "terminationGracePeriodSeconds"));
    assert!(
        grace > egret_server::DRAIN_DEADLINE,
        "terminationGracePeriodSeconds ({grace:?}) must exceed DRAIN_DEADLINE \
         ({:?}), or SIGKILL ends the drain before the drain does",
        egret_server::DRAIN_DEADLINE
    );
}

/// And the drain must outlast the request it is draining, or it cuts in half the
/// very requests it exists to protect.
#[test]
fn the_drain_deadline_outlasts_a_full_length_request() {
    assert!(
        egret_server::DRAIN_DEADLINE > egret_dataapi::serving::SERVING_REQUEST_DEADLINE,
        "DRAIN_DEADLINE ({:?}) must exceed SERVING_REQUEST_DEADLINE ({:?}): a \
         request admitted just before SIGTERM has to be able to finish or time \
         out on its own terms",
        egret_server::DRAIN_DEADLINE,
        egret_dataapi::serving::SERVING_REQUEST_DEADLINE
    );
}
