//! Set one task run's state, and change a finished execution's status,
//! through the worker entry points (workflow package 7a, spec
//! `docs/en/specs/2026-11-13-workflow-execution-ops-restart-design.md` D11;
//! Kestra `ExecutionService.markAs` / `changeState`, `ChangeStateTestCase`,
//! `ExecutionServiceTest.changeStateWithErrorBranch` / `changeState`).
//!
//! Every test is `#[sqlx::test]`: the worker's `claim` and `tick` are global,
//! so each test owns its database.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5465/egret_test
//!   cargo test -p egret-workflow --test mark_as

use std::collections::HashMap;
use std::sync::Mutex;

use egret_metastore::Db;
use egret_workflow::executor::RunState;
use egret_workflow::worker::{
    set_execution_status, set_task_run_state, Outcome, Registry, StateChangeRefused, TaskInput,
};
use serde_json::{json, Value};
use uuid::Uuid;

mod common;

const LOG: &str = "io.kestra.plugin.core.log.Log";
const RETURN: &str = "io.kestra.plugin.core.debug.Return";

// ── The registry ────────────────────────────────────────────────────────────

/// Fails each task named in `fails` that many times, then succeeds with
/// `{"value": "<task>-<attempt>"}`. Records every run it was handed.
struct Flaky {
    fails: Mutex<HashMap<String, usize>>,
    seen: Mutex<Vec<(String, usize)>>,
    inner: Option<egret_workflow::worker::Handlers>,
}

impl Flaky {
    fn new(fails: &[(&str, usize)]) -> Self {
        Self {
            fails: Mutex::new(fails.iter().map(|(t, n)| (t.to_string(), *n)).collect()),
            seen: Mutex::new(Vec::new()),
            inner: None,
        }
    }

    fn over(mut self, inner: egret_workflow::worker::Handlers) -> Self {
        self.inner = Some(inner);
        self
    }

    fn seen(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|(t, _)| t.clone())
            .collect()
    }
}

