//! Replay into a new execution -- whole, from a task run, or a restart on
//! another source -- through the worker entry point (workflow package 7b,
//! spec `docs/en/specs/2026-11-14-workflow-replay-from-task-design.md`
//! D1-D10; Kestra `ExecutionService.replay`, `ES:393-512`, and `.restart`
//! with a revision, `ES:281-373`).
//!
//! These tests drive the real worker with a registry that fails each named
//! task a given number of times and then succeeds, so a run can fail and
//! then pass in the copy.
//!
//! Every test is `#[sqlx::test]`: the worker's `claim` and `tick` are global,
//! so each test owns its database.
//!
//! Run with:
//!
//! ```text
//! export DATABASE_URL=postgres://egret:egret@localhost:5465/egret_test
//! cargo test -p egret-workflow --test replay
//! ```

use std::collections::HashMap;
use std::sync::Mutex;

use egret_metastore::Db;
use egret_workflow::executor::MatchFault;
use egret_workflow::worker::{
    Outcome, Registry, ReplayInputs, ReplayKind, ReplayRefused, ReplayRequest, TaskInput,
};
use serde_json::{json, Value};
use uuid::Uuid;

mod common;

const LOG: &str = "io.kestra.plugin.core.log.Log";
const RETURN: &str = "io.kestra.plugin.core.debug.Return";

// ── The registry ────────────────────────────────────────────────────────────

/// One task run the registry was handed.
#[derive(Clone, Debug)]
struct Seen {
    execution: Uuid,
    task: String,
    type_id: String,
    attempt: usize,
    spec: Value,
}

/// Fails each task named in `fails` that many times, then answers like
/// `inner` (every task `SUCCESS` when there is none). Records every run it
/// was handed.
struct Flaky {
    fails: Mutex<HashMap<String, usize>>,
    seen: Mutex<Vec<Seen>>,
    /// Fails the n-th (0-based) run this registry is handed of the task.
    fail_nth: Mutex<HashMap<String, usize>>,
    inner: Option<egret_workflow::worker::Handlers>,
}

impl Flaky {
    fn new(fails: &[(&str, usize)]) -> Self {
        Self {
            fails: Mutex::new(fails.iter().map(|(t, n)| (t.to_string(), *n)).collect()),
            seen: Mutex::new(Vec::new()),
            fail_nth: Mutex::new(HashMap::new()),
            inner: None,
        }
    }

    fn failing_nth_run_of(self, task: &str, nth: usize) -> Self {
        self.fail_nth.lock().unwrap().insert(task.to_string(), nth);
        self
    }

    fn over(mut self, inner: egret_workflow::worker::Handlers) -> Self {
        self.inner = Some(inner);
        self
    }

    /// The attempts this registry was handed for `task`, in order.
    fn attempts_of(&self, task: &str) -> Vec<usize> {
        self.seen_of(task).iter().map(|s| s.attempt).collect()
    }

    /// Every run of `task` this registry was handed, in order.
    fn seen_of(&self, task: &str) -> Vec<Seen> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.task == task)
            .cloned()
            .collect()
    }
}

#[async_trait::async_trait]
impl Registry for Flaky {
    async fn run(&self, task: &TaskInput) -> egret_core::Result<Outcome> {
        let earlier = self.seen_of(&task.task_id).len();
        self.seen.lock().unwrap().push(Seen {
            execution: task.execution_id,
            task: task.task_id.clone(),
            type_id: task.type_id.clone(),
            attempt: task.attempt,
            spec: task.spec.clone(),
        });
        let fail = {
            let mut fails = self.fails.lock().unwrap();
            let nth = self.fail_nth.lock().unwrap().get(&task.task_id).copied();
            match fails.get_mut(&task.task_id) {
                _ if nth == Some(earlier) => true,
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

#[test]
fn database_url_is_exported_rather_than_read_from_dotenv() {
    egret_test_db::assert_database_url_is_exported();
}

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

fn history_tail(h: &Value, n: usize) -> Vec<String> {
    let a = h.as_array().expect("history is an array");
    a[a.len().saturating_sub(n)..]
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect()
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

/// `three(id)` with `c`'s message changed: the edited source.
fn three_edited(id: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n{}  - id: c\n    type: {LOG}\n    message: c edited\n",
        tasks(&["a", "b"])
    )
}

/// The workspace owner: the person every replay here is made by.
async fn owner(pool: &sqlx::PgPool, ws: Uuid) -> Uuid {
    sqlx::query_scalar("SELECT created_by FROM workspaces WHERE id = $1")
        .bind(ws)
        .fetch_one(pool)
        .await
        .expect("owner")
}

async fn try_replay(
    pool: &sqlx::PgPool,
    ws: Uuid,
    from: Uuid,
    source: &str,
    kind: ReplayKind,
    inputs: ReplayInputs,
    started_by: Uuid,
) -> egret_core::Result<Result<Uuid, ReplayRefused>> {
    egret_workflow::worker::replay_execution(
        pool,
        &ReplayRequest {
            workspace_id: ws,
            from,
            source,
            kind,
            inputs,
            started_by,
            id: Uuid::new_v4(),
            input_files: Vec::new(),
        },
    )
    .await
}

async fn replay(
    pool: &sqlx::PgPool,
    ws: Uuid,
    from: Uuid,
    source: &str,
    kind: ReplayKind,
    inputs: ReplayInputs,
) -> Result<Uuid, ReplayRefused> {
    let user = owner(pool, ws).await;
    try_replay(pool, ws, from, source, kind, inputs, user)
        .await
        .expect("replay")
}

/// The frozen source of `ex`.
async fn source_of(pool: &sqlx::PgPool, ex: Uuid) -> String {
    sqlx::query_scalar("SELECT source FROM workflow.executions WHERE id = $1")
        .bind(ex)
        .fetch_one(pool)
        .await
        .expect("source")
}

fn run_id(runs: &[RunRow], task: &str) -> Uuid {
    run(runs, task).2
}

fn label_count(labels: &Option<Value>, key: &str) -> usize {
    labels.as_ref().and_then(Value::as_array).map_or(0, |ls| {
        ls.iter()
            .filter(|l| l["key"] == key && l["value"] == "true")
            .count()
    })
}

async fn execution_count(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM workflow.executions")
        .fetch_one(pool)
        .await
        .expect("count")
}

async fn replayed_from(pool: &sqlx::PgPool, ex: Uuid) -> Option<Uuid> {
    sqlx::query_scalar("SELECT replayed_from_execution_id FROM workflow.executions WHERE id = $1")
        .bind(ex)
        .fetch_one(pool)
        .await
        .expect("replayed_from")
}

/// `three("seq")` with `b` failing once, driven to `FAILED`.
async fn failed_three(pool: &sqlx::PgPool, ws: Uuid, reg: &Flaky) -> Uuid {
    let ex = start(pool, ws, &three("seq")).await;
    drive(pool, reg, ex).await;
    assert_eq!(exec_state(pool, ex).await, "FAILED");
    ex
}

// ── 1. Replay from a task ──────────────────────────────────────────────────

/// Spec D1, D2 (Kestra `ExecutionServiceTest.replaySimple`): the runs before
/// the chosen one are kept with their outputs under new ids; the chosen run
/// and what follows run again in a new execution; the old one is untouched.
#[sqlx::test(migrations = "../../migrations")]
async fn replay_from_a_task_keeps_earlier_runs_and_their_outputs(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("b", 1)]);
    let old = failed_three(&pool, ws, &reg).await;
    let old_runs = runs_of(&pool, old).await;
    let old_meta = exec_meta(&pool, old).await;
    let frozen = source_of(&pool, old).await;

    // Act
    let new = replay(
        &pool,
        ws,
        old,
        &frozen,
        ReplayKind::FromTaskRun(run_id(&old_runs, "b")),
        ReplayInputs::Reuse,
    )
    .await
    .expect("replayed");
    drive(&pool, &reg, new).await;

    // Assert
    assert_ne!(new, old);
    assert_eq!(exec_state(&pool, new).await, "SUCCESS");
    assert_eq!(reg.attempts_of("a"), [1], "a is not run again");
    let runs = runs_of(&pool, new).await;
    assert_eq!(run(&runs, "a").6, run(&old_runs, "a").6, "a's outputs");
    assert!(run(&runs, "a").6.is_some());
    assert_ne!(run_id(&runs, "a"), run_id(&old_runs, "a"), "a new id");
    assert_eq!(run(&runs, "b").3, "SUCCESS");
    assert_eq!(run(&runs, "c").3, "SUCCESS");
    let new_seen: Vec<String> = reg
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter(|s| s.execution == new)
        .map(|s| s.task.clone())
        .collect();
    assert_eq!(new_seen, ["b", "c"]);
    let h = exec_history(&pool, new).await;
    assert_eq!(
        history_tail(&h, 4),
        ["CREATED", "RESTARTED", "RUNNING", "SUCCESS"],
        "{h}"
    );
    assert_eq!(replayed_from(&pool, new).await, Some(old));
    assert_eq!(
        exec_meta(&pool, old).await,
        old_meta,
        "the old row is unchanged"
    );
    assert_eq!(runs_of(&pool, old).await, old_runs, "and its runs");
}

// ── 2. A Parallel branch ───────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn replay_from_a_parallel_branch(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[]);
    let source = format!(
        "id: par\nnamespace: t\ntasks:\n  - id: p\n    type: io.kestra.plugin.core.flow.Parallel\n\
         \x20   tasks:\n      - id: x\n        type: {LOG}\n        message: x\n\
         \x20     - id: y\n        type: {LOG}\n        message: y\n{}",
        tasks(&["z"])
    );
    let old = start(&pool, ws, &source).await;
    // By hand, so `y` ends SUCCESS before `x` fails: a sibling still running
    // when `x` fails is failed with it (Parallel rule 1), and a FAILED
    // sibling, kept by the copy, would fail `p` again.
    let claimed = egret_workflow::worker::claim(&pool, 10).await.unwrap();
    let by_task = |t: &str| {
        claimed
            .iter()
            .find(|c| c.task_id == t)
            .unwrap_or_else(|| panic!("{t} claimed"))
    };
    egret_workflow::worker::commit_outcome(
        &pool,
        by_task("y"),
        &Outcome::succeeded_with(json!({"value": "y"})),
    )
    .await
    .unwrap();
    egret_workflow::worker::commit_outcome(&pool, by_task("x"), &Outcome::failed())
        .await
        .unwrap();
    drive(&pool, &reg, old).await;
    assert_eq!(exec_state(&pool, old).await, "FAILED");
    let old_runs = runs_of(&pool, old).await;
    assert_eq!(run(&old_runs, "y").3, "SUCCESS");
    let x = run_id(&old_runs, "x");

    // Act
    let new = replay(
        &pool,
        ws,
        old,
        &source,
        ReplayKind::FromTaskRun(x),
        ReplayInputs::Reuse,
    )
    .await
    .expect("replayed");
    drive(&pool, &reg, new).await;

    // Assert
    assert_eq!(exec_state(&pool, new).await, "SUCCESS");
    assert_eq!(
        reg.attempts_of("y"),
        Vec::<usize>::new(),
        "y is not run again"
    );
    assert_eq!(reg.attempts_of("x"), [2], "x runs again, its next attempt");
    assert_eq!(reg.attempts_of("z"), [1], "z runs");
    let runs = runs_of(&pool, new).await;
    assert!(runs.iter().all(|r| r.3 == "SUCCESS"), "{runs:?}");
    assert_eq!(
        run(&runs, "y").6,
        run(&old_runs, "y").6,
        "y's outputs are kept"
    );
}

// ── 3. No lease, no gap ────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn a_copied_run_holds_no_lease(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("b", 1)]);
    let old = failed_three(&pool, ws, &reg).await;
    let b = run_id(&runs_of(&pool, old).await, "b");
    let frozen = source_of(&pool, old).await;

