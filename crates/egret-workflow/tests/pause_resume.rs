//! Pause an execution by hand and resume it, through the worker entry points
//! (workflow package 7c, spec
//! `docs/en/specs/2026-11-15-workflow-pause-resume-design.md` D2, D3, D10,
//! Q3, Q5; Kestra `ExecutionService.pause`, `ES:907-916`).
//!
//! A manual pause writes `PAUSED` on the execution and changes no task run.
//! Runs already running finish and are recorded; nothing new starts, no
//! render pass runs, and the waker leaves the execution alone until resume.
//! A `Loop` iteration that ends still starts the next one, as in Kestra
//! (spec Q3).
//!
//! Every test is `#[sqlx::test]`: the worker's `claim` and `tick` are global,
//! so each test owns its database.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5465/egret_test
//!   cargo test -p egret-workflow --test pause_resume

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use egret_metastore::Db;
use egret_workflow::worker::{
    kill_execution, pause_by_hand, restart_execution, Handlers, Outcome, Paused, Registry,
    RestartRefused, Restarted, TaskInput,
};
use serde_json::{json, Value};
use uuid::Uuid;

mod common;

// ── Fixtures (from `tests/worker.rs`) ───────────────────────────────────────

fn one_task(id: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    )
}

fn two_tasks(id: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n  - id: b\n    type: io.kestra.plugin.core.log.Log\n    message: bye\n"
    )
}

async fn file_flow(db: &Db, ws: Uuid, by: egret_core::UserId, source: &str) -> Uuid {
    let flow = egret_workflow::spec::parse(source).expect("the fixture flow parses");
    let input = egret_workflow::CreateFlowInput {
        namespace: flow.namespace.clone(),
        flow_id: flow.id.clone(),
        source: source.to_string(),
        plan: serde_json::to_value(&flow).expect("a parsed flow serialises"),
        folder_id: None,
    };
    egret_workflow::FlowRepo::new(db.clone())
        .create(ws, by, &input)
        .await
        .expect("file the flow")
        .id
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

fn subflow_parent(id: &str, target: &str, extra: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: sub\n    \
         type: io.kestra.plugin.core.flow.Subflow\n    namespace: t\n    \
         flowId: {target}\n{extra}  - id: after\n    \
         type: io.kestra.plugin.core.log.Log\n    message: after\n"
    )
}

/// `a` and `b` inside a `Sequential`, then a top-level `c`.
const SEQ: &str = "id: seq\nnamespace: t\ntasks:\n  - id: seq\n    \
                   type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n      \
                   - id: a\n        type: io.kestra.plugin.core.log.Log\n        message: a\n      \
                   - id: b\n        type: io.kestra.plugin.core.log.Log\n        message: b\n  \
                   - id: c\n    type: io.kestra.plugin.core.log.Log\n    message: c\n";

// ── The registry for the restart test (from `tests/restart_routes.rs`) ──────

/// Fails each task named in `fails` that many times, then succeeds.
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
            return Ok(Outcome::failed_with(format!(
                "{} fails on purpose",
                task.task_id
            )));
        }
        Ok(Outcome::succeeded_with(json!({ "value": task.task_id })))
    }
    async fn cancel(&self, _task: &TaskInput) -> egret_core::Result<()> {
        Ok(())
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

async fn setup(pool: &sqlx::PgPool) -> (Db, Uuid, egret_core::UserId) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    (db, ws, user)
}

async fn start(
    pool: &sqlx::PgPool,
    db: &Db,
    ws: Uuid,
    user: egret_core::UserId,
    src: &str,
) -> Uuid {
    let flow = file_flow(db, ws, user, src).await;
    egret_workflow::worker::start_execution(pool, ws, Some(flow), src, json!({}))
        .await
        .expect("start")
}

async fn exec_state(pool: &sqlx::PgPool, id: Uuid) -> String {
    sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("state")
}

async fn exec_history(pool: &sqlx::PgPool, id: Uuid) -> Value {
    sqlx::query_scalar("SELECT history FROM workflow.executions WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("history")
}

/// `task_id -> state` of every run, in run order.
async fn runs(pool: &sqlx::PgPool, id: Uuid) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT task_id, state FROM workflow.task_runs WHERE execution_id = $1 ORDER BY run_ix",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .expect("runs")
}

fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

