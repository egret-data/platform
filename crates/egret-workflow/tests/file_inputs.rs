//! W9a Task 7: FILE inputs (spec §7.1) -- uploaded as `multipart/form-data`
//! on `POST .../flows/{id}/executions`, or given as a `kestra://` URI (JSON,
//! a `Subflow`, a `Flow` trigger, a loop iteration, a replay). Requires
//! DATABASE_URL.
//!
//! **One store, installed once by this binary** ([`installed`]): a
//! `MemStore` with `max_file_bytes` = [`CAP`], so the cap and the route's
//! body limit can be crossed in a test, and with [`Faulty`] in front of it:
//! every write under the namespace [`OUTAGE_NS`] fails as an unreachable S3
//! does. Nothing here touches MinIO.
//!
//! **The tests take turns** ([`SERIAL`]): several drive the real worker,
//! whose claim is global, so one test must never run another's runs. Each
//! test seeds its own workspace; this binary owns its database
//! (`egret_test_db`).

mod common;

use std::sync::Arc;

use egret_core::UserId;
use egret_metastore::Db;
use egret_storage::{ObjectStore, StorageError};
use egret_workflow::storage::{self, FileStore};
use egret_workflow::worker::Handlers;
use serde_json::{json, Value};
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const NS: &str = "w9a.t7";
const OUTAGE_NS: &str = "w9a.outage";
/// `max_file_bytes` for this binary.
const CAP: u64 = 4096;

/// `MemStore`, failing every write under [`OUTAGE_NS`] (spec §11's outage).
struct Faulty(egret_storage::MemStore);

fn in_outage(uri: &str) -> bool {
    uri.contains("/w9a/outage/")
}

#[async_trait::async_trait]
impl ObjectStore for Faulty {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError> {
        self.0.put(uri, bytes).await
    }
    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
        self.0.get(uri).await
    }
    async fn delete(&self, uri: &str) -> Result<(), StorageError> {
        self.0.delete(uri).await
    }
    async fn copy(&self, src: &str, dst: &str) -> Result<(), StorageError> {
        self.0.copy(src, dst).await
    }
    async fn list(
        &self,
        prefix: &str,
        delimiter: Option<&str>,
    ) -> Result<egret_storage::Listing, StorageError> {
        self.0.list(prefix, delimiter).await
    }
    async fn put_stream(
        &self,
        uri: &str,
        body: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
        size_hint: Option<u64>,
    ) -> Result<u64, StorageError> {
        if in_outage(uri) {
            return Err(StorageError::S3(format!(
                "{uri}: dispatch failure: connection refused"
            )));
        }
        self.0.put_stream(uri, body, size_hint).await
    }
    async fn get_stream(
        &self,
        uri: &str,
    ) -> Result<Box<dyn tokio::io::AsyncBufRead + Send + Unpin>, StorageError> {
        self.0.get_stream(uri).await
    }
    async fn head(&self, uri: &str) -> Result<Option<u64>, StorageError> {
        self.0.head(uri).await
    }
    async fn delete_prefix(&self, uri: &str) -> Result<u64, StorageError> {
        self.0.delete_prefix(uri).await
    }
}

fn installed() -> Arc<FileStore> {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        storage::install(Arc::new(FileStore {
            store: Arc::new(Faulty(egret_storage::MemStore::new())),
            bucket: "egret-test".to_string(),
            prefix: "workflow/".to_string(),
            caps: storage::Caps {
                max_file_bytes: CAP,
                ..Default::default()
            },
            retention_days: 30,
        }))
        .expect("this binary installs the only store");
    });
    storage::current().expect("installed above")
}

struct Fx {
    db: Db,
    ws: Uuid,
    user: UserId,
    fs: Arc<FileStore>,
    app: common::App,
    _turn: tokio::sync::MutexGuard<'static, ()>,
}

async fn fx() -> Option<Fx> {
    let turn = SERIAL.lock().await;
    let db = common::db().await?;
    let fs = installed();
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage", "workflow:read"]).await;
    let app = common::app(&db).await;
    Some(Fx {
        db,
        ws,
        user,
        fs,
        app,
        _turn: turn,
    })
}

enum Part<'a> {
    Text(&'a str, &'a str),
    File(&'a str, &'a str, &'a [u8]),
}

const BOUNDARY: &str = "w9a-t7-boundary-5f1c";