    // Act
    let new = replay(
        &pool,
        ws,
        old,
        &frozen,
        ReplayKind::FromTaskRun(b),
        ReplayInputs::Reuse,
    )
    .await
    .expect("replayed");

    // Assert
    let rows: Vec<(i32, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT run_ix, claimed_at FROM workflow.task_runs WHERE execution_id = $1 ORDER BY run_ix",
    )
    .bind(new)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(!rows.is_empty());
    for (i, (ix, claimed)) in rows.iter().enumerate() {
        assert_eq!(*ix as usize, i, "run_ix has no gap: {rows:?}");
        assert!(claimed.is_none(), "no lease: {rows:?}");
    }
}

// ── 4. Subflow ─────────────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn replay_from_a_subflow_run_spawns_a_new_child(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    common::seed_flow(
        &db,
        ws,
        user,
        &format!("id: kid\nnamespace: t\ntasks:\n{}", tasks(&["k"])),
        "draft",
    )
    .await;
    let parent = format!(
        "id: parent\nnamespace: t\ntasks:\n  - id: call\n    type: io.kestra.plugin.core.flow.Subflow\n\
         \x20   namespace: t\n    flowId: kid\n    wait: true\n    transmitFailed: true\n{}",
        tasks(&["after"])
    );
    let reg = Flaky::new(&[]);
    let old = start(&pool, ws, &parent).await;
    drive(&pool, &reg, old).await;
    assert_eq!(exec_state(&pool, old).await, "SUCCESS");
    let kids = |pool: sqlx::PgPool| async move {
        sqlx::query_as::<_, (Uuid, Option<Uuid>, Value)>(
            "SELECT id, trigger_execution_id, history FROM workflow.executions \
              WHERE flow_key = 'kid' ORDER BY created_at, id",
        )
        .fetch_all(&pool)
        .await
        .unwrap()
    };
    let before = kids(pool.clone()).await;
    assert_eq!(before.len(), 1);
    let call = run_id(&runs_of(&pool, old).await, "call");

    // Act
    let new = replay(
        &pool,
        ws,
        old,
        &parent,
        ReplayKind::FromTaskRun(call),
        ReplayInputs::Reuse,
    )
    .await
    .expect("replayed");
    drive(&pool, &reg, new).await;

    // Assert
    assert_eq!(exec_state(&pool, new).await, "SUCCESS");
    let after = kids(pool.clone()).await;
    assert_eq!(after.len(), 2, "{after:?}");
    assert_eq!(after[0], before[0], "the old child is unchanged");
    let fresh: Vec<_> = after.iter().filter(|k| k.1 == Some(new)).collect();
    assert_eq!(
        fresh.len(),
        1,
        "one new child of the new execution: {after:?}"
    );
}

// ── 5. Loop (S4) ───────────────────────────────────────────────────────────

fn loop_flow() -> String {
    format!(
        "id: lp\nnamespace: t\ntasks:\n  - id: l\n    type: io.kestra.plugin.core.flow.Loop\n\
         \x20   values: '[\"a\", \"b\"]'\n    tasks:\n      - id: item\n        type: {LOG}\n        \
         message: hi\n{}",
        tasks(&["after"])
    )
}

