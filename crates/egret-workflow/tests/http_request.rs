//! Package #4 Task C3: `io.kestra.plugin.core.http.Request` through the real
//! worker, over the registry `runtime::handlers_for` builds, against a local
//! server on 127.0.0.1. Requires DATABASE_URL.
//!
//! Loopback is refused by the egress guard (spec D1), so the positive tests
//! run with `workflow.http.allow_hosts = ["127.0.0.1"]`, and the refusals run
//! with it unset (or with only the first hop listed) and check that the
//! server saw nothing.

mod common;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use egret_metastore::Db;
use egret_workflow::logs::{spawn_writer, LogLimits, LogRepo, WriterConfig};
use egret_workflow::runners::secrets::{SecretError, SecretSource};
use egret_workflow::worker::{start_execution, tick, Handlers};
use serde_json::{json, Value};
use uuid::Uuid;

const TOKEN: &str = "s3cr3t-t0ken-value";
const REQUEST: &str = "io.kestra.plugin.core.http.Request";

// ---- the local server -------------------------------------------------------

#[derive(Clone, Default)]
struct Hits(Arc<AtomicUsize>);

impl Hits {
    fn count(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

struct Server {
    addr: SocketAddr,
    hits: Hits,
}

impl Server {
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.addr.port())
    }
}