fn state_of(runs: &[(String, String)], task: &str) -> Option<String> {
    runs.iter().find(|(t, _)| t == task).map(|(_, s)| s.clone())
}

fn handlers() -> Handlers {
    Handlers::with_core_tasks(egret_workflow::storage::current())
}

// ── 1-3: what a pause writes, and what still moves ──────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn a_running_execution_pauses_and_no_run_changes(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    let ex = start(&pool, &db, ws, user, &two_tasks("p")).await;
    assert_eq!(runs(&pool, ex).await, pairs(&[("a", "RUNNING")]));

    // Act
    let paused = pause_by_hand(&pool, ws, ex).await.expect("pause");

    // Assert
    assert_eq!(paused, Paused::Done);
    assert_eq!(exec_state(&pool, ex).await, "PAUSED");
    let history = exec_history(&pool, ex).await;
    let h = history.as_array().expect("history is an array");
    assert_eq!(&h[h.len() - 2..], &[json!("RUNNING"), json!("PAUSED")]);
    assert_eq!(runs(&pool, ex).await, pairs(&[("a", "RUNNING")]));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_run_completing_during_the_pause_is_recorded_and_nothing_new_is_created(
    pool: sqlx::PgPool,
) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    let ex = start(&pool, &db, ws, user, &two_tasks("p")).await;
    assert_eq!(pause_by_hand(&pool, ws, ex).await.unwrap(), Paused::Done);

    // Act
    settle_everything(&pool, &handlers()).await;

    // Assert
    assert_eq!(runs(&pool, ex).await, pairs(&[("a", "SUCCESS")]));
    assert_eq!(exec_state(&pool, ex).await, "PAUSED");
    let ended: bool = sqlx::query_scalar(
        "SELECT ended_at IS NOT NULL FROM workflow.task_runs \
          WHERE execution_id = $1 AND task_id = 'a'",
    )
    .bind(ex)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(ended, "a's outcome is written");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_run_ending_inside_a_flowable_does_not_complete_its_parent(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    let ex = start(&pool, &db, ws, user, SEQ).await;
    assert_eq!(pause_by_hand(&pool, ws, ex).await.unwrap(), Paused::Done);

    // Act
    settle_everything(&pool, &handlers()).await;

    // Assert
    let r = runs(&pool, ex).await;
    assert_eq!(state_of(&r, "seq").as_deref(), Some("RUNNING"));
    assert_eq!(state_of(&r, "a").as_deref(), Some("SUCCESS"));
    assert_eq!(state_of(&r, "b"), None, "{r:?}");
    assert_eq!(state_of(&r, "c"), None, "{r:?}");
    assert_eq!(exec_state(&pool, ex).await, "PAUSED");
}

// ── 4-5: the pause against a kill, the end, and another workspace ───────────