#[async_trait::async_trait]
impl Registry for Flaky {
    async fn run(&self, task: &TaskInput) -> egret_core::Result<Outcome> {
        self.seen
            .lock()
            .unwrap()
            .push((task.task_id.clone(), task.attempt));
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
        match &self.inner {
            Some(inner) => inner.run(task).await,
            None => Ok(Outcome::succeeded_with(
                json!({ "value": format!("{}-{}", task.task_id, task.attempt) }),
            )),
        }
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

async fn start(pool: &sqlx::PgPool, ws: Uuid, source: &str) -> Uuid {
    egret_workflow::worker::start_execution(pool, ws, None, source, json!({}))
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

/// `(history, attempt_number, labels)`.
async fn exec_meta(pool: &sqlx::PgPool, id: Uuid) -> (Value, i32, Option<Value>) {
    sqlx::query_as("SELECT history, attempt_number, labels FROM workflow.executions WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("execution row")
}

/// One run: `(run_ix, task_id, id, state, history, attempts, outputs)`.
type RunRow = (i32, String, Uuid, String, Value, Value, Option<Value>);

async fn runs_of(pool: &sqlx::PgPool, execution: Uuid) -> Vec<RunRow> {
    sqlx::query_as(
        "SELECT run_ix, task_id, id, state, history, attempts, outputs FROM workflow.task_runs \
          WHERE execution_id = $1 ORDER BY run_ix",
    )
    .bind(execution)
    .fetch_all(pool)
    .await
    .expect("runs")
}

fn run<'a>(runs: &'a [RunRow], task: &str) -> &'a RunRow {
    runs.iter()
        .find(|r| r.1 == task)
        .unwrap_or_else(|| panic!("no run of {task}: {runs:?}"))
}

fn is_empty_history(r: &RunRow) -> bool {
    r.4.as_array().is_some_and(Vec::is_empty)
}

/// The last attempt's last state.
fn last_attempt_state(r: &RunRow) -> String {
    let attempts = r.5.as_array().expect("attempts is an array");
    let last = attempts.last().expect("an attempt");
    last["history"]
        .as_array()
        .and_then(|h| h.last())
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no attempt state: {last}"))
        .to_string()
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

fn history_tail(h: &Value, n: usize) -> Vec<String> {
    let a = h.as_array().expect("history is an array");
    a[a.len().saturating_sub(n)..]
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect()
}

fn has_restarted_label(labels: &Option<Value>) -> bool {
    labels.as_ref().and_then(Value::as_array).is_some_and(|ls| {
        ls.iter()
            .any(|l| l["key"] == "system.restarted" && l["value"] == "true")
    })
}

fn tasks(ids: &[&str]) -> String {
    ids.iter()
        .map(|id| format!("  - id: {id}\n    type: {LOG}\n    message: {id}\n"))
        .collect()
}

fn flow(id: &str, ids: &[&str]) -> String {
    format!("id: {id}\nnamespace: t\ntasks:\n{}", tasks(ids))
}

async fn mark(
    pool: &sqlx::PgPool,
    ex: Uuid,
    task_run: Uuid,
    target: RunState,
) -> Result<(), StateChangeRefused> {
    set_task_run_state(pool, ex, task_run, target)
        .await
        .expect("set_task_run_state")
}

async fn status(pool: &sqlx::PgPool, ex: Uuid, target: RunState) -> Result<(), StateChangeRefused> {
    set_execution_status(pool, ex, target)
        .await
        .expect("set_execution_status")
}

// ── 1. ChangeStateTestCase.changeStateShouldEndsInSuccess ──────────────────

/// Kestra `ChangeStateTestCase:28-40` and `shouldRestartAfterChangeTaskState`:
/// `failed-first`'s FAILED first task marked SUCCESS. Right after the call
/// the execution has gone `RESTARTED RUNNING`, with the label and
/// `attempt_number` 2; the run is `SUCCESS` on its history and on its last
/// attempt, with no new attempt and its id kept. Then it runs on to
/// `SUCCESS` with 2 runs.
#[sqlx::test(migrations = "../../migrations")]
async fn a_failed_first_task_marked_success_runs_on_to_success(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("t1", 1)]);
    let ex = start(&pool, ws, &flow("failed-first", &["t1", "t2"])).await;
    drive(&pool, &reg, ex).await;
    assert_eq!(exec_state(&pool, ex).await, "FAILED");
    let failed = runs_of(&pool, ex).await;
    assert_eq!(failed.len(), 1);
    let t1 = run(&failed, "t1").clone();
    assert_eq!(t1.3, "FAILED");

    // Act
    let out = mark(&pool, ex, t1.2, RunState::Success).await;

    // Assert
    assert_eq!(out, Ok(()));
    let (h, attempt_number, labels) = exec_meta(&pool, ex).await;
    assert_eq!(
        history_tail(&h, 3),
        ["FAILED", "RESTARTED", "RUNNING"],
        "{h}"
    );
    assert_eq!(attempt_number, 2);
    assert!(has_restarted_label(&labels), "{labels:?}");
    let after = runs_of(&pool, ex).await;
    let t1_after = run(&after, "t1");
    assert_eq!(t1_after.2, t1.2, "the marked run keeps its id");
    assert_eq!(t1_after.3, "SUCCESS");
    assert_eq!(
        t1_after.5.as_array().map(Vec::len),
        t1.5.as_array().map(Vec::len),
        "no new attempt"
    );
    assert_eq!(last_attempt_state(t1_after), "SUCCESS");

    drive(&pool, &reg, ex).await;
    assert_eq!(exec_state(&pool, ex).await, "SUCCESS");
    let end = runs_of(&pool, ex).await;
    assert_eq!(end.len(), 2, "{end:?}");
    assert_eq!(run(&end, "t1").3, "SUCCESS");
    assert_eq!(run(&end, "t2").3, "SUCCESS");
    assert_eq!(reg.seen(), ["t1", "t2"], "t1 is not run again");
}

/// Kestra keeps the marked run's outputs (`markAs` changes only the state):
/// a SUCCESS run marked FAILED keeps them, and the execution ends FAILED.
#[sqlx::test(migrations = "../../migrations")]
async fn the_marked_run_keeps_its_outputs(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[]);
    let ex = start(&pool, ws, &flow("keeps", &["a", "b", "c"])).await;
    drive(&pool, &reg, ex).await;
    assert_eq!(exec_state(&pool, ex).await, "SUCCESS");
    let before = runs_of(&pool, ex).await;
    let b = run(&before, "b").clone();
    assert!(b.6.is_some(), "b has outputs: {b:?}");