async fn serve() -> Server {
    let hits = Hits::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let port = addr.port();
    let app = Router::new()
        .route(
            "/json",
            get(|| async { ([("x-a", "1"), ("set-cookie", "sid=abc")], r#"{"ok":true}"#) }),
        )
        .route(
            "/status/{code}",
            get(|Path(code): Path<u16>| async move {
                (
                    StatusCode::from_u16(code).unwrap(),
                    format!("status body {code}"),
                )
            }),
        )
        // The Authorization header, echoed with the status asked for.
        .route(
            "/echo-auth/{code}",
            get(|Path(code): Path<u16>, h: HeaderMap| async move {
                let auth = h
                    .get("authorization")
                    .map(|v| v.to_str().unwrap().to_string())
                    .unwrap_or_default();
                (StatusCode::from_u16(code).unwrap(), auth)
            }),
        )
        .route("/big", get(|| async { "x".repeat((1 << 20) + 1) }))
        .route("/at-cap", get(|| async { "x".repeat(1 << 20) }))
        // To `localhost`: loopback by name, which only "127.0.0.1" is listed for.
        .route(
            "/redirect",
            get(move || async move {
                (
                    StatusCode::FOUND,
                    [("location", format!("http://localhost:{port}/json"))],
                )
                    .into_response()
            }),
        )
        .layer(axum::middleware::from_fn_with_state(
            hits.clone(),
            |State(hits): State<Hits>,
             req: axum::extract::Request,
             next: axum::middleware::Next| async move {
                hits.0.fetch_add(1, Ordering::SeqCst);
                next.run(req).await
            },
        ));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Server { addr, hits }
}

// ---- secrets ----------------------------------------------------------------

struct Secrets;

#[async_trait::async_trait]
impl SecretSource for Secrets {
    async fn get(&self, _: Uuid, name: &str) -> Result<BTreeMap<String, String>, SecretError> {
        match name {
            "TOKEN" => Ok(BTreeMap::from([("value".to_string(), TOKEN.to_string())])),
            _ => Err(SecretError::NotFound { name: name.into() }),
        }
    }
}

// ---- running a flow -----------------------------------------------------------

/// `container_runner = none`, as `runtime::handlers_for` builds it, with
/// `workflow.http.allow_hosts = allow`.
async fn registry(pool: &sqlx::PgPool, allow: &[&str]) -> Handlers {
    let mut cfg = egret_config::Config::default().workflow;
    cfg.http.allow_hosts = allow.iter().map(|s| s.to_string()).collect();
    let (h, runner) = egret_workflow::runtime::handlers_for(&cfg, pool.clone(), Arc::new(Secrets))
        .await
        .expect("the shipping registry builds");
    assert!(runner.is_none());
    h
}

struct Run {
    ex: Uuid,
    pool: sqlx::PgPool,
}

impl Run {
    /// `(state, outputs, message)` of `task`.
    async fn task(&self, task: &str) -> (String, Option<Value>, Option<String>) {
        sqlx::query_as(
            "SELECT state, outputs, message FROM workflow.task_runs \
              WHERE execution_id = $1 AND task_id = $2",
        )
        .bind(self.ex)
        .bind(task)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }

    /// Every stored log line of the execution.
    async fn logs(&self) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT message FROM workflow.task_logs WHERE execution_id = $1 ORDER BY id",
        )
        .bind(self.ex)
        .fetch_all(&self.pool)
        .await
        .unwrap()
    }
}

/// One run at a time: `tick` claims any claimable run in the database, so a
/// concurrent test's registry (another allowlist) would run this one's task.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Start `source` in a fresh workspace and drain it through `allow`'s
/// registry, every line stored by the real writer.
async fn run(db: &Db, allow: &[&str], source: &str) -> Run {
    let _serial = SERIAL.lock().await;
    let (ws, _) = common::seed_workspace(db).await;
    let pool = &db.pool;
    let ex = start_execution(pool, ws, None, source, json!({}))
        .await
        .unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let (sink, writer) = spawn_writer(
        LogRepo::new(pool.clone()),
        WriterConfig::default(),
        cancel.clone(),
    );
    let reg = registry(pool, allow)
        .await
        .with_log_sink(Arc::new(sink), LogLimits::default());
    for _ in 0..64 {
        let ticked = tick(pool, 16, &reg).await;
        assert!(ticked.is_ok(), "a worker error: {ticked:?}");
        if ticked.unwrap() == 0 {
            break;
        }
    }
    drop(reg);
    cancel.cancel();
    writer.await.unwrap();
    Run {
        ex,
        pool: pool.clone(),
    }
}

/// A one-task flow: `req` of `ty` with `props` (YAML lines, indented 4).
fn one(ty: &str, props: &str) -> String {
    format!("id: http\nnamespace: h.t\ntasks:\n  - id: req\n    type: {ty}\n{props}")
}

const LOCAL: &[&str] = &["127.0.0.1"];

// ---- success ----------------------------------------------------------------

/// A GET succeeds with Kestra's four outputs, and a later task reads them
/// through `outputs.req.*`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_succeeds_and_a_later_task_reads_its_outputs() {
    let Some(db) = common::db().await else { return };
    let s = serve().await;
    let uri = s.url("/json");
    let source = format!(
        "{}  - id: after\n    type: io.kestra.plugin.core.debug.Return\n    \
         format: \"{{{{ outputs.req.code }}}}|{{{{ outputs.req.body }}}}|{{{{ outputs.req.headers['x-a'][0] }}}}\"\n",
        one(REQUEST, &format!("    uri: {uri}\n"))
    );
    let r = run(&db, LOCAL, &source).await;
    let (state, outputs, message) = r.task("req").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    let o = outputs.unwrap();
    assert_eq!(o["uri"], json!(uri), "the request URI");
    assert_eq!(o["code"], json!(200));
    assert_eq!(o["body"], json!(r#"{"ok":true}"#), "a string, never parsed");
    assert_eq!(o["headers"]["x-a"], json!(["1"]));
    // Kestra keeps Set-Cookie in the output (it is never logged).
    assert_eq!(o["headers"]["set-cookie"], json!(["sid=abc"]));
    let (state, outputs, message) = r.task("after").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    assert_eq!(outputs.unwrap()["value"], json!(r#"200|{"ok":true}|1"#));
    assert_eq!(s.hits.count(), 1);
}

/// Kestra's alias `io.kestra.plugin.fs.http.Request` publishes and runs as
/// the same task (spec D9).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_fs_http_alias_runs_the_same_task() {
    let Some(db) = common::db().await else { return };
    let s = serve().await;
    let source = one(
        "io.kestra.plugin.fs.http.Request",
        &format!("    uri: {}\n", s.url("/json")),
    );
    let flow = egret_workflow::spec::parse(&source).unwrap();
    assert!(
        egret_workflow::spec::validate(&flow).errors.is_empty(),
        "the alias publishes"
    );
    let r = run(&db, LOCAL, &source).await;
    let (state, outputs, message) = r.task("req").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    assert_eq!(outputs.unwrap()["code"], json!(200));
}

// ---- the status rule ----------------------------------------------------------

/// A 4xx fails with Kestra's message, the body quoted, and the secret that
/// went into the request masked there and in every stored line.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_4xx_fails_with_the_body_and_the_secret_masked() {
    let Some(db) = common::db().await else { return };
    let s = serve().await;
    let source = one(
        REQUEST,
        &format!(
            "    uri: {}\n    headers:\n      Authorization: \"Bearer {{{{ secret('TOKEN') }}}}\"\n",
            s.url("/echo-auth/401")
        ),
    );
    let r = run(&db, LOCAL, &source).await;
    let (state, outputs, message) = r.task("req").await;
    assert_eq!(state, "FAILED");
    assert!(outputs.is_none());
    let message = message.unwrap();
    // The whole written Authorization value is a credential, so the echo
    // is masked whole.
    assert_eq!(
        message,
        "Failed http request with response code '401' and body:\n******"
    );
    let logs = r.logs().await;
    assert!(!logs.is_empty(), "the failure line is stored");
    for l in &logs {
        assert!(!l.contains(TOKEN), "a stored line leaks the secret: {l}");
    }
    assert_eq!(s.hits.count(), 1);
}

/// `allowedResponseCodes` replaces the rule: a listed 404 succeeds and an
/// unlisted 200 fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn allowed_response_codes_replace_the_rule() {
    let Some(db) = common::db().await else { return };
    let s = serve().await;
    let props = |path: &str| {
        format!(
            "    uri: {}\n    options:\n      allowedResponseCodes: [404]\n",
            s.url(path)
        )
    };
    let r = run(&db, LOCAL, &one(REQUEST, &props("/status/404"))).await;
    let (state, outputs, message) = r.task("req").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    let o = outputs.unwrap();
    assert_eq!(
        (o["code"].clone(), o["body"].clone()),
        (json!(404), json!("status body 404"))
    );

    let r = run(&db, LOCAL, &one(REQUEST, &props("/status/200"))).await;
    let (state, _, message) = r.task("req").await;
    assert_eq!(state, "FAILED");
    assert!(message
        .unwrap()
        .starts_with("Failed http request with response code '200'"),);
}

/// `allowFailed` lets a 500 succeed with its body; without it the same
/// response fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn allow_failed_lets_a_500_succeed() {
    let Some(db) = common::db().await else { return };
    let s = serve().await;
    let uri = s.url("/status/500");
    let r = run(
        &db,
        LOCAL,
        &one(
            REQUEST,
            &format!("    uri: {uri}\n    options:\n      allowFailed: true\n"),
        ),
    )
    .await;
    let (state, outputs, message) = r.task("req").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    assert_eq!(outputs.unwrap()["code"], json!(500));

    let r = run(&db, LOCAL, &one(REQUEST, &format!("    uri: {uri}\n"))).await;
    assert_eq!(r.task("req").await.0, "FAILED");
}