#[sqlx::test(migrations = "../../migrations")]
async fn replay_from_a_loop_run_runs_its_iterations_anew(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[]);
    let old = start(&pool, ws, &loop_flow()).await;
    drive(&pool, &reg, old).await;
    assert_eq!(exec_state(&pool, old).await, "SUCCESS");
    let l = run_id(&runs_of(&pool, old).await, "l");

    // Act
    let new = replay(
        &pool,
        ws,
        old,
        &loop_flow(),
        ReplayKind::FromTaskRun(l),
        ReplayInputs::Reuse,
    )
    .await
    .expect("replayed");
    drive(&pool, &reg, new).await;

    // Assert
    assert_eq!(exec_state(&pool, new).await, "SUCCESS");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.executions \
          WHERE parent_execution_id = $1 AND loop_index IS NOT NULL",
    )
    .bind(new)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 2, "two iterations under the new execution");
    assert_eq!(reg.attempts_of("item").len(), 4, "two old, two new");
    assert_eq!(run(&runs_of(&pool, new).await, "after").3, "SUCCESS");
}

/// S5: a copy keeps a `Loop` run's outputs, which can name files under the
/// source's iterations, so the read rule must reach them.
#[sqlx::test(migrations = "../../migrations")]
async fn a_copy_reads_its_source_loop_iterations_files(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[]);
    let x = start(&pool, ws, &loop_flow()).await;
    drive(&pool, &reg, x).await;
    let after = run_id(&runs_of(&pool, x).await, "after");
    let y = replay(
        &pool,
        ws,
        x,
        &loop_flow(),
        ReplayKind::FromTaskRun(after),
        ReplayInputs::Reuse,
    )
    .await
    .expect("Y");
    drive(&pool, &reg, y).await;
    let iterations: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM workflow.executions \
          WHERE parent_execution_id = $1 AND loop_index IS NOT NULL",
    )
    .bind(x)
    .fetch_all(&pool)
    .await
    .unwrap();

    // Act
    let mut conn = pool.acquire().await.unwrap();
    let readable = egret_workflow::storage::index::readable_executions(&mut conn, ws, y)
        .await
        .unwrap();

    // Assert
    assert_eq!(iterations.len(), 2, "{iterations:?}");
    for it in &iterations {
        assert!(readable.contains(it), "{it} missing from {readable:?}");
    }
}

// ── 6. An edited source ────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn replay_on_an_edited_source_runs_the_edited_task(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("b", 1)]);
    let old = failed_three(&pool, ws, &reg).await;
    let b = run_id(&runs_of(&pool, old).await, "b");
    let edited = three_edited("seq");

    // Act
    let new = replay(
        &pool,
        ws,
        old,
        &edited,
        ReplayKind::FromTaskRun(b),
        ReplayInputs::Reuse,
    )
    .await
    .expect("replayed");
    drive(&pool, &reg, new).await;

    // Assert
    assert_eq!(exec_state(&pool, new).await, "SUCCESS");
    assert_eq!(source_of(&pool, new).await, edited);
    let c = reg.seen_of("c");
    assert_eq!(c.len(), 1, "{c:?}");
    assert_eq!(c[0].execution, new);
    assert_eq!(c[0].spec["message"], "c edited", "{:?}", c[0].spec);
}

/// A2 review hand-off: the chosen source changes a leaf's plugin type; a
/// copied leaf matches on the kind's variant only, so the copy must take
/// `type_id` from the target plan, or the reset leaf would run the OLD
/// plugin with the NEW configuration.
#[sqlx::test(migrations = "../../migrations")]
async fn a_leaf_whose_type_changed_runs_as_the_new_type(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("b", 1)]);
    let old = failed_three(&pool, ws, &reg).await;
    let b = run_id(&runs_of(&pool, old).await, "b");
    let retyped = format!(
        "id: seq\nnamespace: t\ntasks:\n{}  - id: b\n    type: {RETURN}\n    format: b\n{}",
        tasks(&["a"]),
        tasks(&["c"])
    );

    // Act
    let new = replay(
        &pool,
        ws,
        old,
        &retyped,
        ReplayKind::FromTaskRun(b),
        ReplayInputs::Reuse,
    )
    .await
    .expect("replayed");
    drive(&pool, &reg, new).await;

    // Assert
    assert_eq!(exec_state(&pool, new).await, "SUCCESS");
    let seen = reg.seen_of("b");
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert_eq!(seen[0].type_id, LOG);
    assert_eq!(seen[1].execution, new);
    assert_eq!(seen[1].type_id, RETURN, "the new type runs");
    assert_eq!(seen[1].spec["format"], "b");
    let stored: Option<String> = sqlx::query_scalar(
        "SELECT type_id FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'b'",
    )
    .bind(new)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored.as_deref(), Some(RETURN));
}

// ── 7. A task missing from the source ──────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn a_task_missing_from_the_source_is_refused_and_writes_nothing(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("b", 1)]);
    let old = failed_three(&pool, ws, &reg).await;
    let b = run_id(&runs_of(&pool, old).await, "b");
    let without_b = format!("id: seq\nnamespace: t\ntasks:\n{}", tasks(&["a", "c"]));
    let count = execution_count(&pool).await;

    // Act
    let out = replay(
        &pool,
        ws,
        old,
        &without_b,
        ReplayKind::FromTaskRun(b),
        ReplayInputs::Reuse,
    )
    .await;

    // Assert
    assert_eq!(
        out,
        Err(ReplayRefused::TaskNotInSource {
            task_id: "b".into(),
            why: MatchFault::Missing
        })
    );
    assert_eq!(execution_count(&pool).await, count);
}

// ── 8. New inputs ──────────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn new_inputs_reach_the_replayed_task_and_bad_ones_are_refused(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[]);
    let source = format!(
        "id: inp\nnamespace: t\ninputs:\n  - id: n\n    type: INT\n    min: 1\ntasks:\n\
         \x20 - id: a\n    type: {LOG}\n    message: \"{{{{ inputs.n }}}}\"\n"
    );
    let old = egret_workflow::worker::start_execution(&pool, ws, None, &source, json!({"n": 2}))
        .await
        .expect("start");
    drive(&pool, &reg, old).await;
    assert_eq!(exec_state(&pool, old).await, "SUCCESS");
    let a = run_id(&runs_of(&pool, old).await, "a");

    // Act
    let new = replay(
        &pool,
        ws,
        old,
        &source,
        ReplayKind::FromTaskRun(a),
        ReplayInputs::Replace(json!({"n": 5})),
    )
    .await
    .expect("replayed");
    drive(&pool, &reg, new).await;
    let count = execution_count(&pool).await;
    let user = owner(&pool, ws).await;
    let bad = try_replay(
        &pool,
        ws,
        old,
        &source,
        ReplayKind::FromTaskRun(a),
        ReplayInputs::Replace(json!({"n": 0})),
        user,
    )
    .await;

    // Assert
    let inputs: Value = sqlx::query_scalar("SELECT inputs FROM workflow.executions WHERE id = $1")
        .bind(new)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(inputs, json!({"n": 5}));
    assert_eq!(exec_state(&pool, new).await, "SUCCESS");
    match bad {
        Err(egret_core::Error::InvalidInput(m)) => {
            assert!(m.contains("Invalid value for input `n`"), "{m}")
        }
        other => panic!("expected InvalidInput, got {other:?}"),
    }
    assert_eq!(execution_count(&pool).await, count, "nothing written");
}

