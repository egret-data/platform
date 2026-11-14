//! The replay route and the restart route's source choice (workflow package
//! 7b, spec `docs/en/specs/2026-11-14-workflow-replay-from-task-design.md`
//! D5, D8, D9, D11, D12, D13, Q3):
//!
//! ```text
//! POST /api/ws/{workspace}/workflows/executions/{id}/replay?taskRunId=&source=&revision=
//! POST /api/ws/{workspace}/workflows/executions/{id}/restart?source=&revision=
//! ```
//!
//! These tests check the HTTP shape: status codes, `ErrorBody.code`s, the
//! permission and entitlement gates, the audit records and Kestra's
//! `revision` alias. What a copy contains is `tests/replay.rs`'s (worker
//! level). Every refusal here is paired with an accepted control call.
//!
//! Every test is `#[sqlx::test]`: the worker's `claim` and `tick` are global,
//! so each test owns its database.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5465/egret_test
//!   cargo test -p egret-workflow --test replay_routes

use std::collections::HashMap;
use std::sync::Mutex;

use egret_metastore::Db;
use egret_workflow::worker::{Outcome, Registry, TaskInput};
use serde_json::{json, Value};
use uuid::Uuid;

mod common;

const LOG: &str = "io.kestra.plugin.core.log.Log";

#[test]
fn database_url_is_exported_rather_than_read_from_dotenv() {
    egret_test_db::assert_database_url_is_exported();
}

// ── Fixtures (from `tests/restart_routes.rs`) ───────────────────────────────

/// Fails each named task that many times, then succeeds.
struct Flaky {
    fails: Mutex<HashMap<String, usize>>,
}

impl Flaky {
    fn new(fails: &[(&str, usize)]) -> Self {
        Self {
            fails: Mutex::new(fails.iter().map(|(t, n)| (t.to_string(), *n)).collect()),
        }
    }
}

#[async_trait::async_trait]
impl Registry for Flaky {
    async fn run(&self, task: &TaskInput) -> egret_core::Result<Outcome> {
        let fail = {
            let mut fails = self.fails.lock().unwrap();
            match fails.get_mut(&task.task_id) {
                Some(n) if *n > 0 => {
                    *n -= 1;
                    true
                }
                _ => false,
            }
        };
        if fail {
            Ok(Outcome::failed_with(format!("{} fails", task.task_id)))
        } else {
            Ok(Outcome::succeeded_with(json!({ "value": task.task_id })))
        }
    }
    async fn cancel(&self, _task: &TaskInput) -> egret_core::Result<()> {
        Ok(())
    }
}

fn flow(id: &str, ids: &[&str]) -> String {
    let tasks: String = ids
        .iter()
        .map(|t| format!("  - id: {t}\n    type: {LOG}\n    message: {t}\n"))
        .collect();
    format!("id: {id}\nnamespace: t\ntasks:\n{tasks}")
}

async fn setup(pool: &sqlx::PgPool) -> (Db, Uuid, egret_core::UserId, common::App) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage", "workflow:read"]).await;
    let app = common::app(&db).await;
    (db, ws, user, app)
}

/// Start `source` as an execution of a real, open flow the workspace's
/// creator owns.
async fn start(pool: &sqlx::PgPool, ws: Uuid, source: &str) -> Uuid {
    start_with(pool, ws, source, json!({})).await
}

async fn start_with(pool: &sqlx::PgPool, ws: Uuid, source: &str, inputs: Value) -> Uuid {
    let flow = flow_of(pool, ws, source).await;
    egret_workflow::worker::start_execution(pool, ws, Some(flow), source, inputs)
        .await
        .expect("start execution")
}

/// The flow row `source` is a draft of, created on first use (the workspace's
/// creator owns it).
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

async fn exec_state(pool: &sqlx::PgPool, id: Uuid) -> String {
    sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("execution state")
}

async fn drive(pool: &sqlx::PgPool, reg: &dyn Registry, ex: Uuid) {
    for _ in 0..60 {
        let done = egret_workflow::worker::tick(pool, 64, reg)
            .await
            .expect("a worker tick");
        let state = exec_state(pool, ex).await;
        let terminal = matches!(
            state.as_str(),
            "SUCCESS" | "FAILED" | "WARNING" | "KILLED" | "CANCELLED"
        );
        if terminal && done == 0 {
            return;
        }
    }
    panic!("execution {ex} did not come to rest in 60 ticks");
}

