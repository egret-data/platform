//! The three execution-operation routes (workflow package 7a, spec
//! `docs/en/specs/2026-11-13-workflow-execution-ops-restart-design.md` D10,
//! D11, D13):
//!
//! ```text
//! POST /api/ws/{workspace}/workflows/executions/{id}/restart
//! POST /api/ws/{workspace}/workflows/executions/{id}/task-runs/{task_run_id}/state
//! POST /api/ws/{workspace}/workflows/executions/{id}/status
//! ```
//!
//! Every test is `#[sqlx::test]`: the worker's `claim` and `tick` are global,
//! so each test owns its database.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5465/egret_test
//!   cargo test -p egret-workflow --test restart_routes

use std::collections::HashMap;
use std::sync::Mutex;

use egret_metastore::Db;
use egret_workflow::worker::{Outcome, Registry, TaskInput};
use serde_json::{json, Value};
use uuid::Uuid;

mod common;

const LOG: &str = "io.kestra.plugin.core.log.Log";

// ── Fixtures ────────────────────────────────────────────────────────────────

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
/// creator owns: restart and a task run's state change answer to the flow's
/// current state as replay does (final review I1), and refuse an execution
/// whose flow is gone.
async fn start(pool: &sqlx::PgPool, ws: Uuid, source: &str) -> Uuid {
    let flow = flow_of(pool, ws, source).await;
    egret_workflow::worker::start_execution(pool, ws, Some(flow), source, json!({}))
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

/// `a` ok, `b` fails once, `c` after it: ends FAILED with one run to redo.
async fn failed_execution(pool: &sqlx::PgPool, ws: Uuid) -> (Uuid, Flaky) {
    let reg = Flaky::new(&[("b", 1)]);
    let ex = start(pool, ws, &flow("ops", &["a", "b", "c"])).await;
    drive(pool, &reg, ex).await;
    assert_eq!(exec_state(pool, ex).await, "FAILED");
    (ex, reg)
}

fn restart_path(ws: Uuid, id: Uuid) -> String {
    format!("/api/ws/{ws}/workflows/executions/{id}/restart")
}
fn run_state_path(ws: Uuid, id: Uuid, run: Uuid) -> String {
    format!("/api/ws/{ws}/workflows/executions/{id}/task-runs/{run}/state")
}
fn status_path(ws: Uuid, id: Uuid) -> String {
    format!("/api/ws/{ws}/workflows/executions/{id}/status")
}

async fn expect_error(res: axum::response::Response, status: u16, code: Option<&str>) -> String {
    assert_eq!(res.status(), status);
    let body = common::json(res).await;
    assert_eq!(body["code"].as_str(), code, "{body}");
    body["error"].as_str().unwrap_or_default().to_string()
}

// ── Restart ─────────────────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn restart_answers_200_with_the_execution_and_is_audited(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let (ex, _reg) = failed_execution(&pool, ws).await;

    let res = common::post_no_body(&app, &restart_path(ws, ex), user).await;

    assert_eq!(res.status(), 200);
    let body = common::json(res).await;
    assert_eq!(body["id"], json!(ex));
    assert_ne!(body["state"], "FAILED", "{body}");
    assert_eq!(body["can_manage"], true);
    assert_eq!(
        common::audit_mutations(&app),
        vec![(
            "workflow_execution:restart".to_string(),
            Some(ex.to_string()),
            json!({ "from_state": "FAILED" }),
        )]
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn restart_needs_workflow_manage(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read"]).await;
    let app = common::app(&db).await;
    let (ex, _reg) = failed_execution(&pool, ws).await;

    let res = common::post_no_body(&app, &restart_path(ws, ex), user).await;

    assert_eq!(res.status(), 403);
    assert_eq!(exec_state(&pool, ex).await, "FAILED");
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_workflows_entitlement_gates_the_restart_route(pool: sqlx::PgPool) {
    let (db, ws, user, app) = setup(&pool).await;
    let (ex, _reg) = failed_execution(&pool, ws).await;
    common::disable_feature(&db, ws, "workflows").await;

    let res = common::post_no_body(&app, &restart_path(ws, ex), user).await;

    assert_eq!(res.status(), 403);
    assert_eq!(exec_state(&pool, ex).await, "FAILED");
}

#[sqlx::test(migrations = "../../migrations")]
async fn restart_of_a_foreign_execution_is_404(pool: sqlx::PgPool) {
    let (db, _ws, user, app) = setup(&pool).await;
    let (other_ws, _other_user) = common::seed_workspace(&db).await;
    let (ex, _reg) = failed_execution(&pool, other_ws).await;
    let (own_ws, _) = (_ws, ());

    let res = common::post_no_body(&app, &restart_path(own_ws, ex), user).await;

    assert_eq!(res.status(), 404);
    assert_eq!(exec_state(&pool, ex).await, "FAILED");
}

#[sqlx::test(migrations = "../../migrations")]
async fn restart_with_a_revision_is_400_naming_package_7b(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let (ex, _reg) = failed_execution(&pool, ws).await;

    for q in ["?revision=1", "?revision=", "?revision", "?revision=abc"] {
        let res = common::post_no_body(&app, &format!("{}{q}", restart_path(ws, ex)), user).await;
        let msg = expect_error(res, 400, Some("restart_revision_unsupported")).await;
        assert!(msg.contains("7b"), "{msg}");
    }
    assert_eq!(exec_state(&pool, ex).await, "FAILED");
}

#[sqlx::test(migrations = "../../migrations")]
async fn restart_of_a_success_is_409_not_restartable(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let reg = Flaky::new(&[]);
    let ex = start(&pool, ws, &flow("ok", &["a"])).await;
    drive(&pool, &reg, ex).await;

    let res = common::post_no_body(&app, &restart_path(ws, ex), user).await;

    let msg = expect_error(res, 409, Some("not_restartable")).await;
    assert!(msg.contains("SUCCESS"), "{msg}");
    assert!(common::audit_mutations(&app).is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn restart_with_nothing_failed_is_409_nothing_to_restart(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let (ex, _reg) = failed_execution(&pool, ws).await;
    // A FAILED execution none of whose runs is FAILED or PAUSED.
    sqlx::query(
        "UPDATE workflow.task_runs SET state = 'SUCCESS', \
         history = history || '[\"SUCCESS\"]'::jsonb WHERE execution_id = $1 AND state = 'FAILED'",
    )
    .bind(ex)
    .execute(&pool)
    .await
    .unwrap();

    let res = common::post_no_body(&app, &restart_path(ws, ex), user).await;

    expect_error(res, 409, Some("nothing_to_restart")).await;
    assert!(common::audit_mutations(&app).is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn restart_after_the_files_were_pruned_is_409(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let (ex, _reg) = failed_execution(&pool, ws).await;
    sqlx::query("UPDATE workflow.executions SET files_pruned_at = now() WHERE id = $1")
        .bind(ex)
        .execute(&pool)
        .await
        .unwrap();

    let res = common::post_no_body(&app, &restart_path(ws, ex), user).await;

    expect_error(res, 409, Some("files_pruned")).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn restart_while_after_execution_runs_is_409(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let source = format!(
        "id: live-after\nnamespace: t\ntasks:\n  - id: a\n    type: {LOG}\n    message: a\n\
         afterExecution:\n  - id: end\n    type: {LOG}\n    message: end\n"
    );
    let ex = start(&pool, ws, &source).await;
    let claimed = egret_workflow::worker::claim(&pool, 10).await.unwrap();
    assert_eq!(claimed.len(), 1);
    egret_workflow::worker::commit_outcome(&pool, &claimed[0], &Outcome::failed())
        .await
        .unwrap();
    assert_eq!(exec_state(&pool, ex).await, "FAILED");

    let res = common::post_no_body(&app, &restart_path(ws, ex), user).await;

    expect_error(res, 409, Some("after_execution_running")).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn restart_of_a_loop_iteration_is_400_naming_the_root(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let (ex, _reg) = failed_execution(&pool, ws).await;
    let root = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow.executions (id, workspace_id, namespace, flow_key, state, history, \
         inputs, source, parent_execution_id, parent_run_ix, loop_index, loop_value) \
         SELECT $1, workspace_id, namespace, flow_key, 'FAILED', history, inputs, source, id, 0, 0, '1' \
           FROM workflow.executions WHERE id = $2",
    )
    .bind(root)
    .bind(ex)
    .execute(&pool)
    .await
    .expect("a loop iteration row");

    let res = common::post_no_body(&app, &restart_path(ws, root), user).await;

    assert_eq!(res.status(), 400);
    let body = common::json(res).await;
    assert!(
        body["error"].as_str().unwrap().contains(&ex.to_string()),
        "{body}"
    );
    assert!(
        body.get("code").is_none() || body["code"].is_null(),
        "{body}"
    );
}

// ── Task-run state ──────────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn marking_a_failed_run_success_answers_200_and_is_audited(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let (ex, _reg) = failed_execution(&pool, ws).await;
    let b = task_run_id(&pool, ex, "b").await;

    let res = common::post(
        &app,
        &run_state_path(ws, ex, b),
        user,
        json!({ "state": "SUCCESS" }),
    )
    .await;

    assert_eq!(res.status(), 200);
    let body = common::json(res).await;
    let run = body["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["task_id"] == "b")
        .unwrap();
    assert_eq!(run["state"], "SUCCESS");
    assert_eq!(
        common::audit_mutations(&app),
        vec![(
            "workflow_execution:set_task_run_state".to_string(),
            Some(ex.to_string()),
            json!({ "from_state": "FAILED", "task_run_id": b, "to_state": "SUCCESS" }),
        )]
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_bad_body_is_exactly_400_and_changes_nothing(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let (ex, _reg) = failed_execution(&pool, ws).await;
    let b = task_run_id(&pool, ex, "b").await;
    let bodies = [
        "{\"state\": ",
        "{\"state\":\"RUNNING\"}",
        "{\"state\":\"KILLED\"}",
        "{\"state\":\"success\"}",
        "{\"state\":\"SUCCESS\",\"x\":1}",
        "{}",
    ];
    let before = exec_state(&pool, ex).await;

    for path in [run_state_path(ws, ex, b), status_path(ws, ex)] {
        for body in bodies {
            let res = common::send_raw(
                &app,
                "POST",
                &path,
                user,
                "application/json",
                body.as_bytes().to_vec(),
            )
            .await;
            assert_eq!(res.status(), 400, "{path} {body}");
            let json = common::json(res).await;
            assert!(json["error"].is_string(), "{json}");
            assert_eq!(exec_state(&pool, ex).await, before, "{path} {body}");
        }
    }
    assert!(common::audit_mutations(&app).is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn marking_needs_workflow_manage(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read"]).await;
    let app = common::app(&db).await;
    let (ex, _reg) = failed_execution(&pool, ws).await;
    let b = task_run_id(&pool, ex, "b").await;

    let res = common::post(
        &app,
        &run_state_path(ws, ex, b),
        user,
        json!({"state": "SUCCESS"}),
    )
    .await;
    assert_eq!(res.status(), 403);
    let res = common::post(
        &app,
        &status_path(ws, ex),
        user,
        json!({"state": "SUCCESS"}),
    )
    .await;
    assert_eq!(res.status(), 403);
}

#[sqlx::test(migrations = "../../migrations")]
async fn marking_a_foreign_run_or_execution_is_404(pool: sqlx::PgPool) {
    let (db, ws, user, app) = setup(&pool).await;
    let (ex, _reg) = failed_execution(&pool, ws).await;
    let b = task_run_id(&pool, ex, "b").await;
    let (other_ws, _) = common::seed_workspace(&db).await;
    let (foreign, _r) = failed_execution(&pool, other_ws).await;
    let foreign_run = task_run_id(&pool, foreign, "b").await;

    // A run of another execution under this execution's path.
    let res = common::post(
        &app,
        &run_state_path(ws, ex, foreign_run),
        user,
        json!({"state": "SUCCESS"}),
    )
    .await;
    assert_eq!(res.status(), 404);
    // A run id that does not exist.
    let res = common::post(
        &app,
        &run_state_path(ws, ex, Uuid::new_v4()),
        user,
        json!({"state": "SUCCESS"}),
    )
    .await;
    assert_eq!(res.status(), 404);
    // A foreign execution.
    let res = common::post(
        &app,
        &run_state_path(ws, foreign, foreign_run),
        user,
        json!({"state": "SUCCESS"}),
    )
    .await;
    assert_eq!(res.status(), 404);
    let res = common::post(
        &app,
        &status_path(ws, foreign),
        user,
        json!({"state": "SUCCESS"}),
    )
    .await;
    assert_eq!(res.status(), 404);
    let _ = b;
    assert!(common::audit_mutations(&app).is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn marking_a_run_to_the_state_it_is_in_is_409_state_unchanged(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let (ex, _reg) = failed_execution(&pool, ws).await;
    let b = task_run_id(&pool, ex, "b").await;

    let res = common::post(
        &app,
        &run_state_path(ws, ex, b),
        user,
        json!({"state": "FAILED"}),
    )
    .await;
    expect_error(res, 409, Some("state_unchanged")).await;
    let res = common::post(&app, &status_path(ws, ex), user, json!({"state": "FAILED"})).await;
    expect_error(res, 409, Some("state_unchanged")).await;
    assert!(common::audit_mutations(&app).is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn changing_a_running_or_killed_execution_is_409_not_restartable(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = start(&pool, ws, &flow("running", &["a", "b"])).await;
    let a = task_run_id_opt(&pool, ex, "a").await;

    // RUNNING: not ended.
    let res = common::post(
        &app,
        &status_path(ws, ex),
        user,
        json!({"state": "SUCCESS"}),
    )
    .await;
    let msg = expect_error(res, 409, Some("not_restartable")).await;
    assert!(msg.contains("RUNNING") && msg.contains("KILLED"), "{msg}");
    if let Some(a) = a {
        let res = common::post(
            &app,
            &run_state_path(ws, ex, a),
            user,
            json!({"state": "SUCCESS"}),
        )
        .await;
        expect_error(res, 409, Some("not_restartable")).await;
    }

    // KILLED.
    sqlx::query("UPDATE workflow.executions SET state = 'KILLED', history = history || '[\"KILLED\"]'::jsonb WHERE id = $1")
        .bind(ex)
        .execute(&pool)
        .await
        .unwrap();
    let res = common::post(
        &app,
        &status_path(ws, ex),
        user,
        json!({"state": "SUCCESS"}),
    )
    .await;
    let msg = expect_error(res, 409, Some("not_restartable")).await;
    assert!(msg.contains("KILLED"), "{msg}");
    assert!(common::audit_mutations(&app).is_empty());
}

async fn task_run_id_opt(pool: &sqlx::PgPool, ex: Uuid, task: &str) -> Option<Uuid> {
    sqlx::query_scalar("SELECT id FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2")
        .bind(ex)
        .bind(task)
        .fetch_optional(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn marking_inside_a_working_directory_is_409(pool: sqlx::PgPool) {
    egret_workflow::storage::install_mem_for_tests();
    let (_db, ws, user, app) = setup(&pool).await;
    let reg = Flaky::new(&[("b", 1)]);
    let source = format!(
        "id: wd\nnamespace: t\ntasks:\n  - id: block\n    type: io.kestra.plugin.core.flow.WorkingDirectory\n\
         \x20   tasks:\n      - id: a\n        type: {LOG}\n        message: a\n\
         \x20     - id: b\n        type: {LOG}\n        message: b\n"
    );
    let ex = start(&pool, ws, &source).await;
    drive(&pool, &reg, ex).await;
    assert_eq!(exec_state(&pool, ex).await, "FAILED");
    let b = task_run_id(&pool, ex, "b").await;

    let res = common::post(
        &app,
        &run_state_path(ws, ex, b),
        user,
        json!({"state": "SUCCESS"}),
    )
    .await;

    let msg = expect_error(res, 409, Some("inside_working_directory")).await;
    assert!(msg.contains("restart"), "{msg}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn marking_after_the_files_were_pruned_is_409(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let (ex, _reg) = failed_execution(&pool, ws).await;
    let b = task_run_id(&pool, ex, "b").await;
    sqlx::query("UPDATE workflow.executions SET files_pruned_at = now() WHERE id = $1")
        .bind(ex)
        .execute(&pool)
        .await
        .unwrap();

    let res = common::post(
        &app,
        &run_state_path(ws, ex, b),
        user,
        json!({"state": "SUCCESS"}),
    )
    .await;
    expect_error(res, 409, Some("files_pruned")).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn marking_a_loop_iteration_is_400_naming_the_root(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let (root, _reg) = failed_execution(&pool, ws).await;
    let iteration = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO workflow.executions (id, workspace_id, namespace, flow_key, state, history, \
         inputs, source, parent_execution_id, parent_run_ix, loop_index, loop_value) \
         SELECT $1, workspace_id, namespace, flow_key, 'FAILED', history, inputs, source, id, 0, 0, '1' \
           FROM workflow.executions WHERE id = $2",
    )
    .bind(iteration)
    .bind(root)
    .execute(&pool)
    .await
    .expect("a loop iteration row");

    let res = common::post(
        &app,
        &status_path(ws, iteration),
        user,
        json!({"state": "SUCCESS"}),
    )
    .await;

    assert_eq!(res.status(), 400);
    let body = common::json(res).await;
    assert!(
        body["error"].as_str().unwrap().contains(&root.to_string()),
        "{body}"
    );
}

// ── Status ──────────────────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn changing_the_status_answers_200_and_is_audited(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let (ex, _reg) = failed_execution(&pool, ws).await;

    let res = common::post(
        &app,
        &status_path(ws, ex),
        user,
        json!({"state": "WARNING"}),
    )
    .await;

    assert_eq!(res.status(), 200);
    let body = common::json(res).await;
    assert_eq!(body["state"], "WARNING");
    assert_eq!(
        common::audit_mutations(&app),
        vec![(
            "workflow_execution:set_status".to_string(),
            Some(ex.to_string()),
            json!({ "from_state": "FAILED", "to_state": "WARNING" }),
        )]
    );
}

// ── The read model of an emptied run (A2 carry) ─────────────────────────────

/// A restart empties the runs it does not reach again: Kestra removes them.
/// egret keeps the row (state CREATED, empty history), so the read model must
/// not show it as a run.
#[sqlx::test(migrations = "../../migrations")]
async fn an_emptied_run_is_not_shown_in_the_detail_or_the_graph(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let source = format!(
        "id: hooks\nnamespace: t\ntasks:\n  - id: a\n    type: {LOG}\n    message: a\n\
         \x20 - id: b\n    type: {LOG}\n    message: b\n\
         errors:\n  - id: global_err\n    type: {LOG}\n    message: global\n"
    );
    let reg = Flaky::new(&[("b", 1)]);
    let ex = start(&pool, ws, &source).await;
    drive(&pool, &reg, ex).await;
    assert_eq!(exec_state(&pool, ex).await, "FAILED");
    let res = common::post_no_body(&app, &restart_path(ws, ex), user).await;
    assert_eq!(res.status(), 200);
    drive(&pool, &reg, ex).await;
    assert_eq!(exec_state(&pool, ex).await, "SUCCESS");
    let emptied: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.task_runs WHERE execution_id = $1 \
         AND task_id = 'global_err' AND jsonb_array_length(history) = 0",
    )
    .bind(ex)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(emptied, 1, "the premise: the row exists, emptied");

    let detail = common::json(
        common::get(
            &app,
            &format!("/api/ws/{ws}/workflows/executions/{ex}"),
            user,
        )
        .await,
    )
    .await;
    let ids: Vec<&str> = detail["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["task_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["a", "b"], "{detail}");

    let graph = common::json(
        common::get(
            &app,
            &format!("/api/ws/{ws}/workflows/executions/{ex}/graph"),
            user,
        )
        .await,
    )
    .await;
    assert!(
        !graph.to_string().contains("\"CREATED\""),
        "the graph shows a phantom CREATED run: {graph}"
    );
}

#[allow(dead_code)]
fn _unused(_: Value) {}

// ── Final review I1: the flow's current state and the `egret.sql` gate ──────

/// The flow of `ex`.
async fn flow_id_of(pool: &sqlx::PgPool, ex: Uuid) -> Uuid {
    sqlx::query_scalar("SELECT flow_id FROM workflow.executions WHERE id = $1")
        .bind(ex)
        .fetch_one(pool)
        .await
        .expect("flow id")
}

/// A second person in `ws` with `workflow:manage`, who did not create its flows.
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

/// The FAILED execution of [`failed_execution`], its frozen source then
/// given an `egret.sql` task at `c` (the run not reached yet), as if the
/// creator had started a flow that runs SQL as them.
async fn failed_sql_execution(pool: &sqlx::PgPool, ws: Uuid) -> Uuid {
    let (ex, _reg) = failed_execution(pool, ws).await;
    let source = format!(
        "id: ops\nnamespace: t\ntasks:\n  - id: a\n    type: {LOG}\n    message: a\n\
         \x20 - id: b\n    type: {LOG}\n    message: b\n\
         \x20 - id: c\n    type: egret.sql\n    database: trino_prod\n    sql: SELECT 1\n"
    );
    sqlx::query("UPDATE workflow.executions SET source = $2 WHERE id = $1")
        .bind(ex)
        .bind(&source)
        .execute(pool)
        .await
        .unwrap();
    ex
}

/// Which operation a gate test sends.
#[derive(Clone, Copy, Debug)]
enum Op {
    Restart,
    MarkB,
}

async fn send(
    app: &common::App,
    pool: &sqlx::PgPool,
    op: Op,
    ws: Uuid,
    ex: Uuid,
    user: egret_core::UserId,
) -> axum::response::Response {
    match op {
        Op::Restart => common::post_no_body(app, &restart_path(ws, ex), user).await,
        Op::MarkB => {
            let b = task_run_id(pool, ex, "b").await;
            common::post(
                app,
                &run_state_path(ws, ex, b),
                user,
                json!({ "state": "SUCCESS" }),
            )
            .await
        }
    }
}

async fn a_trashed_flow_is_refused_404(op: Op, pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let (ex, _reg) = failed_execution(&pool, ws).await;
    sqlx::query("UPDATE workflow.flows SET deleted_at = now() WHERE id = $1")
        .bind(flow_id_of(&pool, ex).await)
        .execute(&pool)
        .await
        .unwrap();

    let res = send(&app, &pool, op, ws, ex, user).await;

    assert_eq!(res.status(), 404, "{op:?}");
    assert_eq!(exec_state(&pool, ex).await, "FAILED", "{op:?}");
    assert!(common::audit_mutations(&app).is_empty(), "{op:?}");
}

async fn a_disabled_flow_is_refused_409(op: Op, pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let (ex, _reg) = failed_execution(&pool, ws).await;
    sqlx::query("UPDATE workflow.flows SET state = 'disabled' WHERE id = $1")
        .bind(flow_id_of(&pool, ex).await)
        .execute(&pool)
        .await
        .unwrap();

    let res = send(&app, &pool, op, ws, ex, user).await;

    let msg = expect_error(res, 409, None).await;
    assert!(msg.contains("disabled"), "{op:?}: {msg}");
    assert_eq!(exec_state(&pool, ex).await, "FAILED", "{op:?}");
    assert!(common::audit_mutations(&app).is_empty(), "{op:?}");
}

async fn a_flow_less_execution_is_refused_404(op: Op, pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let (ex, _reg) = failed_execution(&pool, ws).await;
    sqlx::query("UPDATE workflow.executions SET flow_id = NULL WHERE id = $1")
        .bind(ex)
        .execute(&pool)
        .await
        .unwrap();

    let res = send(&app, &pool, op, ws, ex, user).await;

    assert_eq!(res.status(), 404, "{op:?}");
    assert_eq!(exec_state(&pool, ex).await, "FAILED", "{op:?}");
}

async fn a_non_creator_cannot_rerun_an_egret_sql_flow(op: Op, pool: sqlx::PgPool) {
    let (db, ws, creator, _app) = setup(&pool).await;
    let ex = failed_sql_execution(&pool, ws).await;
    let other = other_manager(&db, ws).await;
    // An app built after the grant, which it reads at build.
    let app = common::app(&db).await;

    let res = send(&app, &pool, op, ws, ex, other).await;

    let msg = expect_error(res, 400, None).await;
    assert!(
        msg.contains("only its creator can start it"),
        "{op:?}: {msg}"
    );
    assert_eq!(exec_state(&pool, ex).await, "FAILED", "{op:?}");
    assert!(common::audit_mutations(&app).is_empty(), "{op:?}");

    // The control: the creator passes the same gate.
    let res = send(&app, &pool, op, ws, ex, creator).await;
    assert_eq!(res.status(), 200, "{op:?}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn restart_of_an_execution_of_a_trashed_flow_is_404(pool: sqlx::PgPool) {
    a_trashed_flow_is_refused_404(Op::Restart, pool).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn marking_a_run_of_a_trashed_flow_is_404(pool: sqlx::PgPool) {
    a_trashed_flow_is_refused_404(Op::MarkB, pool).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn restart_of_an_execution_of_a_disabled_flow_is_409(pool: sqlx::PgPool) {
    a_disabled_flow_is_refused_409(Op::Restart, pool).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn marking_a_run_of_a_disabled_flow_is_409(pool: sqlx::PgPool) {
    a_disabled_flow_is_refused_409(Op::MarkB, pool).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn restart_of_an_execution_whose_flow_was_purged_is_404(pool: sqlx::PgPool) {
    a_flow_less_execution_is_refused_404(Op::Restart, pool).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn marking_a_run_whose_flow_was_purged_is_404(pool: sqlx::PgPool) {
    a_flow_less_execution_is_refused_404(Op::MarkB, pool).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn restart_of_an_egret_sql_flow_by_a_non_creator_is_400(pool: sqlx::PgPool) {
    a_non_creator_cannot_rerun_an_egret_sql_flow(Op::Restart, pool).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn marking_a_run_of_an_egret_sql_flow_by_a_non_creator_is_400(pool: sqlx::PgPool) {
    a_non_creator_cannot_rerun_an_egret_sql_flow(Op::MarkB, pool).await;
}

/// Change status runs no task, so neither gate applies to it (final review
/// I1): a disabled flow's execution, changed by a person who did not create
/// its `egret.sql` flow, still takes the status.
#[sqlx::test(migrations = "../../migrations")]
async fn changing_the_status_needs_neither_gate(pool: sqlx::PgPool) {
    let (db, ws, _creator, _app) = setup(&pool).await;
    let ex = failed_sql_execution(&pool, ws).await;
    sqlx::query("UPDATE workflow.flows SET state = 'disabled' WHERE id = $1")
        .bind(flow_id_of(&pool, ex).await)
        .execute(&pool)
        .await
        .unwrap();
    let other = other_manager(&db, ws).await;
    let app = common::app(&db).await;

    let res = common::post(
        &app,
        &status_path(ws, ex),
        other,
        json!({ "state": "SUCCESS" }),
    )
    .await;

    assert_eq!(res.status(), 200);
    assert_eq!(exec_state(&pool, ex).await, "SUCCESS");
}
