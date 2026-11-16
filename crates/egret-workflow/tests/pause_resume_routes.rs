//! The pause, resume and validate-resume routes (workflow package 7c, spec
//! `docs/en/specs/2026-11-15-workflow-pause-resume-design.md` D3, D5-D9,
//! Q3-Q6):
//!
//! ```text
//! POST /api/ws/{workspace}/workflows/executions/{id}/pause
//! POST /api/ws/{workspace}/workflows/executions/{id}/resume           (optional JSON or multipart body)
//! POST /api/ws/{workspace}/workflows/executions/{id}/resume/validate
//! ```
//!
//! Every refusal is paired with an accepted control. Every test is
//! `#[sqlx::test]`: the worker's `claim` and `tick` are global, so each test
//! owns its database.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5465/egret_test
//!   cargo test -p egret-workflow --test pause_resume_routes

use std::time::Duration;

use egret_metastore::Db;
use egret_workflow::worker::{Handlers, Registry};
use serde_json::{json, Value};
use uuid::Uuid;

mod common;

const LOG: &str = "io.kestra.plugin.core.log.Log";

// ── Fixtures ────────────────────────────────────────────────────────────────

fn one_task(id: &str) -> String {
    format!("id: {id}\nnamespace: t\ntasks:\n  - id: a\n    type: {LOG}\n    message: hi\n")
}

fn two_tasks(id: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: a\n    type: {LOG}\n    message: hi\n  \
         - id: b\n    type: {LOG}\n    message: bye\n"
    )
}

/// Spec §4's flow (T4 test 16): a `Pause` asking who approves, a count and
/// an optional file, then a `Log` reading them.
const APPROVE: &str = r#"id: approve
namespace: t
tasks:
  - id: p
    type: io.kestra.plugin.core.flow.Pause
    onResume:
      - id: who
        type: STRING
      - id: n
        type: INT
      - id: doc
        type: FILE
        required: false
  - id: log
    type: io.kestra.plugin.core.log.Log
    message: "who={{ outputs.p.onResume.who }} n={{ outputs.p.onResume.n }}"
"#;

/// An unbounded `Pause` with no `onResume`.
const PLAIN: &str = "id: plain\nnamespace: t\ntasks:\n  - id: p\n    \
                     type: io.kestra.plugin.core.flow.Pause\n  \
                     - id: after\n    type: io.kestra.plugin.core.log.Log\n    message: after\n";

/// `resume-validate.yaml`'s shape.
const BOOLY: &str = "id: booly\nnamespace: t\ntasks:\n  - id: p\n    \
                     type: io.kestra.plugin.core.flow.Pause\n    \
                     onResume:\n      - id: approved\n        type: BOOL\n        \
                     defaults: true\n  \
                     - id: after\n    type: io.kestra.plugin.core.log.Log\n    message: after\n";

const UNTIL: &str = "id: until\nnamespace: t\ntasks:\n  - id: lu\n    \
                     type: io.kestra.plugin.core.flow.LoopUntil\n    \
                     condition: \"{{ false }}\"\n    \
                     checkFrequency:\n      interval: PT1H\n      maxIterations: 3\n    \
                     tasks:\n      \
                     - id: body\n        type: io.kestra.plugin.core.log.Log\n        message: hi\n";

/// T3 test 6's `Loop` flow.
const LOOPY: &str = r#"id: loopy
namespace: t
tasks:
  - id: each
    type: io.kestra.plugin.core.flow.Loop
    values: ["1", "2"]
    concurrencyLimit: 1
    tasks:
      - id: a
        type: io.kestra.plugin.core.log.Log
        message: "{{ item.value }}"
  - id: after
    type: io.kestra.plugin.core.log.Log
    message: done
"#;

async fn setup(pool: &sqlx::PgPool) -> (Db, Uuid, egret_core::UserId, common::App) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage", "workflow:read"]).await;
    let app = common::app(&db).await;
    (db, ws, user, app)
}

/// The flow row `source` is a draft of, created on first use (the
/// workspace's creator owns it).
async fn flow_of(pool: &sqlx::PgPool, ws: Uuid, source: &str) -> Uuid {
    let parsed = egret_workflow::spec::parse(source).expect("parse");
    let found: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM workflow.flows WHERE workspace_id = $1 AND namespace = $2 AND flow_id = $3",
    )
    .bind(ws)
    .bind(&parsed.namespace)
    .bind(&parsed.id)
    .fetch_optional(pool)
    .await
    .expect("look up the flow");
    if let Some(id) = found {
        return id;
    }
    let owner: Uuid = sqlx::query_scalar("SELECT created_by FROM workspaces WHERE id = $1")
        .bind(ws)
        .fetch_one(pool)
        .await
        .expect("workspace owner");
    let db = Db { pool: pool.clone() };
    common::seed_flow(&db, ws, egret_core::UserId(owner), source, "draft").await
}

async fn start(pool: &sqlx::PgPool, ws: Uuid, source: &str) -> Uuid {
    let flow = flow_of(pool, ws, source).await;
    egret_workflow::worker::start_execution(pool, ws, Some(flow), source, json!({}))
        .await
        .expect("start execution")
}

