//! Restart an execution in place, through the worker entry point (workflow
//! package 7a, spec `docs/en/specs/2026-11-13-workflow-execution-ops-restart-design.md`
//! D1–D7, D10; Kestra `RestartCaseTest`, `ExecutionServiceTest.restartSimple`).
//!
//! These tests drive the real worker with a registry that fails each named
//! task a given number of times and then succeeds, so a run can fail and
//! then pass after the restart.
//!
//! Every test is `#[sqlx::test]`: the worker's `claim` and `tick` are global,
//! so each test owns its database.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5465/egret_test
//!   cargo test -p egret-workflow --test restart

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use egret_metastore::Db;
use egret_workflow::worker::{
    restart_execution, Outcome, Registry, RestartRefused, Restarted, TaskInput,
};
use serde_json::{json, Value};
use uuid::Uuid;

mod common;

const LOG: &str = "io.kestra.plugin.core.log.Log";
const RETURN: &str = "io.kestra.plugin.core.debug.Return";

// ── The registry ────────────────────────────────────────────────────────────

/// Fails each task named in `fails` that many times, then answers like
/// `inner` (every task `SUCCESS` when there is none). Records every run it
/// was handed as `(task_id, attempt)`.
struct Flaky {
    fails: Mutex<HashMap<String, usize>>,
    /// Tasks that fail on their first attempt and pass on any later one
    /// (Kestra's `restart-loop`: `{{ 1 / taskrun.attemptsCount }}`).
    first_attempt: Vec<String>,
    seen: Mutex<Vec<(String, usize)>>,
    inner: Option<egret_workflow::worker::Handlers>,
}

impl Flaky {
    fn new(fails: &[(&str, usize)]) -> Self {
        Self {
            fails: Mutex::new(fails.iter().map(|(t, n)| (t.to_string(), *n)).collect()),
            first_attempt: Vec::new(),
            seen: Mutex::new(Vec::new()),
            inner: None,
        }
    }

    /// Each of `tasks` fails on attempt 1 of its run and passes after.
    fn failing_first_attempt_of(mut self, tasks: &[&str]) -> Self {
        self.first_attempt = tasks.iter().map(|t| t.to_string()).collect();
        self
    }

    fn over(mut self, inner: egret_workflow::worker::Handlers) -> Self {
        self.inner = Some(inner);
        self
    }

    /// The attempts this registry was handed for `task`, in order.
    fn attempts_of(&self, task: &str) -> Vec<usize> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(t, _)| t == task)
            .map(|(_, a)| *a)
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
        let fail = (task.attempt == 1 && self.first_attempt.contains(&task.task_id)) || {
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

async fn exec_history(pool: &sqlx::PgPool, id: Uuid) -> Value {
    sqlx::query_scalar("SELECT history FROM workflow.executions WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("execution history")
}

/// What a refused restart must leave alone: `(history, attempt_number, labels)`.
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

fn attempts_len(r: &RunRow) -> usize {
    r.5.as_array().map_or(0, Vec::len)
}

/// Worker ticks with `reg` until `ex` is terminal and a tick finds nothing
/// more to do (an `afterExecution:` phase runs after the terminal state).
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

async fn restart(pool: &sqlx::PgPool, ex: Uuid) -> Restarted {
    restart_execution(pool, ex).await.expect("restart")
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

/// `a`, `b`, `c` in sequence.
fn three(id: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n{}",
        tasks(&["a", "b", "c"])
    )
}

// ── 1. restartSimple ───────────────────────────────────────────────────────

/// Kestra `ExecutionServiceTest.restartSimple` (:73-91): the same execution,
/// the failed run reset under the same task-run id, `RESTARTED RUNNING`
/// appended, the label set and `attempt_number` 2.
#[sqlx::test(migrations = "../../migrations")]
async fn restart_simple_keeps_the_ids_and_appends_restarted_running(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("b", 1)]);
    let ex = start(&pool, ws, &three("simple")).await;
    drive(&pool, &reg, ex).await;
    assert_eq!(exec_state(&pool, ex).await, "FAILED");
    let failed = runs_of(&pool, ex).await;
    let b_before = run(&failed, "b").clone();
    assert_eq!(b_before.3, "FAILED");

    // Act
    let out = restart(&pool, ex).await;

    // Assert
    assert_eq!(out, Restarted::Done);
    let h = exec_history(&pool, ex).await;
    assert_eq!(
        history_tail(&h, 3),
        ["FAILED", "RESTARTED", "RUNNING"],
        "{h}"
    );
    let (_, attempt_number, labels) = exec_meta(&pool, ex).await;
    assert_eq!(attempt_number, 2);
    assert!(has_restarted_label(&labels), "{labels:?}");
    let after = runs_of(&pool, ex).await;
    let b = run(&after, "b");
    assert_eq!(b.2, b_before.2, "the reset run keeps its task-run id");
    assert_ne!(b.3, "FAILED", "the failed run is reset: {b:?}");
    assert_eq!(b.6, None, "a reset run's outputs are cleared");
    assert_eq!(
        run(&after, "a").2,
        run(&failed, "a").2,
        "a SUCCESS run is kept, id and all"
    );
    assert_eq!(run(&after, "a").4, run(&failed, "a").4, "and its history");
}

// ── 2. restartFailedThenSuccess ────────────────────────────────────────────

/// Kestra `RestartCaseTest.restartFailedThenSuccess` (:38-71): runs 3 → 4,
/// the failed run ends `SUCCESS` with 2 attempts, the execution `SUCCESS`.
#[sqlx::test(migrations = "../../migrations")]
async fn restart_failed_then_success_runs_on_to_the_end(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("c", 1)]);
    let source = format!(
        "id: then-ok\nnamespace: t\ntasks:\n{}",
        tasks(&["a", "b", "c", "d"])
    );
    let ex = start(&pool, ws, &source).await;
    drive(&pool, &reg, ex).await;
    assert_eq!(exec_state(&pool, ex).await, "FAILED");
    assert_eq!(runs_of(&pool, ex).await.len(), 3);

    // Act
    assert_eq!(restart(&pool, ex).await, Restarted::Done);
    drive(&pool, &reg, ex).await;

    // Assert
    assert_eq!(exec_state(&pool, ex).await, "SUCCESS");
    let runs = runs_of(&pool, ex).await;
    assert_eq!(runs.len(), 4, "{runs:?}");
    assert!(runs.iter().all(|r| r.3 == "SUCCESS"), "{runs:?}");
    assert_eq!(attempts_len(run(&runs, "c")), 2, "{:?}", run(&runs, "c"));
    let h = exec_history(&pool, ex).await;
    assert_eq!(
        h,
        json!([
            "CREATED",
            "RUNNING",
            "FAILED",
            "RESTARTED",
            "RUNNING",
            "SUCCESS"
        ])
    );
}

// ── 3. End hooks: errors (global and local), finally, afterExecution ─────

/// A flow with every kind of end hook around one flaky task `b`.
fn hooks(id: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n\
         \x20   tasks:\n      - id: b\n        type: {LOG}\n        message: b\n\
         \x20   errors:\n      - id: local_err\n        type: {LOG}\n        message: local\n\
         errors:\n  - id: global_err\n    type: {LOG}\n    message: global\n\
         finally:\n  - id: fin\n    type: {LOG}\n    message: fin\n\
         afterExecution:\n  - id: after\n    type: {LOG}\n    message: after\n"
    )
}

/// Seed one log line under a run's task-run id, as its worker would have.
async fn log_under(pool: &sqlx::PgPool, ws: Uuid, ex: Uuid, r: &RunRow) {
    sqlx::query(
        "INSERT INTO workflow.task_logs \
           (workspace_id, execution_id, task_run_id, task_id, attempt, level, ts, message) \
         VALUES ($1, $2, $3, $4, 0, 2, now(), 'first time')",
    )
    .bind(ws)
    .bind(ex)
    .bind(r.2)
    .bind(&r.1)
    .execute(pool)
    .await
    .expect("seed a log line");
}