// ── 9, 10. The label and the attempt (D10) ─────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn the_whole_replay_gets_the_label_and_the_attempt(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[]);
    let old = start(&pool, ws, &three("whole")).await;
    drive(&pool, &reg, old).await;
    assert_eq!(exec_state(&pool, old).await, "SUCCESS");
    assert_eq!(exec_meta(&pool, old).await.1, 1);
    let frozen = source_of(&pool, old).await;

    // Act
    let first = replay(
        &pool,
        ws,
        old,
        &frozen,
        ReplayKind::Whole,
        ReplayInputs::Reuse,
    )
    .await
    .expect("replayed");
    drive(&pool, &reg, first).await;
    let second = replay(
        &pool,
        ws,
        first,
        &frozen,
        ReplayKind::Whole,
        ReplayInputs::Reuse,
    )
    .await
    .expect("replayed again");
    drive(&pool, &reg, second).await;

    // Assert
    let (h1, attempt1, labels1) = exec_meta(&pool, first).await;
    assert_eq!(attempt1, 2);
    assert_eq!(label_count(&labels1, "system.replay"), 1, "{labels1:?}");
    assert_eq!(h1.as_array().unwrap()[0], "CREATED");
    assert_eq!(exec_state(&pool, first).await, "SUCCESS");
    let (_, attempt2, labels2) = exec_meta(&pool, second).await;
    assert_eq!(attempt2, 3);
    assert_eq!(label_count(&labels2, "system.replay"), 1, "{labels2:?}");
    assert_eq!(replayed_from(&pool, second).await, Some(first));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_copy_gets_the_label_once_and_the_attempt(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("b", 1)]);
    let old = failed_three(&pool, ws, &reg).await;
    let b = run_id(&runs_of(&pool, old).await, "b");
    let frozen = source_of(&pool, old).await;

    // Act
    let new = replay(
        &pool,
        ws,
        old,
        &frozen,
        ReplayKind::FromTaskRun(b),
        ReplayInputs::Reuse,
    )
    .await
    .expect("replayed");
    drive(&pool, &reg, new).await;

    // Assert
    let (_, attempt, labels) = exec_meta(&pool, new).await;
    assert_eq!(attempt, 2);
    assert_eq!(label_count(&labels, "system.replay"), 1, "{labels:?}");
    assert_eq!(label_count(&labels, "system.restarted"), 0, "{labels:?}");
}

// ── 11. Restart on a source (D3) ───────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn restart_on_a_source_resets_the_failed_and_keeps_the_rest(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("b", 1)]);
    let errors = format!("errors:\n  - id: e\n    type: {LOG}\n    message: e\n");
    let source = format!("{}{errors}", three("rs"));
    let old = start(&pool, ws, &source).await;
    drive(&pool, &reg, old).await;
    assert_eq!(exec_state(&pool, old).await, "FAILED");
    assert_eq!(reg.attempts_of("e"), [1], "the errors branch ran");
    let edited = format!("{}{errors}", three_edited("rs"));

    // Act
    let new = replay(
        &pool,
        ws,
        old,
        &edited,
        ReplayKind::RestartOnSource,
        ReplayInputs::Reuse,
    )
    .await
    .expect("restarted");
    let copied: Vec<String> = runs_of(&pool, new).await.into_iter().map(|r| r.1).collect();
    drive(&pool, &reg, new).await;

    // Assert
    assert!(
        !copied.contains(&"e".to_string()),
        "e is not copied: {copied:?}"
    );
    assert_eq!(exec_state(&pool, new).await, "SUCCESS");
    let (_, attempt, labels) = exec_meta(&pool, new).await;
    assert_eq!(attempt, 2);
    assert_eq!(label_count(&labels, "system.restarted"), 1, "{labels:?}");
    assert_eq!(label_count(&labels, "system.replay"), 0, "{labels:?}");
    assert_eq!(reg.attempts_of("a"), [1], "a is kept");
    assert_eq!(reg.attempts_of("b").len(), 2, "b runs again");
    assert_eq!(reg.attempts_of("e"), [1], "e does not run again");
    let c = reg.seen_of("c");
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].spec["message"], "c edited");
    assert!(runs_of(&pool, new).await.iter().all(|r| r.1 != "e"));
    assert_eq!(
        exec_state(&pool, old).await,
        "FAILED",
        "the old one is untouched"
    );
    assert_eq!(replayed_from(&pool, new).await, Some(old));
}

// ── 12. Refusals ───────────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn refusals_write_nothing(pool: sqlx::PgPool) {
    egret_workflow::storage::install_mem_for_tests();
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("b", 1)]);

    // A loop iteration.
    let root = start(&pool, ws, &loop_flow()).await;
    drive(&pool, &reg, root).await;
    let iteration: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE parent_execution_id = $1 AND loop_index = 0",
    )
    .bind(root)
    .fetch_one(&pool)
    .await
    .unwrap();
    let src = source_of(&pool, iteration).await;
    let n = execution_count(&pool).await;
    let out = replay(
        &pool,
        ws,
        iteration,
        &src,
        ReplayKind::Whole,
        ReplayInputs::Reuse,
    )
    .await;
    assert_eq!(out, Err(ReplayRefused::LoopIteration { root }));
    assert_eq!(execution_count(&pool).await, n);

    // A task run of no run of this execution.
    let failed = failed_three(&pool, ws, &reg).await;
    let frozen = source_of(&pool, failed).await;
    let n = execution_count(&pool).await;
    let out = replay(
        &pool,
        ws,
        failed,
        &frozen,
        ReplayKind::FromTaskRun(Uuid::new_v4()),
        ReplayInputs::Reuse,
    )
    .await;
    assert_eq!(out, Err(ReplayRefused::TaskRunNotFound));
    assert_eq!(execution_count(&pool).await, n);

    // A run inside a WorkingDirectory.
    let wd_reg = Flaky::new(&[("wb", 1)]).over(egret_workflow::worker::Handlers::with_core_tasks(
        egret_workflow::storage::current(),
    ));
    let wd = format!(
        "id: wd\nnamespace: t\ntasks:\n  - id: block\n    type: io.kestra.plugin.core.flow.WorkingDirectory\n\
         \x20   tasks:\n      - id: wa\n        type: {RETURN}\n        format: a\n\
         \x20     - id: wb\n        type: {RETURN}\n        format: b\n"
    );
    let wd_ex = start(&pool, ws, &wd).await;
    drive(&pool, &wd_reg, wd_ex).await;
    assert_eq!(exec_state(&pool, wd_ex).await, "FAILED");
    let wa = run_id(&runs_of(&pool, wd_ex).await, "wa");
    let n = execution_count(&pool).await;
    let out = replay(
        &pool,
        ws,
        wd_ex,
        &wd,
        ReplayKind::FromTaskRun(wa),
        ReplayInputs::Reuse,
    )
    .await;
    assert_eq!(out, Err(ReplayRefused::InsideWorkingDirectory));
    assert_eq!(execution_count(&pool).await, n);

    // `files_pruned_at` on the execution's replay source (S7).
    let whole = replay(
        &pool,
        ws,
        failed,
        &frozen,
        ReplayKind::Whole,
        ReplayInputs::Reuse,
    )
    .await
    .expect("a whole replay");
    drive(&pool, &reg, whole).await;
    sqlx::query("UPDATE workflow.executions SET files_pruned_at = now() WHERE id = $1")
        .bind(failed)
        .execute(&pool)
        .await
        .unwrap();
    let a = run_id(&runs_of(&pool, whole).await, "a");
    let n = execution_count(&pool).await;
    let out = replay(
        &pool,
        ws,
        whole,
        &frozen,
        ReplayKind::FromTaskRun(a),
        ReplayInputs::Reuse,
    )
    .await;
    assert_eq!(out, Err(ReplayRefused::FilesPruned));
    let out = replay(
        &pool,
        ws,
        whole,
        &frozen,
        ReplayKind::RestartOnSource,
        ReplayInputs::Reuse,
    )
    .await;
    assert_eq!(out, Err(ReplayRefused::FilesPruned));
    assert_eq!(execution_count(&pool).await, n);

    // A running execution (last: a tick would drive it).
    let running = start(&pool, ws, &three("live")).await;
    let n = execution_count(&pool).await;
    let out = replay(
        &pool,
        ws,
        running,
        &three("live"),
        ReplayKind::Whole,
        ReplayInputs::Reuse,
    )
    .await;
    assert!(matches!(out, Err(ReplayRefused::NotTerminal(_))), "{out:?}");
    assert_eq!(execution_count(&pool).await, n);
}

