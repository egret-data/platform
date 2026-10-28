//! Package #4 Task C4: `io.kestra.plugin.core.http.Download` and
//! `io.kestra.plugin.core.http.SseRequest` through the real worker, over the
//! registry `runtime::handlers_for` builds, against a local server on
//! 127.0.0.1, with internal storage (`install_mem_for_tests`) and the
//! database. Requires DATABASE_URL.
//!
//! Also the storage-backed path of `http.Request` C3 could not cover: a
//! multipart `formData` file part read from a file an earlier task stored.
//!
//! As in `tests/http_request.rs`, positive tests list `127.0.0.1` in
//! `workflow.http.allow_hosts`, and the refusals check that the server
//! received nothing.

mod common;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use egret_metastore::Db;
use egret_storage::StorageError;
use egret_workflow::logs::{spawn_writer, LogLimits, LogRepo, WriterConfig};
use egret_workflow::runners::secrets::{SecretError, SecretSource};
use egret_workflow::storage::{self, FileStore};
use egret_workflow::worker::{start_execution, tick, Handlers};
use serde_json::{json, Value};
use uuid::Uuid;

const DOWNLOAD: &str = "io.kestra.plugin.core.http.Download";
const SSE: &str = "io.kestra.plugin.core.http.SseRequest";
const CSV: &str = "a,b\n1,2\n";
const TOKEN: &str = "sse-t0ken-value";

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

/// `n` bytes of `x`, streamed in 1 KiB chunks with no `Content-Length`.
fn chunked(n: usize) -> Body {
    let chunks: Vec<Result<Bytes, std::io::Error>> = (0..n)
        .step_by(1024)
        .map(|i| Ok(Bytes::from(vec![b'x'; (n - i).min(1024)])))
        .collect();
    Body::from_stream(futures_util::stream::iter(chunks))
}

const SSE_BODY: &str = "data: {\"m\":\"he\"}\n\n\
                        : a comment\r\nid: 3\r\nevent: done\r\nretry: 1500\r\ndata: {\"m\":\"llo\"}\r\n\r\n";

async fn serve() -> Server {
    let hits = Hits::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = Router::new()
        .route(
            "/data/orders.csv",
            get(|| async { ([("content-type", "text/csv")], CSV) }),
        )
        .route(
            "/cd",
            get(|| async {
                (
                    [(
                        "content-disposition",
                        "attachment; filename=\"/secure-path/report 1.txt\"",
                    )],
                    "hello",
                )
            }),
        )
        .route("/empty", get(|| async { "" }))
        .route(
            "/sized/{n}",
            get(|Path(n): Path<usize>| async move { "x".repeat(n) }),
        )
        .route(
            "/chunked/{n}",
            get(|Path(n): Path<usize>| async move { chunked(n) }),
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
        // The raw multipart body, echoed.
        .route("/upload", post(|body: Bytes| async move { body }))
        .route(
            "/sse",
            get(|| async { ([("content-type", "text/event-stream")], SSE_BODY) }),
        )
        .route(
            "/sse-auth",
            get(|h: axum::http::HeaderMap| async move {
                let auth = h
                    .get("authorization")
                    .map(|v| v.to_str().unwrap().to_string())
                    .unwrap_or_default();
                (
                    [("content-type", "text/event-stream")],
                    format!("data: {{\"auth\":\"{auth}\"}}\n\n"),
                )
            }),
        )
        .route(
            "/sse-many",
            get(|| async {
                let body: String = (0..50).map(|i| format!("data: {i}\n\n")).collect();
                ([("content-type", "text/event-stream")], body)
            }),
        )
        .route(
            "/sse-401",
            get(|| async {
                (
                    StatusCode::UNAUTHORIZED,
                    [("content-type", "text/event-stream")],
                    "data: {\"m\":\"no\"}\n\n",
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

/// The deployment's `workflow.http` for one run.
#[derive(Clone, Copy)]
struct Http<'a> {
    allow: &'a [&'a str],
    max_download_bytes: Option<u64>,
    max_sse_events: Option<u64>,
}

const LOCAL: Http<'static> = Http {
    allow: &["127.0.0.1"],
    max_download_bytes: None,
    max_sse_events: None,
};

async fn registry(pool: &sqlx::PgPool, http: Http<'_>) -> Handlers {
    let mut cfg = egret_config::Config::default().workflow;
    cfg.http.allow_hosts = http.allow.iter().map(|s| s.to_string()).collect();
    if let Some(n) = http.max_download_bytes {
        cfg.http.max_download_bytes = n;
    }
    if let Some(n) = http.max_sse_events {
        cfg.http.max_sse_events = n;
    }
    let (h, runner) = egret_workflow::runtime::handlers_for(
        &cfg,
        pool.clone(),
        Arc::new(Secrets),
        common::no_query(),
    )
    .await
    .expect("the shipping registry builds");
    assert!(runner.is_none());
    h
}

struct Run {
    ws: Uuid,
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

    async fn logs(&self) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT message FROM workflow.task_logs WHERE execution_id = $1 ORDER BY id",
        )
        .bind(self.ex)
        .fetch_all(&self.pool)
        .await
        .unwrap()
    }

    /// The index rows of this execution: `(uri, size)`.
    async fn files(&self) -> Vec<(String, i64)> {
        sqlx::query_as(
            "SELECT uri, size FROM workflow.execution_files WHERE execution_id = $1 ORDER BY uri",
        )
        .bind(self.ex)
        .fetch_all(&self.pool)
        .await
        .unwrap()
    }

    async fn object(&self, fs: &FileStore, file: &str) -> Option<Vec<u8>> {
        match fs.store.get(&fs.object_uri(self.ws, file).unwrap()).await {
            Ok(bytes) => Some(bytes),
            Err(StorageError::NotFound(_)) => None,
            Err(e) => panic!("{file}: {e}"),
        }
    }
}

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn run(db: &Db, http: Http<'_>, source: &str) -> Run {
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
    let reg = registry(pool, http)
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
        ws,
        ex,
        pool: pool.clone(),
    }
}