/// Spec D4 (Kestra `restartFailedWithGlobalErrors` / `…LocalErrors` /
/// `restartFailedWithFinally` / `…WithAfterExecution`, :73-137, :397-461):
/// every end-hook run is emptied, gets a new task-run id, and runs again on
/// the new failure; its old id still owns its old log lines.
#[sqlx::test(migrations = "../../migrations")]
async fn end_hook_runs_are_emptied_renewed_and_run_again(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("b", 2)]);
    let ex = start(&pool, ws, &hooks("hooks")).await;
    drive(&pool, &reg, ex).await;
    assert_eq!(exec_state(&pool, ex).await, "FAILED");
    let first = runs_of(&pool, ex).await;
    let hook_ids = ["local_err", "global_err", "fin", "after"];
    for h in hook_ids {
        assert_eq!(
            run(&first, h).3,
            "SUCCESS",
            "{h} ran the first time: {first:?}"
        );
        log_under(&pool, ws, ex, run(&first, h)).await;
    }

    // Act
    assert_eq!(restart(&pool, ex).await, Restarted::Done);
    drive(&pool, &reg, ex).await;

    // Assert
    assert_eq!(exec_state(&pool, ex).await, "FAILED", "b failed again");
    let second = runs_of(&pool, ex).await;
    assert_eq!(
        second.len(),
        first.len(),
        "rows are emptied, never added: {second:?}"
    );
    for h in hook_ids {
        let (old, new) = (run(&first, h), run(&second, h));
        assert_eq!(new.0, old.0, "{h} keeps its run_ix");
        assert_ne!(new.2, old.2, "{h} is a new task run with a new id");
        assert_eq!(new.3, "SUCCESS", "{h} ran again: {new:?}");
        assert_eq!(
            new.4,
            json!(["CREATED", "SUBMITTED", "RUNNING", "SUCCESS"]),
            "{h} was built back from empty: {new:?}"
        );
        let owners: Vec<Option<Uuid>> = sqlx::query_scalar(
            "SELECT task_run_id FROM workflow.task_logs WHERE execution_id = $1 AND task_id = $2 \
               AND message = 'first time'",
        )
        .bind(ex)
        .bind(h)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            owners,
            vec![Some(old.2)],
            "{h}'s old log stays with its old id"
        );
    }
    assert_eq!(
        run(&second, "b").2,
        run(&first, "b").2,
        "b is reset, not renewed"
    );
    assert_eq!(attempts_len(run(&second, "b")), 2);
}

/// **Measured, not assumed:** an end-hook run the restarted execution never
/// reaches again (`errors:` after a success) stays emptied. Does the
/// execution still come to rest as idle, so a later restart, the pruner and
/// the container sweep see it as ended?
#[sqlx::test(migrations = "../../migrations")]
async fn an_execution_that_succeeds_after_restart_is_idle(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("b", 1)]);
    let ex = start(&pool, ws, &hooks("idle")).await;
    drive(&pool, &reg, ex).await;

    // Act
    assert_eq!(restart(&pool, ex).await, Restarted::Done);
    drive(&pool, &reg, ex).await;

    // Assert
    assert_eq!(exec_state(&pool, ex).await, "SUCCESS");
    let idle: Option<bool> = sqlx::query_scalar("SELECT workflow.execution_is_idle($1)")
        .bind(ex)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        idle,
        Some(true),
        "the execution is idle: {:?}",
        runs_of(&pool, ex).await
    );
}

// ── 4. restartMultiple ─────────────────────────────────────────────────────

/// Kestra `restartMultiple` (:313-337): restart, fail, restart, succeed.
#[sqlx::test(migrations = "../../migrations")]
async fn restart_multiple_times(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("b", 2)]);
    let ex = start(&pool, ws, &three("multiple")).await;
    drive(&pool, &reg, ex).await;

    // Act + Assert
    assert_eq!(restart(&pool, ex).await, Restarted::Done);
    drive(&pool, &reg, ex).await;
    assert_eq!(exec_state(&pool, ex).await, "FAILED");
    assert_eq!(restart(&pool, ex).await, Restarted::Done);
    drive(&pool, &reg, ex).await;
    assert_eq!(exec_state(&pool, ex).await, "SUCCESS");

    let (h, attempt_number, labels) = exec_meta(&pool, ex).await;
    assert_eq!(
        h,
        json!([
            "CREATED",
            "RUNNING",
            "FAILED",
            "RESTARTED",
            "RUNNING",
            "FAILED",
            "RESTARTED",
            "RUNNING",
            "SUCCESS"
        ])
    );
    assert_eq!(attempt_number, 3);
    let restarted = labels.as_ref().and_then(Value::as_array).map_or(0, |ls| {
        ls.iter().filter(|l| l["key"] == "system.restarted").count()
    });
    assert_eq!(restarted, 1, "the label is added once: {labels:?}");
    let runs = runs_of(&pool, ex).await;
    assert_eq!(runs.len(), 3);
    assert_eq!(attempts_len(run(&runs, "b")), 3);
}

// ── 5. Parallel ─────────────────────────────────────────────────────────────

/// Spec D2: every failed run is reset, so a `Parallel` whose two branches
/// failed passes after the restart instead of failing them again at once.
/// Only `p1` fails on its own; `p2` is failed by the `Parallel`'s rule 1
/// (`flowable/parallel.rs`) while it is live -- the case D2 names.
#[sqlx::test(migrations = "../../migrations")]
async fn a_parallel_with_two_failed_branches_ends_success(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("p1", 1)]);
    let source = format!(
        "id: par\nnamespace: t\ntasks:\n  - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n\
         \x20   tasks:\n      - id: p1\n        type: {LOG}\n        message: p1\n\
         \x20     - id: p2\n        type: {LOG}\n        message: p2\n"
    );
    let ex = start(&pool, ws, &source).await;
    drive(&pool, &reg, ex).await;
    assert_eq!(exec_state(&pool, ex).await, "FAILED");
    let first = runs_of(&pool, ex).await;
    assert_eq!(run(&first, "p1").3, "FAILED", "{first:?}");
    assert_eq!(run(&first, "p2").3, "FAILED", "{first:?}");

    // Act
    assert_eq!(restart(&pool, ex).await, Restarted::Done);
    drive(&pool, &reg, ex).await;

    // Assert
    let runs = runs_of(&pool, ex).await;
    assert_eq!(exec_state(&pool, ex).await, "SUCCESS", "{runs:?}");
    assert!(runs.iter().all(|r| r.3 == "SUCCESS"), "{runs:?}");
    assert_eq!(runs.len(), first.len());
}

// ── 6. WorkingDirectory ────────────────────────────────────────────────────