async fn task_run_id(pool: &sqlx::PgPool, ex: Uuid, task: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2")
        .bind(ex)
        .bind(task)
        .fetch_one(pool)
        .await
        .expect("task run id")
}

/// `a` ok, `b` fails once, `c` after it: ends FAILED.
async fn failed_execution(pool: &sqlx::PgPool, ws: Uuid) -> Uuid {
    let reg = Flaky::new(&[("b", 1)]);
    let ex = start(pool, ws, &flow("ops", &["a", "b", "c"])).await;
    drive(pool, &reg, ex).await;
    assert_eq!(exec_state(pool, ex).await, "FAILED");
    ex
}

fn replay_path(ws: Uuid, id: Uuid, query: &str) -> String {
    format!("/api/ws/{ws}/workflows/executions/{id}/replay{query}")
}

fn restart_path(ws: Uuid, id: Uuid, query: &str) -> String {
    format!("/api/ws/{ws}/workflows/executions/{id}/restart{query}")
}

async fn expect_error(res: axum::response::Response, status: u16, code: Option<&str>) -> String {
    assert_eq!(res.status(), status);
    let body = common::json(res).await;
    assert_eq!(body["code"].as_str(), code, "{body}");
    body["error"].as_str().unwrap_or_default().to_string()
}

/// A 201's new execution id.
async fn created(res: axum::response::Response) -> (Uuid, Value) {
    assert_eq!(res.status(), 201);
    let body = common::json(res).await;
    let id: Uuid = body["id"].as_str().expect("an id").parse().unwrap();
    (id, body)
}

async fn execution_count(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM workflow.executions")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn source_of(pool: &sqlx::PgPool, ex: Uuid) -> String {
    sqlx::query_scalar("SELECT source FROM workflow.executions WHERE id = $1")
        .bind(ex)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn flow_id_of(pool: &sqlx::PgPool, ex: Uuid) -> Uuid {
    sqlx::query_scalar("SELECT flow_id FROM workflow.executions WHERE id = $1")
        .bind(ex)
        .fetch_one(pool)
        .await
        .expect("flow id")
}

async fn set_draft(pool: &sqlx::PgPool, flow: Uuid, source: &str) {
    sqlx::query("UPDATE workflow.flows SET source = $2 WHERE id = $1")
        .bind(flow)
        .bind(source)
        .execute(pool)
        .await
        .expect("edit the draft");
}

/// A person in `ws` with `perms`, who did not create its flows.
async fn other_user(db: &Db, ws: Uuid, perms: &[&str]) -> egret_core::UserId {
    let uid: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, name, provider, status, password_hash) \
         VALUES ($1, 't', 'local', 'active', 'x') RETURNING id",
    )
    .bind(format!("u_{}@t.local", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("seed user");
    let user = egret_core::UserId(uid);
    common::grant(db, ws, user, perms).await;
    user
}

// ── 1. Replay from a task ───────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn replay_from_a_task_answers_201_with_the_new_execution(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = failed_execution(&pool, ws).await;
    let b = task_run_id(&pool, ex, "b").await;

    // Act
    let res = common::post(
        &app,
        &replay_path(ws, ex, &format!("?taskRunId={b}")),
        user,
        json!({}),
    )
    .await;

    // Assert
    let (new, body) = created(res).await;
    assert_ne!(new, ex);
    assert_eq!(body["replayed_from_execution_id"], json!(ex));
    let a_old = task_run_id(&pool, ex, "a").await;
    let a_new = task_run_id(&pool, new, "a").await;
    assert_ne!(a_old, a_new, "a copied run has a new id");
    let a_state: String = sqlx::query_scalar("SELECT state FROM workflow.task_runs WHERE id = $1")
        .bind(a_new)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(a_state, "SUCCESS", "`a` is kept, not run again");
    assert_eq!(
        exec_state(&pool, ex).await,
        "FAILED",
        "the old one is untouched"
    );
}