    // Act
    let out = mark(&pool, ex, b.2, RunState::Failed).await;
    drive(&pool, &reg, ex).await;

    // Assert
    assert_eq!(out, Ok(()));
    assert_eq!(exec_state(&pool, ex).await, "FAILED");
    let after = runs_of(&pool, ex).await;
    let b_after = run(&after, "b");
    assert_eq!(b_after.3, "FAILED");
    assert_eq!(b_after.6, b.6, "the marked run's outputs are kept");
    assert_eq!(last_attempt_state(b_after), "FAILED");
    assert_eq!(reg.seen(), ["a", "b", "c"], "nothing runs again");
}

// ── 2. changeStateWithErrorBranch ──────────────────────────────────────────

/// Kestra `change-state-errors`.
fn change_state_errors() -> String {
    format!(
        "id: change-state-errors\nnamespace: t\nerrors:\n  - id: print_error_log\n    \
         type: {LOG}\n    message: failure\ntasks:\n{}",
        tasks(&["before_task", "make_error", "after-task"])
    )
}

/// Kestra `ExecutionServiceTest.changeStateWithErrorBranch` (:518-538): the
/// error run is emptied (Kestra removes it) under a new id, `attempt_number`
/// is 2, and the execution runs on to SUCCESS without the error branch.
#[sqlx::test(migrations = "../../migrations")]
async fn marking_the_failed_run_empties_the_error_branch(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("make_error", 1)]);
    let ex = start(&pool, ws, &change_state_errors()).await;
    drive(&pool, &reg, ex).await;
    assert_eq!(exec_state(&pool, ex).await, "FAILED");
    let failed = runs_of(&pool, ex).await;
    assert_eq!(failed.len(), 3, "{failed:?}");
    let err = run(&failed, "print_error_log").clone();
    assert_eq!(err.3, "SUCCESS");
    let make_error = run(&failed, "make_error").2;

    // Act
    let out = mark(&pool, ex, make_error, RunState::Success).await;

    // Assert
    assert_eq!(out, Ok(()));
    let (h, attempt_number, labels) = exec_meta(&pool, ex).await;
    assert_eq!(attempt_number, 2);
    assert!(has_restarted_label(&labels));
    assert_eq!(
        history_tail(&h, 3),
        ["FAILED", "RESTARTED", "RUNNING"],
        "{h}"
    );
    let after = runs_of(&pool, ex).await;
    assert_eq!(run(&after, "make_error").3, "SUCCESS");
    let err_after = after
        .iter()
        .find(|r| r.0 == err.0)
        .expect("the error run's row");
    assert!(is_empty_history(err_after), "emptied: {err_after:?}");
    assert_ne!(err_after.2, err.2, "an emptied run gets a new id");
    assert_eq!(err_after.6, None, "and its outputs are cleared");

    drive(&pool, &reg, ex).await;
    assert_eq!(exec_state(&pool, ex).await, "SUCCESS");
    let end = runs_of(&pool, ex).await;
    assert_eq!(run(&end, "after-task").3, "SUCCESS");
    let live: Vec<&str> = end
        .iter()
        .filter(|r| !is_empty_history(r))
        .map(|r| r.1.as_str())
        .collect();
    assert_eq!(live, ["before_task", "make_error", "after-task"], "{end:?}");
}

// ── 3. CANCELLED ───────────────────────────────────────────────────────────

/// Spec D11.5: a CANCELLED target makes the execution CANCELLED at once, and
/// nothing runs.
#[sqlx::test(migrations = "../../migrations")]
async fn marking_cancelled_cancels_the_execution_and_runs_nothing(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("b", 1)]);
    let ex = start(&pool, ws, &flow("cancel", &["a", "b", "c"])).await;
    drive(&pool, &reg, ex).await;
    assert_eq!(exec_state(&pool, ex).await, "FAILED");
    let b = run(&runs_of(&pool, ex).await, "b").2;

    // Act
    let out = mark(&pool, ex, b, RunState::Cancelled).await;
    drive(&pool, &reg, ex).await;

    // Assert
    assert_eq!(out, Ok(()));
    assert_eq!(exec_state(&pool, ex).await, "CANCELLED");
    let (h, attempt_number, _) = exec_meta(&pool, ex).await;
    assert_eq!(history_tail(&h, 2), ["FAILED", "CANCELLED"], "{h}");
    assert_eq!(attempt_number, 2);
    let runs = runs_of(&pool, ex).await;
    assert_eq!(runs.len(), 2, "c never runs: {runs:?}");
    assert_eq!(run(&runs, "b").3, "CANCELLED");
    assert_eq!(reg.seen(), ["a", "b"]);
}

