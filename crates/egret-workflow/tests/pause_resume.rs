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
    kill_execution, pause_by_hand, restart_execution, resume_with, Handlers, Outcome, Paused,
    Registry, RestartRefused, Restarted, ResumeRefused, ResumeRequest, Resumed, TaskInput,
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
async fn s1_after_the_loop_a_resume_runs_the_rest(pool: sqlx::PgPool) {
    // Arrange
    let (_ws, ex) = loopy_paused_and_settled(&pool).await;

    // Act
    let resumed = egret_workflow::worker::resume_by_hand(&pool, ex)
        .await
        .expect("resume");
    settle_everything(&pool, &handlers()).await;

    // Assert
    assert_eq!(resumed, Resumed::ByHand);
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

    // The resume ends in a settle, which renders what the pause deferred.
    let resumed = egret_workflow::worker::resume_by_hand(&pool, ex)
        .await
        .expect("resume");
    assert_eq!(resumed, Resumed::ByHand);
    let rendered: Option<String> = sqlx::query_scalar(
        "SELECT rendered FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'lu'",
    )
    .bind(ex)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        rendered.is_some(),
        "the resume's settle renders the condition"
    );
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

// ── 14-25: resume, by hand and with `onResume` inputs (T4) ──────────────────

fn req(id: Uuid, inputs: Option<&serde_json::Map<String, Value>>) -> ResumeRequest<'_> {
    ResumeRequest {
        id,
        inputs,
        files: &[],
        expected: None,
    }
}

/// [`req`] as the route sends it for a `Pause` it read waiting: `expected`
/// is that run (T5 review C1: inputs with no `expected` are `Moved`).
fn at(id: Uuid, inputs: Option<&serde_json::Map<String, Value>>, run: Uuid) -> ResumeRequest<'_> {
    ResumeRequest {
        expected: Some(run),
        ..req(id, inputs)
    }
}

fn obj(v: Value) -> serde_json::Map<String, Value> {
    match v {
        Value::Object(m) => m,
        other => panic!("not an object: {other}"),
    }
}

/// `outputs` of `task`'s run.
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

async fn run_id_of(pool: &sqlx::PgPool, ex: Uuid, task: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2")
        .bind(ex)
        .bind(task)
        .fetch_one(pool)
        .await
        .expect("run id")
}

/// [`settle_everything`] with a log sink, so a `Log` task's line reaches
/// `workflow.task_logs` (the harness of `tests/http_request.rs`).
async fn settle_with_logs(pool: &sqlx::PgPool) {
    use egret_workflow::logs::{spawn_writer, LogLimits, LogRepo, WriterConfig};
    let cancel = tokio_util::sync::CancellationToken::new();
    let (sink, writer) = spawn_writer(
        LogRepo::new(pool.clone()),
        WriterConfig::default(),
        cancel.clone(),
    );
    let reg = handlers().with_log_sink(std::sync::Arc::new(sink), LogLimits::default());
    settle_everything(pool, &reg).await;
    drop(reg);
    cancel.cancel();
    writer.await.expect("the log writer ends");
}

async fn log_lines(pool: &sqlx::PgPool, ex: Uuid) -> Vec<String> {
    sqlx::query_scalar("SELECT message FROM workflow.task_logs WHERE execution_id = $1 ORDER BY id")
        .bind(ex)
        .fetch_all(pool)
        .await
        .expect("logs")
}

/// Spec §4's flow: a `Pause` asking who approves, a count and an optional
/// file, then a `Log` reading them.
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
    message: "who={{ outputs.p.onResume.who }} n={{ outputs.p.onResume.n }} by={{ outputs.p.resumed.to }}"
"#;

async fn approve_waiting(pool: &sqlx::PgPool) -> (Uuid, Uuid) {
    let (db, ws, user) = setup(pool).await;
    let ex = start(pool, &db, ws, user, APPROVE).await;
    settle_everything(pool, &handlers()).await;
    assert_eq!(exec_state(pool, ex).await, "PAUSED");
    assert_eq!(
        state_of(&runs(pool, ex).await, "p").as_deref(),
        Some("PAUSED")
    );
    (ws, ex)
}