// ── 13. The egret.sql gate (D8) ────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn the_egret_sql_gate_judges_the_chosen_source(pool: sqlx::PgPool) {
    // Arrange: a flow created by someone other than the workspace owner, who
    // makes every replay here.
    let (db, ws, _) = setup(&pool).await;
    let other: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, name, provider, status, password_hash) \
         VALUES ($1, 't', 'local', 'active', 'x') RETURNING id",
    )
    .bind(format!("u_{}@t.local", Uuid::new_v4().simple()))
    .fetch_one(&pool)
    .await
    .unwrap();
    let frozen = three("gate");
    let flow = common::seed_flow(&db, ws, egret_core::UserId(other), &frozen, "draft").await;
    let reg = Flaky::new(&[("b", 1)]);
    let old = egret_workflow::worker::start_execution(&pool, ws, Some(flow), &frozen, json!({}))
        .await
        .expect("start");
    drive(&pool, &reg, old).await;
    let b = run_id(&runs_of(&pool, old).await, "b");
    let with_sql = format!(
        "id: gate\nnamespace: t\ntasks:\n{}  - id: c\n    type: egret.sql\n    database: trino_prod\n    \
         sql: SELECT 1\n",
        tasks(&["a", "b"])
    );

    // Act
    let allowed = replay(
        &pool,
        ws,
        old,
        &frozen,
        ReplayKind::FromTaskRun(b),
        ReplayInputs::Reuse,
    )
    .await;
    let n = execution_count(&pool).await;
    let user = owner(&pool, ws).await;
    let refused = try_replay(
        &pool,
        ws,
        old,
        &with_sql,
        ReplayKind::FromTaskRun(b),
        ReplayInputs::Reuse,
        user,
    )
    .await;

    // Assert
    assert!(allowed.is_ok(), "{allowed:?}");
    match refused {
        Err(egret_core::Error::InvalidInput(m)) => {
            assert!(m.contains("only its creator can start it"), "{m}")
        }
        other => panic!("expected the gate's refusal, got {other:?}"),
    }
    assert_eq!(execution_count(&pool).await, n, "nothing written");
}

/// The gate in the worker's transaction holds for a restart on a source as
/// well: the route's `rerun_gate` refuses first, so only a direct call
/// reaches this check (B1 fix round 1).
#[sqlx::test(migrations = "../../migrations")]
async fn the_egret_sql_gate_judges_a_restart_on_a_source(pool: sqlx::PgPool) {
    // Arrange: a flow created by someone other than the workspace owner, who
    // makes every call here.
    let (db, ws, _) = setup(&pool).await;
    let other: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, name, provider, status, password_hash) \
         VALUES ($1, 't', 'local', 'active', 'x') RETURNING id",
    )
    .bind(format!("u_{}@t.local", Uuid::new_v4().simple()))
    .fetch_one(&pool)
    .await
    .unwrap();
    let frozen = three("gate-restart");
    let flow = common::seed_flow(&db, ws, egret_core::UserId(other), &frozen, "draft").await;
    let reg = Flaky::new(&[("b", 1)]);
    let old = egret_workflow::worker::start_execution(&pool, ws, Some(flow), &frozen, json!({}))
        .await
        .expect("start");
    drive(&pool, &reg, old).await;
    assert_eq!(exec_state(&pool, old).await, "FAILED");
    let with_sql = format!(
        "id: gate-restart\nnamespace: t\ntasks:\n{}  - id: c\n    type: egret.sql\n    \
         database: trino_prod\n    sql: SELECT 1\n",
        tasks(&["a", "b"])
    );
    let user = owner(&pool, ws).await;

    // Act
    let allowed = try_replay(
        &pool,
        ws,
        old,
        &frozen,
        ReplayKind::RestartOnSource,
        ReplayInputs::Reuse,
        user,
    )
    .await;
    let n = execution_count(&pool).await;
    let refused = try_replay(
        &pool,
        ws,
        old,
        &with_sql,
        ReplayKind::RestartOnSource,
        ReplayInputs::Reuse,
        user,
    )
    .await;

    // Assert
    assert!(matches!(allowed, Ok(Ok(_))), "{allowed:?}");
    match refused {
        Err(egret_core::Error::InvalidInput(m)) => {
            assert!(m.contains("only its creator can start it"), "{m}")
        }
        other => panic!("expected the gate's refusal, got {other:?}"),
    }
    assert_eq!(execution_count(&pool).await, n, "nothing written");
    assert_eq!(
        exec_state(&pool, old).await,
        "FAILED",
        "the old one is untouched"
    );
}

// ── 14. A kept attempt keeps its wall clock ────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn a_kept_attempt_keeps_its_wall_clock(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("b", 1)]);
    let old = failed_three(&pool, ws, &reg).await;
    let b = run_id(&runs_of(&pool, old).await, "b");
    let frozen = source_of(&pool, old).await;
    // A measurable gap between the two executions' `created_at`.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Act
    let new = replay(
        &pool,
        ws,
        old,
        &frozen,
        ReplayKind::FromTaskRun(b),
        ReplayInputs::Reuse,
    )
    .await
    .expect("replayed");

    // Assert: `started_at` tick + the execution's `created_at`, in ms.
    let wall = |pool: sqlx::PgPool, ex: Uuid| async move {
        sqlx::query_scalar::<_, f64>(
            "SELECT (EXTRACT(EPOCH FROM e.created_at) * 1000)::float8 \
                    + (r.attempts->0->>'started_at')::float8 \
               FROM workflow.task_runs r JOIN workflow.executions e ON e.id = r.execution_id \
              WHERE r.execution_id = $1 AND r.task_id = 'a'",
        )
        .bind(ex)
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    let (old_wall, new_wall) = (wall(pool.clone(), old).await, wall(pool.clone(), new).await);
    assert!(
        (old_wall - new_wall).abs() <= 1.0,
        "old {old_wall}, new {new_wall}"
    );
}

// ── Fix round 1: `variables` on another source (review Important 1) ──────

/// `variables: {x: <x>}`, then `set` (`SetVariables y: 1`, which stores the
/// run's WHOLE variables value in `executions.variables`), then `out`, which
/// outputs `vars.x`.
fn vars_flow(x: &str) -> String {
    format!(
        "id: vars\nnamespace: t\nvariables:\n  x: {x}\ntasks:\n  - id: set\n    \
         type: io.kestra.plugin.core.execution.SetVariables\n    variables:\n      y: 1\n  \
         - id: out\n    type: io.kestra.plugin.core.output.OutputValues\n    values:\n      \
         v: \"{{{{ vars.x }}}}\"\n"
    )
}

async fn out_value(pool: &sqlx::PgPool, ex: Uuid) -> Value {
    let outputs: Option<Value> = sqlx::query_scalar(
        "SELECT outputs FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'out'",
    )
    .bind(ex)
    .fetch_one(pool)
    .await
    .expect("out");
    outputs.map_or(Value::Null, |o| o["values"]["v"].clone())
}

async fn exec_variables(pool: &sqlx::PgPool, ex: Uuid) -> Option<Value> {
    sqlx::query_scalar("SELECT variables FROM workflow.executions WHERE id = $1")
        .bind(ex)
        .fetch_one(pool)
        .await
        .expect("variables")
}