#[sqlx::test(migrations = "../../migrations")]
async fn pause_against_kill_and_against_the_end(pool: sqlx::PgPool) {
    let (db, ws, user) = setup(&pool).await;

    // (a) kill first, then pause: refused with the kill's state.
    let ex = start(&pool, &db, ws, user, &two_tasks("ka")).await;
    kill_execution(&pool, ws, ex, true).await.expect("kill");
    let killed_state = exec_state(&pool, ex).await;
    let paused = pause_by_hand(&pool, ws, ex).await.expect("pause");
    assert!(
        matches!(&paused, Paused::NotRunning(s) if s == "KILLING" || s == "KILLED"),
        "{paused:?}"
    );
    assert_eq!(exec_state(&pool, ex).await, killed_state);

    // (b) pause first, then kill: the kill wins, and `b` never starts.
    let ex = start(&pool, &db, ws, user, &two_tasks("kb")).await;
    assert_eq!(pause_by_hand(&pool, ws, ex).await.unwrap(), Paused::Done);
    kill_execution(&pool, ws, ex, true).await.expect("kill");
    assert_eq!(exec_state(&pool, ex).await, "KILLING");
    settle_everything(&pool, &handlers()).await;
    assert_eq!(exec_state(&pool, ex).await, "KILLED");
    assert_eq!(state_of(&runs(&pool, ex).await, "b"), None);

    // (c) a finished execution.
    let ex = start(&pool, &db, ws, user, &one_task("done")).await;
    settle_everything(&pool, &handlers()).await;
    assert_eq!(exec_state(&pool, ex).await, "SUCCESS");
    assert_eq!(
        pause_by_hand(&pool, ws, ex).await.unwrap(),
        Paused::NotRunning("SUCCESS".into())
    );
    assert_eq!(exec_state(&pool, ex).await, "SUCCESS");

    // (d) concurrent: both answer, the kill decides the end, `b` never runs.
    let ex = start(&pool, &db, ws, user, &two_tasks("kd")).await;
    let (p, k) = tokio::join!(
        pause_by_hand(&pool, ws, ex),
        kill_execution(&pool, ws, ex, true)
    );
    p.expect("pause answers");
    k.expect("kill answers");
    settle_everything(&pool, &handlers()).await;
    assert_eq!(exec_state(&pool, ex).await, "KILLED");
    assert_eq!(state_of(&runs(&pool, ex).await, "b"), None);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_foreign_workspace_cannot_pause(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    let ex = start(&pool, &db, ws, user, &two_tasks("p")).await;
    let (other_ws, _) = common::seed_workspace(&db).await;

    // Act
    let refused = pause_by_hand(&pool, other_ws, ex).await;

    // Assert
    assert!(
        matches!(refused, Err(egret_core::Error::NotFound(_))),
        "{refused:?}"
    );
    assert_eq!(exec_state(&pool, ex).await, "RUNNING");
}

// ── 6-7: a `Loop` and a `Subflow` under a paused root ───────────────────────

/// The body reads `item.value`, egret's name for the iteration value inside
/// a `Loop` (`render.rs`'s ancestor walk); `taskrun.value` is undefined there.
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

async fn loopy_paused_and_settled(pool: &sqlx::PgPool) -> (Uuid, Uuid) {
    let (db, ws, user) = setup(pool).await;
    let ex = start(pool, &db, ws, user, LOOPY).await;
    assert_eq!(pause_by_hand(pool, ws, ex).await.unwrap(), Paused::Done);
    settle_everything(pool, &handlers()).await;
    (ws, ex)
}

/// Spec Q3 (S1): Kestra's `LoopExecutionEventMessageHandler` starts the next
/// iteration whatever the parent's state, and so does egret's
/// `spawn::on_execution_terminal_inner`.
#[sqlx::test(migrations = "../../migrations")]
async fn s1_a_loop_iteration_ending_during_the_pause_starts_the_next_and_the_flow_waits(
    pool: sqlx::PgPool,
) {
    // Arrange + Act
    let (_ws, ex) = loopy_paused_and_settled(&pool).await;

    // Assert
    let iterations: Vec<String> = sqlx::query_scalar(
        "SELECT state FROM workflow.executions \
          WHERE parent_execution_id = $1 AND loop_index IS NOT NULL ORDER BY loop_index",
    )
    .bind(ex)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(iterations, ["SUCCESS", "SUCCESS"]);
    let r = runs(&pool, ex).await;
    assert_eq!(state_of(&r, "each").as_deref(), Some("SUCCESS"), "{r:?}");
    assert_eq!(state_of(&r, "after"), None, "{r:?}");
    assert_eq!(exec_state(&pool, ex).await, "PAUSED");
}

/// The second half of test 6: the by-hand resume (T4 teaches
/// `resume_by_hand` the paused-by-hand case).
#[sqlx::test(migrations = "../../migrations")]
#[ignore = "T4"]
async fn s1_after_the_loop_a_resume_runs_the_rest(pool: sqlx::PgPool) {
    // Arrange
    let (_ws, ex) = loopy_paused_and_settled(&pool).await;

    // Act
    egret_workflow::worker::resume_by_hand(&pool, ex)
        .await
        .expect("resume");
    settle_everything(&pool, &handlers()).await;

    // Assert
    assert_eq!(exec_state(&pool, ex).await, "SUCCESS");
    assert_eq!(
        state_of(&runs(&pool, ex).await, "after").as_deref(),
        Some("SUCCESS")
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_child_ending_during_the_pause_completes_its_run_and_nothing_after(
    pool: sqlx::PgPool,
) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    file_flow(&db, ws, user, &one_task("child")).await;
    let parent = start(&pool, &db, ws, user, &subflow_parent("parent", "child", "")).await;
    assert_eq!(
        pause_by_hand(&pool, ws, parent).await.unwrap(),
        Paused::Done
    );

    // Act
    settle_everything(&pool, &handlers()).await;

    // Assert
    let children: Vec<String> =
        sqlx::query_scalar("SELECT state FROM workflow.executions WHERE trigger_execution_id = $1")
            .bind(parent)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(children, ["SUCCESS"]);
    let r = runs(&pool, parent).await;
    assert_eq!(state_of(&r, "sub").as_deref(), Some("SUCCESS"), "{r:?}");
    assert_eq!(state_of(&r, "after"), None, "{r:?}");
    assert_eq!(exec_state(&pool, parent).await, "PAUSED");
}

// ── 8: S4, no render pass while paused by hand ──────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn s4_a_loop_until_condition_is_not_rendered_during_the_pause(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    let src = "id: until\nnamespace: t\ntasks:\n  - id: lu\n    \
               type: io.kestra.plugin.core.flow.LoopUntil\n    \
               condition: \"{{ false }}\"\n    \
               checkFrequency:\n      interval: PT1H\n      maxIterations: 2\n    \
               tasks:\n      \
               - id: a\n        type: io.kestra.plugin.core.log.Log\n        message: hi\n";
    let ex = start(&pool, &db, ws, user, src).await;
    assert_eq!(pause_by_hand(&pool, ws, ex).await.unwrap(), Paused::Done);

    // Act
    settle_everything(&pool, &handlers()).await;

    // Assert
    assert_eq!(
        state_of(&runs(&pool, ex).await, "a").as_deref(),
        Some("SUCCESS")
    );
    let rendered: Option<String> = sqlx::query_scalar(
        "SELECT rendered FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'lu'",
    )
    .bind(ex)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rendered, None, "the condition waits for the resume");
    assert_eq!(exec_state(&pool, ex).await, "PAUSED");
}