// ── 4. changeState ─────────────────────────────────────────────────────────

/// Kestra `ExecutionServiceTest.changeState` (:669-677) and `ES:1385-1392`:
/// one state appended to the history, no run touched, no label, no attempt,
/// and exactly one Flow-trigger event.
#[sqlx::test(migrations = "../../migrations")]
async fn a_success_execution_set_to_warning_appends_one_state(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    common::seed_flow(
        &db,
        ws,
        user,
        "id: listener\nnamespace: t\ntriggers:\n  - id: on_any\n    \
         type: io.kestra.plugin.core.trigger.Flow\n    \
         states: [RESTARTED, RUNNING, SUCCESS, FAILED, WARNING]\n\
         tasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n",
        "draft",
    )
    .await;
    let reg = Flaky::new(&[]);
    let ex = start(&pool, ws, &flow("minimal", &["a"])).await;
    drive(&pool, &reg, ex).await;
    assert_eq!(exec_state(&pool, ex).await, "SUCCESS");
    let (h0, attempt0, labels0) = exec_meta(&pool, ex).await;
    let runs0 = runs_of(&pool, ex).await;
    let mine = |all: Vec<(Uuid, String)>| -> Vec<String> {
        all.into_iter()
            .filter(|(id, _)| *id == ex)
            .map(|(_, s)| s)
            .collect()
    };
    let events0 = mine(common::events(&db).await);

    // Act
    let out = status(&pool, ex, RunState::Warning).await;

    // Assert
    assert_eq!(out, Ok(()));
    assert_eq!(exec_state(&pool, ex).await, "WARNING");
    let (h, attempt, labels) = exec_meta(&pool, ex).await;
    let mut want = h0.as_array().unwrap().clone();
    want.push(json!("WARNING"));
    assert_eq!(h, Value::Array(want), "history + 1");
    assert_eq!(attempt, attempt0, "no attempt change");
    assert_eq!(labels, labels0, "no label");
    assert_eq!(runs_of(&pool, ex).await, runs0, "no run touched");
    let events = mine(common::events(&db).await);
    assert_eq!(events[events0.len()..], ["WARNING"], "{events:?}");
    drive(&pool, &reg, ex).await;
    assert_eq!(exec_state(&pool, ex).await, "WARNING");
    assert_eq!(runs_of(&pool, ex).await, runs0, "nothing runs after");
}

// ── 5. Refusals write nothing ──────────────────────────────────────────────

async fn assert_mark_refused(
    pool: &sqlx::PgPool,
    ex: Uuid,
    task_run: Uuid,
    target: RunState,
    want: StateChangeRefused,
) {
    let before = exec_meta(pool, ex).await;
    let runs = runs_of(pool, ex).await;
    assert_eq!(mark(pool, ex, task_run, target).await, Err(want));
    assert_eq!(
        exec_meta(pool, ex).await,
        before,
        "a refusal writes nothing"
    );
    assert_eq!(runs_of(pool, ex).await, runs, "nor any run");
}