/// Spec D3: a reset `WorkingDirectory` re-runs whole in a fresh directory.
/// Its children are emptied (new ids) and its `working_dir` phase starts
/// again: the prepare claim writes a new snapshot pointer. No Docker, as
/// `working_dir_claims.rs`: the children are `Return`.
#[sqlx::test(migrations = "../../migrations")]
async fn a_working_directory_re_runs_in_a_new_directory(pool: sqlx::PgPool) {
    // Arrange
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
    let block_row = |pool: sqlx::PgPool| async move {
        sqlx::query_as::<_, (String, Option<String>, Option<String>)>(
            "SELECT state, working_dir, working_dir_snapshot FROM workflow.task_runs \
              WHERE execution_id = $1 AND task_id = 'block'",
        )
        .bind(ex)
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    let (_, _, first_snapshot) = block_row(pool.clone()).await;
    let first = runs_of(&pool, ex).await;

    // Act
    assert_eq!(restart(&pool, ex).await, Restarted::Done);
    // The restart's own settle opens the block again from `CREATED`: its
    // phase starts over at the prepare claim, not at the old `FINISHED`.
    // (The snapshot pointer is overwritten by the new prepare claim, as for a
    // block a `LoopUntil` restarts; children read it only once `PREPARED`.)
    let (state, phase, _) = block_row(pool.clone()).await;
    assert_eq!(
        (state.as_str(), phase.as_deref()),
        ("RUNNING", Some("PREPARING")),
        "the block is prepared again"
    );
    drive(&pool, &reg, ex).await;

    // Assert
    assert_eq!(exec_state(&pool, ex).await, "SUCCESS");
    let (state, phase, snapshot) = block_row(pool.clone()).await;
    assert_eq!(state, "SUCCESS");
    assert_eq!(phase.as_deref(), Some("FINISHED"));
    assert!(snapshot.is_some(), "the prepare claim wrote a new snapshot");
    assert!(first_snapshot.is_some());
    let second = runs_of(&pool, ex).await;
    for child in ["a", "b"] {
        assert_ne!(
            run(&second, child).2,
            run(&first, child).2,
            "{child} is emptied and renewed"
        );
        assert_eq!(run(&second, child).3, "SUCCESS");
    }
    assert_eq!(
        run(&second, "block").2,
        run(&first, "block").2,
        "the block is reset in place"
    );
}

// ── 7. A kept container name is not reused (D5) ────────────────────────────

/// Spec D5: attempts are kept and appended to, so the new attempt the worker
/// is handed has the next index, and `egret-run-{run_id}-{attempt}` names a
/// new container.
#[sqlx::test(migrations = "../../migrations")]
async fn the_restarted_attempt_has_a_new_index(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("b", 1)]);
    let ex = start(&pool, ws, &three("attempt")).await;
    drive(&pool, &reg, ex).await;
    let before = attempts_len(run(&runs_of(&pool, ex).await, "b"));

    // Act
    assert_eq!(restart(&pool, ex).await, Restarted::Done);
    drive(&pool, &reg, ex).await;

    // Assert
    let handed = reg.attempts_of("b");
    assert_eq!(handed.len(), 2, "{handed:?}");
    assert!(handed[1] > handed[0], "{handed:?}");
    // `TaskInput::attempt` is the run's attempt count once the attempt is
    // open (`jsonb_array_length(attempts)` at the claim): the old attempts
    // are kept, so the new one is `before + 1`, never the first one's again.
    assert_eq!(
        handed[1],
        before + 1,
        "the old attempts are kept and appended to"
    );
}

// ── 8. Flow-trigger outbox ─────────────────────────────────────────────────

/// Spec D7: one Flow-trigger event each for `RESTARTED`, `RUNNING` and the
/// new terminal state.
#[sqlx::test(migrations = "../../migrations")]
async fn flow_trigger_events_are_written_for_each_appended_state(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    common::seed_flow(
        &db,
        ws,
        user,
        "id: listener\nnamespace: t\ntriggers:\n  - id: on_any\n    \
         type: io.kestra.plugin.core.trigger.Flow\n    \
         states: [RESTARTED, RUNNING, SUCCESS, FAILED]\n\
         tasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n",
        "draft",
    )
    .await;
    let reg = Flaky::new(&[("b", 1)]);
    let ex = start(&pool, ws, &three("outbox")).await;
    drive(&pool, &reg, ex).await;
    let mine = |all: Vec<(Uuid, String)>| -> Vec<String> {
        all.into_iter()
            .filter(|(id, _)| *id == ex)
            .map(|(_, s)| s)
            .collect()
    };
    let first = mine(common::events(&db).await);
    assert_eq!(first, ["RUNNING", "FAILED"]);

    // Act
    assert_eq!(restart(&pool, ex).await, Restarted::Done);
    drive(&pool, &reg, ex).await;

    // Assert
    let all = mine(common::events(&db).await);
    assert_eq!(
        all[first.len()..],
        ["RESTARTED", "RUNNING", "SUCCESS"],
        "{all:?}"
    );
}

// ── 9. Refusals write nothing ──────────────────────────────────────────────

async fn assert_refused(pool: &sqlx::PgPool, ex: Uuid, want: RestartRefused) {
    let before = exec_meta(pool, ex).await;
    let runs = runs_of(pool, ex).await;
    let out = restart(pool, ex).await;
    assert_eq!(out, Restarted::Refused(want));
    assert_eq!(
        exec_meta(pool, ex).await,
        before,
        "a refusal writes nothing"
    );
    assert_eq!(runs_of(pool, ex).await, runs, "nor any run");
}

/// Spec D2 (Kestra 409 on a SUCCESS restart).
#[sqlx::test(migrations = "../../migrations")]
async fn a_success_execution_is_refused(pool: sqlx::PgPool) {
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[]);
    let ex = start(&pool, ws, &three("ok")).await;
    drive(&pool, &reg, ex).await;

    assert_refused(&pool, ex, RestartRefused::NotRestartable("SUCCESS".into())).await;
}

/// Kestra refuses a KILLED restart too.
#[sqlx::test(migrations = "../../migrations")]
async fn a_killed_execution_is_refused(pool: sqlx::PgPool) {
    let (_db, ws, _) = setup(&pool).await;
    let ex = start(&pool, ws, &three("killed")).await;
    egret_workflow::worker::kill_execution(&pool, ws, ex, false)
        .await
        .expect("kill");
    let reg = Flaky::new(&[]);
    drive(&pool, &reg, ex).await;
    assert_eq!(exec_state(&pool, ex).await, "KILLED");

    assert_refused(&pool, ex, RestartRefused::NotRestartable("KILLED".into())).await;
}

/// Spec D10: the pruner deleted the files.
#[sqlx::test(migrations = "../../migrations")]
async fn an_execution_whose_files_were_pruned_is_refused(pool: sqlx::PgPool) {
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("b", 1)]);
    let ex = start(&pool, ws, &three("pruned")).await;
    drive(&pool, &reg, ex).await;
    sqlx::query("UPDATE workflow.executions SET files_pruned_at = now() WHERE id = $1")
        .bind(ex)
        .execute(&pool)
        .await
        .unwrap();

    assert_refused(&pool, ex, RestartRefused::FilesPruned).await;
}

/// Spec D10: an `afterExecution:` still runs.
#[sqlx::test(migrations = "../../migrations")]
async fn an_execution_whose_after_execution_runs_is_refused(pool: sqlx::PgPool) {
    let (_db, ws, _) = setup(&pool).await;
    let source = format!(
        "id: live-after\nnamespace: t\ntasks:\n{}afterExecution:\n  - id: end\n    type: {LOG}\n    \
         message: after\n",
        tasks(&["a"])
    );
    let ex = start(&pool, ws, &source).await;
    // Fail `a` by hand; the settle that ends the execution starts `end`.
    let claimed = egret_workflow::worker::claim(&pool, 10).await.unwrap();
    assert_eq!(claimed.len(), 1);
    egret_workflow::worker::commit_outcome(&pool, &claimed[0], &Outcome::failed())
        .await
        .unwrap();
    assert_eq!(exec_state(&pool, ex).await, "FAILED");
    let runs = runs_of(&pool, ex).await;
    assert_eq!(run(&runs, "end").3, "RUNNING", "{runs:?}");

    assert_refused(&pool, ex, RestartRefused::AfterExecutionRunning).await;
}

/// Spec D10: a loop iteration is restarted through its root.
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

    assert_refused(&pool, iteration, RestartRefused::LoopIteration { root }).await;
}

// ── Loop (spec D8) ─────────────────────────────────────────────────────────

/// One iteration row: `(loop_index, id, state, history, attempt_number)`.
type IterRow = (i32, Uuid, String, Value, i32);

async fn iterations(pool: &sqlx::PgPool, root: Uuid) -> Vec<IterRow> {
    sqlx::query_as(
        "SELECT loop_index, id, state, history, attempt_number FROM workflow.executions \
          WHERE parent_execution_id = $1 AND loop_index IS NOT NULL ORDER BY loop_index",
    )
    .bind(root)
    .fetch_all(pool)
    .await
    .expect("iterations")
}