fn multipart_body(parts: &[Part<'_>]) -> Vec<u8> {
    let mut out = Vec::new();
    for p in parts {
        out.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        match p {
            Part::Text(name, value) => {
                out.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
                );
                out.extend_from_slice(value.as_bytes());
            }
            Part::File(name, file, bytes) => {
                out.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{name}\"; filename=\"{file}\"\r\n\
                         Content-Type: text/plain\r\n\r\n"
                    )
                    .as_bytes(),
                );
                out.extend_from_slice(bytes);
            }
        }
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    out
}

impl Fx {
    async fn flow(&self, source: &str) -> Uuid {
        common::seed_flow(&self.db, self.ws, self.user, source, "draft").await
    }

    fn start_path(&self, flow: Uuid) -> String {
        format!("/api/ws/{}/workflows/flows/{flow}/executions", self.ws)
    }

    async fn upload(&self, flow: Uuid, parts: &[Part<'_>]) -> axum::response::Response {
        common::send_raw(
            &self.app,
            "POST",
            &self.start_path(flow),
            self.user,
            &format!("multipart/form-data; boundary={BOUNDARY}"),
            multipart_body(parts),
        )
        .await
    }

    async fn start_json(&self, flow: Uuid, inputs: Value) -> axum::response::Response {
        common::post(
            &self.app,
            &self.start_path(flow),
            self.user,
            json!({ "inputs": inputs }),
        )
        .await
    }

    async fn executions(&self) -> i64 {
        common::execution_count(&self.db, self.ws).await
    }

    async fn file_rows(&self) -> Vec<(Uuid, Option<Uuid>, String, i64)> {
        sqlx::query_as(
            "SELECT execution_id, task_run_id, uri, size FROM workflow.execution_files \
              WHERE workspace_id = $1 ORDER BY id",
        )
        .bind(self.ws)
        .fetch_all(&self.db.pool)
        .await
        .expect("file rows")
    }

    /// Every object this workspace has in the store.
    async fn objects(&self) -> Vec<String> {
        self.fs
            .store
            .list(&format!("workflow/{}/", self.ws), None)
            .await
            .expect("list")
            .objects
            .into_iter()
            .map(|o| o.key)
            .collect()
    }

    async fn inputs_of(&self, execution: Uuid) -> Value {
        sqlx::query_scalar("SELECT inputs FROM workflow.executions WHERE id = $1")
            .bind(execution)
            .fetch_one(&self.db.pool)
            .await
            .expect("inputs")
    }

    async fn state_of(&self, execution: Uuid) -> String {
        sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
            .bind(execution)
            .fetch_one(&self.db.pool)
            .await
            .expect("state")
    }

    /// `(state, outputs, message)` of the run of `task` in `execution`.
    async fn run(&self, execution: Uuid, task: &str) -> (String, Option<Value>, Option<String>) {
        sqlx::query_as(
            "SELECT state, outputs, message FROM workflow.task_runs \
              WHERE execution_id = $1 AND task_id = $2",
        )
        .bind(execution)
        .bind(task)
        .fetch_one(&self.db.pool)
        .await
        .unwrap_or_else(|e| panic!("run {task} of {execution}: {e}"))
    }

    /// Tick until nothing is claimable.
    async fn drain(&self) {
        let reg = Handlers::with_core_tasks();
        for _ in 0..64 {
            if egret_workflow::worker::tick(&self.db.pool, 16, &reg)
                .await
                .expect("a worker tick")
                == 0
            {
                return;
            }
        }
        panic!("tick never stopped claiming");
    }
}

async fn id_of(res: axum::response::Response) -> Uuid {
    let dto = common::json(res).await;
    dto["id"]
        .as_str()
        .unwrap_or_else(|| panic!("no id in {dto}"))
        .parse()
        .expect("uuid")
}

async fn refusal(res: axum::response::Response, status: u16) -> String {
    assert_eq!(res.status(), status);
    common::json(res).await["error"]
        .as_str()
        .expect("an error message")
        .to_string()
}

fn oracle() -> Value {
    serde_json::from_str(include_str!("../oracle-exec/w9a-storage.json")).expect("parses")
}

/// A flow with a FILE input `f` and a STRING input `t`, whose Return reads
/// the file and prints the text.
fn reader_flow(id: &str, ns: &str, extra_f: &str) -> String {
    format!(
        "id: {id}\nnamespace: {ns}\ninputs:\n  - id: f\n    type: FILE\n{extra_f}  - id: t\n    type: STRING\n    required: false\n\
         tasks:\n  - id: r\n    type: io.kestra.plugin.core.debug.Return\n    \
         format: \"{{{{ read(inputs.f) }}}}|{{{{ inputs.t ?? 'none' }}}}\"\n"
    )
}

/// `w9a-storage.json` `file_input`: the part NAME is the input id, a file
/// part's file name is the URI's last segment, `inputs/f/hello.txt`; the
/// text part's value is the text input's. The index row is the input's (no
/// task run), and the run reads the bytes back.
#[tokio::test(flavor = "multi_thread")]
async fn a_multipart_start_stores_the_file_and_the_input_is_its_uri() {
    let Some(fx) = fx().await else { return };
    let cell = &oracle()["egret_expected"]["file_input"];
    let parts = cell["request"]["parts"].as_array().unwrap();
    let (text, file) = (&parts[0], &parts[1]);
    let flow = fx.flow(&reader_flow("file-in", NS, "")).await;

    let res = fx
        .upload(
            flow,
            &[
                Part::Text(
                    text["name"].as_str().unwrap(),
                    text["body"].as_str().unwrap(),
                ),
                Part::File(
                    file["name"].as_str().unwrap(),
                    "hello.txt",
                    file["body"].as_str().unwrap().as_bytes(),
                ),
            ],
        )
        .await;
    assert_eq!(res.status(), 201);
    let ex = id_of(res).await;

    let want = format!(
        "kestra:///w9a/t7/file-in/executions/{ex}/{}",
        cell["uri_tail"].as_str().unwrap()
    );
    let inputs = fx.inputs_of(ex).await;
    assert_eq!(inputs["f"], json!(want));
    assert_eq!(inputs["t"], cell["text_part_value"]);
    let body = file["body"].as_str().unwrap();
    assert_eq!(
        fx.file_rows().await,
        vec![(ex, None, want.clone(), body.len() as i64)],
        "one index row, the input's: no task run"
    );
    let object = fx.fs.object_uri(fx.ws, &want).unwrap();
    assert_eq!(fx.fs.store.get(&object).await.unwrap(), body.as_bytes());

    fx.drain().await;
    let (state, outputs, msg) = fx.run(ex, "r").await;
    assert_eq!(state, "SUCCESS", "{msg:?}");
    assert_eq!(
        outputs.unwrap()["value"],
        json!(format!(
            "{body}|{}",
            cell["text_part_value"].as_str().unwrap()
        ))
    );
}

/// File names kept verbatim (`file_input.filenames.percent20`), and one no
/// storage URI can hold is a 400 naming the input -- never a 500, and
/// nothing is created or left stored.
#[tokio::test]
async fn a_file_name_is_kept_verbatim_and_an_unholdable_one_is_a_400_naming_the_input() {
    let Some(fx) = fx().await else { return };
    let flow = fx.flow(&reader_flow("names", NS, "")).await;
    let res = fx.upload(flow, &[Part::File("f", "a%20b.txt", b"x")]).await;
    assert_eq!(res.status(), 201);
    let ex = id_of(res).await;
    assert!(
        fx.inputs_of(ex).await["f"]
            .as_str()
            .unwrap()
            .ends_with("/inputs/f/a%20b.txt"),
        "kept verbatim, not decoded"
    );

    let before = (fx.executions().await, fx.objects().await.len());
    for bad in ["a?b.txt", "a#b.txt", "..", ""] {
        let msg = refusal(fx.upload(flow, &[Part::File("f", bad, b"x")]).await, 400).await;
        assert!(msg.contains("input `f`"), "{bad:?}: {msg}");
    }
    assert_eq!(
        (fx.executions().await, fx.objects().await.len()),
        before,
        "nothing created, nothing left stored"
    );
}

/// The parts must name declared inputs, once each, and a file only for a
/// FILE input.
#[tokio::test]
async fn parts_must_name_declared_inputs_once_and_files_only_for_file_inputs() {
    let Some(fx) = fx().await else { return };
    let flow = fx.flow(&reader_flow("parts", NS, "")).await;
    let cases: Vec<(Vec<Part<'_>>, &str)> = vec![
        (vec![Part::Text("nope", "x")], "no input `nope`"),
        (
            vec![Part::Text("t", "a"), Part::Text("t", "b")],
            "input `t` is given by more than one part",
        ),
        (vec![Part::File("t", "a.txt", b"x")], "input `t`"),
    ];
    for (parts, want) in cases {
        let msg = refusal(fx.upload(flow, &parts).await, 400).await;
        assert!(msg.contains(want), "{msg}");
    }
    assert_eq!(fx.executions().await, 0);
    assert!(fx.objects().await.is_empty(), "{:?}", fx.objects().await);
}

/// **The row and its index rows commit together** (spec §7.1). The
/// execution's INSERT is made to fail AT COMMIT (a deferred constraint
/// trigger scoped to this workspace), after the upload, the index rows and
/// the first settle were all written: neither row survives, and the uploaded
/// object is deleted. The control -- the same request once disarmed -- starts.
#[tokio::test(flavor = "multi_thread")]
async fn the_execution_row_and_its_input_rows_commit_together() {
    let Some(fx) = fx().await else { return };
    let flow = fx.flow(&reader_flow("atomic", NS, "")).await;
    let tag = Uuid::new_v4().simple().to_string();
    let func = format!("workflow.w9a_t7_fail_commit_{tag}");
    let trig = format!("w9a_t7_fail_commit_{tag}");
    for ddl in [
        format!(
            "CREATE FUNCTION {func}() RETURNS trigger LANGUAGE plpgsql AS $$ \
               BEGIN RAISE EXCEPTION 'w9a t7 test: the commit fails'; END $$"
        ),
        format!(
            "CREATE CONSTRAINT TRIGGER {trig} AFTER INSERT ON workflow.executions \
               DEFERRABLE INITIALLY DEFERRED FOR EACH ROW \
               WHEN (NEW.workspace_id = '{}') EXECUTE FUNCTION {func}()",
            fx.ws
        ),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(ddl))
            .execute(&fx.db.pool)
            .await
            .expect("arm");
    }
    let res = fx
        .upload(flow, &[Part::File("f", "a.txt", b"atomic")])
        .await;
    let status = res.status();
    for ddl in [
        format!("DROP TRIGGER {trig} ON workflow.executions"),
        format!("DROP FUNCTION {func}()"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(ddl))
            .execute(&fx.db.pool)
            .await
            .expect("disarm");
    }
    assert_eq!(status, 500, "the commit failed");
    assert_eq!(fx.executions().await, 0, "no execution row");
    assert_eq!(fx.file_rows().await, vec![], "no index row");
    assert_eq!(
        fx.objects().await,
        Vec::<String>::new(),
        "the upload is deleted"
    );