async fn exec_state(pool: &sqlx::PgPool, id: Uuid) -> String {
    sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("execution state")
}

async fn exec_history(pool: &sqlx::PgPool, id: Uuid) -> Value {
    sqlx::query_scalar("SELECT history FROM workflow.executions WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("history")
}

async fn run_state(pool: &sqlx::PgPool, ex: Uuid, task: &str) -> Option<String> {
    sqlx::query_scalar(
        "SELECT state FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(ex)
    .bind(task)
    .fetch_optional(pool)
    .await
    .expect("run state")
}

async fn run_id_of(pool: &sqlx::PgPool, ex: Uuid, task: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2")
        .bind(ex)
        .bind(task)
        .fetch_one(pool)
        .await
        .expect("run id")
}

async fn outputs_of(pool: &sqlx::PgPool, ex: Uuid, task: &str) -> Option<Value> {
    sqlx::query_scalar(
        "SELECT outputs FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(ex)
    .bind(task)
    .fetch_one(pool)
    .await
    .expect("outputs")
}

async fn indexed_files(pool: &sqlx::PgPool, ex: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM workflow.execution_files WHERE execution_id = $1")
        .bind(ex)
        .fetch_one(pool)
        .await
        .expect("count files")
}

/// What the store holds under `prefix` (an `s3://bucket/...` URI, as
/// `FileStore`'s prefixes are): `list` takes the key, without the bucket.
async fn listed_under(
    fs: &egret_workflow::storage::FileStore,
    prefix: &str,
) -> egret_storage::Listing {
    let key = prefix
        .strip_prefix("s3://")
        .and_then(|r| r.split_once('/'))
        .map(|(_, k)| k)
        .expect("an s3:// URI");
    fs.store.list(key, None).await.unwrap()
}

fn handlers() -> Handlers {
    Handlers::with_core_tasks(egret_workflow::storage::current())
}

async fn settle_everything(pool: &sqlx::PgPool, reg: &dyn Registry) {
    for _ in 0..64 {
        let ran = egret_workflow::worker::tick(pool, 16, reg)
            .await
            .expect("tick");
        if ran == 0 {
            return;
        }
    }
    panic!("the loop never settled in 64 rounds");
}

/// `source` started and driven until its `Pause` waits.
async fn waiting(pool: &sqlx::PgPool, ws: Uuid, source: &str) -> Uuid {
    let ex = start(pool, ws, source).await;
    settle_everything(pool, &handlers()).await;
    assert_eq!(exec_state(pool, ex).await, "PAUSED");
    ex
}

fn pause_path(ws: Uuid, id: Uuid) -> String {
    format!("/api/ws/{ws}/workflows/executions/{id}/pause")
}
fn resume_path(ws: Uuid, id: Uuid) -> String {
    format!("/api/ws/{ws}/workflows/executions/{id}/resume")
}
fn validate_path(ws: Uuid, id: Uuid) -> String {
    format!("/api/ws/{ws}/workflows/executions/{id}/resume/validate")
}

async fn expect_error(res: axum::response::Response, status: u16, code: Option<&str>) -> String {
    assert_eq!(res.status(), status);
    let body = common::json(res).await;
    assert_eq!(body["code"].as_str(), code, "{body}");
    body["error"].as_str().unwrap_or_default().to_string()
}

enum Part<'a> {
    Text(&'a str, &'a str),
    File(&'a str, &'a str, &'a [u8]),
}

const BOUNDARY: &str = "w7c-t5-boundary-9a2e";

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

async fn post_multipart(
    app: &common::App,
    path: &str,
    user: egret_core::UserId,
    parts: &[Part<'_>],
) -> axum::response::Response {
    common::send_raw(
        app,
        "POST",
        path,
        user,
        &format!("multipart/form-data; boundary={BOUNDARY}"),
        multipart_body(parts),
    )
    .await
}

/// A second person in `ws` with `workflow:manage`, who did not create its
/// flows.
async fn other_manager(db: &Db, ws: Uuid) -> egret_core::UserId {
    let uid: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, name, provider, status, password_hash) \
         VALUES ($1, 't', 'local', 'active', 'x') RETURNING id",
    )
    .bind(format!("u_{}@t.local", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("seed user");
    let user = egret_core::UserId(uid);
    common::grant(db, ws, user, &["workflow:manage", "workflow:read"]).await;
    user
}

/// `ex`'s frozen source with an `egret.sql` task appended to its root list
/// (the run not reached yet), as if its creator had started a flow that runs
/// SQL as them (the approach of `tests/restart_routes.rs`).
async fn give_it_sql(pool: &sqlx::PgPool, ex: Uuid) {
    let source: String = sqlx::query_scalar("SELECT source FROM workflow.executions WHERE id = $1")
        .bind(ex)
        .fetch_one(pool)
        .await
        .unwrap();
    let source = format!(
        "{source}  - id: q\n    type: egret.sql\n    database: trino_prod\n    sql: SELECT 1\n"
    );
    egret_workflow::spec::parse(&source).expect("the SQL source parses");
    sqlx::query("UPDATE workflow.executions SET source = $2 WHERE id = $1")
        .bind(ex)
        .bind(&source)
        .execute(pool)
        .await
        .unwrap();
}

async fn flow_id_of(pool: &sqlx::PgPool, ex: Uuid) -> Uuid {
    sqlx::query_scalar("SELECT flow_id FROM workflow.executions WHERE id = $1")
        .bind(ex)
        .fetch_one(pool)
        .await
        .expect("flow id")
}

// ── 1-4: pause ──────────────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn pause_answers_200_paused_and_is_audited(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = start(&pool, ws, &one_task("one")).await;
    assert_eq!(run_state(&pool, ex, "a").await.as_deref(), Some("RUNNING"));

    // Act
    let res = common::post_no_body(&app, &pause_path(ws, ex), user).await;

    // Assert
    assert_eq!(res.status(), 200);
    let body = common::json(res).await;
    assert_eq!(body["state"], "PAUSED", "{body}");
    assert_eq!(body["paused_by_hand"], true, "{body}");
    assert_eq!(
        common::audit_mutations(&app),
        vec![(
            "workflow_execution:pause".to_string(),
            Some(ex.to_string()),
            json!({ "from_state": "RUNNING" }),
        )]
    );
    let got = common::json(
        common::get(
            &app,
            &format!("/api/ws/{ws}/workflows/executions/{ex}"),
            user,
        )
        .await,
    )
    .await;
    assert_eq!(got["paused_by_hand"], true, "{got}");

    // A `Pause` task waiting is not paused by hand.
    let other = waiting(&pool, ws, PLAIN).await;
    let got = common::json(
        common::get(
            &app,
            &format!("/api/ws/{ws}/workflows/executions/{other}"),
            user,
        )
        .await,
    )
    .await;
    assert_eq!(got["state"], "PAUSED", "{got}");
    assert_eq!(got["paused_by_hand"], false, "{got}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn pause_resume_and_validate_need_workflow_manage(pool: sqlx::PgPool) {
    // Arrange
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read"]).await;
    let app = common::app(&db).await;
    let paused = waiting(&pool, ws, APPROVE).await;
    let running = start(&pool, ws, &one_task("one")).await;

    // Act
    let pause = common::post_no_body(&app, &pause_path(ws, running), user).await;
    let resume = common::post_no_body(&app, &resume_path(ws, paused), user).await;
    let validate = common::post_no_body(&app, &validate_path(ws, paused), user).await;

    // Assert
    assert_eq!(pause.status(), 403);
    assert_eq!(resume.status(), 403);
    assert_eq!(validate.status(), 403);
    assert_eq!(exec_state(&pool, running).await, "RUNNING");
    assert_eq!(exec_state(&pool, paused).await, "PAUSED");
    assert!(common::audit_mutations(&app).is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn pause_refuses_what_is_not_running(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;

    // Paused already.
    let ex = start(&pool, ws, &one_task("one")).await;
    assert_eq!(
        common::post_no_body(&app, &pause_path(ws, ex), user)
            .await
            .status(),
        200
    );
    let msg = expect_error(
        common::post_no_body(&app, &pause_path(ws, ex), user).await,
        409,
        Some("not_running"),
    )
    .await;
    assert!(msg.contains("PAUSED"), "{msg}");

    // KILLING: a kill of a claimed run stays KILLING.
    let killing = start(&pool, ws, &one_task("killme")).await;
    egret_workflow::worker::claim(&pool, 10)
        .await
        .expect("claim");
    egret_workflow::worker::kill_execution(&pool, ws, killing, false)
        .await
        .expect("kill");
    assert_eq!(exec_state(&pool, killing).await, "KILLING");
    let msg = expect_error(
        common::post_no_body(&app, &pause_path(ws, killing), user).await,
        409,
        Some("not_running"),
    )
    .await;
    assert!(msg.contains("KILLING"), "{msg}");
    assert_eq!(exec_state(&pool, killing).await, "KILLING");

    // Terminal.
    let done = start(&pool, ws, &one_task("done")).await;
    settle_everything(&pool, &handlers()).await;
    assert_eq!(exec_state(&pool, done).await, "SUCCESS");
    let msg = expect_error(
        common::post_no_body(&app, &pause_path(ws, done), user).await,
        409,
        Some("not_running"),
    )
    .await;
    assert!(msg.contains("SUCCESS"), "{msg}");

    // A task waiting (spec Q5): `quick` ended, `slow` waits, RUNNING.
    let src = "id: two\nnamespace: t\ntasks:\n  - id: both\n    \
               type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      \
               - id: quick\n        type: io.kestra.plugin.core.flow.Pause\n        \
               pauseDuration: PT0.1S\n      \
               - id: slow\n        type: io.kestra.plugin.core.flow.Pause\n        \
               pauseDuration: PT1H\n  \
               - id: after\n    type: io.kestra.plugin.core.log.Log\n    message: after\n";
    let ex = start(&pool, ws, src).await;
    let mut reached = false;
    for _ in 0..20 {
        settle_everything(&pool, &handlers()).await;
        if run_state(&pool, ex, "quick").await.as_deref() == Some("SUCCESS")
            && run_state(&pool, ex, "slow").await.as_deref() == Some("PAUSED")
            && exec_state(&pool, ex).await == "RUNNING"
        {
            reached = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
        egret_workflow::worker::wake(&pool, 16).await.expect("wake");
    }
    assert!(reached);
    let msg = expect_error(
        common::post_no_body(&app, &pause_path(ws, ex), user).await,
        409,
        Some("not_running"),
    )
    .await;
    assert!(msg.contains("already waiting"), "{msg}");
    assert_eq!(exec_state(&pool, ex).await, "RUNNING");
}

#[sqlx::test(migrations = "../../migrations")]
async fn resume_of_a_running_execution_is_409_not_paused(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = start(&pool, ws, &one_task("one")).await;

    // Act
    let resume = common::post_no_body(&app, &resume_path(ws, ex), user).await;
    let validate = common::post_no_body(&app, &validate_path(ws, ex), user).await;

    // Assert
    let msg = expect_error(resume, 409, Some("not_paused")).await;
    assert!(msg.contains("RUNNING"), "{msg}");
    expect_error(validate, 409, Some("not_paused")).await;
    assert_eq!(exec_state(&pool, ex).await, "RUNNING");
    assert!(common::audit_mutations(&app).is_empty());
}

// ── 5-8: resume ─────────────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn resume_by_hand_runs_on_and_inputs_are_refused(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = start(&pool, ws, &two_tasks("hand")).await;
    assert_eq!(
        common::post_no_body(&app, &pause_path(ws, ex), user)
            .await
            .status(),
        200
    );
    common::audit_mutations(&app);
    let history = exec_history(&pool, ex).await;

    // Act + Assert: JSON inputs are refused.
    let res = common::post(
        &app,
        &resume_path(ws, ex),
        user,
        json!({ "inputs": { "x": 1 } }),
    )
    .await;
    expect_error(res, 400, Some("resume_inputs_unexpected")).await;
    assert_eq!(exec_history(&pool, ex).await, history);

    // A multipart text part is refused.
    let res = post_multipart(&app, &resume_path(ws, ex), user, &[Part::Text("x", "1")]).await;
    expect_error(res, 400, Some("resume_inputs_unexpected")).await;
    assert_eq!(exec_history(&pool, ex).await, history);
    assert!(common::audit_mutations(&app).is_empty());

    // The control: no body runs it on.
    let res = common::post_no_body(&app, &resume_path(ws, ex), user).await;
    assert_eq!(res.status(), 200);
    let body = common::json(res).await;
    assert_ne!(body["state"], "PAUSED", "{body}");
    assert_eq!(body["paused_by_hand"], false, "{body}");
    assert_ne!(exec_state(&pool, ex).await, "PAUSED");
    assert_eq!(
        common::audit_mutations(&app),
        vec![(
            "workflow_execution:resume".to_string(),
            Some(ex.to_string()),
            json!({
                "from_state": "PAUSED",
                "by_hand": true,
                "task_run_id": null,
                "inputs_given": false,
            }),
        )]
    );
}

/// Carried from T4's review (item 2): a FILE part on an execution paused by
/// hand is refused before anything is stored.
#[sqlx::test(migrations = "../../migrations")]
async fn a_file_part_on_a_resume_by_hand_stores_nothing(pool: sqlx::PgPool) {
    // Arrange
    let fs = egret_workflow::storage::install_mem_for_tests();
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = start(&pool, ws, &two_tasks("hand")).await;
    assert_eq!(
        common::post_no_body(&app, &pause_path(ws, ex), user)
            .await
            .status(),
        200
    );

    // Act
    let res = post_multipart(
        &app,
        &resume_path(ws, ex),
        user,
        &[Part::File("doc", "a.txt", b"hello")],
    )
    .await;

    // Assert
    expect_error(res, 400, Some("resume_inputs_unexpected")).await;
    let listed = listed_under(&fs, &fs.execution_prefix(ws, "t", "hand", ex)).await;
    assert!(listed.objects.is_empty(), "{listed:?}");
    assert_eq!(indexed_files(&pool, ex).await, 0);
    assert_eq!(exec_state(&pool, ex).await, "PAUSED");
}

#[sqlx::test(migrations = "../../migrations")]
async fn resume_with_inputs_writes_them_and_a_refused_one_is_named(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = waiting(&pool, ws, APPROVE).await;
    let p = run_id_of(&pool, ex, "p").await;
    let history = exec_history(&pool, ex).await;

    // Act + Assert: a refused value is named.
    let res = common::post(
        &app,
        &resume_path(ws, ex),
        user,
        json!({ "inputs": { "who": "ada", "n": "x" } }),
    )
    .await;
    let msg = expect_error(res, 400, None).await;
    assert!(msg.contains("`n`"), "{msg}");

    // An undeclared key is refused (S6), named.
    let res = common::post(
        &app,
        &resume_path(ws, ex),
        user,
        json!({ "inputs": { "who": "ada", "n": "3", "zz": 1 } }),
    )
    .await;
    let msg = expect_error(res, 400, None).await;
    assert!(msg.contains("`zz`"), "{msg}");

    // `inputs` that is not an object is refused as a start refuses it.
    let res = common::post(&app, &resume_path(ws, ex), user, json!({ "inputs": [1] })).await;
    let msg = expect_error(res, 400, None).await;
    assert!(msg.contains("inputs must be a JSON object"), "{msg}");
    assert_eq!(exec_history(&pool, ex).await, history);
    assert_eq!(outputs_of(&pool, ex, "p").await, None);
    assert!(common::audit_mutations(&app).is_empty());

    // The control.
    let res = common::post(
        &app,
        &resume_path(ws, ex),
        user,
        json!({ "inputs": { "who": "ada", "n": "3" } }),
    )
    .await;
    assert_eq!(res.status(), 200);
    let out = outputs_of(&pool, ex, "p").await.expect("p has outputs");
    assert_eq!(
        out["onResume"],
        json!({ "who": "ada", "n": 3, "doc": null }),
        "{out}"
    );
    assert_eq!(
        common::audit_mutations(&app),
        vec![(
            "workflow_execution:resume".to_string(),
            Some(ex.to_string()),
            json!({
                "from_state": "PAUSED",
                "by_hand": false,
                "task_run_id": p,
                "inputs_given": true,
            }),
        )]
    );
}

/// Carried from T4's review (item 5): no body on a `Pause` whose `onResume`
/// requires a value is a 400 that says the resume needs a body.
#[sqlx::test(migrations = "../../migrations")]
async fn no_body_on_a_pause_that_requires_inputs_is_400_saying_so(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = waiting(&pool, ws, APPROVE).await;

    // Act
    let res = common::post_no_body(&app, &resume_path(ws, ex), user).await;

    // Assert
    let msg = expect_error(res, 400, None).await;
    assert!(msg.contains("Missing required input:who"), "{msg}");
    assert!(msg.contains("onResume"), "{msg}");
    assert_eq!(exec_state(&pool, ex).await, "PAUSED");
}

/// Carried from T4's review (item 4): a SECRET value never reaches the audit
/// record.
#[sqlx::test(migrations = "../../migrations")]
async fn a_secret_on_resume_value_is_not_in_the_audit_record(pool: sqlx::PgPool) {
    // Arrange
    egret_workflow::secret_inputs::install_for_tests();
    let (_db, ws, user, app) = setup(&pool).await;
    let src = "id: sec\nnamespace: t\ntasks:\n  - id: p\n    \
               type: io.kestra.plugin.core.flow.Pause\n    \
               onResume:\n      - id: tok\n        type: SECRET\n  \
               - id: after\n    type: io.kestra.plugin.core.log.Log\n    message: after\n";
    let ex = waiting(&pool, ws, src).await;

    // Act
    let res = common::post(
        &app,
        &resume_path(ws, ex),
        user,
        json!({ "inputs": { "tok": "s3cr3t-value" } }),
    )
    .await;

    // Assert
    assert_eq!(res.status(), 200);
    let body = common::body_text(res).await;
    assert!(!body.contains("s3cr3t-value"), "{body}");
    let audit = common::audit_mutations(&app);
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert_eq!(audit[0].0, "workflow_execution:resume");
    assert_eq!(audit[0].2["inputs_given"], true, "{audit:?}");
    assert!(
        !audit[0].2.to_string().contains("s3cr3t-value"),
        "{audit:?}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_pause_without_on_resume_keeps_todays_resume(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let refused = waiting(&pool, ws, PLAIN).await;
    let accepted = waiting(&pool, ws, PLAIN).await;

    // Act
    let refusal = common::post(
        &app,
        &resume_path(ws, refused),
        user,
        json!({ "inputs": { "a": 1 } }),
    )
    .await;
    let control = common::post_no_body(&app, &resume_path(ws, accepted), user).await;

    // Assert
    expect_error(refusal, 400, Some("resume_inputs_unexpected")).await;
    assert_eq!(exec_state(&pool, refused).await, "PAUSED");
    assert_eq!(control.status(), 200);
    assert_eq!(
        run_state(&pool, accepted, "p").await.as_deref(),
        Some("SUCCESS")
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn only_a_loop_until_waiting_is_still_409(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = waiting(&pool, ws, UNTIL).await;
    let history = exec_history(&pool, ex).await;

    // Act
    let res = common::post_no_body(&app, &resume_path(ws, ex), user).await;

    // Assert
    let msg = expect_error(res, 409, None).await;
    assert!(msg.contains("Nothing to resume"), "{msg}");
    assert_eq!(exec_history(&pool, ex).await, history);

    // T5 review C1: inputs where no `onResume` applies are refused up front.
    let res = common::post(
        &app,
        &resume_path(ws, ex),
        user,
        json!({ "inputs": { "who": "x" } }),
    )
    .await;
    expect_error(res, 400, Some("resume_inputs_unexpected")).await;
    assert_eq!(exec_history(&pool, ex).await, history);
}

/// T5 review: a file part for an input that is not FILE is refused naming
/// it, as a start refuses it, and a file the request already stored goes.
#[sqlx::test(migrations = "../../migrations")]
async fn a_file_part_for_a_non_file_input_is_refused_and_its_uploads_deleted(pool: sqlx::PgPool) {
    // Arrange
    let fs = egret_workflow::storage::install_mem_for_tests();
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = waiting(&pool, ws, APPROVE).await;
    let run = run_id_of(&pool, ex, "p").await;

    // Act: `doc` is stored first, then `who` (a STRING) comes as a file.
    let res = post_multipart(
        &app,
        &resume_path(ws, ex),
        user,
        &[
            Part::File("doc", "a.txt", b"hello"),
            Part::Text("n", "3"),
            Part::File("who", "who.txt", b"ada"),
        ],
    )
    .await;
    let validate = post_multipart(
        &app,
        &validate_path(ws, ex),
        user,
        &[Part::File("who", "who.txt", b"ada")],
    )
    .await;

    // Assert
    let msg = expect_error(res, 400, None).await;
    assert!(msg.contains("`who`"), "{msg}");
    assert!(msg.contains("not a FILE input"), "{msg}");
    let listed = listed_under(&fs, &fs.resume_root(ws, "t", "approve", ex, "p", run)).await;
    assert!(listed.objects.is_empty(), "{listed:?}");
    assert_eq!(indexed_files(&pool, ex).await, 0);
    assert_eq!(exec_state(&pool, ex).await, "PAUSED");
    let msg = expect_error(validate, 400, None).await;
    assert!(msg.contains("`who`"), "{msg}");
}

// ── 9: FILE inputs (S2, D6; carried items 1-3) ──────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn a_multipart_file_resume_stores_under_the_pause_run_and_a_refused_one_deletes_only_its_own(
    pool: sqlx::PgPool,
) {
    // Arrange
    let fs = egret_workflow::storage::install_mem_for_tests();
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = waiting(&pool, ws, APPROVE).await;
    let run = run_id_of(&pool, ex, "p").await;
    // Another execution with a FILE start input, uploaded by multipart.
    let starter = "id: starter\nnamespace: t\ninputs:\n  - id: f\n    type: FILE\n\
                   tasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n";
    let starter_flow = flow_of(&pool, ws, starter).await;
    let res = post_multipart(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/{starter_flow}/executions"),
        user,
        &[Part::File("f", "in.txt", b"start input")],
    )
    .await;
    assert_eq!(res.status(), 201);
    let other: Uuid = serde_json::from_value(common::json(res).await["id"].clone()).unwrap();
    let other_prefix = fs.execution_prefix(ws, "t", "starter", other);
    let listed = listed_under(&fs, &other_prefix).await;
    assert_eq!(listed.objects.len(), 1, "{listed:?}");
    // An object of the paused execution's own, outside the resume's prefix.
    let own_prefix = fs.execution_prefix(ws, "t", "approve", ex);
    fs.store
        .put(
            &format!("{own_prefix}inputs/keep/keep.txt"),
            b"keep".to_vec(),
        )
        .await
        .unwrap();
    let before_own = listed_under(&fs, &own_prefix).await;
    assert_eq!(before_own.objects.len(), 1, "{before_own:?}");
    common::audit_mutations(&app);

    // Act: a refused resume carrying a file.
    let res = post_multipart(
        &app,
        &resume_path(ws, ex),
        user,
        &[
            Part::Text("who", "ada"),
            Part::Text("n", "x"),
            Part::File("doc", "a.txt", b"hello"),
        ],
    )
    .await;

    // Assert: refused, its own upload deleted, no index row, others untouched.
    let msg = expect_error(res, 400, None).await;
    assert!(msg.contains("`n`"), "{msg}");
    let resume_root = fs.resume_root(ws, "t", "approve", ex, "p", run);
    let listed = listed_under(&fs, &resume_root).await;
    assert!(listed.objects.is_empty(), "{listed:?}");
    assert_eq!(indexed_files(&pool, ex).await, 0);
    assert_eq!(
        listed_under(&fs, &own_prefix).await,
        before_own,
        "the execution's own prefix is not deleted"
    );
    let other_uri: String =
        sqlx::query_scalar("SELECT uri FROM workflow.execution_files WHERE execution_id = $1")
            .bind(other)
            .fetch_one(&pool)
            .await
            .expect("the start input is indexed");
    let other_object = fs.object_uri(ws, &other_uri).expect("an object URI");
    assert!(
        fs.store.head(&other_object).await.unwrap().is_some(),
        "the other execution's start input is still stored"
    );
    assert_eq!(exec_state(&pool, ex).await, "PAUSED");

    // Act: the accepted resume.
    let res = post_multipart(
        &app,
        &resume_path(ws, ex),
        user,
        &[
            Part::Text("who", "ada"),
            Part::Text("n", "3"),
            Part::File("doc", "a.txt", b"hello"),
        ],
    )
    .await;

    // Assert
    assert_eq!(res.status(), 200);
    let out = outputs_of(&pool, ex, "p").await.expect("p outputs");
    let uri = out["onResume"]["doc"].as_str().expect("a URI").to_string();
    assert!(uri.contains(&format!("/tasks/p/{run}/resume/")), "{uri}");
    assert_eq!(listed_under(&fs, &resume_root).await.objects.len(), 1);
    assert_eq!(indexed_files(&pool, ex).await, 1);
    let enc: String = url::form_urlencoded::byte_serialize(uri.as_bytes()).collect();
    let res = common::get(
        &app,
        &format!("/api/ws/{ws}/workflows/executions/{ex}/file?path={enc}"),
        user,
    )
    .await;
    assert_eq!(res.status(), 200);
    assert_eq!(common::body_text(res).await, "hello");
}

// ── 10-12: the gates ────────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn the_egret_sql_gate_applies_only_with_on_resume(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, creator, _app) = setup(&pool).await;
    let other = other_manager(&db, ws).await;
    // An app built after the grant, which it reads at build.
    let app = common::app(&db).await;
    let with_inputs = waiting(&pool, ws, APPROVE).await;
    give_it_sql(&pool, with_inputs).await;

    // Act + Assert: a non-creator is refused before any body is read.
    let res = common::post(
        &app,
        &resume_path(ws, with_inputs),
        other,
        json!({ "inputs": { "who": "ada", "n": "3" } }),
    )
    .await;
    let msg = expect_error(res, 400, None).await;
    assert!(
        msg.contains("only the flow's creator can resume it"),
        "{msg}"
    );
    assert_eq!(exec_state(&pool, with_inputs).await, "PAUSED");
    assert!(common::audit_mutations(&app).is_empty());

    // The creator passes.
    let res = common::post(
        &app,
        &resume_path(ws, with_inputs),
        creator,
        json!({ "inputs": { "who": "ada", "n": "3" } }),
    )
    .await;
    assert_eq!(res.status(), 200);

    // A `Pause` without `onResume`: anyone with workflow:manage.
    let plain = waiting(&pool, ws, PLAIN).await;
    give_it_sql(&pool, plain).await;
    let res = common::post_no_body(&app, &resume_path(ws, plain), other).await;
    assert_eq!(res.status(), 200);

    // Paused by hand: anyone with workflow:manage.
    let hand = start(&pool, ws, &two_tasks("hand")).await;
    give_it_sql(&pool, hand).await;
    let res = common::post_no_body(&app, &pause_path(ws, hand), other).await;
    assert_eq!(res.status(), 200);
    let res = common::post_no_body(&app, &resume_path(ws, hand), other).await;
    assert_eq!(res.status(), 200);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_trashed_flow_is_404_and_a_disabled_one_is_not(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let paused = waiting(&pool, ws, APPROVE).await;
    let running = start(&pool, ws, &two_tasks("gated")).await;
    let flows = [
        flow_id_of(&pool, running).await,
        flow_id_of(&pool, paused).await,
    ];
    for f in flows {
        sqlx::query("UPDATE workflow.flows SET deleted_at = now() WHERE id = $1")
            .bind(f)
            .execute(&pool)
            .await
            .unwrap();
    }

    // Act + Assert: the trash.
    let pause = common::post_no_body(&app, &pause_path(ws, running), user).await;
    let resume = common::post_no_body(&app, &resume_path(ws, paused), user).await;
    let validate = common::post_no_body(&app, &validate_path(ws, paused), user).await;
    assert_eq!(pause.status(), 404);
    assert_eq!(resume.status(), 404);
    assert_eq!(validate.status(), 404);
    assert_eq!(exec_state(&pool, running).await, "RUNNING");
    assert_eq!(exec_state(&pool, paused).await, "PAUSED");
    assert!(common::audit_mutations(&app).is_empty());

    // The control: restored, then disabled by its YAML.
    for f in flows {
        sqlx::query(
            "UPDATE workflow.flows SET deleted_at = NULL, source = source || 'disabled: true\n' \
              WHERE id = $1",
        )
        .bind(f)
        .execute(&pool)
        .await
        .unwrap();
    }
    let pause = common::post_no_body(&app, &pause_path(ws, running), user).await;
    assert_eq!(pause.status(), 200);
    let resume = common::post_no_body(&app, &resume_path(ws, running), user).await;
    assert_eq!(resume.status(), 200);
    let resume = common::post(
        &app,
        &resume_path(ws, paused),
        user,
        json!({ "inputs": { "who": "ada", "n": "3" } }),
    )
    .await;
    assert_eq!(resume.status(), 200);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_flow_less_execution_is_404(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = start(&pool, ws, &one_task("gone")).await;
    let paused = waiting(&pool, ws, PLAIN).await;
    sqlx::query("UPDATE workflow.executions SET flow_id = NULL WHERE id = ANY($1)")
        .bind(vec![ex, paused])
        .execute(&pool)
        .await
        .unwrap();

    // Act
    let pause = common::post_no_body(&app, &pause_path(ws, ex), user).await;
    let resume = common::post_no_body(&app, &resume_path(ws, paused), user).await;

    // Assert
    assert_eq!(pause.status(), 404);
    assert_eq!(resume.status(), 404);
    assert_eq!(exec_state(&pool, paused).await, "PAUSED");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_loop_iteration_is_400_naming_its_root(pool: sqlx::PgPool) {
    // Arrange: an iteration row of a `Loop` execution, RUNNING.
    let (_db, ws, user, app) = setup(&pool).await;
    let root = start(&pool, ws, LOOPY).await;
    let iteration = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow.executions (id, workspace_id, flow_id, namespace, flow_key, state, \
         history, inputs, source, parent_execution_id, parent_run_ix, loop_index, loop_value) \
         SELECT $1, workspace_id, flow_id, namespace, flow_key, 'RUNNING', history, inputs, \
                source, id, 0, 99, '1' \
           FROM workflow.executions WHERE id = $2",
    )
    .bind(iteration)
    .bind(root)
    .execute(&pool)
    .await
    .expect("a loop iteration row");
    let history = exec_history(&pool, iteration).await;

    // Act
    let res = common::post_no_body(&app, &pause_path(ws, iteration), user).await;

    // Assert
    let msg = expect_error(res, 400, None).await;
    assert!(msg.contains(&root.to_string()), "{msg}");
    assert_eq!(exec_history(&pool, iteration).await, history);
    assert_eq!(exec_state(&pool, iteration).await, "RUNNING");

    // The control: its root pauses.
    let res = common::post_no_body(&app, &pause_path(ws, root), user).await;
    assert_eq!(res.status(), 200);
}

// ── 13-15: validate ─────────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn validate_with_an_empty_body_answers_the_defaults(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = waiting(&pool, ws, BOOLY).await;
    let hand = start(&pool, ws, &two_tasks("hand")).await;
    assert_eq!(
        common::post_no_body(&app, &pause_path(ws, hand), user)
            .await
            .status(),
        200
    );
    common::audit_mutations(&app);

    // Act
    let res = common::post_no_body(&app, &validate_path(ws, ex), user).await;
    let by_hand = common::post_no_body(&app, &validate_path(ws, hand), user).await;

    // Assert
    assert_eq!(res.status(), 200);
    let body = common::json(res).await;
    let inputs = body["inputs"].as_array().expect("inputs");
    assert_eq!(inputs.len(), 1, "{body}");
    assert_eq!(inputs[0]["input"]["id"], "approved", "{body}");
    assert_eq!(inputs[0]["value"], true, "{body}");
    assert_eq!(inputs[0]["isDefault"], true, "{body}");
    assert_eq!(inputs[0]["errors"], json!([]), "{body}");
    assert_eq!(by_hand.status(), 200);
    let body = common::json(by_hand).await;
    assert_eq!(body["inputs"], json!([]), "{body}");
    assert_eq!(exec_state(&pool, ex).await, "PAUSED");
    assert_eq!(exec_state(&pool, hand).await, "PAUSED");
    assert!(common::audit_mutations(&app).is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn validate_with_values_answers_per_input_errors_and_stores_nothing(pool: sqlx::PgPool) {
    // Arrange
    let fs = egret_workflow::storage::install_mem_for_tests();
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = waiting(&pool, ws, APPROVE).await;
    let run = run_id_of(&pool, ex, "p").await;

    // Act
    let json_res = common::post(
        &app,
        &validate_path(ws, ex),
        user,
        json!({ "inputs": { "n": "x" } }),
    )
    .await;
    let multipart_res = post_multipart(
        &app,
        &validate_path(ws, ex),
        user,
        &[
            Part::Text("who", "ada"),
            Part::Text("n", "3"),
            Part::File("doc", "a.txt", b"hello"),
        ],
    )
    .await;

    // Assert
    assert_eq!(json_res.status(), 200);
    let body = common::json(json_res).await;
    let n = body["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["input"]["id"] == "n")
        .expect("n listed")
        .clone();
    let errors = n["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{body}");
    assert!(
        errors[0]["message"].as_str().unwrap().contains("`n`"),
        "{body}"
    );
    assert_eq!(multipart_res.status(), 200);
    let body = common::json(multipart_res).await;
    let doc = body["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["input"]["id"] == "doc")
        .expect("doc listed")
        .clone();
    assert_eq!(doc["value"], "a.txt", "{body}");
    assert!(
        body["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["errors"] == json!([])),
        "{body}"
    );
    let listed = listed_under(&fs, &fs.resume_root(ws, "t", "approve", ex, "p", run)).await;
    assert!(listed.objects.is_empty(), "{listed:?}");
    assert_eq!(indexed_files(&pool, ex).await, 0);
    assert_eq!(exec_state(&pool, ex).await, "PAUSED");
    assert_eq!(outputs_of(&pool, ex, "p").await, None);
    assert!(common::audit_mutations(&app).is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_workflows_entitlement_gates_the_three_routes(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user, app) = setup(&pool).await;
    let paused = waiting(&pool, ws, APPROVE).await;
    let running = start(&pool, ws, &one_task("one")).await;
    common::disable_feature(&db, ws, "workflows").await;

    // Act
    let pause = common::post_no_body(&app, &pause_path(ws, running), user).await;
    let resume = common::post_no_body(&app, &resume_path(ws, paused), user).await;
    let validate = common::post_no_body(&app, &validate_path(ws, paused), user).await;

    // Assert
    assert_eq!(pause.status(), 403);
    assert_eq!(resume.status(), 403);
    assert_eq!(validate.status(), 403);
    assert_eq!(exec_state(&pool, running).await, "RUNNING");
    assert_eq!(exec_state(&pool, paused).await, "PAUSED");
}

#[test]
fn database_url_is_exported_rather_than_read_from_dotenv() {
    egret_test_db::assert_database_url_is_exported();
}
