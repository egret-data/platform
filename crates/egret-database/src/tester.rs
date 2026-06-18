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
                // Re-attach auth on every poll: a secured Trino requires it on the
                // nextUri GETs too, not just the initial POST.
                let mut r = client.get(&next).header("X-Trino-User", user);
                if let Some(s) = secret {
                    r = r.basic_auth(user, Some(s));
                }
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

// ── StarRocks (MySQL protocol) ────────────────────────────────────────────────

pub async fn test_starrocks(
    host: &str,
    port: i32,
    options: &ConnOptions,
    secret: Option<&str>,
) -> TestOutcome {
    use mysql_async::prelude::Queryable;

    let started = Instant::now();
    let port = match u16::try_from(port) {
        Ok(p) => p,
        Err(_) => return fail(started, format!("invalid port: {port}")),
    };
    let user = options.user.clone().unwrap_or_else(|| "root".into());
    let mut builder = mysql_async::OptsBuilder::default()
        .ip_or_hostname(host.to_string())
        .tcp_port(port)
        .user(Some(user))
        .prefer_socket(false);
    if let Some(s) = secret {
        builder = builder.pass(Some(s.to_string()));
    }
    if let Some(db) = &options.database {
        builder = builder.db_name(Some(db.clone()));
    }
    let opts: mysql_async::Opts = builder.into();

    let work = async {
        let mut conn = mysql_async::Conn::new(opts)
            .await
            .map_err(|e| format!("connect: {e}"))?;
        let got: Option<i64> = conn
            .query_first::<i64, _>("SELECT 1")
            .await
            .map_err(|e| format!("query: {e}"))?;
        let _ = conn.disconnect().await;
        if got == Some(1) {
            Ok(())
        } else {
            Err(format!("unexpected result: {got:?}"))
        }
    };

    match tokio::time::timeout(TIMEOUT, work).await {
        Ok(Ok(())) => ok(started),
        Ok(Err(e)) => fail(started, e),
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