// ── 2. Kestra's `revision` alias ────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn revision_is_version(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = failed_execution(&pool, ws).await;
    let flow_id = flow_id_of(&pool, ex).await;
    let v1 = flow("ops", &["a", "b", "c"]).replace("message: c", "message: c of version 1");
    sqlx::query(
        "INSERT INTO workflow.flow_versions (flow_id, version, snapshot, created_by) \
         VALUES ($1, 1, jsonb_build_object('source', $2::text), $3)",
    )
    .bind(flow_id)
    .bind(&v1)
    .bind(user.0)
    .execute(&pool)
    .await
    .expect("publish version 1");
    let b = task_run_id(&pool, ex, "b").await;

    // Act + Assert: the alias, whole and from a task.
    let res = common::post_no_body(&app, &replay_path(ws, ex, "?revision=1"), user).await;
    let (new, _) = created(res).await;
    assert_eq!(source_of(&pool, new).await, v1);
    let res = common::post_no_body(
        &app,
        &replay_path(ws, ex, &format!("?revision=1&taskRunId={b}")),
        user,
    )
    .await;
    let (new, _) = created(res).await;
    assert_eq!(source_of(&pool, new).await, v1);
    let res = common::post_no_body(&app, &replay_path(ws, ex, "?source=version:1"), user).await;
    let (new, _) = created(res).await;
    assert_eq!(source_of(&pool, new).await, v1);

    // The refusals write nothing.
    let n = execution_count(&pool).await;
    let res = common::post_no_body(
        &app,
        &replay_path(ws, ex, "?revision=1&source=current"),
        user,
    )
    .await;
    let msg = expect_error(res, 400, Some("invalid_source")).await;
    assert!(msg.contains("not both"), "{msg}");
    for q in [
        "?source=version:x",
        "?source=version:0",
        "?source=latest",
        "?revision=abc",
        "?revision=0",
        "?revision=",
    ] {
        let res = common::post_no_body(&app, &replay_path(ws, ex, q), user).await;
        expect_error(res, 400, Some("invalid_source")).await;
    }
    let res = common::post_no_body(&app, &replay_path(ws, ex, "?revision=9"), user).await;
    let msg = expect_error(res, 404, Some("flow_version_not_found")).await;
    assert!(msg.contains('9'), "{msg}");
    assert_eq!(execution_count(&pool).await, n);
    assert_eq!(common::audit_mutations(&app).len(), 3, "the three accepted");
}

// ── 3. `current` ────────────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn source_current_runs_the_draft(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = failed_execution(&pool, ws).await;
    let draft = flow("ops", &["a", "b", "c"]).replace("message: c", "message: c edited");
    set_draft(&pool, flow_id_of(&pool, ex).await, &draft).await;

    // Act
    let res = common::post_no_body(&app, &replay_path(ws, ex, "?source=current"), user).await;
    let (current, _) = created(res).await;
    let res = common::post_no_body(&app, &replay_path(ws, ex, "?source=frozen"), user).await;
    let (frozen, _) = created(res).await;

    // Assert
    assert_eq!(source_of(&pool, current).await, draft);
    assert_eq!(
        source_of(&pool, frozen).await,
        flow("ops", &["a", "b", "c"]),
        "frozen is the replayed execution's own source"
    );
}

// ── 4. Breakpoints (spec Q3) ────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn breakpoints_are_refused(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = failed_execution(&pool, ws).await;
    let n = execution_count(&pool).await;

    // Act
    let res = common::post_no_body(&app, &replay_path(ws, ex, "?breakpoints=a"), user).await;

    // Assert
    let msg = expect_error(res, 400, Some("replay_breakpoints_unsupported")).await;
    assert!(msg.contains("breakpoints"), "{msg}");
    assert_eq!(execution_count(&pool).await, n);
    assert!(common::audit_mutations(&app).is_empty());

    // The control.
    let res = common::post_no_body(&app, &replay_path(ws, ex, ""), user).await;
    created(res).await;
}