    // Control: disarmed, the same request commits all three.
    let res = fx
        .upload(flow, &[Part::File("f", "a.txt", b"atomic")])
        .await;
    assert_eq!(res.status(), 201);
    assert_eq!(fx.executions().await, 1);
    assert_eq!(fx.file_rows().await.len(), 1);
    assert_eq!(fx.objects().await.len(), 1);
}

/// **A store outage during the upload is a 503, not the caller's 4xx**
/// (spec §11), and nothing is inserted.
#[tokio::test]
async fn a_store_outage_during_the_upload_is_a_503_and_inserts_nothing() {
    let Some(fx) = fx().await else { return };
    let flow = fx.flow(&reader_flow("down", OUTAGE_NS, "")).await;
    let msg = refusal(
        fx.upload(flow, &[Part::File("f", "a.txt", b"x")]).await,
        503,
    )
    .await;
    assert!(msg.contains("inputs/f/a.txt"), "names the URI: {msg}");
    assert!(msg.contains("unavailable"), "{msg}");
    assert_eq!(fx.executions().await, 0);
    assert_eq!(fx.file_rows().await, vec![]);
    // The control: the same flow in a namespace the store serves starts.
    let up = fx.flow(&reader_flow("up", NS, "")).await;
    assert_eq!(
        fx.upload(up, &[Part::File("f", "a.txt", b"x")])
            .await
            .status(),
        201
    );
}