/// A replayed execution that ran a variables-updating task, on the frozen
/// source and its own variables.
async fn vars_execution(pool: &sqlx::PgPool, ws: Uuid, reg: &Flaky) -> Uuid {
    let old = start(pool, ws, &vars_flow("old")).await;
    drive(pool, reg, old).await;
    assert_eq!(exec_state(pool, old).await, "SUCCESS");
    assert_eq!(out_value(pool, old).await, "old");
    assert!(
        exec_variables(pool, old).await.is_some(),
        "SetVariables stored the run's variables"
    );
    old
}

/// Kestra `applyNewRevision` (`ES:514-517`): a source other than the frozen
/// one runs with ITS `variables:`, for every kind of replay.
#[sqlx::test(migrations = "../../migrations")]
async fn a_replay_on_another_source_reads_that_sources_variables(pool: sqlx::PgPool) {
    // Arrange
    egret_workflow::storage::install_mem_for_tests();
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[]).over(egret_workflow::worker::Handlers::with_core_tasks(
        egret_workflow::storage::current(),
    ));
    let old = vars_execution(&pool, ws, &reg).await;
    let out = run_id(&runs_of(&pool, old).await, "out");
    let edited = vars_flow("new");

    // Act
    let mut replays = Vec::new();
    for kind in [ReplayKind::Whole, ReplayKind::FromTaskRun(out)] {
        let new = replay(&pool, ws, old, &edited, kind, ReplayInputs::Reuse)
            .await
            .expect("replayed");
        drive(&pool, &reg, new).await;
        replays.push((kind, new));
    }

    // Assert
    for (kind, new) in replays {
        assert_eq!(exec_state(&pool, new).await, "SUCCESS", "{kind:?}");
        assert_eq!(out_value(&pool, new).await, "new", "{kind:?}");
    }
}

/// The control: on the frozen source the old execution's variables travel
/// with the replay, as before (W9a D3, final review I3).
#[sqlx::test(migrations = "../../migrations")]
async fn a_replay_on_the_frozen_source_keeps_the_executions_variables(pool: sqlx::PgPool) {
    // Arrange
    egret_workflow::storage::install_mem_for_tests();
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[]).over(egret_workflow::worker::Handlers::with_core_tasks(
        egret_workflow::storage::current(),
    ));
    let old = vars_execution(&pool, ws, &reg).await;
    let out = run_id(&runs_of(&pool, old).await, "out");
    let frozen = source_of(&pool, old).await;

    // Act
    let new = replay(
        &pool,
        ws,
        old,
        &frozen,
        ReplayKind::FromTaskRun(out),
        ReplayInputs::Reuse,
    )
    .await
    .expect("replayed");
    let copied = exec_variables(&pool, new).await;
    drive(&pool, &reg, new).await;

    // Assert
    assert_eq!(copied, exec_variables(&pool, old).await);
    assert_eq!(out_value(&pool, new).await, "old");
}

/// A restart on another source CARRIES the old execution's variables, as
/// Kestra's restart on a revision does: `ExecutionService.restart(.., revision)`
/// never calls `applyNewRevision` and `Execution.childExecution` copies
/// `this.variables` (user decision 2026-11-14, final review I2). The column
/// holds the run's whole variables value, so the old `x` wins over the chosen
/// source's `variables:`.
#[sqlx::test(migrations = "../../migrations")]
async fn a_restart_on_another_source_carries_the_executions_variables(pool: sqlx::PgPool) {
    // Arrange
    egret_workflow::storage::install_mem_for_tests();
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("out", 1)]).over(egret_workflow::worker::Handlers::with_core_tasks(
        egret_workflow::storage::current(),
    ));
    let old = start(&pool, ws, &vars_flow("old")).await;
    drive(&pool, &reg, old).await;
    assert_eq!(exec_state(&pool, old).await, "FAILED");
    let old_variables = exec_variables(&pool, old).await;
    assert!(old_variables.is_some());

    // Act
    let new = replay(
        &pool,
        ws,
        old,
        &vars_flow("new"),
        ReplayKind::RestartOnSource,
        ReplayInputs::Reuse,
    )
    .await
    .expect("restarted");
    let copied = exec_variables(&pool, new).await;
    drive(&pool, &reg, new).await;

    // Assert
    assert_eq!(copied, old_variables);
    assert_eq!(exec_state(&pool, new).await, "SUCCESS");
    assert_eq!(out_value(&pool, new).await, "old");
}

// ── Fix round 1: the refusals `refusals_write_nothing` did not reach ──────

/// S7: a copy must not copy live runs. An ended execution whose
/// `afterExecution:` still runs is refused, for both kinds of copy.
#[sqlx::test(migrations = "../../migrations")]
async fn a_copy_of_an_execution_whose_after_execution_runs_is_refused(pool: sqlx::PgPool) {
    // Arrange
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
    let n = execution_count(&pool).await;

    // Act
    let from_task = replay(
        &pool,
        ws,
        ex,
        &source,
        ReplayKind::FromTaskRun(run_id(&runs, "a")),
        ReplayInputs::Reuse,
    )
    .await;
    let restart = replay(
        &pool,
        ws,
        ex,
        &source,
        ReplayKind::RestartOnSource,
        ReplayInputs::Reuse,
    )
    .await;

    // Assert
    assert_eq!(from_task, Err(ReplayRefused::AfterExecutionRunning));
    assert_eq!(restart, Err(ReplayRefused::AfterExecutionRunning));
    assert_eq!(execution_count(&pool).await, n);
    assert_eq!(runs_of(&pool, ex).await, runs, "the old runs are untouched");
}

/// D2: a replay from a task needs an ended execution, as a whole replay does.
#[sqlx::test(migrations = "../../migrations")]
async fn a_replay_from_a_task_of_a_running_execution_is_refused(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let ex = start(&pool, ws, &three("live")).await;
    let a = run_id(&runs_of(&pool, ex).await, "a");
    let n = execution_count(&pool).await;

    // Act
    let out = replay(
        &pool,
        ws,
        ex,
        &three("live"),
        ReplayKind::FromTaskRun(a),
        ReplayInputs::Reuse,
    )
    .await;

    // Assert
    assert_eq!(out, Err(ReplayRefused::NotTerminal("RUNNING".into())));
    assert_eq!(execution_count(&pool).await, n);
}

// ── A4. Shapes 7a's in-place restart treats specially, now as copies ──────

/// S4: a `Loop` whose second iteration failed.
#[sqlx::test(migrations = "../../migrations")]
async fn a_restart_copy_of_a_failed_loop_creates_new_iterations(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let source = format!(
        "id: lp2\nnamespace: t\ntasks:\n  - id: l\n    type: io.kestra.plugin.core.flow.Loop\n\
         \x20   values: '[\"a\", \"b\"]'\n    tasks:\n      - id: item\n        type: {LOG}\n        \
         message: \"{{{{ item.value }}}}\"\n"
    );
    let reg = Flaky::new(&[]).failing_nth_run_of("item", 1);
    let old = start(&pool, ws, &source).await;
    drive(&pool, &reg, old).await;
    assert_eq!(exec_state(&pool, old).await, "FAILED");
    let iterations = |pool: sqlx::PgPool, of: Uuid| async move {
        sqlx::query_as::<_, (i32, Uuid, String, i32)>(
            "SELECT loop_index, id, state, attempt_number FROM workflow.executions \
              WHERE parent_execution_id = $1 AND loop_index IS NOT NULL ORDER BY loop_index",
        )
        .bind(of)
        .fetch_all(&pool)
        .await
        .unwrap()
    };
    let before = iterations(pool.clone(), old).await;
    assert_eq!(before.len(), 2, "{before:?}");
    assert_eq!(before[1].2, "FAILED");

    // Act
    let new = replay(
        &pool,
        ws,
        old,
        &source,
        ReplayKind::RestartOnSource,
        ReplayInputs::Reuse,
    )
    .await
    .expect("restarted");
    drive(&pool, &reg, new).await;

    // Assert
    assert_eq!(exec_state(&pool, new).await, "SUCCESS");
    let fresh = iterations(pool.clone(), new).await;
    assert_eq!(fresh.len(), 2, "every iteration is new: {fresh:?}");
    assert!(fresh.iter().all(|i| i.2 == "SUCCESS"), "{fresh:?}");
    assert_eq!(
        iterations(pool.clone(), old).await,
        before,
        "no iteration is restarted in the old execution"
    );
    assert_eq!(reg.seen_of("item").len(), 2 + 2, "two old, two new");
    assert_eq!(exec_state(&pool, old).await, "FAILED");
}