async fn assert_status_refused(
    pool: &sqlx::PgPool,
    ex: Uuid,
    target: RunState,
    want: StateChangeRefused,
) {
    let before = exec_meta(pool, ex).await;
    let runs = runs_of(pool, ex).await;
    assert_eq!(status(pool, ex, target).await, Err(want));
    assert_eq!(
        exec_meta(pool, ex).await,
        before,
        "a refusal writes nothing"
    );
    assert_eq!(runs_of(pool, ex).await, runs, "nor any run");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_killed_execution_is_not_changeable(pool: sqlx::PgPool) {
    let (_db, ws, _) = setup(&pool).await;
    let ex = start(&pool, ws, &flow("killed", &["a", "b"])).await;
    egret_workflow::worker::kill_execution(&pool, ws, ex, false)
        .await
        .expect("kill");
    let reg = Flaky::new(&[]);
    drive(&pool, &reg, ex).await;
    assert_eq!(exec_state(&pool, ex).await, "KILLED");
    let a = run(&runs_of(&pool, ex).await, "a").2;

    let killed = StateChangeRefused::NotChangeable("KILLED".into());
    assert_mark_refused(&pool, ex, a, RunState::Success, killed.clone()).await;
    assert_status_refused(&pool, ex, RunState::Success, killed).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_running_execution_is_not_changeable(pool: sqlx::PgPool) {
    let (_db, ws, _) = setup(&pool).await;
    let ex = start(&pool, ws, &flow("running", &["a", "b"])).await;
    assert_eq!(exec_state(&pool, ex).await, "RUNNING");
    let a = run(&runs_of(&pool, ex).await, "a").2;

    let running = StateChangeRefused::NotChangeable("RUNNING".into());
    assert_mark_refused(&pool, ex, a, RunState::Success, running.clone()).await;
    assert_status_refused(&pool, ex, RunState::Success, running).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_same_state_is_refused(pool: sqlx::PgPool) {
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("b", 1)]);
    let ex = start(&pool, ws, &flow("same", &["a", "b"])).await;
    drive(&pool, &reg, ex).await;
    assert_eq!(exec_state(&pool, ex).await, "FAILED");
    let b = run(&runs_of(&pool, ex).await, "b").2;

    assert_mark_refused(
        &pool,
        ex,
        b,
        RunState::Failed,
        StateChangeRefused::StateUnchanged,
    )
    .await;
    assert_status_refused(
        &pool,
        ex,
        RunState::Failed,
        StateChangeRefused::StateUnchanged,
    )
    .await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_foreign_or_unknown_task_run_id_is_refused(pool: sqlx::PgPool) {
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("b", 2)]);
    let ex = start(&pool, ws, &flow("mine", &["a", "b"])).await;
    let other = start(&pool, ws, &flow("other", &["a", "b"])).await;
    drive(&pool, &reg, ex).await;
    drive(&pool, &reg, other).await;
    let foreign = run(&runs_of(&pool, other).await, "b").2;

    assert_mark_refused(
        &pool,
        ex,
        foreign,
        RunState::Success,
        StateChangeRefused::NoSuchTaskRun,
    )
    .await;
    assert_mark_refused(
        &pool,
        ex,
        Uuid::new_v4(),
        RunState::Success,
        StateChangeRefused::NoSuchTaskRun,
    )
    .await;
}

/// A1 review: a run with an empty history (an end-hook run a mark emptied
/// and the execution never rebuilt) is "removed" in Kestra's terms.
#[sqlx::test(migrations = "../../migrations")]
async fn an_emptied_run_is_no_such_task_run(pool: sqlx::PgPool) {
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("make_error", 1)]);
    let ex = start(&pool, ws, &change_state_errors()).await;
    drive(&pool, &reg, ex).await;
    let make_error = run(&runs_of(&pool, ex).await, "make_error").2;
    assert_eq!(mark(&pool, ex, make_error, RunState::Success).await, Ok(()));
    drive(&pool, &reg, ex).await;
    assert_eq!(exec_state(&pool, ex).await, "SUCCESS");
    let emptied = run(&runs_of(&pool, ex).await, "print_error_log").clone();
    assert!(is_empty_history(&emptied), "{emptied:?}");

    assert_mark_refused(
        &pool,
        ex,
        emptied.2,
        RunState::Success,
        StateChangeRefused::NoSuchTaskRun,
    )
    .await;
}

/// Spec D11: a loop iteration is changed through its root.
#[sqlx::test(migrations = "../../migrations")]
async fn a_loop_iteration_is_refused_naming_its_root(pool: sqlx::PgPool) {
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("body", 1)]);
    let source = format!(
        "id: loop\nnamespace: t\ntasks:\n  - id: loop\n    type: io.kestra.plugin.core.flow.Loop\n\
         \x20   values: '[\"1\"]'\n    tasks:\n      - id: body\n        type: {LOG}\n        \
         message: hi\n"
    );
    let root = start(&pool, ws, &source).await;
    drive(&pool, &reg, root).await;
    let iteration: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE parent_execution_id = $1 AND loop_index = 0",
    )
    .bind(root)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(exec_state(&pool, iteration).await, "FAILED");
    let body = run(&runs_of(&pool, iteration).await, "body").2;

    let refused = StateChangeRefused::LoopIteration { root };
    assert_mark_refused(&pool, iteration, body, RunState::Success, refused.clone()).await;
    assert_status_refused(&pool, iteration, RunState::Success, refused).await;
}