// ── 5. A task run that is not this execution's ──────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn a_foreign_or_unknown_task_run_is_404(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user, app) = setup(&pool).await;
    let ex = failed_execution(&pool, ws).await;
    let sibling = failed_execution(&pool, ws).await;
    let (other_ws, _) = common::seed_workspace(&db).await;
    let foreign = failed_execution(&pool, other_ws).await;
    let sibling_b = task_run_id(&pool, sibling, "b").await;
    let foreign_b = task_run_id(&pool, foreign, "b").await;
    let n = execution_count(&pool).await;

    // Act + Assert
    for t in [sibling_b.to_string(), foreign_b.to_string(), "nope".into()] {
        let res =
            common::post_no_body(&app, &replay_path(ws, ex, &format!("?taskRunId={t}")), user)
                .await;
        let msg = expect_error(res, 404, Some("task_run_not_found")).await;
        assert!(msg.contains(&ex.to_string()), "{msg}");
    }
    assert_eq!(execution_count(&pool).await, n);
    assert!(common::audit_mutations(&app).is_empty());

    // The control: this execution's own run.
    let b = task_run_id(&pool, ex, "b").await;
    let res =
        common::post_no_body(&app, &replay_path(ws, ex, &format!("?taskRunId={b}")), user).await;
    created(res).await;
}

// ── 6. A run inside a loop iteration ────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn a_task_run_inside_a_loop_iteration_names_the_loop(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let source = format!(
        "id: lp\nnamespace: t\ntasks:\n  - id: l\n    type: io.kestra.plugin.core.flow.Loop\n\
         \x20   values: '[\"x\", \"y\"]'\n    tasks:\n      - id: item\n        type: {LOG}\n        \
         message: hi\n  - id: after\n    type: {LOG}\n    message: after\n"
    );
    let ex = start(&pool, ws, &source).await;
    drive(&pool, &Flaky::new(&[]), ex).await;
    assert_eq!(exec_state(&pool, ex).await, "SUCCESS");
    let item: Uuid = sqlx::query_scalar(
        "SELECT t.id FROM workflow.task_runs t JOIN workflow.executions e ON e.id = t.execution_id \
          WHERE e.parent_execution_id = $1 AND e.loop_index IS NOT NULL AND t.task_id = 'item' \
          ORDER BY e.loop_index LIMIT 1",
    )
    .bind(ex)
    .fetch_one(&pool)
    .await
    .expect("an iteration's run");
    let l = task_run_id(&pool, ex, "l").await;
    let n = execution_count(&pool).await;

    // Act
    let res = common::post_no_body(
        &app,
        &replay_path(ws, ex, &format!("?taskRunId={item}")),
        user,
    )
    .await;

    // Assert
    let msg = expect_error(res, 400, None).await;
    assert!(msg.contains("`l`"), "{msg}");
    assert!(msg.contains(&ex.to_string()), "{msg}");
    assert!(msg.contains(&l.to_string()), "{msg}");
    assert_eq!(execution_count(&pool).await, n);

    // The control: the Loop's own run.
    let res =
        common::post_no_body(&app, &replay_path(ws, ex, &format!("?taskRunId={l}")), user).await;
    created(res).await;
}

// ── 7. A task that is not in the chosen source ──────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn a_task_missing_from_the_source_is_400(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = failed_execution(&pool, ws).await;
    set_draft(
        &pool,
        flow_id_of(&pool, ex).await,
        &flow("ops", &["a", "c"]),
    )
    .await;
    let b = task_run_id(&pool, ex, "b").await;
    let n = execution_count(&pool).await;

    // Act
    let res = common::post_no_body(
        &app,
        &replay_path(ws, ex, &format!("?source=current&taskRunId={b}")),
        user,
    )
    .await;

    // Assert
    let msg = expect_error(res, 400, Some("replay_task_not_in_source")).await;
    assert!(msg.contains("`b`"), "{msg}");
    assert!(msg.contains("not in the chosen source"), "{msg}");
    assert_eq!(execution_count(&pool).await, n);

    // A restart on that source is refused alike.
    let res = common::post_no_body(&app, &restart_path(ws, ex, "?source=current"), user).await;
    expect_error(res, 400, Some("replay_task_not_in_source")).await;
    assert_eq!(execution_count(&pool).await, n);
    assert!(common::audit_mutations(&app).is_empty());

    // The control: the frozen source has `b`.
    let res = common::post_no_body(
        &app,
        &replay_path(ws, ex, &format!("?source=frozen&taskRunId={b}")),
        user,
    )
    .await;
    created(res).await;
}