/// A flow with tasks given as YAML (each starting `  - id:`).
fn flow(tasks: &str) -> String {
    format!("id: http\nnamespace: h.t\ntasks:\n{tasks}")
}

/// One task `t` of `ty` with `props` (YAML lines, indented 4).
fn task(t: &str, ty: &str, props: &str) -> String {
    format!("  - id: {t}\n    type: {ty}\n{props}")
}

/// The store, installed for this binary before any registry is built.
fn store() -> Arc<FileStore> {
    storage::install_mem_for_tests()
}

// ---- Download -------------------------------------------------------------------

/// The body lands in storage under a name keeping the URI's extension, the
/// commit indexes it with its size, the outputs are Kestra's four, and a
/// later task reads the file through `read()`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_download_is_stored_indexed_and_readable_by_a_later_task() {
    let fs = store();
    let Some(db) = common::db().await else { return };
    let s = serve().await;
    let source = flow(&format!(
        "{}{}",
        task(
            "dl",
            DOWNLOAD,
            &format!("    uri: {}\n", s.url("/data/orders.csv"))
        ),
        task(
            "after",
            "io.kestra.plugin.core.debug.Return",
            "    format: \"{{ read(outputs.dl.uri) }}|{{ outputs.dl.length }}\"\n"
        ),
    ));
    let r = run(&db, LOCAL, &source).await;
    let (state, outputs, message) = r.task("dl").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    let o = outputs.unwrap();
    let uri = o["uri"].as_str().unwrap().to_string();
    assert!(uri.starts_with("kestra:///h/t/http/executions/"), "{uri}");
    assert!(uri.ends_with("/download.csv"), "{uri}");
    assert_eq!(o["code"], json!(200));
    assert_eq!(o["length"], json!(CSV.len()));
    assert_eq!(o["headers"]["content-type"], json!(["text/csv"]));
    assert_eq!(r.files().await, vec![(uri.clone(), CSV.len() as i64)]);
    assert_eq!(r.object(&fs, &uri).await.unwrap(), CSV.as_bytes());
    let (state, outputs, message) = r.task("after").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    assert_eq!(
        outputs.unwrap()["value"],
        json!(format!("{CSV}|{}", CSV.len()))
    );
    assert_eq!(s.hits.count(), 1);
}