/// Spec D10, carried to D11: the pruner deleted the files.
#[sqlx::test(migrations = "../../migrations")]
async fn an_execution_whose_files_were_pruned_is_refused(pool: sqlx::PgPool) {
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("b", 1)]);
    let ex = start(&pool, ws, &flow("pruned", &["a", "b"])).await;
    drive(&pool, &reg, ex).await;
    sqlx::query("UPDATE workflow.executions SET files_pruned_at = now() WHERE id = $1")
        .bind(ex)
        .execute(&pool)
        .await
        .unwrap();
    let b = run(&runs_of(&pool, ex).await, "b").2;

    assert_mark_refused(
        &pool,
        ex,
        b,
        RunState::Success,
        StateChangeRefused::FilesPruned,
    )
    .await;
}

/// Spec D10, carried to D11: an `afterExecution:` still runs, so the
/// execution has not ended.
#[sqlx::test(migrations = "../../migrations")]
async fn an_execution_whose_after_execution_runs_is_refused(pool: sqlx::PgPool) {
    let (_db, ws, _) = setup(&pool).await;
    let source = format!(
        "id: live-after\nnamespace: t\ntasks:\n{}afterExecution:\n  - id: end\n    type: {LOG}\n    \
         message: after\n",
        tasks(&["a"])
    );
    let ex = start(&pool, ws, &source).await;
    let claimed = egret_workflow::worker::claim(&pool, 10).await.unwrap();
    assert_eq!(claimed.len(), 1);
    egret_workflow::worker::commit_outcome(&pool, &claimed[0], &Outcome::failed())
        .await
        .unwrap();
    assert_eq!(exec_state(&pool, ex).await, "FAILED");
    let runs = runs_of(&pool, ex).await;
    assert_eq!(run(&runs, "end").3, "RUNNING", "{runs:?}");
    let a = run(&runs, "a").2;

    let refused = StateChangeRefused::AfterExecutionRunning;
    assert_mark_refused(&pool, ex, a, RunState::Success, refused.clone()).await;
    assert_status_refused(&pool, ex, RunState::Success, refused).await;
}

/// A1: a run beneath a `WorkingDirectory` cannot be marked (its directory is
/// closed); restart the execution instead.
#[sqlx::test(migrations = "../../migrations")]
async fn a_run_inside_a_working_directory_is_refused(pool: sqlx::PgPool) {
    egret_workflow::storage::install_mem_for_tests();
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("b", 1)]).over(egret_workflow::worker::Handlers::with_core_tasks(
        egret_workflow::storage::current(),
    ));
    let source = format!(
        "id: wd\nnamespace: t\ntasks:\n  - id: block\n    type: io.kestra.plugin.core.flow.WorkingDirectory\n\
         \x20   tasks:\n      - id: a\n        type: {RETURN}\n        format: a\n\
         \x20     - id: b\n        type: {RETURN}\n        format: b\n"
    );
    let ex = start(&pool, ws, &source).await;
    drive(&pool, &reg, ex).await;
    assert_eq!(exec_state(&pool, ex).await, "FAILED");
    let b = run(&runs_of(&pool, ex).await, "b").2;

    assert_mark_refused(
        &pool,
        ex,
        b,
        RunState::Success,
        StateChangeRefused::InsideWorkingDirectory,
    )
    .await;
}