// ---- secret() ---------------------------------------------------------------

/// `secret()` resolves in the Authorization header (the server received the
/// value), and the value is masked in the `options.logs` lines.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn secret_resolves_in_a_header_and_is_masked_in_the_logs() {
    let Some(db) = common::db().await else { return };
    let s = serve().await;
    let source = one(
        REQUEST,
        &format!(
            "    uri: {}\n    headers:\n      Authorization: \"Bearer {{{{ secret('TOKEN') }}}}\"\n    \
             options:\n      logs: [REQUEST_HEADERS, RESPONSE_BODY]\n",
            s.url("/echo-auth/200")
        ),
    );
    let r = run(&db, LOCAL, &source).await;
    let (state, outputs, message) = r.task("req").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    // The server echoed what it received: the resolved value. Outputs are not
    // masked, as in Kestra (the plugin tasks' rule too).
    assert_eq!(outputs.unwrap()["body"], json!(format!("Bearer {TOKEN}")));
    let logs = r.logs().await;
    assert!(
        logs.iter().any(|l| l.starts_with("request headers")
            || l.contains("authorization")
            || l.contains("Authorization")),
        "the request headers line is stored: {logs:?}"
    );
    assert!(
        logs.iter().any(|l| l.contains("******")),
        "masked where the value was: {logs:?}"
    );
    for l in &logs {
        assert!(!l.contains(TOKEN), "a stored line leaks the secret: {l}");
    }
}

/// A missing secret fails the task naming it, and nothing is sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_secret_fails_naming_it_and_sends_nothing() {
    let Some(db) = common::db().await else { return };
    let s = serve().await;
    let source = one(
        REQUEST,
        &format!(
            "    uri: {}\n    options:\n      auth:\n        type: BEARER\n        \
             token: \"{{{{ secret('NOPE') }}}}\"\n",
            s.url("/json")
        ),
    );
    let r = run(&db, LOCAL, &source).await;
    let (state, _, message) = r.task("req").await;
    assert_eq!(state, "FAILED");
    assert!(message.unwrap().contains("secret 'NOPE' was not found"));
    assert_eq!(s.hits.count(), 0);
}

// ---- the body cap -------------------------------------------------------------

/// A body over 1 MiB fails with Kestra's message pointing at `Download`; one
/// exactly at the cap is stored.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_body_over_one_mib_fails_pointing_at_download() {
    let Some(db) = common::db().await else { return };
    let s = serve().await;
    let r = run(
        &db,
        LOCAL,
        &one(REQUEST, &format!("    uri: {}\n", s.url("/big"))),
    )
    .await;
    let (state, outputs, message) = r.task("req").await;
    assert_eq!(state, "FAILED");
    assert!(outputs.is_none());
    assert_eq!(
        message.unwrap(),
        "Response body is too large to store in task outputs (1048577 bytes > max 1048576 \
         bytes). Use io.kestra.plugin.core.http.Download to fetch large payloads as files \
         instead."
    );
    let r = run(
        &db,
        LOCAL,
        &one(REQUEST, &format!("    uri: {}\n", s.url("/at-cap"))),
    )
    .await;
    let (state, outputs, message) = r.task("req").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    assert_eq!(outputs.unwrap()["body"].as_str().unwrap().len(), 1 << 20);
}