// ── 8. Inside a WorkingDirectory ────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn inside_working_directory_is_409(pool: sqlx::PgPool) {
    // Arrange
    egret_workflow::storage::install_mem_for_tests();
    let (_db, ws, user, app) = setup(&pool).await;
    let source = format!(
        "id: wd\nnamespace: t\ntasks:\n  - id: block\n    type: io.kestra.plugin.core.flow.WorkingDirectory\n\
         \x20   tasks:\n      - id: a\n        type: {LOG}\n        message: a\n\
         \x20     - id: b\n        type: {LOG}\n        message: b\n"
    );
    let ex = start(&pool, ws, &source).await;
    drive(&pool, &Flaky::new(&[("b", 1)]), ex).await;
    assert_eq!(exec_state(&pool, ex).await, "FAILED");
    let b = task_run_id(&pool, ex, "b").await;
    let n = execution_count(&pool).await;

    // Act
    let res =
        common::post_no_body(&app, &replay_path(ws, ex, &format!("?taskRunId={b}")), user).await;

    // Assert
    let msg = expect_error(res, 409, Some("inside_working_directory")).await;
    assert!(msg.contains("WorkingDirectory"), "{msg}");
    assert_eq!(execution_count(&pool).await, n);

    // The control: the block's own run.
    let block = task_run_id(&pool, ex, "block").await;
    let res = common::post_no_body(
        &app,
        &replay_path(ws, ex, &format!("?taskRunId={block}")),
        user,
    )
    .await;
    created(res).await;
}

// ── 9. New inputs ───────────────────────────────────────────────────────────

const BOUNDARY: &str = "pkg7b-b1-boundary-3f9a";

fn text_part(name: &str, value: &str) -> Vec<u8> {
    format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n\
         --{BOUNDARY}--\r\n"
    )
    .into_bytes()
}

async fn inputs_of(pool: &sqlx::PgPool, ex: Uuid) -> Value {
    sqlx::query_scalar("SELECT inputs FROM workflow.executions WHERE id = $1")
        .bind(ex)
        .fetch_one(pool)
        .await
        .expect("inputs")
}

#[sqlx::test(migrations = "../../migrations")]
async fn new_inputs_by_json_and_by_multipart(pool: sqlx::PgPool) {
    // Arrange
    egret_workflow::storage::install_mem_for_tests();
    let (_db, ws, user, app) = setup(&pool).await;
    let source = format!(
        "id: inp\nnamespace: t\ninputs:\n  - id: n\n    type: INT\n    min: 1\n\
         tasks:\n  - id: a\n    type: {LOG}\n    message: a\n"
    );
    let ex = start_with(&pool, ws, &source, json!({ "n": 1 })).await;
    drive(&pool, &Flaky::new(&[]), ex).await;
    let a = task_run_id(&pool, ex, "a").await;

    // JSON, whole and from a task.
    let res = common::post(
        &app,
        &replay_path(ws, ex, ""),
        user,
        json!({"inputs": {"n": 5}}),
    )
    .await;
    let (new, _) = created(res).await;
    assert_eq!(inputs_of(&pool, new).await, json!({ "n": 5 }));
    let res = common::post(
        &app,
        &replay_path(ws, ex, &format!("?taskRunId={a}")),
        user,
        json!({"inputs": {"n": 6}}),
    )
    .await;
    let (new, _) = created(res).await;
    assert_eq!(inputs_of(&pool, new).await, json!({ "n": 6 }));

    // No body, and `inputs: null`, reuse the execution's own.
    let res = common::post_no_body(&app, &replay_path(ws, ex, ""), user).await;
    let (new, _) = created(res).await;
    assert_eq!(inputs_of(&pool, new).await, json!({ "n": 1 }));
    let res = common::post(
        &app,
        &replay_path(ws, ex, ""),
        user,
        json!({"inputs": null}),
    )
    .await;
    let (new, _) = created(res).await;
    assert_eq!(inputs_of(&pool, new).await, json!({ "n": 1 }));

    // Refusals write nothing.
    let n = execution_count(&pool).await;
    let res = common::post(
        &app,
        &replay_path(ws, ex, ""),
        user,
        json!({"inputs": {"n": 0}}),
    )
    .await;
    let msg = expect_error(res, 400, None).await;
    assert!(msg.contains("`n`"), "{msg}");
    let res = common::post(&app, &replay_path(ws, ex, ""), user, json!({"inputs": [1]})).await;
    let msg = expect_error(res, 400, None).await;
    assert!(msg.contains("JSON object"), "{msg}");
    assert_eq!(execution_count(&pool).await, n);

    // Multipart: a text part.
    let res = common::send_raw(
        &app,
        "POST",
        &replay_path(ws, ex, &format!("?taskRunId={a}")),
        user,
        &format!("multipart/form-data; boundary={BOUNDARY}"),
        text_part("n", "7"),
    )
    .await;
    let (new, _) = created(res).await;
    assert_eq!(inputs_of(&pool, new).await, json!({ "n": 7 }));
    let res = common::send_raw(
        &app,
        "POST",
        &replay_path(ws, ex, ""),
        user,
        &format!("multipart/form-data; boundary={BOUNDARY}"),
        text_part("nope", "7"),
    )
    .await;
    let msg = expect_error(res, 400, None).await;
    assert!(msg.contains("`nope`"), "{msg}");
}

