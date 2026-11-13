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

/// **Pins A2's temporary refusal, which A3 (Loop, D8) must flip:** a restart
/// that needs a `Loop` iteration restarted in place is an internal error and
/// writes nothing.
#[sqlx::test(migrations = "../../migrations")]
async fn a_loop_restart_is_not_built_yet(pool: sqlx::PgPool) {
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("body", 1)]);
    let source = format!(
        "id: loop-root\nnamespace: t\ntasks:\n  - id: loop\n    type: io.kestra.plugin.core.flow.Loop\n\
         \x20   values: '[\"1\"]'\n    tasks:\n      - id: body\n        type: {LOG}\n        \
         message: hi\n"
    );
    let root = start(&pool, ws, &source).await;
    drive(&pool, &reg, root).await;
    assert_eq!(exec_state(&pool, root).await, "FAILED");
    let before = exec_meta(&pool, root).await;

    let err = restart_execution(&pool, root)
        .await
        .expect_err("a Loop restart is not built yet");

    assert!(err.to_string().contains("A3 (Loop)"), "{err}");
    assert_eq!(exec_meta(&pool, root).await, before, "nothing written");
}

/// **Pins A2's temporary refusal, which A4 (Subflow, D9) must flip.**
#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_restart_is_not_built_yet(pool: sqlx::PgPool) {
    let (db, ws, user) = setup(&pool).await;
    common::seed_flow(
        &db,
        ws,
        user,
        &format!("id: child\nnamespace: t\ntasks:\n{}", tasks(&["body"])),
        "draft",
    )
    .await;
    let reg = Flaky::new(&[("body", 1)]);
    let source = "id: parent\nnamespace: t\ntasks:\n  - id: sub\n    \
                  type: io.kestra.plugin.core.flow.Subflow\n    namespace: t\n    flowId: child\n";
    let root = start(&pool, ws, source).await;
    drive(&pool, &reg, root).await;
    assert_eq!(exec_state(&pool, root).await, "FAILED");
    let before = exec_meta(&pool, root).await;

    let err = restart_execution(&pool, root)
        .await
        .expect_err("a Subflow restart is not built yet");

    assert!(err.to_string().contains("A4 (Subflow)"), "{err}");
    assert_eq!(exec_meta(&pool, root).await, before, "nothing written");
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