/// The `Loop` run's stored `outputs` (counters and list).
async fn loop_book(pool: &sqlx::PgPool, root: Uuid) -> Value {
    sqlx::query_scalar::<_, Option<Value>>(
        "SELECT outputs FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'loop'",
    )
    .bind(root)
    .fetch_one(pool)
    .await
    .expect("loop run")
    .unwrap_or(Value::Null)
}

/// The iteration indexes of the `Loop` run's list, in list order.
fn listed_iterations(book: &Value) -> Vec<i64> {
    book["outputs"]
        .as_array()
        .map(|l| {
            l.iter()
                .map(|e| e["item"]["iteration"].as_i64().expect("an iteration"))
                .collect()
        })
        .unwrap_or_default()
}

/// Kestra's `restart-loop` (`RestartCaseTest:224-311`): three values, the
/// middle body task failing on each iteration's first attempt.
fn restart_loop(extra: &str) -> String {
    format!(
        "id: restart-loop\nnamespace: t\ntasks:\n  - id: loop\n    \
         type: io.kestra.plugin.core.flow.Loop\n    values: '[\"1\", \"2\", \"3\"]'\n{extra}    \
         tasks:\n{}",
        ["inLoop", "failFirstTime", "logSuccess"]
            .iter()
            .map(|id| format!("      - id: {id}\n        type: {LOG}\n        message: {id}\n"))
            .collect::<String>()
    )
}

/// An output of the iteration's value, which renders whether its body
/// failed or not.
const ITEM_OUTPUT: &str =
    "    outputs:\n      - id: v\n        type: STRING\n        value: \"{{ item.value }}\"\n";

/// **Test 1, Kestra `restartLoop` (`RestartCaseTest:224-311`), and test 4.**
/// Three restarts, each continuing from the failing iteration: the restart
/// restarts iteration k in place (it passes on its second attempt), the
/// `Loop` spawns k+1, which fails on its first. The `Loop` run keeps its id
/// through all three, exactly N iteration rows exist after each (no
/// re-spawn), and an iteration already `SUCCESS` is never touched again.
#[sqlx::test(migrations = "../../migrations")]
async fn restart_loop_continues_from_the_failing_iteration(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[]).failing_first_attempt_of(&["failFirstTime"]);
    let root = start(&pool, ws, &restart_loop("")).await;
    drive(&pool, &reg, root).await;
    assert_eq!(exec_state(&pool, root).await, "FAILED");
    assert_eq!(iterations(&pool, root).await.len(), 1);
    let loop_id = run(&runs_of(&pool, root).await, "loop").2;

    for round in 1..=3usize {
        // Act
        let before = iterations(&pool, root).await;
        let out = restart(&pool, root).await;

        // Assert: restarted in place, the failing iteration with it.
        assert_eq!(out, Restarted::Done, "round {round}");
        let runs = runs_of(&pool, root).await;
        assert_eq!(runs.len(), 1, "round {round}: {runs:?}");
        let lp = run(&runs, "loop");
        assert_eq!(lp.2, loop_id, "round {round}: the Loop run keeps its id");
        assert_eq!(lp.3, "RUNNING", "round {round}: {lp:?}");
        assert_eq!(
            history_tail(&exec_history(&pool, root).await, 3),
            ["FAILED", "RESTARTED", "RUNNING"],
            "round {round}"
        );
        let now = iterations(&pool, root).await;
        assert_eq!(now.len(), round, "round {round}: no iteration is spawned");
        let failing = &now[round - 1];
        assert_eq!(failing.1, before[round - 1].1, "restarted in place");
        assert_eq!(
            history_tail(&failing.3, 3),
            ["FAILED", "RESTARTED", "RUNNING"],
            "round {round}: the last failing iteration is restarted"
        );
        assert_eq!(failing.4, 2, "its attempt_number goes up");
        for done in &now[..round - 1] {
            let was = before.iter().find(|b| b.0 == done.0).unwrap();
            assert_eq!(
                done, was,
                "round {round}: a SUCCESS iteration is not touched"
            );
            assert_eq!(done.2, "SUCCESS");
        }

        drive(&pool, &reg, root).await;
        let want = if round < 3 { "FAILED" } else { "SUCCESS" };
        assert_eq!(exec_state(&pool, root).await, want, "round {round}");
        let after = iterations(&pool, root).await;
        let n = if round < 3 { round + 1 } else { 3 };
        assert_eq!(after.len(), n, "round {round}: exactly N iteration rows");
        assert_eq!(run(&runs_of(&pool, root).await, "loop").2, loop_id);
    }

    let iters = iterations(&pool, root).await;
    assert!(iters.iter().all(|i| i.2 == "SUCCESS"), "{iters:?}");
    let (_, attempt_number, labels) = exec_meta(&pool, root).await;
    assert_eq!(attempt_number, 4);
    assert!(has_restarted_label(&labels));
}

/// **Test 2: the counters and the list after a restarted iteration
/// succeeds** (#6 D3/D4): one entry and one count per index.
#[sqlx::test(migrations = "../../migrations")]
async fn the_counters_and_the_list_after_a_restarted_iteration_succeeds(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[]).failing_first_attempt_of(&["failFirstTime"]);
    let root = start(&pool, ws, &restart_loop(ITEM_OUTPUT)).await;
    drive(&pool, &reg, root).await;

    // Act
    for _ in 0..3 {
        assert_eq!(restart(&pool, root).await, Restarted::Done);
        drive(&pool, &reg, root).await;
    }

    // Assert
    assert_eq!(exec_state(&pool, root).await, "SUCCESS");
    let book = loop_book(&pool, root).await;
    assert_eq!(book["iterationCount"], json!(3), "{book}");
    assert_eq!(book["runningIterations"], json!(0), "{book}");
    assert_eq!(
        book["terminatedIterations"],
        json!({"SUCCESS": 3}),
        "{book}"
    );
    assert_eq!(listed_iterations(&book), [0, 1, 2], "{book}");
    let values: Vec<&str> = book["outputs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["outputs"]["v"].as_str().unwrap())
        .collect();
    assert_eq!(values, ["1", "2", "3"]);
}

/// `Parallel [loop, boom]`: the `Loop` (concurrency 1, `transmitFailed:
/// false`, two values) beside a task that fails.
fn loop_beside_boom(values: &str) -> String {
    format!(
        "id: loop-boom\nnamespace: t\ntasks:\n  - id: par\n    \
         type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      - id: loop\n        \
         type: io.kestra.plugin.core.flow.Loop\n        values: '{values}'\n        \
         concurrencyLimit: 1\n        transmitFailed: false\n        outputs:\n          \
         - id: v\n            type: STRING\n            value: \"{{{{ item.value }}}}\"\n        \
         tasks:\n          - id: body\n            type: {LOG}\n            message: hi\n      \
         - id: boom\n        type: {LOG}\n        message: boom\n"
    )
}

/// Claim what is claimable now and commit `task`'s claim with `outcome`.
async fn commit_one(pool: &sqlx::PgPool, task: &str, outcome: &Outcome) {
    let claimed = egret_workflow::worker::claim(pool, 64)
        .await
        .expect("claim");
    let c = claimed
        .iter()
        .find(|c| c.task_id == task)
        .unwrap_or_else(|| panic!("{task} is not claimable: {claimed:?}"));
    egret_workflow::worker::commit_outcome(pool, c, outcome)
        .await
        .expect("commit");
    // Hand the other leases back, so a later claim takes them again.
    for other in claimed.iter().filter(|c| c.task_id != task) {
        sqlx::query("UPDATE workflow.task_runs SET claimed_at = NULL WHERE id = $1")
            .bind(other.id)
            .execute(pool)
            .await
            .unwrap();
    }
}