/// The old child is not restarted: a restart copy spawns a new one.
#[sqlx::test(migrations = "../../migrations")]
async fn a_restart_copy_of_a_failed_subflow_spawns_a_new_child(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    common::seed_flow(
        &db,
        ws,
        user,
        &format!("id: kid2\nnamespace: t\ntasks:\n{}", tasks(&["k"])),
        "draft",
    )
    .await;
    let parent = format!(
        "id: parent2\nnamespace: t\ntasks:\n  - id: call\n    type: io.kestra.plugin.core.flow.Subflow\n\
         \x20   namespace: t\n    flowId: kid2\n    wait: true\n    transmitFailed: true\n{}",
        tasks(&["after"])
    );
    let reg = Flaky::new(&[("k", 1)]);
    let old = start(&pool, ws, &parent).await;
    drive(&pool, &reg, old).await;
    assert_eq!(exec_state(&pool, old).await, "FAILED");
    let kids = |pool: sqlx::PgPool| async move {
        sqlx::query_as::<_, (Uuid, Option<Uuid>, String, i32)>(
            "SELECT id, trigger_execution_id, state, attempt_number FROM workflow.executions \
              WHERE flow_key = 'kid2' ORDER BY created_at, id",
        )
        .fetch_all(&pool)
        .await
        .unwrap()
    };
    let before = kids(pool.clone()).await;
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].2, "FAILED");

    // Act
    let new = replay(
        &pool,
        ws,
        old,
        &parent,
        ReplayKind::RestartOnSource,
        ReplayInputs::Reuse,
    )
    .await
    .expect("restarted");
    drive(&pool, &reg, new).await;

    // Assert
    assert_eq!(exec_state(&pool, new).await, "SUCCESS");
    let after = kids(pool.clone()).await;
    assert_eq!(after.len(), 2, "{after:?}");
    assert_eq!(after[0], before[0], "the old child is not restarted");
    assert_eq!(
        after.iter().filter(|k| k.1 == Some(new)).count(),
        1,
        "a new child of the new execution: {after:?}"
    );
    assert_eq!(reg.seen_of("after").len(), 1);
}

/// Kestra-faithful: the copy is a new execution; the paused one stays paused
/// and keeps its runs.
#[sqlx::test(migrations = "../../migrations")]
async fn a_restart_copy_of_a_paused_execution_leaves_the_old_one_paused(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let source = format!(
        "id: pz\nnamespace: t\ntasks:\n{}  - id: wait\n    type: io.kestra.plugin.core.flow.Pause\n{}",
        tasks(&["a"]),
        tasks(&["z"])
    );
    let reg = Flaky::new(&[]);
    let old = start(&pool, ws, &source).await;
    for _ in 0..20 {
        egret_workflow::worker::tick(&pool, 64, &reg).await.unwrap();
    }
    assert_eq!(exec_state(&pool, old).await, "PAUSED");
    let runs = runs_of(&pool, old).await;

    // Act
    let out = replay(
        &pool,
        ws,
        old,
        &source,
        ReplayKind::RestartOnSource,
        ReplayInputs::Reuse,
    )
    .await;

    // Assert
    let new = out.expect("the copy is made");
    assert_ne!(new, old);
    assert_eq!(replayed_from(&pool, new).await, Some(old));
    assert_eq!(exec_state(&pool, old).await, "PAUSED");
    assert_eq!(
        runs_of(&pool, old).await,
        runs,
        "the old runs are untouched"
    );
    assert_eq!(
        reg.attempts_of("a").len(),
        1,
        "the finished task is kept, not run again"
    );
}

/// D4, S5: the read rule is transitive over the replay chain.
#[sqlx::test(migrations = "../../migrations")]
async fn a_replay_of_a_replay_reads_two_back(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[]);
    let x = start(&pool, ws, &three("chain")).await;
    drive(&pool, &reg, x).await;
    let b = run_id(&runs_of(&pool, x).await, "b");
    let y = replay(
        &pool,
        ws,
        x,
        &three("chain"),
        ReplayKind::FromTaskRun(b),
        ReplayInputs::Reuse,
    )
    .await
    .expect("Y");
    drive(&pool, &reg, y).await;
    let c = run_id(&runs_of(&pool, y).await, "c");
    let z = replay(
        &pool,
        ws,
        y,
        &three("chain"),
        ReplayKind::FromTaskRun(c),
        ReplayInputs::Reuse,
    )
    .await
    .expect("Z");
    drive(&pool, &reg, z).await;

    // Act
    let mut conn = pool.acquire().await.unwrap();
    let readable = egret_workflow::storage::index::readable_executions(&mut conn, ws, z)
        .await
        .unwrap();

    // Assert
    assert_eq!(exec_state(&pool, z).await, "SUCCESS");
    assert_eq!(replayed_from(&pool, z).await, Some(y));
    assert!(readable.contains(&y), "{readable:?}");
    assert!(readable.contains(&x), "{readable:?}");
    assert_eq!(exec_meta(&pool, z).await.1, 3, "attempt 3");
}

/// The task is in `afterExecution:`; the replay re-runs it after the new
/// execution ends, and the tasks before it are not run again.
#[sqlx::test(migrations = "../../migrations")]
async fn an_after_execution_task_can_be_replayed(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let source = format!(
        "id: ae\nnamespace: t\ntasks:\n{}afterExecution:\n  - id: x\n    type: {LOG}\n    \
         message: after\n",
        tasks(&["a", "b"])
    );
    let reg = Flaky::new(&[]);
    let old = start(&pool, ws, &source).await;
    drive(&pool, &reg, old).await;
    assert_eq!(exec_state(&pool, old).await, "SUCCESS");
    let runs = runs_of(&pool, old).await;
    assert_eq!(run(&runs, "x").3, "SUCCESS", "{runs:?}");

    // Act
    let new = replay(
        &pool,
        ws,
        old,
        &source,
        ReplayKind::FromTaskRun(run_id(&runs, "x")),
        ReplayInputs::Reuse,
    )
    .await
    .expect("replayed");
    drive(&pool, &reg, new).await;

    // Assert
    assert_eq!(exec_state(&pool, new).await, "SUCCESS");
    let fresh = runs_of(&pool, new).await;
    assert_eq!(run(&fresh, "x").3, "SUCCESS", "{fresh:?}");
    assert_eq!(reg.attempts_of("x").len(), 2, "x ran again");
    assert_eq!(reg.attempts_of("a").len(), 1, "a is not run again");
    assert_eq!(reg.attempts_of("b").len(), 1, "b is not run again");
    assert_eq!(exec_state(&pool, old).await, "SUCCESS");
}

// ── Task Z: a copy keeps the stamps rule (`worker::stamps`) ──────────────