// ── 10. Permission ──────────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn workflow_read_is_refused(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, manager, _app) = setup(&pool).await;
    let reader = other_user(&db, ws, &["workflow:read"]).await;
    let app = common::app(&db).await;
    let ex = failed_execution(&pool, ws).await;
    let b = task_run_id(&pool, ex, "b").await;
    let n = execution_count(&pool).await;

    // Act + Assert
    for path in [
        replay_path(ws, ex, &format!("?taskRunId={b}")),
        replay_path(ws, ex, ""),
        restart_path(ws, ex, "?source=current"),
    ] {
        let res = common::post_no_body(&app, &path, reader).await;
        assert_eq!(res.status(), 403, "{path}");
    }
    assert_eq!(execution_count(&pool).await, n);
    assert!(common::audit_mutations(&app).is_empty());

    // The control: a manager.
    let res = common::post_no_body(
        &app,
        &replay_path(ws, ex, &format!("?taskRunId={b}")),
        manager,
    )
    .await;
    created(res).await;
    let res = common::post_no_body(&app, &restart_path(ws, ex, "?source=current"), manager).await;
    created(res).await;
}

// ── 11. The replay's audit record ───────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn the_audit_names_the_task_run_the_source_and_whether_inputs_changed(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = failed_execution(&pool, ws).await;
    let b = task_run_id(&pool, ex, "b").await;

    // Act
    let res = common::post(
        &app,
        &replay_path(ws, ex, &format!("?taskRunId={b}&source=current")),
        user,
        json!({ "inputs": {} }),
    )
    .await;
    let (from_task, _) = created(res).await;
    let res = common::post_no_body(&app, &replay_path(ws, ex, ""), user).await;
    let (whole, _) = created(res).await;

    // Assert
    let audit = common::audit_mutations(&app);
    assert_eq!(
        audit,
        vec![
            (
                "workflow_execution:replay".to_string(),
                Some(from_task.to_string()),
                json!({
                    "replayed_from": ex,
                    "replacement": from_task,
                    "task_run_id": b,
                    "source": "current",
                    "inputs_replaced": true,
                }),
            ),
            (
                "workflow_execution:replay".to_string(),
                Some(whole.to_string()),
                json!({
                    "replayed_from": ex,
                    "replacement": whole,
                    "task_run_id": null,
                    "source": "frozen",
                    "inputs_replaced": false,
                }),
            ),
        ]
    );
    assert!(audit.iter().all(|(_, _, d)| d.get("inputs").is_none()));
}

// ── 12. Restart on a source ─────────────────────────────────────────────────

fn has_label(body: &Value, key: &str) -> bool {
    body["labels"]
        .as_array()
        .is_some_and(|ls| ls.iter().any(|l| l["key"] == key && l["value"] == "true"))
}