/// `w9a-storage.json` `file_input.allowed_file_extensions` (basis `chosen`):
/// egret refuses `bad.csv` for `[".txt"]` with Kestra's message, naming the
/// input, and stores nothing; `hello.txt` and `UP.TXT` start.
#[tokio::test]
async fn a_disallowed_extension_is_refused_with_the_artifacts_message() {
    let Some(fx) = fx().await else { return };
    let cell = &oracle()["egret_expected"]["file_input"]["allowed_file_extensions"];
    let flow = fx
        .flow(&reader_flow(
            "ext",
            NS,
            "    allowedFileExtensions: [\".txt\"]\n",
        ))
        .await;
    let msg = refusal(
        fx.upload(flow, &[Part::File("f", "bad.csv", b"a,b")]).await,
        400,
    )
    .await;
    assert!(msg.contains(cell["message"].as_str().unwrap()), "{msg}");
    assert!(msg.contains("input `f`"), "{msg}");
    assert_eq!(fx.executions().await, 0);
    assert!(fx.objects().await.is_empty());

    for ok in ["hello.txt", "UP.TXT"] {
        assert_eq!(
            fx.upload(flow, &[Part::File("f", ok, b"x")]).await.status(),
            201,
            "{ok}"
        );
    }
}

/// A file over `max_file_bytes` is a 413 naming the input and the cap, and
/// nothing is created or left stored; exactly the cap starts. A body over the
/// route's limit (`max_file_bytes` + 1 MiB) and a text part over 1 MiB are
/// 413s too.
#[tokio::test]
async fn a_file_over_max_file_bytes_is_refused_naming_the_cap() {
    let Some(fx) = fx().await else { return };
    let flow = fx.flow(&reader_flow("cap", NS, "")).await;
    let over = vec![b'x'; CAP as usize + 1];
    let msg = refusal(
        fx.upload(flow, &[Part::File("f", "big.txt", &over)]).await,
        413,
    )
    .await;
    assert!(msg.contains(&CAP.to_string()), "{msg}");
    assert!(msg.contains("max_file_bytes"), "{msg}");
    assert!(msg.contains("input `f`"), "{msg}");
    assert_eq!(fx.executions().await, 0);
    assert!(fx.objects().await.is_empty(), "{:?}", fx.objects().await);

    let at = vec![b'x'; CAP as usize];
    assert_eq!(
        fx.upload(flow, &[Part::File("f", "at.txt", &at)])
            .await
            .status(),
        201,
        "exactly the cap is allowed"
    );

    // A text part is held in memory whole: 1 MiB at most.
    let long = "y".repeat(1024 * 1024 + 1);
    let msg = refusal(fx.upload(flow, &[Part::Text("t", &long)]).await, 413).await;
    assert!(msg.contains("input `t`"), "{msg}");

    // Over the body limit with no single part over its own cap: the route's
    // limit is what refuses.
    let two = "two-inputs";
    let src = format!(
        "id: {two}\nnamespace: {NS}\ninputs:\n  - id: a\n    type: STRING\n  - id: b\n    type: STRING\n\
         tasks:\n  - id: l\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    );
    let flow2 = fx.flow(&src).await;
    let half = "z".repeat(600 * 1024);
    let res = fx
        .upload(flow2, &[Part::Text("a", &half), Part::Text("b", &half)])
        .await;
    assert_eq!(res.status(), 413);
    assert_eq!(fx.executions().await, 1, "only the at-the-cap start");
}

/// **In JSON, a FILE input is a `kestra://` URI readable from the execution
/// being created -- anything else is refused naming the input** (spec §7.1;
/// `file_input.file_as_string`, basis `chosen` for the two URI cells).
#[tokio::test]
async fn a_json_file_input_that_is_not_a_readable_uri_is_refused_naming_the_input() {
    let Some(fx) = fx().await else { return };
    let flow = fx.flow(&reader_flow("json-file", NS, "")).await;
    // The probe: another execution's REAL file, whose row exists.
    let res = fx
        .upload(flow, &[Part::File("f", "other.txt", b"other")])
        .await;
    assert_eq!(res.status(), 201);
    let other = id_of(res).await;
    let other_uri = fx.inputs_of(other).await["f"].as_str().unwrap().to_string();
    assert_eq!(
        fx.file_rows()
            .await
            .into_iter()
            .map(|r| r.2)
            .collect::<Vec<_>>(),
        vec![other_uri.clone()],
        "the probe: the file exists, so a refusal is about readability"
    );

    let cells = &oracle()["egret_expected"]["file_input"]["file_as_string"];
    let no_file = cells["kestra_uri_naming_no_file"]["kestra"]["error_at_run"]
        .as_str()
        .unwrap()
        .split("URI: ")
        .nth(1)
        .unwrap()
        .split(' ')
        .next()
        .unwrap()
        .to_string();
    for (value, want) in [
        (json!(other_uri), "may not read"),
        (json!(no_file), "is not a storage URI egret can read"),
        (json!("not a uri"), "is not a storage URI egret can read"),
        (json!(7), "not a number"),
    ] {
        let msg = refusal(fx.start_json(flow, json!({ "f": value })).await, 400).await;
        assert!(msg.contains("input `f`"), "{value}: {msg}");
        assert!(msg.contains(want), "{value}: {msg}");
    }
    assert_eq!(fx.executions().await, 1, "only the probe");
}

/// **The two-sided guard `start_inputs` has**, unchanged for a flow with no
/// FILE input: non-objects 400; no body, `{}` and `null` start; values are
/// stored verbatim. A FILE input given no value is not checked.
#[tokio::test]
async fn a_json_start_with_no_file_input_is_unchanged() {
    let Some(fx) = fx().await else { return };
    let src = format!(
        "id: plain\nnamespace: {NS}\ninputs:\n  - id: who\n    type: STRING\n\
         tasks:\n  - id: l\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    );
    let flow = fx.flow(&src).await;
    let path = fx.start_path(flow);
    for bad in [
        json!({ "inputs": [1, 2] }),
        json!({ "inputs": "x" }),
        json!({ "inputs": 7 }),
    ] {
        assert_eq!(
            common::post(&fx.app, &path, fx.user, bad.clone())
                .await
                .status(),
            400,
            "{bad}"
        );
    }
    assert_eq!(fx.executions().await, 0);
    assert_eq!(
        common::post_no_body(&fx.app, &path, fx.user).await.status(),
        201
    );
    assert_eq!(
        common::post(&fx.app, &path, fx.user, json!({}))
            .await
            .status(),
        201
    );
    assert_eq!(
        common::post(&fx.app, &path, fx.user, json!({ "inputs": null }))
            .await
            .status(),
        201
    );
    let res = fx.start_json(flow, json!({ "who": "world", "n": 3 })).await;
    assert_eq!(res.status(), 201);
    let ex = id_of(res).await;
    assert_eq!(fx.inputs_of(ex).await, json!({ "who": "world", "n": 3 }));

    // A body with no Content-Type is refused, not read as "no body": the
    // client orval generates for this route's two content types sends none,
    // and `Option<Json>` would have started it with `{}`.
    let res = common::send_raw(
        &fx.app,
        "POST",
        &path,
        fx.user,
        "",
        json!({ "inputs": { "who": "lost" } })
            .to_string()
            .into_bytes(),
    )
    .await;
    assert_eq!(res.status(), 415);
    assert_eq!(
        fx.executions().await,
        4,
        "the refused start created nothing"
    );

    // A FILE input left out is not checked (the render decides).
    let files = fx.flow(&reader_flow("no-value", NS, "")).await;
    assert_eq!(
        fx.start_json(files, json!({ "t": "x" })).await.status(),
        201
    );
}

fn parent_flow(id: &str, value: &str) -> String {
    format!(
        "id: {id}\nnamespace: {NS}\ninputs:\n  - id: f\n    type: FILE\n    required: false\n\
         tasks:\n  - id: call\n    type: io.kestra.plugin.core.flow.Subflow\n    \
         namespace: {NS}\n    flowId: child\n    inputs:\n      f: \"{value}\"\n"
    )
}

fn child_flow() -> String {
    format!(
        "id: child\nnamespace: {NS}\ninputs:\n  - id: f\n    type: FILE\n\
         tasks:\n  - id: r\n    type: io.kestra.plugin.core.debug.Return\n    \
         format: \"{{{{ read(inputs.f) }}}}\"\n"
    )
}

async fn children(fx: &Fx, child: Uuid) -> Vec<(Uuid, Value)> {
    sqlx::query_as("SELECT id, inputs FROM workflow.executions WHERE flow_id = $1")
        .bind(child)
        .fetch_all(&fx.db.pool)
        .await
        .unwrap()
}

/// A `Subflow` passes its parent's uploaded file on; the child's FILE input
/// is that URI, readable from the child (its trigger execution is the
/// parent), and its run reads the bytes.
#[tokio::test(flavor = "multi_thread")]
async fn a_subflow_child_receives_its_parents_file_uri_and_reads_it() {
    let Some(fx) = fx().await else { return };
    let child = fx.flow(&child_flow()).await;
    let parent = fx.flow(&parent_flow("parent", "{{ inputs.f }}")).await;
    let res = fx
        .upload(parent, &[Part::File("f", "p.txt", b"from the parent")])
        .await;
    assert_eq!(res.status(), 201);
    let p = id_of(res).await;
    let uri = fx.inputs_of(p).await["f"].as_str().unwrap().to_string();

    fx.drain().await;
    let kids = children(&fx, child).await;
    assert_eq!(kids.len(), 1, "{kids:?}");
    assert_eq!(kids[0].1["f"], json!(uri), "the parent's URI, verbatim");
    let (state, outputs, msg) = fx.run(kids[0].0, "r").await;
    assert_eq!(state, "SUCCESS", "{msg:?}");
    assert_eq!(outputs.unwrap()["value"], json!("from the parent"));
    assert_eq!(fx.state_of(p).await, "SUCCESS");
}

/// **An unreadable URI fails the parent's `Subflow` run, naming the input --
/// not the worker** (the tick answers `Ok`), and leaves no child row: an
/// unrelated execution's real file (`NotReadable`) and a plain string
/// (`Malformed`).
#[tokio::test(flavor = "multi_thread")]
async fn a_subflow_passing_an_unreadable_uri_fails_the_parents_run_not_the_worker() {
    let Some(fx) = fx().await else { return };
    let child = fx.flow(&child_flow()).await;
    // The probe: a real file of an execution unrelated to the parent.
    let reader = fx.flow(&reader_flow("unrelated", NS, "")).await;
    let res = fx
        .upload(reader, &[Part::File("f", "u.txt", b"unrelated")])
        .await;
    assert_eq!(res.status(), 201);
    let unrelated = id_of(res).await;
    let unrelated_uri = fx.inputs_of(unrelated).await["f"]
        .as_str()
        .unwrap()
        .to_string();
    fx.drain().await;

    for (id, value, want) in [
        ("p-unrelated", unrelated_uri.as_str(), "may not read"),
        (
            "p-plain",
            "not a uri",
            "is not a storage URI egret can read",
        ),
    ] {
        let parent = fx.flow(&parent_flow(id, value)).await;
        let res = fx.start_json(parent, json!({})).await;
        assert_eq!(res.status(), 201, "the parent itself starts");
        let p = id_of(res).await;
        fx.drain().await;
        let (state, _, msg) = fx.run(p, "call").await;
        assert_eq!(state, "FAILED", "{id}");
        let msg = msg.unwrap_or_default();
        assert!(
            msg.contains("input `f`") && msg.contains(want),
            "{id}: {msg}"
        );
        assert_eq!(fx.state_of(p).await, "FAILED");
    }
    assert_eq!(children(&fx, child).await, vec![], "no child row survives");

    // The control: the same parent shape passing a readable URI spawns one.
    let parent = fx.flow(&parent_flow("p-ok", "{{ inputs.f }}")).await;
    let res = fx.upload(parent, &[Part::File("f", "ok.txt", b"ok")]).await;
    assert_eq!(res.status(), 201);
    fx.drain().await;
    assert_eq!(children(&fx, child).await.len(), 1);
}

/// A `Flow` trigger passes its source's uploaded file on (the listener's
/// `inputs:` render against the source); the fired execution's FILE input is
/// that URI, readable from it (its trigger execution is the source), and its
/// run reads the bytes.
#[tokio::test(flavor = "multi_thread")]
async fn a_flow_trigger_passes_its_sources_file_uri_and_it_is_readable() {
    let Some(fx) = fx().await else { return };
    let source = fx.flow(&reader_flow("src", NS, "")).await;
    let listener_src = format!(
        "id: listener\nnamespace: {NS}\ninputs:\n  - id: f\n    type: FILE\n\
         triggers:\n  - id: on_done\n    type: io.kestra.plugin.core.trigger.Flow\n    \
         states: [SUCCESS]\n    when: \"{{{{ flow.id == 'src' }}}}\"\n    inputs:\n      f: \"{{{{ inputs.f }}}}\"\n\
         tasks:\n  - id: r\n    type: io.kestra.plugin.core.debug.Return\n    \
         format: \"{{{{ read(inputs.f) }}}}\"\n"
    );
    let listener = fx.flow(&listener_src).await;
    let res = fx
        .upload(source, &[Part::File("f", "s.txt", b"from the source")])
        .await;
    assert_eq!(res.status(), 201);
    let s = id_of(res).await;
    let uri = fx.inputs_of(s).await["f"].as_str().unwrap().to_string();
    fx.drain().await;
    assert_eq!(fx.state_of(s).await, "SUCCESS");

    egret_workflow::flow_trigger::process_events(&fx.db.pool)
        .await
        .expect("a pass");
    let fired: Vec<(Uuid, Value, Option<Uuid>)> = sqlx::query_as(
        "SELECT id, inputs, trigger_execution_id FROM workflow.executions WHERE flow_id = $1",
    )
    .bind(listener)
    .fetch_all(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(fired.len(), 1, "{fired:?}");
    assert_eq!(fired[0].1["f"], json!(uri));
    assert_eq!(fired[0].2, Some(s));
    fx.drain().await;
    let (state, outputs, msg) = fx.run(fired[0].0, "r").await;
    assert_eq!(state, "SUCCESS", "{msg:?}");
    assert_eq!(outputs.unwrap()["value"], json!("from the source"));
}

/// **Loop iterations and replays copy inputs already checked; their check
/// still runs and passes** (spec §7.1). Each iteration reads the root's
/// uploaded file, and so does a manual replay's -- whose copied URI names
/// the SOURCE execution, readable through `replayed_from_execution_id`.
#[tokio::test(flavor = "multi_thread")]
async fn loop_iterations_and_a_replay_pass_the_check_on_copied_file_inputs() {
    let Some(fx) = fx().await else { return };
    let src = format!(
        "id: looped\nnamespace: {NS}\ninputs:\n  - id: f\n    type: FILE\n\
         tasks:\n  - id: each\n    type: io.kestra.plugin.core.flow.Loop\n    values: '[\"a\", \"b\"]'\n    tasks:\n\
         \x20     - id: r\n        type: io.kestra.plugin.core.debug.Return\n        \
         format: \"{{{{ read(inputs.f) }}}}\"\n"
    );
    let flow = fx.flow(&src).await;
    let res = fx
        .upload(flow, &[Part::File("f", "l.txt", b"looped")])
        .await;
    assert_eq!(res.status(), 201);
    let root = id_of(res).await;
    fx.drain().await;
    assert_eq!(fx.state_of(root).await, "SUCCESS");

    let values = |of: Uuid| {
        let pool = fx.db.pool.clone();
        async move {
            sqlx::query_scalar::<_, Option<Value>>(
                "SELECT tr.outputs FROM workflow.task_runs tr \
                   JOIN workflow.executions e ON e.id = tr.execution_id \
                  WHERE e.parent_execution_id = $1 AND e.loop_index IS NOT NULL \
                    AND tr.task_id = 'r' ORDER BY e.loop_index",
            )
            .bind(of)
            .fetch_all(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|o| o.unwrap()["value"].clone())
            .collect::<Vec<_>>()
        }
    };
    assert_eq!(values(root).await, vec![json!("looped"), json!("looped")]);

    let res = common::post_no_body(
        &fx.app,
        &format!("/api/ws/{}/workflows/executions/{root}/replay", fx.ws),
        fx.user,
    )
    .await;
    assert_eq!(res.status(), 201);
    let replay = id_of(res).await;
    assert_eq!(
        fx.inputs_of(replay).await,
        fx.inputs_of(root).await,
        "a replay copies its source's inputs: the URI names the source"
    );
    fx.drain().await;
    assert_eq!(fx.state_of(replay).await, "SUCCESS");
    assert_eq!(values(replay).await, vec![json!("looped"), json!("looped")]);
}

/// **The first settle already sees the uploaded file**: it runs in the
/// transaction that indexed it (spec §7.1). `r`'s `runIf` reads
/// `fileSize(inputs.f)` while the start is still uncommitted; an index row
/// written anywhere else -- after the commit, on another connection -- is
/// `Missing` there and the run fails instead of starting.
#[tokio::test(flavor = "multi_thread")]
async fn the_first_settle_reads_the_uploaded_file_in_the_starting_transaction() {
    let Some(fx) = fx().await else { return };
    let src = format!(
        "id: first-settle\nnamespace: {NS}\ninputs:\n  - id: f\n    type: FILE\n\
         tasks:\n  - id: r\n    type: io.kestra.plugin.core.log.Log\n    \
         runIf: \"{{{{ fileSize(inputs.f) == 6 }}}}\"\n    message: ran\n"
    );
    let flow = fx.flow(&src).await;
    let res = fx
        .upload(flow, &[Part::File("f", "six.txt", b"sixsix")])
        .await;
    assert_eq!(res.status(), 201);
    let ex = id_of(res).await;
    let (state, _, msg) = fx.run(ex, "r").await;
    assert_eq!(state, "RUNNING", "runIf saw the 6-byte file: {msg:?}");
    fx.drain().await;
}