/// Only SUCCESS, WARNING, FAILED and CANCELLED are targets (D11); anything
/// else is an input error, and nothing is written.
#[sqlx::test(migrations = "../../migrations")]
async fn a_target_outside_the_four_is_invalid_input(pool: sqlx::PgPool) {
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("b", 1)]);
    let ex = start(&pool, ws, &flow("bad-target", &["a", "b"])).await;
    drive(&pool, &reg, ex).await;
    let before = (exec_meta(&pool, ex).await, runs_of(&pool, ex).await);
    let b = run(&before.1, "b").2;

    for target in [RunState::Running, RunState::Killed, RunState::Created] {
        let m = set_task_run_state(&pool, ex, b, target).await;
        assert!(
            matches!(m, Err(egret_core::Error::InvalidInput(_))),
            "{target:?}: {m:?}"
        );
        let s = set_execution_status(&pool, ex, target).await;
        assert!(
            matches!(s, Err(egret_core::Error::InvalidInput(_))),
            "{target:?}: {s:?}"
        );
    }
    assert_eq!(
        (exec_meta(&pool, ex).await, runs_of(&pool, ex).await),
        before
    );
}

// ── 6. A Subflow child changed after its parent's run ended ─────────────────

fn failed_first() -> String {
    format!(
        "id: failed-first\nnamespace: t\ntasks:\n{}",
        tasks(&["t1", "t2"])
    )
}

fn subflow_parent_of_failed() -> String {
    "id: subflow-parent-of-failed\nnamespace: t\ntasks:\n  - id: subflow\n    \
     type: io.kestra.plugin.core.flow.Subflow\n    namespace: t\n    flowId: failed-first\n    \
     wait: true\n    transmitFailed: true\n"
        .to_string()
}

/// `(parent, child)`, both FAILED, the parent's `Subflow` run FAILED.
async fn failed_parent_and_child(
    pool: &sqlx::PgPool,
    db: &Db,
    ws: Uuid,
    user: egret_core::UserId,
    reg: &Flaky,
) -> (Uuid, Uuid) {
    common::seed_flow(db, ws, user, &failed_first(), "draft").await;
    let parent = start(pool, ws, &subflow_parent_of_failed()).await;
    drive(pool, reg, parent).await;
    assert_eq!(exec_state(pool, parent).await, "FAILED");
    assert_eq!(run(&runs_of(pool, parent).await, "subflow").3, "FAILED");
    let child: Uuid =
        sqlx::query_scalar("SELECT id FROM workflow.executions WHERE trigger_execution_id = $1")
            .bind(parent)
            .fetch_one(pool)
            .await
            .expect("the child");
    assert_eq!(exec_state(pool, child).await, "FAILED");
    (parent, child)
}

/// Spec D11 (Kestra `ChangeStateTestCase:43-68`, whose FIXME records it):
/// a child's status changed while its parent's `Subflow` run is terminal
/// leaves the parent alone.
#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_childs_status_change_leaves_its_parent_alone(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    let reg = Flaky::new(&[("t1", 1)]);
    let (parent, child) = failed_parent_and_child(&pool, &db, ws, user, &reg).await;
    let parent_before = (exec_meta(&pool, parent).await, runs_of(&pool, parent).await);

    // Act
    let out = status(&pool, child, RunState::Success).await;
    drive(&pool, &reg, child).await;

    // Assert
    assert_eq!(out, Ok(()));
    assert_eq!(exec_state(&pool, child).await, "SUCCESS");
    assert_eq!(
        (exec_meta(&pool, parent).await, runs_of(&pool, parent).await),
        parent_before,
        "the parent is unchanged"
    );
}

/// The same through a child task run's state (Kestra's own test): the child
/// runs on to SUCCESS; the parent is unchanged.
#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_childs_task_run_marked_leaves_its_parent_alone(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    let reg = Flaky::new(&[("t1", 1)]);
    let (parent, child) = failed_parent_and_child(&pool, &db, ws, user, &reg).await;
    let parent_before = (exec_meta(&pool, parent).await, runs_of(&pool, parent).await);
    let t1 = run(&runs_of(&pool, child).await, "t1").2;

    // Act
    let out = mark(&pool, child, t1, RunState::Success).await;
    drive(&pool, &reg, child).await;

    // Assert
    assert_eq!(out, Ok(()));
    assert_eq!(exec_state(&pool, child).await, "SUCCESS");
    assert_eq!(runs_of(&pool, child).await.len(), 2);
    assert_eq!(
        (exec_meta(&pool, parent).await, runs_of(&pool, parent).await),
        parent_before,
        "the parent is unchanged"
    );
}