#[sqlx::test(migrations = "../../migrations")]
async fn restart_on_a_source_answers_201_and_is_audited(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = failed_execution(&pool, ws).await;
    // Driven before anything else runs: a tick drives every execution.
    let other = failed_execution(&pool, ws).await;
    let draft = flow("ops", &["a", "b", "c"]).replace("message: c", "message: c edited");
    set_draft(&pool, flow_id_of(&pool, ex).await, &draft).await;

    // Act
    let res = common::post_no_body(&app, &restart_path(ws, ex, "?source=current"), user).await;

    // Assert
    let (new, body) = created(res).await;
    assert_ne!(new, ex);
    assert!(has_label(&body, "system.restarted"), "{body}");
    assert_eq!(body["attempt_number"], json!(2), "{body}");
    assert_eq!(body["replayed_from_execution_id"], json!(ex), "{body}");
    assert_eq!(source_of(&pool, new).await, draft);
    assert_eq!(
        exec_state(&pool, ex).await,
        "FAILED",
        "the old one is untouched"
    );
    assert_eq!(
        common::audit_mutations(&app),
        vec![(
            "workflow_execution:restart".to_string(),
            Some(new.to_string()),
            json!({ "from_state": "FAILED", "replacement": new, "source": "current" }),
        )]
    );

    // `?source=frozen` restarts in place, 200.
    let res = common::post_no_body(&app, &restart_path(ws, ex, "?source=frozen"), user).await;
    assert_eq!(res.status(), 200);
    assert_eq!(common::json(res).await["id"], json!(ex));
    // No query too, on another FAILED execution.
    let res = common::post_no_body(&app, &restart_path(ws, other, ""), user).await;
    assert_eq!(res.status(), 200);
    assert_eq!(common::json(res).await["id"], json!(other));
}

#[sqlx::test(migrations = "../../migrations")]
async fn restart_on_a_source_maps_the_worker_refusals(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let ok = start(&pool, ws, &flow("ops", &["a"])).await;
    drive(&pool, &Flaky::new(&[]), ok).await;
    let ex = failed_execution(&pool, ws).await;
    let n = execution_count(&pool).await;

    // Act + Assert
    let res = common::post_no_body(&app, &restart_path(ws, ok, "?source=current"), user).await;
    let msg = expect_error(res, 409, Some("not_restartable")).await;
    assert!(msg.contains("SUCCESS"), "{msg}");
    let res = common::post_no_body(&app, &restart_path(ws, ex, "?revision=1"), user).await;
    expect_error(res, 404, Some("flow_version_not_found")).await;
    sqlx::query("UPDATE workflow.executions SET files_pruned_at = now() WHERE id = $1")
        .bind(ex)
        .execute(&pool)
        .await
        .unwrap();
    let res = common::post_no_body(&app, &restart_path(ws, ex, "?source=current"), user).await;
    expect_error(res, 409, Some("files_pruned")).await;
    let b = task_run_id(&pool, ex, "b").await;
    let res =
        common::post_no_body(&app, &replay_path(ws, ex, &format!("?taskRunId={b}")), user).await;
    expect_error(res, 409, Some("files_pruned")).await;
    assert_eq!(execution_count(&pool).await, n);
    assert!(common::audit_mutations(&app).is_empty());

    // The control: a whole replay carries no file of the old runs (S7).
    let res = common::post_no_body(&app, &replay_path(ws, ex, ""), user).await;
    created(res).await;
}

// ── 13. The `egret.sql` gate, the trash and the disabled flow ───────────────

