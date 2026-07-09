//! On-demand connection test: a real `SELECT 1` per engine.
//!
//! Runs the probe through the shared execution `Connector` (the same client the
//! query engine uses) rather than re-implementing the Trino statement-protocol /
//! StarRocks MySQL clients here. Because both crates are now folded into
//! `egret-query`, calling `crate::execution::connector` introduces no dependency
//! cycle.

use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use crate::execution::connector::{build_connector, ConnSpec};
use crate::execution::model::Limits;
use crate::registry::model::{ConnOptions, Engine, TestOutcome};

const TIMEOUT: Duration = Duration::from_secs(10);

/// Dispatch a connection test by engine. `secret` is the opened plaintext (if any).
///
/// Builds a `ConnSpec` from the raw connection params and runs `SELECT 1`
/// through the shared `Connector`, measuring round-trip latency. The whole probe
/// is bounded by `TIMEOUT` (StarRocks' `execute` has no internal deadline; Trino's
/// reqwest client is already timeout-bounded, so this is a redundant backstop there).
pub async fn test_connection(
    engine: Engine,
    host: &str,
    port: i32,
    options: &ConnOptions,
    secret: Option<&str>,
) -> TestOutcome {
    let started = Instant::now();
    let spec = ConnSpec {
        engine,
        host: host.to_string(),
        port,
        options: options.clone(),
        secret: secret.map(str::to_string),
        session_user: None,
        bearer_token: None,
    };
    let connector = build_connector(&spec);
    let limits = Limits {
        max_rows: 1,
        timeout: TIMEOUT,
    };
    let work = connector.execute("SELECT 1", &limits, CancellationToken::new());
    match tokio::time::timeout(TIMEOUT, work).await {
        Ok(Ok(_)) => ok(started),
        Ok(Err(e)) => fail(started, e.to_string()),
        Err(_) => fail(started, "timeout".into()),
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn ok(started: Instant) -> TestOutcome {
    TestOutcome {
        ok: true,
        latency_ms: started.elapsed().as_millis() as u64,
        detail: "SELECT 1 ok".into(),
    }
}

fn fail(started: Instant, detail: String) -> TestOutcome {
    TestOutcome {
        ok: false,
        latency_ms: started.elapsed().as_millis() as u64,
        detail,
    }
}