#[sqlx::test(migrations = "../../migrations")]
async fn resume_by_hand_runs_on_and_the_flow_finishes(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    let ex = start(&pool, &db, ws, user, &two_tasks("p")).await;
    assert_eq!(pause_by_hand(&pool, ws, ex).await.unwrap(), Paused::Done);
    settle_everything(&pool, &handlers()).await;
    assert_eq!(runs(&pool, ex).await, pairs(&[("a", "SUCCESS")]));

    // Act
    let resumed = resume_with(&pool, &req(ex, None)).await.expect("resume");
    settle_everything(&pool, &handlers()).await;

    // Assert
    assert_eq!(resumed, Resumed::ByHand);
    assert_eq!(
        runs(&pool, ex).await,
        pairs(&[("a", "SUCCESS"), ("b", "SUCCESS")])
    );
    assert_eq!(exec_state(&pool, ex).await, "SUCCESS");
    let history = exec_history(&pool, ex).await;
    let h = history.as_array().unwrap();
    let paused = h.iter().position(|s| s == "PAUSED").expect("PAUSED");
    assert_eq!(h[paused + 1], json!("RUNNING"), "{history}");
    assert!(!h.contains(&json!("RESTARTED")), "{history}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn resume_by_hand_with_inputs_is_refused_and_writes_nothing(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    let ex = start(&pool, &db, ws, user, &two_tasks("p")).await;
    assert_eq!(pause_by_hand(&pool, ws, ex).await.unwrap(), Paused::Done);
    let history = exec_history(&pool, ex).await;
    let inputs = obj(json!({"x": 1}));

    // Act
    let resumed = resume_with(&pool, &req(ex, Some(&inputs))).await.unwrap();

    // Assert
    assert_eq!(resumed, Resumed::Refused(ResumeRefused::InputsUnexpected));
    assert_eq!(exec_state(&pool, ex).await, "PAUSED");
    assert_eq!(exec_history(&pool, ex).await, history);
}

#[sqlx::test(migrations = "../../migrations")]
async fn resume_with_inputs_writes_on_resume_and_resumed_and_the_next_task_reads_them(
    pool: sqlx::PgPool,
) {
    // Arrange
    let (_ws, ex) = approve_waiting(&pool).await;
    let inputs = obj(json!({"who": "ada", "n": "3"}));

    // Act
    let resumed = resume_with(
        &pool,
        &at(ex, Some(&inputs), run_id_of(&pool, ex, "p").await),
    )
    .await
    .unwrap();
    settle_with_logs(&pool).await;

    // Assert
    assert_eq!(resumed, Resumed::Released(1));
    let out = outputs_of(&pool, ex, "p").await.expect("p has outputs");
    // The input check answers every declared input; an absent optional one
    // is `null`, as a start's is.
    assert_eq!(
        out["onResume"],
        json!({"who": "ada", "n": 3, "doc": null}),
        "{out}"
    );
    assert_eq!(out["resumed"]["by"], Value::Null, "{out}");
    assert_eq!(out["resumed"]["to"], json!("SUCCESS"), "{out}");
    let on = out["resumed"]["on"].as_str().expect("`on` is a string");
    chrono::DateTime::parse_from_rfc3339(on).expect("`on` is RFC 3339");
    let logs = log_lines(&pool, ex).await;
    assert!(
        logs.iter().any(|l| l.contains("who=ada n=3 by=SUCCESS")),
        "{logs:?}"
    );
    assert_eq!(exec_state(&pool, ex).await, "SUCCESS");
    let history = exec_history(&pool, ex).await;
    assert!(
        history.as_array().unwrap().contains(&json!("RESTARTED")),
        "{history}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_refused_value_names_the_input_and_writes_nothing(pool: sqlx::PgPool) {
    // Arrange
    let (_ws, ex) = approve_waiting(&pool).await;
    let bad = obj(json!({"who": "ada", "n": "x"}));
    let missing = obj(json!({"n": "3"}));

    // Act
    let refused = resume_with(&pool, &at(ex, Some(&bad), run_id_of(&pool, ex, "p").await))
        .await
        .unwrap();
    let refused_missing = resume_with(
        &pool,
        &at(ex, Some(&missing), run_id_of(&pool, ex, "p").await),
    )
    .await
    .unwrap();

    // Assert
    match &refused {
        Resumed::Refused(ResumeRefused::Invalid(m)) => assert!(m.contains("`n`"), "{m}"),
        other => panic!("{other:?}"),
    }
    match &refused_missing {
        Resumed::Refused(ResumeRefused::Invalid(m)) => {
            assert!(m.contains("Missing required input:who"), "{m}")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(exec_state(&pool, ex).await, "PAUSED");
    assert_eq!(
        state_of(&runs(&pool, ex).await, "p").as_deref(),
        Some("PAUSED")
    );
    assert_eq!(outputs_of(&pool, ex, "p").await, None);
}

#[sqlx::test(migrations = "../../migrations")]
async fn s6_an_undeclared_key_is_refused(pool: sqlx::PgPool) {
    // Arrange
    let (_ws, ex) = approve_waiting(&pool).await;
    let inputs = obj(json!({"who": "ada", "n": "3", "zzz": 1}));

    // Act
    let refused = resume_with(
        &pool,
        &at(ex, Some(&inputs), run_id_of(&pool, ex, "p").await),
    )
    .await
    .unwrap();

    // Assert
    match &refused {
        Resumed::Refused(ResumeRefused::Invalid(m)) => assert!(m.contains("`zzz`"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(exec_state(&pool, ex).await, "PAUSED");
    assert_eq!(outputs_of(&pool, ex, "p").await, None);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_pause_without_on_resume_accepts_no_body_and_refuses_inputs(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    let src = "id: plain\nnamespace: t\ntasks:\n  - id: p\n    \
               type: io.kestra.plugin.core.flow.Pause\n  \
               - id: after\n    type: io.kestra.plugin.core.log.Log\n    message: after\n";
    let ex = start(&pool, &db, ws, user, src).await;
    settle_everything(&pool, &handlers()).await;
    assert_eq!(exec_state(&pool, ex).await, "PAUSED");
    let some = obj(json!({"a": 1}));
    let none = serde_json::Map::new();

    // Act
    let refused = resume_with(&pool, &at(ex, Some(&some), run_id_of(&pool, ex, "p").await))
        .await
        .unwrap();
    let state_after_refusal = exec_state(&pool, ex).await;
    let released = resume_with(&pool, &at(ex, Some(&none), run_id_of(&pool, ex, "p").await))
        .await
        .unwrap();
    settle_everything(&pool, &handlers()).await;

    // Assert
    assert_eq!(refused, Resumed::Refused(ResumeRefused::InputsUnexpected));
    assert_eq!(state_after_refusal, "PAUSED");
    assert_eq!(released, Resumed::Released(1));
    let out = outputs_of(&pool, ex, "p").await.expect("p has outputs");
    assert_eq!(out["onResume"], json!({}), "{out}");
    assert_eq!(out["resumed"]["to"], json!("SUCCESS"), "{out}");
    assert_eq!(exec_state(&pool, ex).await, "SUCCESS");
}

#[sqlx::test(migrations = "../../migrations")]
async fn parallel_pauses_the_first_gets_the_inputs_and_the_others_on_resume_empty(
    pool: sqlx::PgPool,
) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    let src = "id: two\nnamespace: t\ntasks:\n  - id: both\n    \
               type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      \
               - id: p1\n        type: io.kestra.plugin.core.flow.Pause\n        \
               onResume:\n          - id: a\n            type: STRING\n      \
               - id: p2\n        type: io.kestra.plugin.core.flow.Pause\n        \
               onResume:\n          - id: b\n            type: STRING\n";
    let ex = start(&pool, &db, ws, user, src).await;
    settle_everything(&pool, &handlers()).await;
    assert_eq!(exec_state(&pool, ex).await, "PAUSED");
    let inputs = obj(json!({"a": "x"}));

    // Act
    let resumed = resume_with(
        &pool,
        &at(ex, Some(&inputs), run_id_of(&pool, ex, "p1").await),
    )
    .await
    .unwrap();
    settle_everything(&pool, &handlers()).await;

    // Assert
    assert_eq!(resumed, Resumed::Released(2));
    let p1 = outputs_of(&pool, ex, "p1").await.expect("p1 outputs");
    let p2 = outputs_of(&pool, ex, "p2").await.expect("p2 outputs");
    assert_eq!(p1["onResume"], json!({"a": "x"}), "{p1}");
    assert_eq!(p2["onResume"], json!({}), "{p2}");
    assert_eq!(exec_state(&pool, ex).await, "SUCCESS");
}

#[sqlx::test(migrations = "../../migrations")]
async fn only_a_loop_until_waiting_is_no_pause_waiting(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    let src = "id: until\nnamespace: t\ntasks:\n  - id: lu\n    \
               type: io.kestra.plugin.core.flow.LoopUntil\n    \
               condition: \"{{ false }}\"\n    \
               checkFrequency:\n      interval: PT1H\n      maxIterations: 3\n    \
               tasks:\n      \
               - id: body\n        type: io.kestra.plugin.core.log.Log\n        message: hi\n";
    let ex = start(&pool, &db, ws, user, src).await;
    settle_everything(&pool, &handlers()).await;
    assert_eq!(exec_state(&pool, ex).await, "PAUSED");
    let history = exec_history(&pool, ex).await;

    // Act
    let resumed = resume_with(&pool, &req(ex, None)).await.unwrap();

    // Assert
    assert_eq!(resumed, Resumed::NoPauseWaiting);
    assert_eq!(exec_history(&pool, ex).await, history);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_lost_race_is_moved(pool: sqlx::PgPool) {
    // Arrange
    let (_ws, ex) = approve_waiting(&pool).await;
    let inputs = obj(json!({"who": "ada", "n": "3"}));
    let history = exec_history(&pool, ex).await;

    // Act
    let resumed = resume_with(
        &pool,
        &ResumeRequest {
            id: ex,
            inputs: Some(&inputs),
            files: &[],
            expected: Some(Uuid::new_v4()),
        },
    )
    .await
    .unwrap();

    // Assert
    assert_eq!(resumed, Resumed::Refused(ResumeRefused::Moved));
    assert_eq!(exec_history(&pool, ex).await, history);
    assert_eq!(
        state_of(&runs(&pool, ex).await, "p").as_deref(),
        Some("PAUSED")
    );
    assert_eq!(outputs_of(&pool, ex, "p").await, None);
}

/// T5 review C1: inputs sent where the route read no `Pause` waiting
/// (`expected: None`) are not written into a `Pause` that waits under the
/// lock: they were checked against nothing and passed no D7 gate.
#[sqlx::test(migrations = "../../migrations")]
async fn c1_inputs_where_the_route_read_no_pause_are_moved(pool: sqlx::PgPool) {
    // Arrange
    let (_ws, ex) = approve_waiting(&pool).await;
    let inputs = obj(json!({"who": "ada", "n": "3"}));
    let history = exec_history(&pool, ex).await;

    // Act
    let refused = resume_with(&pool, &req(ex, Some(&inputs))).await.unwrap();

    // Assert
    assert_eq!(refused, Resumed::Refused(ResumeRefused::Moved));
    assert_eq!(exec_history(&pool, ex).await, history);
    assert_eq!(outputs_of(&pool, ex, "p").await, None);

    // Control: the same inputs with the run the route read are written.
    let run = run_id_of(&pool, ex, "p").await;
    let resumed = resume_with(&pool, &at(ex, Some(&inputs), run))
        .await
        .unwrap();
    assert_eq!(resumed, Resumed::Released(1));
}

/// T5 review M1: the route read a `Pause` waiting, and by the lock the
/// execution is paused by hand instead: `Moved`, not a resume by hand.
#[sqlx::test(migrations = "../../migrations")]
async fn m1_a_manual_pause_after_the_read_is_moved(pool: sqlx::PgPool) {
    // Arrange: `p` resumed, `log` running, then a manual pause.
    let (ws, ex) = approve_waiting(&pool).await;
    let run = run_id_of(&pool, ex, "p").await;
    let inputs = obj(json!({"who": "ada", "n": "3"}));
    assert_eq!(
        resume_with(&pool, &at(ex, Some(&inputs), run))
            .await
            .unwrap(),
        Resumed::Released(1)
    );
    assert_eq!(pause_by_hand(&pool, ws, ex).await.unwrap(), Paused::Done);
    let history = exec_history(&pool, ex).await;

    // Act: a resume whose read saw `p` waiting.
    let moved = resume_with(&pool, &at(ex, None, run)).await.unwrap();

    // Assert
    assert_eq!(moved, Resumed::Refused(ResumeRefused::Moved));
    assert_eq!(exec_history(&pool, ex).await, history);
    assert_eq!(exec_state(&pool, ex).await, "PAUSED");

    // Control: a resume whose read saw the manual pause resumes it.
    assert_eq!(
        resume_with(&pool, &req(ex, None)).await.unwrap(),
        Resumed::ByHand
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn s2_a_file_stored_under_the_pause_run_is_indexed_and_readable(pool: sqlx::PgPool) {
    // Arrange
    let fs = egret_workflow::storage::install_mem_for_tests();
    let (ws, ex) = approve_waiting(&pool).await;
    let run = run_id_of(&pool, ex, "p").await;
    let flow = egret_workflow::spec::parse(APPROVE).unwrap();
    let doc_input = match &flow.tasks[0].kind {
        egret_workflow::models::TaskKind::Pause(p) => p
            .on_resume
            .as_ref()
            .unwrap()
            .iter()
            .find(|i| i.id == "doc")
            .unwrap()
            .clone(),
        other => panic!("{other:?}"),
    };
    let req_id = Uuid::new_v4();
    let stored = egret_workflow::storage::inputs::upload_resume_input(
        &fs,
        ws,
        "t",
        "approve",
        ex,
        "p",
        run,
        req_id,
        &doc_input,
        "a.txt",
        &mut &b"hello"[..],
    )
    .await
    .expect("stored");
    let inputs = obj(json!({"who": "ada", "n": "1", "doc": stored.uri}));
    let files = [stored.clone()];

    // Act
    let resumed = resume_with(
        &pool,
        &ResumeRequest {
            id: ex,
            inputs: Some(&inputs),
            files: &files,
            expected: Some(run),
        },
    )
    .await
    .unwrap();

    // Assert
    assert_eq!(resumed, Resumed::Released(1));
    assert!(
        stored.uri.ends_with(&format!(
            "/executions/{ex}/tasks/p/{run}/resume/{req_id}/doc/a.txt"
        )),
        "{}",
        stored.uri
    );
    let mut conn = pool.acquire().await.unwrap();
    let readable = egret_workflow::storage::resolve_readable(&mut conn, ws, ex, &stored.uri)
        .await
        .expect("the database answers");
    assert!(readable.is_ok(), "{readable:?}");
    let indexed: Option<Uuid> = sqlx::query_scalar(
        "SELECT task_run_id FROM workflow.execution_files WHERE execution_id = $1 AND uri = $2",
    )
    .bind(ex)
    .bind(&stored.uri)
    .fetch_one(&pool)
    .await
    .expect("the file is indexed");
    assert_eq!(indexed, Some(run));
    let out = outputs_of(&pool, ex, "p").await.unwrap();
    assert_eq!(out["onResume"]["doc"], json!(stored.uri), "{out}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn s7_a_secret_on_resume_value_is_sealed_and_a_later_task_reads_it_masked_in_logs(
    pool: sqlx::PgPool,
) {
    // Arrange
    egret_workflow::secret_inputs::install_for_tests();
    let (db, ws, user) = setup(&pool).await;
    let src = "id: sec\nnamespace: t\ntasks:\n  - id: p\n    \
               type: io.kestra.plugin.core.flow.Pause\n    \
               onResume:\n      - id: tok\n        type: SECRET\n  \
               - id: log\n    type: io.kestra.plugin.core.log.Log\n    \
               message: \"tok={{ outputs.p.onResume.tok }}\"\n  \
               - id: chk\n    type: io.kestra.plugin.core.log.Log\n    \
               message: \"clear={{ outputs.p.onResume.tok == 's3cr3t' }}\"\n";
    let ex = start(&pool, &db, ws, user, src).await;
    settle_everything(&pool, &handlers()).await;
    assert_eq!(exec_state(&pool, ex).await, "PAUSED");
    let inputs = obj(json!({"tok": "s3cr3t"}));

    // Act
    let resumed = resume_with(
        &pool,
        &at(ex, Some(&inputs), run_id_of(&pool, ex, "p").await),
    )
    .await
    .unwrap();
    settle_with_logs(&pool).await;

    // Assert
    assert_eq!(resumed, Resumed::Released(1));
    let out = outputs_of(&pool, ex, "p").await.unwrap();
    assert_eq!(out["onResume"]["tok"], json!("******"), "{out}");
    let sealed: bool = sqlx::query_scalar(
        "SELECT secret_outputs IS NOT NULL FROM workflow.task_runs \
          WHERE execution_id = $1 AND task_id = 'p'",
    )
    .bind(ex)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(sealed, "the clear value is sealed");
    let logs = log_lines(&pool, ex).await;
    assert!(logs.iter().any(|l| l.starts_with("tok=")), "{logs:?}");
    // The later task reads the CLEAR value (the render surface reveals it)...
    assert!(logs.iter().any(|l| l == "clear=true"), "{logs:?}");
    // ...and its log masks it.
    assert!(!logs.iter().any(|l| l.contains("s3cr3t")), "{logs:?}");
    assert_eq!(exec_state(&pool, ex).await, "SUCCESS");
}

#[test]
fn database_url_is_exported_rather_than_read_from_dotenv() {
    egret_test_db::assert_database_url_is_exported();
}