/// `Content-Disposition` names the file (cut to its last segment, spaces as
/// `+`); `saveAs` wins over it; the `fs.http` alias runs the same task.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_file_name_comes_from_save_as_then_content_disposition() {
    let fs = store();
    let Some(db) = common::db().await else { return };
    let s = serve().await;
    let r = run(
        &db,
        LOCAL,
        &flow(&task(
            "dl",
            DOWNLOAD,
            &format!("    uri: {}\n", s.url("/cd")),
        )),
    )
    .await;
    let (state, outputs, message) = r.task("dl").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    let uri = outputs.unwrap()["uri"].as_str().unwrap().to_string();
    assert!(uri.ends_with("/report+1.txt"), "{uri}");
    assert!(!uri.contains("secure-path"), "{uri}");
    assert_eq!(r.object(&fs, &uri).await.unwrap(), b"hello");

    let r = run(
        &db,
        LOCAL,
        &flow(&task(
            "dl",
            "io.kestra.plugin.fs.http.Download",
            &format!("    uri: {}\n    saveAs: sub/mine.txt\n", s.url("/cd")),
        )),
    )
    .await;
    let (state, outputs, message) = r.task("dl").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    let uri = outputs.unwrap()["uri"].as_str().unwrap().to_string();
    assert!(
        uri.ends_with("/mine.txt") && !uri.contains("/sub/"),
        "{uri}"
    );
}

/// Over `workflow.http.max_download_bytes`: refused on `Content-Length`
/// before reading, and mid-stream when there is none. Either way nothing is
/// indexed and no object is left.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_download_over_the_cap_fails_and_stores_nothing() {
    let fs = store();
    let Some(db) = common::db().await else { return };
    let s = serve().await;
    let capped = Http {
        max_download_bytes: Some(4096),
        ..LOCAL
    };
    for (path, size) in [
        ("/sized/4097", "4097 bytes"),
        ("/chunked/10000", "more than 4096 bytes"),
    ] {
        let r = run(
            &db,
            capped,
            &flow(&task(
                "dl",
                DOWNLOAD,
                &format!("    uri: {}\n    saveAs: big.bin\n", s.url(path)),
            )),
        )
        .await;
        let (state, outputs, message) = r.task("dl").await;
        assert_eq!(state, "FAILED", "{path}");
        assert!(outputs.is_none());
        assert_eq!(
            message.unwrap(),
            format!(
                "The response body is too large to download ({size} > max 4096 bytes, \
                 workflow.http.max_download_bytes)."
            ),
            "{path}"
        );
        assert!(r.files().await.is_empty(), "{path}: nothing indexed");
        // Every object of the run's workspace (a fresh one per run).
        let key = format!("{}{}/", fs.prefix, r.ws.hyphenated());
        let objects = fs.store.list(&key, None).await.unwrap().objects;
        assert!(
            objects.is_empty(),
            "{path}: an object was left: {objects:?}"
        );
    }
    // At the cap exactly: stored.
    let r = run(
        &db,
        capped,
        &flow(&task(
            "dl",
            DOWNLOAD,
            &format!("    uri: {}\n", s.url("/chunked/4096")),
        )),
    )
    .await;
    let (state, outputs, message) = r.task("dl").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    assert_eq!(outputs.unwrap()["length"], json!(4096));
    // The control for the listing above: it does see a stored object.
    let key = format!("{}{}/", fs.prefix, r.ws.hyphenated());
    assert_eq!(fs.store.list(&key, None).await.unwrap().objects.len(), 1);
}

/// An empty body fails with Kestra's message by default; with
/// `failOnEmptyResponse: false` it is stored, empty, and warned about.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_empty_response_fails_unless_allowed() {
    store();
    let Some(db) = common::db().await else { return };
    let s = serve().await;
    let uri = s.url("/empty");
    let r = run(
        &db,
        LOCAL,
        &flow(&task("dl", DOWNLOAD, &format!("    uri: {uri}\n"))),
    )
    .await;
    let (state, _, message) = r.task("dl").await;
    assert_eq!(state, "FAILED");
    assert_eq!(message.unwrap(), "No response from server");
    assert!(r.files().await.is_empty());

    let r = run(
        &db,
        LOCAL,
        &flow(&task(
            "dl",
            DOWNLOAD,
            &format!("    uri: {uri}\n    failOnEmptyResponse: false\n"),
        )),
    )
    .await;
    let (state, outputs, message) = r.task("dl").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    assert_eq!(outputs.unwrap()["length"], json!(0));
    assert_eq!(r.files().await.len(), 1);
    let logs = r.logs().await;
    assert!(
        logs.iter().any(|l| l == &format!("File '{uri}' is empty")),
        "{logs:?}"
    );
}

