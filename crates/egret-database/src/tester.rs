//! On-demand connection test: a real `SELECT 1` per engine.

use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::model::{ConnOptions, Engine, TestOutcome};

const TIMEOUT: Duration = Duration::from_secs(10);

/// Dispatch a connection test by engine. `secret` is the opened plaintext (if any).
pub async fn test_connection(
    engine: Engine,
    host: &str,
    port: i32,
    options: &ConnOptions,
    secret: Option<&str>,
) -> TestOutcome {
    match engine {
        Engine::Trino => test_trino(host, port, options, secret).await,
        Engine::StarRocks => test_starrocks(host, port, options, secret).await,
    }
}

// ── Trino (statement protocol) ────────────────────────────────────────────────

#[derive(Deserialize)]
struct TrinoResp {
    #[serde(default)]
    #[serde(rename = "nextUri")]
    next_uri: Option<String>,
    #[serde(default)]
    error: Option<TrinoError>,
}

#[derive(Deserialize)]
struct TrinoError {
    message: String,
}

/// Run `SELECT 1` via `POST /v1/statement` + `nextUri` polling.
pub async fn test_trino(
    host: &str,
    port: i32,
    options: &ConnOptions,
    secret: Option<&str>,
) -> TestOutcome {
    let started = Instant::now();
    let scheme = options.http_scheme.as_deref().unwrap_or("http");
    let user = options.user.as_deref().unwrap_or("egret");
    let client = match reqwest::Client::builder().timeout(TIMEOUT).build() {
        Ok(c) => c,
        Err(e) => return fail(started, format!("client build: {e}")),
    };
    let url = format!("{scheme}://{host}:{port}/v1/statement");
    let mut req = client
        .post(&url)
        .header("X-Trino-User", user)
        .body("SELECT 1");
    if let Some(s) = secret {
        req = req.basic_auth(user, Some(s));
    }
    let mut resp: TrinoResp = match send_json(req).await {
        Ok(r) => r,
        Err(e) => return fail(started, e),
    };
    loop {
        if let Some(err) = resp.error {
            return fail(started, err.message);
        }
        match resp.next_uri.take() {
            Some(next) => {
                let r = client.get(&next);
                resp = match send_json(r).await {
                    Ok(r) => r,
                    Err(e) => return fail(started, e),
                };
            }
            None => break, // FINISHED with no error
        }
    }
    ok(started)
}

async fn send_json(req: reqwest::RequestBuilder) -> Result<TrinoResp, String> {
    let resp = req.send().await.map_err(|e| format!("request: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("http {}", resp.status()));
    }
    resp.json::<TrinoResp>()
        .await
        .map_err(|e| format!("decode: {e}"))
}

// ── StarRocks (filled in Task 8) ──────────────────────────────────────────────

pub async fn test_starrocks(
    _host: &str,
    _port: i32,
    _options: &ConnOptions,
    _secret: Option<&str>,
) -> TestOutcome {
    // Implemented in Task 8.
    TestOutcome {
        ok: false,
        latency_ms: 0,
        detail: "starrocks tester not yet implemented".into(),
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