/// **The #6 double count (red first).** Iteration 0 FAILS and is counted
/// (`transmitFailed: false`), iteration 1 starts, then `boom` fails and
/// `Parallel` rule 1 fails the live `Loop` run; iteration 1 ends after it,
/// uncounted. The restart restarts iteration 0 (the last failing one),
/// which then succeeds: it is counted once, under its new state, and its
/// list entry is replaced, not appended a second time.
#[sqlx::test(migrations = "../../migrations")]
async fn an_iteration_that_ends_twice_is_counted_and_listed_once(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let root = start(&pool, ws, &loop_beside_boom(r#"["a", "b"]"#)).await;
    commit_one(&pool, "body", &Outcome::failed()).await;
    let book = loop_book(&pool, root).await;
    assert_eq!(book["terminatedIterations"], json!({"FAILED": 1}), "{book}");
    assert_eq!(listed_iterations(&book), [0], "{book}");
    commit_one(&pool, "boom", &Outcome::failed()).await;
    assert_eq!(run(&runs_of(&pool, root).await, "loop").3, "FAILED");
    let reg = Flaky::new(&[]);
    drive(&pool, &reg, root).await;
    assert_eq!(exec_state(&pool, root).await, "FAILED");
    let iters = iterations(&pool, root).await;
    assert_eq!(
        iters.iter().map(|i| i.2.as_str()).collect::<Vec<_>>(),
        ["FAILED", "SUCCESS"]
    );

    // Act
    assert_eq!(restart(&pool, root).await, Restarted::Done);
    drive(&pool, &reg, root).await;

    // Assert
    assert_eq!(exec_state(&pool, root).await, "SUCCESS");
    assert_eq!(iterations(&pool, root).await.len(), 2, "no re-spawn");
    let book = loop_book(&pool, root).await;
    assert_eq!(
        book["terminatedIterations"],
        json!({"SUCCESS": 2}),
        "counted per index, once each: {book}"
    );
    assert_eq!(book["runningIterations"], json!(0), "{book}");
    assert_eq!(
        listed_iterations(&book),
        [0, 1],
        "iteration 0's entry replaced in place, not appended again, and iteration 1's \
         entry, recorded when it ended after the Loop run had failed: {book}"
    );
    let values: Vec<&str> = book["outputs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["outputs"]["v"].as_str().unwrap())
        .collect();
    assert_eq!(values, ["a", "b"]);
}

/// **An iteration that ends after its `Loop` run has already failed, with
/// no restart at all** (A3 fix round 1, option (a)): it is recorded as a
/// live `Loop` would record it -- counted, its entry appended -- and the
/// `Loop` run's state and history are not changed by it.
#[sqlx::test(migrations = "../../migrations")]
async fn an_iteration_ending_after_its_loop_failed_is_recorded_without_moving_the_loop(
    pool: sqlx::PgPool,
) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let root = start(&pool, ws, &loop_beside_boom(r#"["a", "b"]"#)).await;
    commit_one(&pool, "body", &Outcome::failed()).await;
    commit_one(&pool, "boom", &Outcome::failed()).await;
    let failed_loop = run(&runs_of(&pool, root).await, "loop").clone();
    assert_eq!(failed_loop.3, "FAILED");
    let iters = iterations(&pool, root).await;
    assert_eq!(iters.len(), 2);
    assert_ne!(iters[1].2, "SUCCESS", "iteration 1 is still live");

    // Act: iteration 1 ends after its Loop run failed.
    drive(&pool, &Flaky::new(&[]), root).await;

    // Assert
    assert_eq!(exec_state(&pool, root).await, "FAILED");
    assert_eq!(iterations(&pool, root).await[1].2, "SUCCESS");
    let lp = run(&runs_of(&pool, root).await, "loop").clone();
    assert_eq!(lp.3, failed_loop.3, "the Loop run's state is unchanged");
    assert_eq!(lp.4, failed_loop.4, "and its history");
    let book = loop_book(&pool, root).await;
    assert_eq!(
        book["terminatedIterations"],
        json!({"FAILED": 1, "SUCCESS": 1}),
        "{book}"
    );
    assert_eq!(book["runningIterations"], json!(0), "{book}");
    assert_eq!(listed_iterations(&book), [0, 1], "{book}");
    assert_eq!(iterations(&pool, root).await.len(), 2, "nothing is spawned");
}

/// **Test 3: no iteration is restartable.** `boom` fails the live `Loop`
/// run, and its only iteration then ends `SUCCESS`, uncounted. The restart
/// finds no `FAILED`/`PAUSED` iteration: the `Loop` run ends `FAILED` with
/// Kestra's log line on it, its counters are kept, nothing is spawned, and
/// the restart itself still succeeds (`Done`); the execution ends `FAILED`
/// again.
#[sqlx::test(migrations = "../../migrations")]
async fn no_restartable_iteration_fails_the_loop_with_the_log_line(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let root = start(&pool, ws, &loop_beside_boom(r#"["a"]"#)).await;
    commit_one(&pool, "boom", &Outcome::failed()).await;
    assert_eq!(run(&runs_of(&pool, root).await, "loop").3, "FAILED");
    let reg = Flaky::new(&[]);
    drive(&pool, &reg, root).await;
    assert_eq!(exec_state(&pool, root).await, "FAILED");
    let before = iterations(&pool, root).await;
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].2, "SUCCESS");
    let book_before = loop_book(&pool, root).await;

    // Act
    let out = restart(&pool, root).await;

    // Assert
    assert_eq!(out, Restarted::Done);
    let runs = runs_of(&pool, root).await;
    let lp = run(&runs, "loop");
    assert_eq!(lp.3, "FAILED", "{lp:?}");
    assert_eq!(
        history_tail(&lp.4, 3),
        ["FAILED", "RUNNING", "FAILED"],
        "reopened, then failed: {lp:?}"
    );
    assert_eq!(
        lp.6.clone().unwrap_or(Value::Null),
        book_before,
        "the counters are kept"
    );
    let lines: Vec<String> = sqlx::query_scalar(
        "SELECT message FROM workflow.task_logs WHERE execution_id = $1 AND task_run_id = $2",
    )
    .bind(root)
    .bind(lp.2)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(
        lines
            .iter()
            .any(|l| l == "No restartable loop sub-execution found"),
        "{lines:?}"
    );
    drive(&pool, &reg, root).await;
    assert_eq!(exec_state(&pool, root).await, "FAILED");
    assert_eq!(
        run(&runs_of(&pool, root).await, "boom").3,
        "FAILED",
        "Parallel rule 1 fails `boom` again beside the FAILED Loop run"
    );
    assert_eq!(
        iterations(&pool, root).await,
        before,
        "the iteration is not touched"
    );
}

// ── Subflow (spec D9, Q1) ──────────────────────────────────────────────────

/// Kestra's `restart-child` (`RestartCaseTest:339-395`): its middle task
/// fails on its first attempt and passes after.
fn restart_child() -> String {
    format!(
        "id: restart-child\nnamespace: t\ntasks:\n{}",
        tasks(&["helloSubflow1", "failFirstTime", "helloSubflow2"])
    )
}

/// The `Subflow` task, indented under `pad`.
fn call_subflow(pad: &str) -> String {
    format!(
        "{pad}- id: callSubflow\n{pad}  type: io.kestra.plugin.core.flow.Subflow\n{pad}  \
         namespace: t\n{pad}  flowId: restart-child\n{pad}  wait: true\n{pad}  \
         transmitFailed: true\n"
    )
}

/// Kestra's `restart-parent`.
fn restart_parent() -> String {
    format!(
        "id: restart-parent\nnamespace: t\ntasks:\n{}{}{}",
        tasks(&["hello"]),
        call_subflow("  "),
        tasks(&["helloParent2"])
    )
}

async fn seed_restart_child(db: &Db, ws: Uuid, user: egret_core::UserId) {
    common::seed_flow(db, ws, user, &restart_child(), "draft").await;
}

/// One child row: `(id, state, history, attempt_number, labels,
/// trigger_execution_id, trigger_run_ix)`.
type ChildRow = (
    Uuid,
    String,
    Value,
    i32,
    Option<Value>,
    Option<Uuid>,
    Option<i32>,
);

/// Every execution of `restart-child`, oldest first.
async fn children(pool: &sqlx::PgPool) -> Vec<ChildRow> {
    sqlx::query_as(
        "SELECT id, state, history, attempt_number, labels, trigger_execution_id, trigger_run_ix \
           FROM workflow.executions WHERE flow_key = 'restart-child' ORDER BY created_at, id",
    )
    .fetch_all(pool)
    .await
    .expect("children")
}

/// **Test 1, Kestra `restartSubflow` (`RestartCaseTest:339-357`).** The
/// failed child is restarted in place, never replaced: one child row, the
/// same id, `RESTARTED RUNNING` on it, its `attempt_number` 2 and the label;
/// the parent's `Subflow` run keeps its id and waits for it, and takes its
/// report (`on_child_terminal`) because the run is not terminal any more.
#[sqlx::test(migrations = "../../migrations")]
async fn restart_subflow_restarts_the_failed_child_in_place(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    seed_restart_child(&db, ws, user).await;
    let reg = Flaky::new(&[]).failing_first_attempt_of(&["failFirstTime"]);
    let root = start(&pool, ws, &restart_parent()).await;
    drive(&pool, &reg, root).await;
    assert_eq!(exec_state(&pool, root).await, "FAILED");
    assert_eq!(runs_of(&pool, root).await.len(), 2, "hello + callSubflow");
    let kids = children(&pool).await;
    assert_eq!(kids.len(), 1);
    let child = kids[0].0;
    assert_eq!(kids[0].1, "FAILED");
    let sub_id = run(&runs_of(&pool, root).await, "callSubflow").2;

    // Act
    let out = restart(&pool, root).await;

    // Assert: the child is restarted in place, nothing is spawned.
    assert_eq!(out, Restarted::Done);
    let kids = children(&pool).await;
    assert_eq!(kids.len(), 1, "no child is spawned: {kids:?}");
    assert_eq!(kids[0].0, child, "the same child");
    assert_eq!(
        history_tail(&kids[0].2, 3),
        ["FAILED", "RESTARTED", "RUNNING"]
    );
    assert_eq!(kids[0].3, 2, "the child's attempt_number goes up");
    assert!(has_restarted_label(&kids[0].4));
    let sub = run(&runs_of(&pool, root).await, "callSubflow").clone();
    assert_eq!(sub.2, sub_id, "the Subflow run keeps its id");
    assert_eq!(sub.3, "RUNNING", "it waits for the child: {sub:?}");

    drive(&pool, &reg, root).await;
    assert_eq!(exec_state(&pool, root).await, "SUCCESS");
    let kids = children(&pool).await;
    assert_eq!(kids.len(), 1, "never replaced: {kids:?}");
    assert_eq!(kids[0].0, child);
    assert_eq!(kids[0].1, "SUCCESS");
    let runs = runs_of(&pool, root).await;
    assert_eq!(runs.len(), 3, "hello + callSubflow + helloParent2");
    let sub = run(&runs, "callSubflow");
    assert_eq!(sub.3, "SUCCESS");
    let outputs = sub.6.as_ref().expect("the child's report");
    assert_eq!(outputs["executionId"], child.to_string());
    assert_eq!(outputs["state"], "SUCCESS");
    assert_eq!(reg.attempts_of("failFirstTime"), vec![1, 2]);
}

/// **Test 2, Kestra `restartSubflowWithLoop` (`RestartCaseTest:359-395`).**
/// A `Loop` of three values over the `Subflow`: each restart restarts the
/// last failing iteration (D8), whose `Subflow` restarts that iteration's
/// child (D9). Three restarts and exactly three children, all `SUCCESS`.
#[sqlx::test(migrations = "../../migrations")]
async fn restart_subflow_with_loop(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    seed_restart_child(&db, ws, user).await;
    let reg = Flaky::new(&[]).failing_first_attempt_of(&["failFirstTime"]);
    let source = format!(
        "id: restart-parent-loop\nnamespace: t\ntasks:\n{}  - id: each\n    \
         type: io.kestra.plugin.core.flow.Loop\n    values: '[\"1\", \"2\", \"3\"]'\n    \
         tasks:\n{}{}",
        tasks(&["hello"]),
        call_subflow("      "),
        tasks(&["helloParent2"])
    );
    let root = start(&pool, ws, &source).await;
    drive(&pool, &reg, root).await;
    assert_eq!(exec_state(&pool, root).await, "FAILED");
    assert_eq!(children(&pool).await.len(), 1);

    for round in 1..=3usize {
        // Act
        let before = children(&pool).await;
        let out = restart(&pool, root).await;

        // Assert
        assert_eq!(out, Restarted::Done, "round {round}");
        let now = children(&pool).await;
        assert_eq!(now.len(), round, "round {round}: no child is spawned");
        assert_eq!(now[round - 1].0, before[round - 1].0, "round {round}");
        assert_eq!(
            history_tail(&now[round - 1].2, 3),
            ["FAILED", "RESTARTED", "RUNNING"],
            "round {round}: the failing child is restarted in place"
        );

        drive(&pool, &reg, root).await;
        let want = if round < 3 { "FAILED" } else { "SUCCESS" };
        assert_eq!(exec_state(&pool, root).await, want, "round {round}");
        let n = if round < 3 { round + 1 } else { 3 };
        assert_eq!(children(&pool).await.len(), n, "round {round}");
    }

    let kids = children(&pool).await;
    assert_eq!(kids.len(), 3, "exactly three children");
    assert!(kids.iter().all(|k| k.1 == "SUCCESS"), "{kids:?}");
    assert!(
        kids.iter().all(|k| k.3 == 2),
        "each restarted once: {kids:?}"
    );
    assert_eq!(
        runs_of(&pool, root).await.len(),
        3,
        "hello + each + helloParent2"
    );
}

/// Write `state` onto `task`'s run and its last attempt, and onto the
/// execution, as `set_task_run_state` (A5, D11) would leave them.
async fn mark_run(pool: &sqlx::PgPool, ex: Uuid, task: &str, state: &str) {
    sqlx::query(
        "UPDATE workflow.task_runs SET state = $3, history = history || jsonb_build_array($3::text), \
                attempts = jsonb_set(attempts, \
                    ARRAY[(jsonb_array_length(attempts) - 1)::text, 'history'], \
                    (attempts -> -1 -> 'history') || jsonb_build_array($3::text)) \
          WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(ex)
    .bind(task)
    .bind(state)
    .execute(pool)
    .await
    .expect("mark the run");
}

/// **Test 3 (Q1, Kestra `ExS:1311-1318`: "don't restart it as it's
/// terminated successfully").** The child ended `SUCCESS` but its `Subflow`
/// run is `FAILED` (as a `set_task_run_state` leaves it), and the parent
/// failed elsewhere too. The restart ends the `Subflow` run `SUCCESS` at
/// once with the child's outputs copied, and no child is restarted or
/// spawned.
#[sqlx::test(migrations = "../../migrations")]
async fn a_successful_child_is_not_restarted_and_its_run_ends_success(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    seed_restart_child(&db, ws, user).await;
    let reg = Flaky::new(&[("boom", 1)]);
    let source = format!(
        "id: restart-parent\nnamespace: t\ntasks:\n{}{}",
        call_subflow("  "),
        tasks(&["boom"])
    );
    let root = start(&pool, ws, &source).await;
    drive(&pool, &reg, root).await;
    assert_eq!(exec_state(&pool, root).await, "FAILED");
    let kids = children(&pool).await;
    assert_eq!(kids.len(), 1);
    assert_eq!(kids[0].1, "SUCCESS");
    mark_run(&pool, root, "callSubflow", "FAILED").await;

    // Act
    let out = restart(&pool, root).await;

    // Assert
    assert_eq!(out, Restarted::Done);
    assert_eq!(children(&pool).await, kids, "the child is not touched");
    let sub = run(&runs_of(&pool, root).await, "callSubflow").clone();
    assert_eq!(
        history_tail(&sub.4, 3),
        ["CREATED", "RUNNING", "SUCCESS"],
        "{sub:?}"
    );
    let outputs = sub.6.as_ref().expect("the child's outputs, copied");
    assert_eq!(outputs["executionId"], kids[0].0.to_string());
    assert_eq!(outputs["state"], "SUCCESS");

    drive(&pool, &reg, root).await;
    assert_eq!(exec_state(&pool, root).await, "SUCCESS");
    assert_eq!(children(&pool).await, kids, "nothing restarted or spawned");
}

/// **Test 4: two children match**, so neither is restarted and a new child
/// is spawned, as before D9 (Kestra: "if there are more than one, we ignore
/// the results and create a new one").
#[sqlx::test(migrations = "../../migrations")]
async fn two_matching_children_spawn_a_new_one(pool: sqlx::PgPool) {
    // Arrange: the child's run fails twice in all, then passes.
    let (db, ws, user) = setup(&pool).await;
    seed_restart_child(&db, ws, user).await;
    let reg = Flaky::new(&[("failFirstTime", 2)]);
    let root = start(&pool, ws, &restart_parent()).await;
    drive(&pool, &reg, root).await;
    assert_eq!(exec_state(&pool, root).await, "FAILED");
    let sub_ix = run(&runs_of(&pool, root).await, "callSubflow").0;
    let other = start(&pool, ws, &restart_child()).await;
    drive(&pool, &reg, other).await;
    assert_eq!(exec_state(&pool, other).await, "FAILED");
    sqlx::query(
        "UPDATE workflow.executions SET trigger_execution_id = $1, trigger_run_ix = $2 \
          WHERE id = $3",
    )
    .bind(root)
    .bind(sub_ix)
    .bind(other)
    .execute(&pool)
    .await
    .expect("link a second child");
    let before = children(&pool).await;
    assert_eq!(before.len(), 2);

    // Act
    let out = restart(&pool, root).await;
    drive(&pool, &reg, root).await;

    // Assert
    assert_eq!(out, Restarted::Done);
    assert_eq!(exec_state(&pool, root).await, "SUCCESS");
    let after = children(&pool).await;
    assert_eq!(after.len(), 3, "a new child: {after:?}");
    for old in &before {
        let now = after.iter().find(|k| k.0 == old.0).unwrap();
        assert_eq!(now, old, "an earlier child is not touched");
    }
    let new = after
        .iter()
        .find(|k| !before.iter().any(|b| b.0 == k.0))
        .unwrap();
    assert_eq!(new.1, "SUCCESS");
    assert_eq!(new.3, 1);
    let sub = run(&runs_of(&pool, root).await, "callSubflow").clone();
    assert_eq!(sub.6.as_ref().unwrap()["executionId"], new.0.to_string());
}

/// **A child whose own restart is refused** (here: its files were pruned,
/// D10) is left alone, and a new child is spawned as before D9.
#[sqlx::test(migrations = "../../migrations")]
async fn a_child_whose_restart_is_refused_is_replaced(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    seed_restart_child(&db, ws, user).await;
    let reg = Flaky::new(&[("failFirstTime", 1)]);
    let root = start(&pool, ws, &restart_parent()).await;
    drive(&pool, &reg, root).await;
    assert_eq!(exec_state(&pool, root).await, "FAILED");
    let first = children(&pool).await;
    assert_eq!(first.len(), 1);
    sqlx::query("UPDATE workflow.executions SET files_pruned_at = now() WHERE id = $1")
        .bind(first[0].0)
        .execute(&pool)
        .await
        .unwrap();

    // Act
    let out = restart(&pool, root).await;
    drive(&pool, &reg, root).await;

    // Assert
    assert_eq!(out, Restarted::Done);
    assert_eq!(exec_state(&pool, root).await, "SUCCESS");
    let after = children(&pool).await;
    assert_eq!(after.len(), 2, "{after:?}");
    assert_eq!(after.iter().find(|k| k.0 == first[0].0), Some(&first[0]));
    let new = after.iter().find(|k| k.0 != first[0].0).unwrap();
    assert_eq!(new.1, "SUCCESS");
    let sub = run(&runs_of(&pool, root).await, "callSubflow").clone();
    assert_eq!(sub.6.as_ref().unwrap()["executionId"], new.0.to_string());
}

/// **Only a run the restart reset looks up a child (D9).** In a restarted
/// execution, a `Subflow` run inside a `LoopUntil` is emptied and built
/// again for each iteration (a new task-run id, as Kestra's new taskrun), so
/// iteration 2 finds iteration 1's child under the same `run_ix` and must
/// still spawn its own: two iterations, two children.
#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_run_built_fresh_after_a_restart_spawns_its_own_child(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    seed_restart_child(&db, ws, user).await;
    let reg = Flaky::new(&[("boom", 1)]);
    let source = format!(
        "id: restart-parent-until\nnamespace: t\ntasks:\n{}  - id: until\n    \
         type: io.kestra.plugin.core.flow.LoopUntil\n    \
         condition: \"{{{{ outputs.until.iterationCount >= 2 }}}}\"\n    \
         checkFrequency:\n      interval: PT0S\n    tasks:\n{}",
        tasks(&["boom"]),
        call_subflow("      "),
    );
    let root = start(&pool, ws, &source).await;
    drive(&pool, &reg, root).await;
    assert_eq!(exec_state(&pool, root).await, "FAILED");
    assert!(children(&pool).await.is_empty(), "boom failed first");

    // Act
    assert_eq!(restart(&pool, root).await, Restarted::Done);
    drive(&pool, &reg, root).await;

    // Assert
    assert_eq!(exec_state(&pool, root).await, "SUCCESS");
    let kids = children(&pool).await;
    assert_eq!(kids.len(), 2, "one child per iteration: {kids:?}");
    assert!(
        kids.iter().all(|k| k.1 == "SUCCESS" && k.3 == 1),
        "{kids:?}"
    );
}

/// The deadlocks Postgres has detected in this test's own database (each
/// `#[sqlx::test]` has its own). The victim's backend reports it after it
/// flushes its statistics, so a caller polls.
async fn deadlocks(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT deadlocks FROM pg_stat_database WHERE datname = current_database()")
        .fetch_one(pool)
        .await
        .expect("pg_stat_database")
}

/// **Test 5: a restart that loses a deadlock to the child's own settle is
/// run again, and both finish.** The restart locks the parent, then the
/// child (D9, kill's order). A child's settle that ends it reports to its
/// parent: child, then parent, the reverse. A test transaction plays that
/// settle: it holds the child, waits until the restart blocks on it, then
/// locks the parent. Its `deadlock_timeout` is raised so the restart is the
/// `40P01` victim; `restart_execution` runs it again (kill's retry), and it
/// finishes once the test transaction commits. The positive control is
/// Postgres's own deadlock count: exactly one.
#[sqlx::test(migrations = "../../migrations")]
async fn a_restart_that_loses_a_deadlock_to_the_childs_settle_still_finishes(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    seed_restart_child(&db, ws, user).await;
    let reg = Flaky::new(&[]).failing_first_attempt_of(&["failFirstTime"]);
    let root = start(&pool, ws, &restart_parent()).await;
    drive(&pool, &reg, root).await;
    assert_eq!(exec_state(&pool, root).await, "FAILED");
    let child = children(&pool).await[0].0;
    assert_eq!(deadlocks(&pool).await, 0, "the fixture: no deadlock yet");

    // The child's "settle": hold the child first.
    let mut settle_tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL deadlock_timeout = '60s'")
        .execute(&mut *settle_tx)
        .await
        .unwrap();
    let settle_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *settle_tx)
        .await
        .unwrap();
    sqlx::query("SELECT 1 FROM workflow.executions WHERE id = $1 FOR UPDATE")
        .bind(child)
        .execute(&mut *settle_tx)
        .await
        .unwrap();

    // Act: the restart holds the parent and blocks on the child.
    let restarting = {
        let pool = pool.clone();
        tokio::spawn(async move { restart_execution(&pool, root).await })
    };
    let waited = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let blocked: Option<i32> = sqlx::query_scalar(
                "SELECT pid FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid))",
            )
            .bind(settle_pid)
            .fetch_optional(&pool)
            .await
            .unwrap();
            if blocked.is_some() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "the restart never waited on the child");
    // The "settle" now wants the parent, which the restart holds.
    sqlx::query("SELECT 1 FROM workflow.executions WHERE id = $1 FOR UPDATE")
        .bind(root)
        .execute(&mut *settle_tx)
        .await
        .expect("the settle gets the parent once the restart is aborted");
    settle_tx.commit().await.unwrap();

    // Assert
    let out = tokio::time::timeout(Duration::from_secs(30), restarting)
        .await
        .expect("the restart returns")
        .expect("the restart task");
    assert_eq!(out.expect("the victim is run again"), Restarted::Done);
    let counted = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let n = deadlocks(&pool).await;
            if n > 0 {
                return n;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the positive control: Postgres counted a deadlock");
    assert_eq!(counted, 1, "one deadlock: the restart's first attempt");
    drive(&pool, &reg, root).await;
    assert_eq!(exec_state(&pool, root).await, "SUCCESS");
    let kids = children(&pool).await;
    assert_eq!(kids.len(), 1);
    assert_eq!(kids[0].0, child);
    assert_eq!(kids[0].1, "SUCCESS");
}

// ── 10. A spent retry does not retry again (D5) ────────────────────────────

/// Spec D5's consequence, as Kestra: `retry:` counts all a run's attempts,
/// so a run whose `maxAttempts: 2` was spent fails again after the restart
/// on its first new attempt.
#[sqlx::test(migrations = "../../migrations")]
async fn a_spent_retry_fails_again_without_retrying(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("b", 10)]);
    let source = format!(
        "id: retry\nnamespace: t\ntasks:\n  - id: b\n    type: {LOG}\n    message: b\n    \
         retry:\n      type: constant\n      interval: PT0S\n      maxAttempts: 2\n"
    );
    let ex = start(&pool, ws, &source).await;
    drive(&pool, &reg, ex).await;
    assert_eq!(exec_state(&pool, ex).await, "FAILED");
    let spent = attempts_len(run(&runs_of(&pool, ex).await, "b"));
    assert_eq!(spent, 2, "the retry ran: {:?}", runs_of(&pool, ex).await);

    // Act
    assert_eq!(restart(&pool, ex).await, Restarted::Done);
    drive(&pool, &reg, ex).await;

    // Assert
    assert_eq!(exec_state(&pool, ex).await, "FAILED");
    assert_eq!(
        attempts_len(run(&runs_of(&pool, ex).await, "b")),
        spent + 1,
        "one new attempt, no retry"
    );
}

// ── 11. restartOrReplayLoopUntil (D6) ──────────────────────────────────────

/// Kestra `RestartCaseTest.restartOrReplayLoopUntil` (:463-505), on its flow
/// `loop-until-restart`: a `LoopUntil` that failed on its `maxDuration` waits
/// again from the restart, and its `maxDuration` is measured from the
/// restart's tick, not from the first start.
#[sqlx::test(migrations = "../../migrations")]
async fn a_loop_until_waits_again_from_the_restart(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let source = format!(
        "id: loop-until-restart\nnamespace: t\ntasks:\n  - id: loop_test\n    \
         type: io.kestra.plugin.core.flow.LoopUntil\n    condition: \"{{{{ false }}}}\"\n    \
         failOnMaxReached: true\n    checkFrequency:\n      interval: PT0.2S\n      \
         maxDuration: PT1S\n    tasks:\n      - id: return_test\n        type: {RETURN}\n        \
         format: x\n"
    );
    let ex = start(&pool, ws, &source).await;
    let to_rest = |pool: sqlx::PgPool| async move {
        for _ in 0..80 {
            egret_workflow::worker::tick(&pool, 64, &Flaky::new(&[]))
                .await
                .expect("tick");
            let state = exec_state(&pool, ex).await;
            if matches!(state.as_str(), "SUCCESS" | "FAILED" | "WARNING" | "KILLED") {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("the loop did not end");
    };
    to_rest(pool.clone()).await;
    assert_eq!(exec_state(&pool, ex).await, "FAILED");
    let first_attempts = run(&runs_of(&pool, ex).await, "loop_test").5.clone();

    // Act
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let restarted_at = chrono::Utc::now();
    assert_eq!(restart(&pool, ex).await, Restarted::Done);
    // The restart's settle reopens the loop and its next wait begins at once.
    let loop_run = run(&runs_of(&pool, ex).await, "loop_test").clone();
    assert_eq!(loop_run.3, "PAUSED", "the loop waits again: {loop_run:?}");
    to_rest(pool.clone()).await;

    // Assert
    assert_eq!(exec_state(&pool, ex).await, "FAILED");
    let ended: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT ended_at FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'loop_test'",
    )
    .bind(ex)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        ended - restarted_at >= chrono::Duration::seconds(1),
        "maxDuration is measured from the restart: ended {ended}, restarted {restarted_at}"
    );
    let attempts = run(&runs_of(&pool, ex).await, "loop_test").5.clone();
    let (a0, a1) = (
        first_attempts.as_array().unwrap().len(),
        attempts.as_array().unwrap().len(),
    );
    assert_eq!(
        a1,
        a0 + 1,
        "one new attempt opened at the restart: {attempts}"
    );
}

// ── 12. D7: SLA and flow outputs ───────────────────────────────────────────

/// Spec D7: an `EXECUTION_ASSERTION` SLA is evaluated again while the
/// restarted execution runs. The assertion holds until the execution carries
/// `system.restarted`, so the restart's own settle violates it.
#[sqlx::test(migrations = "../../migrations")]
async fn an_execution_assertion_sla_is_evaluated_again(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("b", 1)]);
    let source = format!(
        "id: sla\nnamespace: t\nsla:\n  - id: not_restarted\n    type: EXECUTION_ASSERTION\n    \
         behavior: CANCEL\n    assert: \"{{{{ (labels['system.restarted'] ?? 'no') == 'no' }}}}\"\n\
         tasks:\n{}",
        tasks(&["a", "b", "c"])
    );
    let ex = start(&pool, ws, &source).await;
    drive(&pool, &reg, ex).await;
    assert_eq!(
        exec_state(&pool, ex).await,
        "FAILED",
        "the SLA held the first time"
    );

    // Act
    assert_eq!(restart(&pool, ex).await, Restarted::Done);

    // Assert
    let h = exec_history(&pool, ex).await;
    assert_eq!(history_tail(&h, 2), ["RESTARTED", "CANCELLED"], "{h}");
}