/// A 5xx fails with Request's message and stores nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_status_fails_the_download() {
    store();
    let Some(db) = common::db().await else { return };
    let s = serve().await;
    let r = run(
        &db,
        LOCAL,
        &flow(&task(
            "dl",
            DOWNLOAD,
            &format!("    uri: {}\n", s.url("/status/500")),
        )),
    )
    .await;
    let (state, _, message) = r.task("dl").await;
    assert_eq!(state, "FAILED");
    assert_eq!(
        message.unwrap(),
        "Failed http request with response code '500' and body:\nstatus body 500"
    );
    assert!(r.files().await.is_empty());
}

// ---- Request's multipart file part, from storage ----------------------------------

/// A `formData` file part reads the file an earlier task stored (C3's
/// `StoreFiles`, through the run's readable files): the server receives the
/// bytes under the file's name.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_multipart_file_part_is_read_from_storage() {
    store();
    let Some(db) = common::db().await else { return };
    let s = serve().await;
    let source = flow(&format!(
        "{}{}",
        task(
            "dl",
            DOWNLOAD,
            &format!("    uri: {}\n", s.url("/data/orders.csv"))
        ),
        task(
            "up",
            "io.kestra.plugin.core.http.Request",
            &format!(
                "    uri: {}\n    method: POST\n    contentType: multipart/form-data\n    \
                 formData:\n      file: \"{{{{ outputs.dl.uri }}}}\"\n      note: hi\n",
                s.url("/upload")
            ),
        ),
    ));
    let r = run(&db, LOCAL, &source).await;
    let (state, outputs, message) = r.task("up").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    let body = outputs.unwrap()["body"].as_str().unwrap().to_string();
    assert!(
        body.contains("name=\"file\"; filename=\"download.csv\""),
        "{body}"
    );
    assert!(body.contains(&format!("\r\n\r\n{CSV}\r\n")), "{body}");
    assert!(body.contains("name=\"note\""), "{body}");
    assert_eq!(s.hits.count(), 2);
}

// ---- SseRequest ---------------------------------------------------------------------

/// Without JQ: every event, data read as JSON, the fields Kestra keeps.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sse_events_are_parsed_as_kestra_does() {
    store();
    let Some(db) = common::db().await else { return };
    let s = serve().await;
    let uri = s.url("/sse");
    let r = run(
        &db,
        LOCAL,
        &flow(&task("sse", SSE, &format!("    uri: {uri}\n"))),
    )
    .await;
    let (state, outputs, message) = r.task("sse").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    assert_eq!(
        outputs.unwrap(),
        json!({
            "uri": uri,
            "size": 2,
            "events": [
                {"data": {"m": "he"}},
                {"data": {"m": "llo"}, "id": "3", "name": "done", "comment": "a comment", "retry": 1.5},
            ],
        })
    );
}

/// With JQ: the results are joined into `result`, and `events` is left out;
/// a later task reads it. `secret()` resolves in a header and the value is
/// in no stored line.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sse_jq_concatenates_and_a_later_task_reads_it() {
    store();
    let Some(db) = common::db().await else { return };
    let s = serve().await;
    let source = flow(&format!(
        "{}{}{}",
        task(
            "sse",
            SSE,
            &format!(
                "    uri: {}\n    concatJqExpression: .data.m\n",
                s.url("/sse")
            )
        ),
        task(
            "auth",
            SSE,
            &format!(
                "    uri: {}\n    concatJqExpression: .data.auth\n    headers:\n      \
                 Authorization: \"Bearer {{{{ secret('TOKEN') }}}}\"\n    \
                 options:\n      logs: [REQUEST_HEADERS]\n",
                s.url("/sse-auth")
            )
        ),
        task(
            "after",
            "io.kestra.plugin.core.debug.Return",
            "    format: \"{{ outputs.sse.result }}|{{ outputs.sse.size }}\"\n"
        ),
    ));
    let r = run(&db, LOCAL, &source).await;
    let (state, outputs, message) = r.task("sse").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    let o = outputs.unwrap();
    assert_eq!(o["result"], json!("hello"));
    assert!(o.get("events").is_none(), "{o}");
    let (state, outputs, message) = r.task("auth").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    assert_eq!(outputs.unwrap()["result"], json!(format!("Bearer {TOKEN}")));
    let (state, outputs, message) = r.task("after").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    assert_eq!(outputs.unwrap()["value"], json!("hello|2"));
    for l in r.logs().await {
        assert!(!l.contains(TOKEN), "a stored line leaks the secret: {l}");
    }
}