#[sqlx::test(migrations = "../../migrations")]
async fn the_egret_sql_gate_applies_to_the_chosen_source(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, creator, _app) = setup(&pool).await;
    let ex = failed_execution(&pool, ws).await;
    let sql_draft = format!(
        "id: ops\nnamespace: t\ntasks:\n  - id: a\n    type: {LOG}\n    message: a\n\
         \x20 - id: b\n    type: {LOG}\n    message: b\n\
         \x20 - id: c\n    type: egret.sql\n    database: trino_prod\n    sql: SELECT 1\n"
    );
    set_draft(&pool, flow_id_of(&pool, ex).await, &sql_draft).await;
    let other = other_user(&db, ws, &["workflow:manage", "workflow:read"]).await;
    let app = common::app(&db).await;
    let b = task_run_id(&pool, ex, "b").await;
    let n = execution_count(&pool).await;

    // Act + Assert: the caller, judged against the source that would run.
    for path in [
        replay_path(ws, ex, "?source=current"),
        replay_path(ws, ex, &format!("?source=current&taskRunId={b}")),
        restart_path(ws, ex, "?source=current"),
    ] {
        let res = common::post_no_body(&app, &path, other).await;
        let msg = expect_error(res, 400, None).await;
        assert!(
            msg.contains("only its creator can start it"),
            "{path}: {msg}"
        );
    }
    assert_eq!(execution_count(&pool).await, n);
    assert!(common::audit_mutations(&app).is_empty());

    // The controls: the frozen source runs no SQL, so the same person passes;
    // and the creator passes on the SQL source.
    let res = common::post_no_body(&app, &replay_path(ws, ex, "?source=frozen"), other).await;
    created(res).await;
    let res = common::post_no_body(
        &app,
        &replay_path(ws, ex, &format!("?source=frozen&taskRunId={b}")),
        other,
    )
    .await;
    created(res).await;
    let res = common::post_no_body(
        &app,
        &replay_path(ws, ex, &format!("?source=current&taskRunId={b}")),
        creator,
    )
    .await;
    created(res).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_trashed_flow_refuses_a_replay_and_a_restart_on_a_source(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = failed_execution(&pool, ws).await;
    let b = task_run_id(&pool, ex, "b").await;
    let from_b = format!("?source=current&taskRunId={b}");
    // The control, before the trash.
    created(common::post_no_body(&app, &replay_path(ws, ex, &from_b), user).await).await;
    common::audit_mutations(&app);
    sqlx::query("UPDATE workflow.flows SET deleted_at = now() WHERE id = $1")
        .bind(flow_id_of(&pool, ex).await)
        .execute(&pool)
        .await
        .unwrap();
    let n = execution_count(&pool).await;

    // Act + Assert
    for path in [
        replay_path(ws, ex, &from_b),
        restart_path(ws, ex, "?source=current"),
    ] {
        let res = common::post_no_body(&app, &path, user).await;
        assert_eq!(res.status(), 404, "{path}");
    }
    assert_eq!(execution_count(&pool).await, n);
    assert!(common::audit_mutations(&app).is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_disabled_flow_refuses_a_replay_and_a_restart_on_a_source(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = failed_execution(&pool, ws).await;
    let b = task_run_id(&pool, ex, "b").await;
    let from_b = format!("?source=current&taskRunId={b}");
    // The control, before the switch.
    created(common::post_no_body(&app, &replay_path(ws, ex, &from_b), user).await).await;
    common::audit_mutations(&app);
    sqlx::query("UPDATE workflow.flows SET state = 'disabled' WHERE id = $1")
        .bind(flow_id_of(&pool, ex).await)
        .execute(&pool)
        .await
        .unwrap();
    let n = execution_count(&pool).await;

    // Act + Assert
    for path in [
        replay_path(ws, ex, &from_b),
        restart_path(ws, ex, "?source=current"),
    ] {
        let res = common::post_no_body(&app, &path, user).await;
        let msg = expect_error(res, 409, None).await;
        assert!(msg.contains("disabled"), "{path}: {msg}");
    }
    assert_eq!(execution_count(&pool).await, n);
    assert!(common::audit_mutations(&app).is_empty());
}

// ── 14. The entitlement ─────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn the_entitlement_gate_covers_replay(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user, app) = setup(&pool).await;
    let ex = failed_execution(&pool, ws).await;
    let b = task_run_id(&pool, ex, "b").await;
    let from_b = replay_path(ws, ex, &format!("?taskRunId={b}"));
    // The control, before the switch.
    created(common::post_no_body(&app, &from_b, user).await).await;
    common::disable_feature(&db, ws, "workflows").await;
    let n = execution_count(&pool).await;

    // Act + Assert
    for path in [from_b, restart_path(ws, ex, "?source=current")] {
        let res = common::post_no_body(&app, &path, user).await;
        assert_eq!(res.status(), 403, "{path}");
    }
    assert_eq!(execution_count(&pool).await, n);
}