// ---- egress refusals, the allowlist unset --------------------------------------

/// Without `workflow.http.allow_hosts`, loopback is refused naming the key,
/// and the server receives nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_is_refused_without_the_allowlist_naming_the_key() {
    let Some(db) = common::db().await else { return };
    let s = serve().await;
    let r = run(
        &db,
        &[],
        &one(REQUEST, &format!("    uri: {}\n", s.url("/json"))),
    )
    .await;
    let (state, _, message) = r.task("req").await;
    assert_eq!(state, "FAILED");
    let message = message.unwrap();
    assert!(message.contains("workflow.http.allow_hosts"), "{message}");
    assert_eq!(s.hits.count(), 0, "nothing reached the server");
}

/// With only the first host listed, a redirect to a private address
/// (`localhost`, loopback by name) is refused at the hop: the redirecting
/// response is the only request the server saw.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_redirect_to_a_private_address_is_refused() {
    let Some(db) = common::db().await else { return };
    let s = serve().await;
    let r = run(
        &db,
        LOCAL,
        &one(REQUEST, &format!("    uri: {}\n", s.url("/redirect"))),
    )
    .await;
    let (state, _, message) = r.task("req").await;
    assert_eq!(state, "FAILED");
    let message = message.unwrap();
    assert!(message.contains("workflow.http.allow_hosts"), "{message}");
    assert_eq!(s.hits.count(), 1, "only the redirecting hop");
}

// ---- the D4 refusals --------------------------------------------------------------

/// Each refused property (spec D4, and userinfo in the URI) is an error at
/// publish, and a stored flow carrying it starts, then fails at the task
/// naming the property -- before anything is sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_refused_property_is_a_publish_error_and_a_run_failure() {
    let Some(db) = common::db().await else { return };
    let s = serve().await;
    let port = s.addr.port();
    let uri = s.url("/json");
    for (props, key) in [
        (
            format!("    uri: {uri}\n    options:\n      proxy:\n        address: proxy.corp\n        port: 3128\n"),
            "options.proxy",
        ),
        (
            format!("    uri: {uri}\n    options:\n      ssl:\n        insecureTrustAllCertificates: true\n"),
            "options.ssl.insecureTrustAllCertificates",
        ),
        (
            format!("    uri: {uri}\n    options:\n      auth:\n        type: DIGEST\n        username: u\n        password: p\n"),
            "options.auth.type",
        ),
        (format!("    uri: {uri}\n    encryptBody: true\n"), "encryptBody"),
        (
            format!("    uri: http://user:pw@127.0.0.1:{port}/json\n"),
            "uri",
        ),
    ] {
        let source = one(REQUEST, &props);
        let flow = egret_workflow::spec::parse(&source).unwrap();
        let report = egret_workflow::spec::validate(&flow);
        let found: Vec<_> = report
            .errors
            .iter()
            .filter(|e| e.code == "http_option_refused")
            .collect();
        assert_eq!(found.len(), 1, "{key}: {:?}", report.errors);
        assert_eq!(found[0].path, format!("tasks.req.{key}"));
        let r = run(&db, LOCAL, &source).await;
        let (state, _, message) = r.task("req").await;
        assert_eq!(state, "FAILED", "{key}");
        let message = message.unwrap();
        assert!(message.starts_with(key), "{key}: {message}");
        assert!(!message.contains("pw@"), "{message}");
    }
    assert_eq!(s.hits.count(), 0, "no refused request was sent");
}

// ---- Download without [storage] (package #4 Task C4) ---------------------------

/// This binary installs no store: `http.Download` fails naming the missing
/// configuration, before anything is sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_download_without_storage_fails_naming_it_and_sends_nothing() {
    let Some(db) = common::db().await else { return };
    let s = serve().await;
    let r = run(
        &db,
        LOCAL,
        &one(
            "io.kestra.plugin.core.http.Download",
            &format!("    uri: {}\n", s.url("/json")),
        ),
    )
    .await;
    let (state, outputs, message) = r.task("req").await;
    assert_eq!(state, "FAILED");
    assert!(outputs.is_none());
    let message = message.unwrap();
    assert!(message.contains("`[storage]`"), "{message}");
    assert_eq!(s.hits.count(), 0);
}