/// A JQ program with no text answer for an event fails the task by default,
/// with Kestra's message; with `failedOnMissingJq: false` that event is
/// skipped. A program jaq cannot parse fails before anything is sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sse_jq_missing_values_and_a_bad_program() {
    store();
    let Some(db) = common::db().await else { return };
    let s = serve().await;
    let uri = s.url("/sse");
    let r = run(
        &db,
        LOCAL,
        &flow(&task(
            "sse",
            SSE,
            &format!("    uri: {uri}\n    concatJqExpression: .id\n"),
        )),
    )
    .await;
    let (state, _, message) = r.task("sse").await;
    assert_eq!(state, "FAILED");
    assert_eq!(
        message.unwrap(),
        "Failed to resolve JQ expression '.id' and value '{\"data\":{\"m\":\"he\"}}'"
    );

    let r = run(
        &db,
        LOCAL,
        &flow(&task(
            "sse",
            SSE,
            &format!("    uri: {uri}\n    concatJqExpression: .id\n    failedOnMissingJq: false\n"),
        )),
    )
    .await;
    let (state, outputs, message) = r.task("sse").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    assert_eq!(outputs.unwrap()["result"], json!("3"));

    let before = s.hits.count();
    let r = run(
        &db,
        LOCAL,
        &flow(&task(
            "sse",
            SSE,
            &format!("    uri: {uri}\n    concatJqExpression: \"$.message\"\n"),
        )),
    )
    .await;
    let (state, _, message) = r.task("sse").await;
    assert_eq!(state, "FAILED");
    let message = message.unwrap();
    assert!(
        message.starts_with("concatJqExpression: \"$.message\" is not a valid jq expression"),
        "{message}"
    );
    assert_eq!(s.hits.count(), before, "nothing was sent");
}

/// A 4xx fails (Kestra reads the stream whatever the status; spec D8).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_sse_4xx_fails() {
    store();
    let Some(db) = common::db().await else { return };
    let s = serve().await;
    let r = run(
        &db,
        LOCAL,
        &flow(&task(
            "sse",
            SSE,
            &format!("    uri: {}\n", s.url("/sse-401")),
        )),
    )
    .await;
    let (state, _, message) = r.task("sse").await;
    assert_eq!(state, "FAILED");
    assert_eq!(
        message.unwrap(),
        "Failed http request with response code '401' and body:\ndata: {\"m\":\"no\"}\n\n"
    );
}

/// At `workflow.http.max_sse_events` the stream is closed and the task
/// succeeds with that many events, warning that it stopped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_event_cap_stops_the_stream_and_succeeds() {
    store();
    let Some(db) = common::db().await else { return };
    let s = serve().await;
    let r = run(
        &db,
        Http {
            max_sse_events: Some(5),
            ..LOCAL
        },
        &flow(&task(
            "sse",
            SSE,
            &format!("    uri: {}\n", s.url("/sse-many")),
        )),
    )
    .await;
    let (state, outputs, message) = r.task("sse").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    let o = outputs.unwrap();
    assert_eq!(o["size"], json!(5));
    assert_eq!(
        o["events"],
        json!([{"data": 0}, {"data": 1}, {"data": 2}, {"data": 3}, {"data": 4}])
    );
    let logs = r.logs().await;
    assert!(
        logs.iter()
            .any(|l| l.contains("after 5 events") && l.contains("workflow.http.max_sse_events")),
        "{logs:?}"
    );
}