// ── 9: S3 / Q5, a manual pause is refused while a `Pause` waits ─────────────

#[sqlx::test(migrations = "../../migrations")]
async fn s3_a_manual_pause_is_refused_while_a_pause_waits(pool: sqlx::PgPool) {
    // Arrange: `quick` ends, `slow` still waits, the execution is RUNNING.
    let (db, ws, user) = setup(&pool).await;
    let src = "id: two\nnamespace: t\ntasks:\n  - id: both\n    \
               type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      \
               - id: quick\n        type: io.kestra.plugin.core.flow.Pause\n        \
               pauseDuration: PT0.1S\n      \
               - id: slow\n        type: io.kestra.plugin.core.flow.Pause\n        \
               pauseDuration: PT1H\n  \
               - id: after\n    type: io.kestra.plugin.core.log.Log\n    message: after\n";
    let ex = start(&pool, &db, ws, user, src).await;
    let mut reached = false;
    for _ in 0..20 {
        settle_everything(&pool, &handlers()).await;
        let r = runs(&pool, ex).await;
        if state_of(&r, "quick").as_deref() == Some("SUCCESS")
            && state_of(&r, "slow").as_deref() == Some("PAUSED")
            && exec_state(&pool, ex).await == "RUNNING"
        {
            reached = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
        egret_workflow::worker::wake(&pool, 16).await.expect("wake");
    }
    assert!(reached, "{:?}", runs(&pool, ex).await);
    let history = exec_history(&pool, ex).await;

    // Act
    let paused = pause_by_hand(&pool, ws, ex).await.expect("pause");

    // Assert
    assert_eq!(paused, Paused::TaskWaiting);
    assert_eq!(exec_state(&pool, ex).await, "RUNNING");
    assert_eq!(exec_history(&pool, ex).await, history);

    // Control: once `slow` has ended and `after` runs, the pause is taken.
    sqlx::query(
        "UPDATE workflow.task_runs SET not_before_ms = 0 \
          WHERE execution_id = $1 AND task_id = 'slow'",
    )
    .bind(ex)
    .execute(&pool)
    .await
    .unwrap();
    egret_workflow::worker::wake(&pool, 16).await.expect("wake");
    let r = runs(&pool, ex).await;
    assert_eq!(state_of(&r, "slow").as_deref(), Some("SUCCESS"), "{r:?}");
    assert_eq!(state_of(&r, "after").as_deref(), Some("RUNNING"), "{r:?}");
    assert_eq!(pause_by_hand(&pool, ws, ex).await.unwrap(), Paused::Done);
}

// ── 10: S5, the waker leaves a manual pause alone ───────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn s5_a_manual_pause_with_a_due_retry_is_not_woken(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    let ex = start(&pool, &db, ws, user, &two_tasks("p")).await;
    assert_eq!(pause_by_hand(&pool, ws, ex).await.unwrap(), Paused::Done);
    sqlx::query(
        "UPDATE workflow.task_runs \
            SET state = 'RETRYING', history = history || '[\"RETRYING\"]', not_before_ms = 0 \
          WHERE execution_id = $1 AND task_id = 'a'",
    )
    .bind(ex)
    .execute(&pool)
    .await
    .unwrap();

    // Act
    let due = egret_workflow::worker::due_retries(&pool, 10)
        .await
        .unwrap();

    // Assert
    assert!(!due.contains(&ex), "{due:?}");

    // Control: the same row under a RUNNING execution is woken.
    sqlx::query("UPDATE workflow.executions SET state = 'RUNNING' WHERE id = $1")
        .bind(ex)
        .execute(&pool)
        .await
        .unwrap();
    let due = egret_workflow::worker::due_retries(&pool, 10)
        .await
        .unwrap();
    assert!(due.contains(&ex), "{due:?}");
}