/// Spec D7: flow `outputs:` keep their old value until the new terminal
/// state, which re-evaluates them.
#[sqlx::test(migrations = "../../migrations")]
async fn flow_outputs_are_evaluated_again_at_the_new_terminal_state(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("b", 1)]);
    let source = format!(
        "id: outs\nnamespace: t\ntasks:\n{}outputs:\n  - id: o\n    type: STRING\n    \
         value: \"{{{{ outputs.b.value ?? 'none' }}}}\"\n",
        tasks(&["a", "b"])
    );
    let ex = start(&pool, ws, &source).await;
    drive(&pool, &reg, ex).await;
    let outputs = |pool: sqlx::PgPool| async move {
        sqlx::query_scalar::<_, Option<Value>>(
            "SELECT outputs FROM workflow.executions WHERE id = $1",
        )
        .bind(ex)
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    assert_eq!(exec_state(&pool, ex).await, "FAILED");
    let first = outputs(pool.clone()).await;
    assert_eq!(first, Some(json!({ "o": "none" })));

    // Act
    assert_eq!(restart(&pool, ex).await, Restarted::Done);
    let while_running = outputs(pool.clone()).await;
    drive(&pool, &reg, ex).await;

    // Assert
    assert_eq!(
        while_running, first,
        "the old value stays visible while it runs"
    );
    assert_eq!(exec_state(&pool, ex).await, "SUCCESS");
    let attempt = reg.attempts_of("b")[1];
    assert_eq!(
        outputs(pool.clone()).await,
        Some(json!({ "o": format!("b-{attempt}") }))
    );
}