// ---- refusals: nothing reaches the server -----------------------------------------

/// Without the allowlist, loopback is refused for both tasks, and the server
/// sees nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_is_refused_for_both_tasks_without_the_allowlist() {
    store();
    let Some(db) = common::db().await else { return };
    let s = serve().await;
    let none = Http {
        allow: &[],
        ..LOCAL
    };
    for (ty, path) in [(DOWNLOAD, "/data/orders.csv"), (SSE, "/sse")] {
        let r = run(
            &db,
            none,
            &flow(&task("t", ty, &format!("    uri: {}\n", s.url(path)))),
        )
        .await;
        let (state, _, message) = r.task("t").await;
        assert_eq!(state, "FAILED", "{ty}");
        let message = message.unwrap();
        assert!(
            message.contains("workflow.http.allow_hosts"),
            "{ty}: {message}"
        );
        assert!(r.files().await.is_empty());
    }
    assert_eq!(s.hits.count(), 0, "nothing reached the server");
}

/// The D4 refusals are publish errors and run failures for Download (both
/// spellings) and SseRequest, with nothing sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_d4_refusals_apply_to_download_and_sse() {
    store();
    let Some(db) = common::db().await else { return };
    let s = serve().await;
    let port = s.addr.port();
    let uri = s.url("/sse");
    for ty in [DOWNLOAD, "io.kestra.plugin.fs.http.Download", SSE] {
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
            (format!("    uri: http://user:pw@127.0.0.1:{port}/sse\n"), "uri"),
        ] {
            let source = flow(&task("t", ty, &props));
            let parsed = egret_workflow::spec::parse(&source).unwrap();
            let report = egret_workflow::spec::validate(&parsed);
            let found: Vec<_> = report
                .errors
                .iter()
                .filter(|e| e.code == "http_option_refused")
                .collect();
            assert_eq!(found.len(), 1, "{ty} {key}: {:?}", report.errors);
            assert_eq!(found[0].path, format!("tasks.t.{key}"));
            let r = run(&db, LOCAL, &source).await;
            let (state, _, message) = r.task("t").await;
            assert_eq!(state, "FAILED", "{ty} {key}");
            let message = message.unwrap();
            assert!(message.starts_with(key), "{ty} {key}: {message}");
            assert!(!message.contains("pw@"), "{message}");
        }
    }
    assert_eq!(s.hits.count(), 0, "no refused request was sent");
}

/// Package #4 Task C7 (the C6 review's concern): a stored Download or
/// SseRequest carrying `encryptBody: true` (publish reports it as
/// `unknown_property`, not as the Request-only D4 refusal) fails at run time
/// naming its own type, not with Request's text about the `body` output, and
/// sends nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn encrypt_body_on_download_and_sse_fails_naming_the_type() {
    store();
    let Some(db) = common::db().await else { return };
    let s = serve().await;
    let uri = s.url("/sse");
    for ty in [DOWNLOAD, "io.kestra.plugin.fs.http.Download", SSE] {
        let source = flow(&task(
            "t",
            ty,
            &format!("    uri: {uri}\n    encryptBody: true\n"),
        ));
        let parsed = egret_workflow::spec::parse(&source).unwrap();
        let report = egret_workflow::spec::validate_source(&source, &parsed);
        let at_key: Vec<_> = report
            .errors
            .iter()
            .filter(|e| e.path == "tasks.t.encryptBody")
            .map(|e| e.code)
            .collect();
        assert_eq!(at_key, ["unknown_property"], "{ty}: {:?}", report.errors);
        let r = run(&db, LOCAL, &source).await;
        let (state, _, message) = r.task("t").await;
        assert_eq!(state, "FAILED", "{ty}");
        let message = message.unwrap();
        assert!(
            message.starts_with("encryptBody: not a property of "),
            "{ty}: {message}"
        );
        assert!(!message.contains("`body` output"), "{ty}: {message}");
        let canonical = if ty == SSE { SSE } else { DOWNLOAD };
        assert!(message.contains(canonical), "{ty}: {message}");
        assert!(r.files().await.is_empty());
    }
    assert_eq!(s.hits.count(), 0, "nothing was sent");
}