// ── 11-12: kill and restart of a manual pause ───────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn kill_on_a_manual_pause_with_a_finished_child_ends_killed(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    let ex = start(&pool, &db, ws, user, SEQ).await;
    assert_eq!(pause_by_hand(&pool, ws, ex).await.unwrap(), Paused::Done);
    settle_everything(&pool, &handlers()).await;
    let r = runs(&pool, ex).await;
    assert_eq!(state_of(&r, "a").as_deref(), Some("SUCCESS"));
    assert_eq!(state_of(&r, "seq").as_deref(), Some("RUNNING"));

    // Act
    kill_execution(&pool, ws, ex, true).await.expect("kill");

    // Assert
    let after_kill = exec_state(&pool, ex).await;
    assert!(
        after_kill == "KILLING" || after_kill == "KILLED",
        "{after_kill}"
    );
    settle_everything(&pool, &handlers()).await;
    assert_eq!(exec_state(&pool, ex).await, "KILLED");
    let r = runs(&pool, ex).await;
    assert_eq!(state_of(&r, "seq").as_deref(), Some("KILLED"), "{r:?}");
    assert_eq!(state_of(&r, "b"), None, "{r:?}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn restart_of_a_manual_pause_follows_7a(pool: sqlx::PgPool) {
    let (db, ws, user) = setup(&pool).await;

    // (a) nothing failed: nothing to restart, the pause stands.
    let ex = start(&pool, &db, ws, user, &two_tasks("ra")).await;
    assert_eq!(pause_by_hand(&pool, ws, ex).await.unwrap(), Paused::Done);
    settle_everything(&pool, &handlers()).await;
    assert_eq!(
        restart_execution(&pool, ex).await.unwrap(),
        Restarted::Refused(RestartRefused::NothingToRestart)
    );
    assert_eq!(exec_state(&pool, ex).await, "PAUSED");

    // (b) `a` fails during the pause: the restart takes it from there.
    let ex = start(&pool, &db, ws, user, &two_tasks("rb")).await;
    assert_eq!(pause_by_hand(&pool, ws, ex).await.unwrap(), Paused::Done);
    let flaky = Flaky::new(&[("a", 1)]);
    settle_everything(&pool, &flaky).await;
    assert_eq!(runs(&pool, ex).await, pairs(&[("a", "FAILED")]));
    assert_eq!(exec_state(&pool, ex).await, "PAUSED");
    assert_eq!(restart_execution(&pool, ex).await.unwrap(), Restarted::Done);
    let history = exec_history(&pool, ex).await;
    assert!(
        history.as_array().unwrap().contains(&json!("RESTARTED")),
        "{history}"
    );
    settle_everything(&pool, &flaky).await;
    assert_eq!(
        state_of(&runs(&pool, ex).await, "b").as_deref(),
        Some("SUCCESS")
    );
    assert_eq!(exec_state(&pool, ex).await, "SUCCESS");
}

#[test]
fn database_url_is_exported_rather_than_read_from_dotenv() {
    egret_test_db::assert_database_url_is_exported();
}