/// `(state, history, started_at, ended_at)` of `task`'s run in `ex`.
type Stamped = (
    String,
    Value,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<chrono::DateTime<chrono::Utc>>,
);

async fn stamped(pool: &sqlx::PgPool, ex: Uuid, task: &str) -> Stamped {
    sqlx::query_as(
        "SELECT state, history, started_at, ended_at FROM workflow.task_runs \
          WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(ex)
    .bind(task)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| panic!("the run of {task} in {ex}: {e}"))
}

async fn created_at(pool: &sqlx::PgPool, ex: Uuid) -> chrono::DateTime<chrono::Utc> {
    sqlx::query_scalar("SELECT created_at FROM workflow.executions WHERE id = $1")
        .bind(ex)
        .fetch_one(pool)
        .await
        .expect("created_at")
}

/// `worker::copy`'s `INSERT_COPIED_RUNS` copies `state`, `history`,
/// `started_at` and `ended_at` verbatim (the guard's one allowance,
/// `execution_timestamps.rs::copies_verbatim`); the reset is a later stamped
/// write (`restart::copy_job`'s settle, through `persist`'s `stamp_set!`).
///
/// - A carried `SUCCESS` run keeps the old run's start and end exactly.
/// - The reset run, right after the replay and before any worker tick, is
///   not terminal and has no end. Its history still holds the old `RUNNING`,
///   so the rule KEEPS its start, as it does for 7a's restart in place and a
///   retry reopen (`stamps` module doc: "kept while the new history holds
///   `RUNNING`").
/// - Once the new execution has finished, the rerun run has a fresh end
///   (after the new execution's `created_at`) and the run never ran before
///   (`c`) has a fresh start and end.
/// - §3.2's invariants hold on every run of the new execution.
#[sqlx::test(migrations = "../../migrations")]
async fn a_copy_keeps_the_stamps_rule(pool: sqlx::PgPool) {
    // Arrange: `a` SUCCESS, `b` FAILED, `c` never created.
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("b", 1)]);
    let old = failed_three(&pool, ws, &reg).await;
    let b = run_id(&runs_of(&pool, old).await, "b");
    let (old_a, old_b) = (
        stamped(&pool, old, "a").await,
        stamped(&pool, old, "b").await,
    );
    assert_eq!(old_a.0, "SUCCESS");
    assert_eq!(old_b.0, "FAILED");
    assert!(old_a.2.is_some() && old_a.3.is_some() && old_b.3.is_some());
    let frozen = source_of(&pool, old).await;
    // A measurable gap: a fresh stamp is after the new `created_at`, an old
    // one before it.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Act 1: the copy, and its reset settle; no worker tick yet.
    let new = replay(
        &pool,
        ws,
        old,
        &frozen,
        ReplayKind::FromTaskRun(b),
        ReplayInputs::Reuse,
    )
    .await
    .expect("replayed");
    let born = created_at(&pool, new).await;
    let (a, reset) = (
        stamped(&pool, new, "a").await,
        stamped(&pool, new, "b").await,
    );

    // Assert 1: carried exactly; reset consistent with its new state.
    assert!(old_a.3.unwrap() < born, "the old end precedes the copy");
    assert_eq!(
        a, old_a,
        "a carried SUCCESS run is the old run's, times included"
    );
    assert_ne!(reset.0, "FAILED", "{reset:?}");
    let reset_terminal: bool = sqlx::query_scalar("SELECT workflow.state_is_terminal($1)")
        .bind(&reset.0)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!reset_terminal, "the reset run is live: {reset:?}");
    assert_eq!(reset.3, None, "a live run has no end: {reset:?}");
    assert!(
        reset.1.as_array().unwrap().iter().any(|s| s == "RUNNING"),
        "the reset history keeps the old RUNNING: {reset:?}"
    );
    assert_eq!(reset.2, old_b.2, "so the rule keeps its start: {reset:?}");

    // Act 2
    drive(&pool, &reg, new).await;

    // Assert 2: the rerun runs end fresh; `c` starts fresh too.
    assert_eq!(exec_state(&pool, new).await, "SUCCESS");
    assert_eq!(stamped(&pool, new, "a").await, old_a, "a is untouched");
    let (b2, c2) = (
        stamped(&pool, new, "b").await,
        stamped(&pool, new, "c").await,
    );
    assert_eq!((b2.0.as_str(), c2.0.as_str()), ("SUCCESS", "SUCCESS"));
    assert!(b2.3.is_some_and(|e| e > born), "b's end is fresh: {b2:?}");
    assert_eq!(b2.2, old_b.2, "b's start is still its first: {b2:?}");
    assert!(c2.2.is_some_and(|s| s > born), "c's start is fresh: {c2:?}");
    assert!(c2.3 >= c2.2, "c ends after it starts: {c2:?}");
    // §3.2: an end is never before its start, and only a terminal run has one.
    let broken: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.task_runs WHERE execution_id = $1 \
            AND (ended_at < started_at \
                 OR (ended_at IS NOT NULL) <> workflow.state_is_terminal(state))",
    )
    .bind(new)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(broken, 0);
}

// ── Final fix wave: `files_pruned` follows the retry link (review triage c) ──

/// A `CREATE_NEW_EXECUTION` retry replacement carries its parent's files (the
/// read rule's chain follows `parent_execution_id` when `loop_index` is NULL).
/// When the retry parent's files are pruned, a copy of the replacement is
/// refused with `FilesPruned` instead of failing at run time.
#[sqlx::test(migrations = "../../migrations")]
async fn a_copy_of_a_retry_replacement_whose_parent_was_pruned_is_refused(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = setup(&pool).await;
    let reg = Flaky::new(&[("b", 2)]);
    let source = format!(
        "id: retried\nnamespace: t\nretry:\n  behavior: CREATE_NEW_EXECUTION\n  \
         type: constant\n  maxAttempts: 2\n  interval: PT0S\ntasks:\n{}",
        tasks(&["a", "b", "c"])
    );
    let parent = start(&pool, ws, &source).await;
    // The parent ends `RETRIED`, which `drive` does not wait for: tick until
    // its replacement exists.
    let mut replacement = None;
    for _ in 0..60 {
        egret_workflow::worker::tick(&pool, 64, &reg)
            .await
            .expect("a worker tick");
        replacement = sqlx::query_scalar(
            "SELECT id FROM workflow.executions \
              WHERE parent_execution_id = $1 AND loop_index IS NULL",
        )
        .bind(parent)
        .fetch_optional(&pool)
        .await
        .expect("replacement");
        if replacement.is_some() {
            break;
        }
    }
    let replacement: Uuid = replacement.expect("a retry replacement");
    drive(&pool, &reg, replacement).await;
    assert_eq!(exec_state(&pool, replacement).await, "FAILED");
    sqlx::query("UPDATE workflow.executions SET files_pruned_at = now() WHERE id = $1")
        .bind(parent)
        .execute(&pool)
        .await
        .unwrap();
    let a = run_id(&runs_of(&pool, replacement).await, "a");
    let frozen = source_of(&pool, replacement).await;
    let n = execution_count(&pool).await;

    // Act
    let from_task = replay(
        &pool,
        ws,
        replacement,
        &frozen,
        ReplayKind::FromTaskRun(a),
        ReplayInputs::Reuse,
    )
    .await;
    let restart = replay(
        &pool,
        ws,
        replacement,
        &frozen,
        ReplayKind::RestartOnSource,
        ReplayInputs::Reuse,
    )
    .await;

    // Assert
    assert_eq!(from_task, Err(ReplayRefused::FilesPruned));
    assert_eq!(restart, Err(ReplayRefused::FilesPruned));
    assert_eq!(execution_count(&pool).await, n);
}
