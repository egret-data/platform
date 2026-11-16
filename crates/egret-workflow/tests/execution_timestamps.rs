//! Real start and end times for executions, task runs and attempts (W7b
//! Task 1, migration 0183; spec
//! `docs/en/specs/2026-10-11-workflow-w7b-executions-design.md` §3).
//!
//! Every test here drives the **real worker** on a private database
//! (`#[sqlx::test]`), because `worker::claim` is global and would take a
//! neighbour's runs on a shared one, and because what is under test is what
//! the production writer leaves behind — not what a fixture chose to insert.
//!
//! The one exception is [`every_state_write_stamps_the_timestamps`], which
//! reads the crate's SOURCE: it is the guard that a state-writing statement
//! added later cannot forget the timestamps.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5442/egret_test
//!   cargo test -p egret-workflow --test execution_timestamps

use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use egret_metastore::Db;
use egret_workflow::worker::{self, Outcome, Registry, TaskInput};
use serde_json::Value;
use uuid::Uuid;

mod common;

type Ts = Option<DateTime<Utc>>;

// ── Registries ──────────────────────────────────────────────────────────────

/// How long every handler here takes. Long enough that an attempt's start and
/// end land on different milliseconds, so `started_at < ended_at` is a strict
/// measurement and not a coincidence of the clock's resolution.
const HANDLER_TIME: Duration = Duration::from_millis(15);

/// Succeeds every attempt.
struct Succeeds;

#[async_trait::async_trait]
impl Registry for Succeeds {
    async fn run(&self, _task: &TaskInput) -> egret_core::Result<Outcome> {
        tokio::time::sleep(HANDLER_TIME).await;
        Ok(Outcome::success())
    }
    async fn cancel(&self, task: &TaskInput) -> egret_core::Result<()> {
        panic!(
            "nothing here has a timeout, so {} is never killed",
            task.task_id
        );
    }
}

/// Fails every attempt.
struct Fails;

#[async_trait::async_trait]
impl Registry for Fails {
    async fn run(&self, _task: &TaskInput) -> egret_core::Result<Outcome> {
        tokio::time::sleep(HANDLER_TIME).await;
        Ok(Outcome::failed_with("this fixture always fails"))
    }
    async fn cancel(&self, task: &TaskInput) -> egret_core::Result<()> {
        panic!(
            "nothing here has a timeout, so {} is never killed",
            task.task_id
        );
    }
}

/// Fails the first attempt of every run and succeeds every later one.
#[derive(Default)]
struct FailsOnce {
    seen: Mutex<Vec<usize>>,
}

#[async_trait::async_trait]
impl Registry for FailsOnce {
    async fn run(&self, task: &TaskInput) -> egret_core::Result<Outcome> {
        self.seen.lock().unwrap().push(task.attempt);
        tokio::time::sleep(HANDLER_TIME).await;
        match task.attempt {
            1 => Ok(Outcome::failed_with("the first attempt always fails")),
            _ => Ok(Outcome::success()),
        }
    }
    async fn cancel(&self, task: &TaskInput) -> egret_core::Result<()> {
        panic!(
            "nothing here has a timeout, so {} is never killed",
            task.task_id
        );
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

async fn seed(pool: &sqlx::PgPool) -> (Db, Uuid, egret_core::UserId) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    (db, ws, user)
}

async fn start(pool: &sqlx::PgPool, ws: Uuid, source: &str) -> Uuid {
    worker::start_execution(pool, ws, None, source, serde_json::json!({}))
        .await
        .expect("start execution")
}

/// Tick until the worker claims nothing.
async fn drain(pool: &sqlx::PgPool, reg: &dyn Registry) {
    for _ in 0..64 {
        if worker::tick(pool, 16, reg).await.expect("tick") == 0 {
            return;
        }
    }
    panic!("the worker never stopped claiming");
}

/// Sleep past a `PT1S` deadline in REAL time, then let the waker move what
/// came due. Nothing here rewinds `created_at`: the timestamps under test are
/// wall-clock, and a rewound origin would make them disagree with the ticks.
async fn pass_a_second(pool: &sqlx::PgPool) -> Vec<Uuid> {
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    worker::wake(pool, 16).await.expect("wake")
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct ExecTimes {
    state: String,
    history: Value,
    created_at: DateTime<Utc>,
    started_at: Ts,
    ended_at: Ts,
}

async fn execution(pool: &sqlx::PgPool, id: Uuid) -> ExecTimes {
    sqlx::query_as(
        "SELECT state, history, created_at, started_at, ended_at \
           FROM workflow.executions WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .expect("read execution")
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct RunTimes {
    task_id: String,
    state: String,
    attempts: Value,
    started_at: Ts,
    ended_at: Ts,
}

async fn runs(pool: &sqlx::PgPool, execution: Uuid) -> Vec<RunTimes> {
    sqlx::query_as(
        "SELECT task_id, state, attempts, started_at, ended_at \
           FROM workflow.task_runs WHERE execution_id = $1 ORDER BY run_ix",
    )
    .bind(execution)
    .fetch_all(pool)
    .await
    .expect("read runs")
}

async fn run(pool: &sqlx::PgPool, execution: Uuid, task_id: &str) -> RunTimes {
    runs(pool, execution)
        .await
        .into_iter()
        .find(|r| r.task_id == task_id)
        .unwrap_or_else(|| panic!("no run of `{task_id}` in {execution}"))
}

/// One attempt's `(started_at, ended_at)`, in ms after the execution's
/// `created_at`.
fn attempt_ticks(r: &RunTimes) -> Vec<(i64, Option<i64>)> {
    r.attempts
        .as_array()
        .expect("attempts is an array")
        .iter()
        .map(|a| {
            (
                a["started_at"].as_i64().expect("attempt.started_at"),
                a.get("ended_at").and_then(Value::as_i64),
            )
        })
        .collect()
}

/// `origin + ms`, the wall clock a `Tick` names.
fn at(origin: DateTime<Utc>, ms: i64) -> DateTime<Utc> {
    origin + chrono::Duration::milliseconds(ms)
}

/// `a` and `b` name the same instant to the millisecond. A `Tick` is
/// `(now() - created_at)` in ms rounded to an integer, so a column written by
/// `now()` in the same transaction is within half a millisecond of it.
fn same_ms(a: DateTime<Utc>, b: DateTime<Utc>) -> bool {
    (a - b).num_microseconds().unwrap().abs() <= 1_000
}

const TERMINAL: [&str; 7] = [
    "SUCCESS",
    "FAILED",
    "RETRIED",
    "KILLED",
    "SKIPPED",
    "WARNING",
    "CANCELLED",
];

/// §3.2's invariants, over EVERY row the test produced: an end is never
/// before its start, and only a terminal row has an end. With `strict`, every
/// terminal row also HAS one -- true of every row a post-0183 writer produced,
/// and false by construction of a row made to look pre-0183.
///
/// **Not asserted: "no execution leaves a terminal state".** Spec §3.2 says
/// nothing does today, and that was measured false here: a
/// `RETRY_FAILED_TASK` takes the EXECUTION `RUNNING, FAILED, RETRYING,
/// RUNNING, SUCCESS` (Kestra's own history shape), so the end-clearing half of
/// the rule is reachable, and
/// [`a_retried_run_keeps_its_first_start_and_each_attempt_has_an_end`] pins
/// it: no end while it waits, a fresh end once it finishes.
async fn assert_invariants(pool: &sqlx::PgPool, strict: bool) {
    for table in ["workflow.executions", "workflow.task_runs"] {
        let bad: Vec<(Uuid, String, Ts, Ts)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT id, state, started_at, ended_at FROM {table} \
              WHERE ended_at < started_at \
                 OR (ended_at IS NOT NULL AND NOT workflow.state_is_terminal(state)) \
                 OR ($1 AND ended_at IS NULL AND workflow.state_is_terminal(state))"
        )))
        .bind(strict)
        .fetch_all(pool)
        .await
        .expect("scan invariants");
        assert!(bad.is_empty(), "{table}: rows breaking §3.2: {bad:?}");
    }
}

fn one_log(id: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    )
}

fn two_logs(id: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  \
         - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: one\n  \
         - id: b\n    type: io.kestra.plugin.core.log.Log\n    message: two\n"
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

// ── 1. Simple success ───────────────────────────────────────────────────────

/// The execution's span contains every run's; every instant is set; a run's
/// start is its first attempt's start; an attempt ends after it starts.
#[sqlx::test(migrations = "../../migrations")]
async fn a_successful_execution_contains_every_runs_span(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _) = seed(&pool).await;
    let ex = start(&pool, ws, &two_logs("ok")).await;

    // Act
    drain(&pool, &Succeeds).await;

    // Assert
    let e = execution(&pool, ex).await;
    assert_eq!(e.state, "SUCCESS", "{e:?}");
    let (e_start, e_end) = (e.started_at.expect("started"), e.ended_at.expect("ended"));
    assert!(e_start >= e.created_at && e_start <= e_end, "{e:?}");
    let rs = runs(&pool, ex).await;
    assert_eq!(rs.len(), 2, "{rs:?}");
    for r in &rs {
        let (start, end) = (
            r.started_at.expect("run started"),
            r.ended_at.expect("run ended"),
        );
        assert!(e_start <= start, "the execution starts no later than {r:?}");
        assert!(end <= e_end, "the execution ends no earlier than {r:?}");
        assert!(start < end, "a worker task takes time: {r:?}");
        let ticks = attempt_ticks(r);
        assert_eq!(ticks.len(), 1, "{r:?}");
        let (a_start, a_end) = (ticks[0].0, ticks[0].1.expect("the attempt ended"));
        assert!(a_start < a_end, "{r:?}");
        assert!(
            same_ms(start, at(e.created_at, a_start)),
            "the run's start IS its first attempt's: {start} vs {}",
            at(e.created_at, a_start)
        );
        assert!(same_ms(end, at(e.created_at, a_end)), "{r:?}");
    }
    // `b` is created when `a` ends, so its bar starts no earlier.
    assert!(rs[1].started_at >= rs[0].ended_at, "{rs:?}");
    assert_invariants(&pool, true).await;
}

// ── 2. Failure ──────────────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn a_failed_execution_and_its_run_both_end(pool: sqlx::PgPool) {
    let (_db, ws, _) = seed(&pool).await;
    let ex = start(&pool, ws, &one_log("boom")).await;

    drain(&pool, &Fails).await;

    let e = execution(&pool, ex).await;
    assert_eq!(e.state, "FAILED", "{e:?}");
    let r = run(&pool, ex, "a").await;
    assert_eq!(r.state, "FAILED", "{r:?}");
    let (start, end) = (r.started_at.expect("started"), r.ended_at.expect("ended"));
    assert!(
        e.started_at.unwrap() <= start && end <= e.ended_at.unwrap(),
        "{e:?} {r:?}"
    );
    assert!(
        attempt_ticks(&r)[0].1.is_some(),
        "the failed attempt ended: {r:?}"
    );
    assert_invariants(&pool, true).await;
}

// ── 3. RETRY_FAILED_TASK ────────────────────────────────────────────────────

fn one_task_with_retry(id: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n\
         \x20   message: hi\n    retry:\n      type: constant\n      interval: PT1S\n\
         \x20     maxAttempts: 2\n"
    )
}

/// Two attempts, each with a start before its end, the first ending no later
/// than the second starts; the run's start is the first attempt's and does
/// not move; the run's end is cleared while it waits to retry.
#[sqlx::test(migrations = "../../migrations")]
async fn a_retried_run_keeps_its_first_start_and_each_attempt_has_an_end(pool: sqlx::PgPool) {
    let (_db, ws, _) = seed(&pool).await;
    let reg = FailsOnce::default();
    let ex = start(&pool, ws, &one_task_with_retry("retry")).await;

    drain(&pool, &reg).await;
    let waiting = run(&pool, ex, "a").await;
    assert_eq!(waiting.state, "RETRYING", "{waiting:?}");
    assert_eq!(
        waiting.ended_at, None,
        "RETRYING is not terminal, so the run has no end yet: {waiting:?}"
    );
    let first_start = waiting.started_at.expect("the run started");
    assert!(
        attempt_ticks(&waiting)[0].1.is_some(),
        "attempt 1 ended: {waiting:?}"
    );
    // The EXECUTION passed through FAILED and is back out of it (RETRYING):
    // it has no end while it waits -- the clearing half of the rule, reached.
    let e_waiting = execution(&pool, ex).await;
    assert_eq!(e_waiting.ended_at, None, "{e_waiting:?}");

    assert_eq!(pass_a_second(&pool).await, vec![ex]);
    drain(&pool, &reg).await;

    let e = execution(&pool, ex).await;
    let r = run(&pool, ex, "a").await;
    assert_eq!(
        (e.state.as_str(), r.state.as_str()),
        ("SUCCESS", "SUCCESS"),
        "{r:?}"
    );
    assert_eq!(*reg.seen.lock().unwrap(), vec![1, 2]);
    let ticks = attempt_ticks(&r);
    assert_eq!(ticks.len(), 2, "{r:?}");
    let ends: Vec<i64> = ticks
        .iter()
        .map(|t| t.1.expect("every attempt ended"))
        .collect();
    assert!(ticks[0].0 < ends[0] && ticks[1].0 < ends[1], "{ticks:?}");
    assert!(
        ends[0] <= ticks[1].0,
        "attempt 1 ends before attempt 2 starts: {ticks:?}"
    );
    assert!(
        ticks[1].0 - ends[0] >= 1_000,
        "the PT1S interval is between them: {ticks:?}"
    );
    assert_eq!(
        r.started_at,
        Some(first_start),
        "a retry does not move the start"
    );
    assert!(same_ms(first_start, at(e.created_at, ticks[0].0)), "{r:?}");
    let end = r.ended_at.expect("the run ended");
    assert!(same_ms(end, at(e.created_at, ends[1])), "{r:?}");
    assert!(end <= e.ended_at.unwrap(), "{e:?}");
    assert_eq!(
        e.history,
        serde_json::json!(["CREATED", "RUNNING", "FAILED", "RETRYING", "RUNNING", "SUCCESS"]),
        "the execution leaves a terminal state on a RETRY_FAILED_TASK retry"
    );
    assert_invariants(&pool, true).await;
}

// ── 4. CREATE_NEW_EXECUTION ─────────────────────────────────────────────────

/// The replacement execution has its own times, after the original's.
#[sqlx::test(migrations = "../../migrations")]
async fn a_create_new_execution_retry_has_its_own_times(pool: sqlx::PgPool) {
    let (_db, ws, _) = seed(&pool).await;
    let source =
        "id: cne\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n\
                  \x20   message: hi\n    retry:\n      behavior: CREATE_NEW_EXECUTION\n\
                  \x20     type: constant\n      interval: PT1S\n      maxAttempts: 2\n";
    let first = start(&pool, ws, source).await;
    drain(&pool, &Fails).await;
    pass_a_second(&pool).await;
    drain(&pool, &Fails).await;

    let replay: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE parent_execution_id = $1 AND parent_run_ix IS NULL",
    )
    .bind(first)
    .fetch_one(&pool)
    .await
    .expect("the replacement exists");
    let (orig, new) = (
        execution(&pool, first).await,
        execution(&pool, replay).await,
    );
    assert!(TERMINAL.contains(&new.state.as_str()), "{new:?}");
    let (o_start, o_end) = (
        orig.started_at.unwrap(),
        orig.ended_at.expect("original ended"),
    );
    let (n_start, n_end) = (
        new.started_at.expect("own start"),
        new.ended_at.expect("own end"),
    );
    assert!(
        o_start < n_start && o_end <= n_start,
        "{orig:?} then {new:?}"
    );
    assert!(n_start >= new.created_at && n_start <= n_end, "{new:?}");
    let r = run(&pool, replay, "a").await;
    assert!(
        r.started_at.unwrap() >= n_start && r.ended_at.unwrap() <= n_end,
        "{r:?}"
    );
    assert_invariants(&pool, true).await;
}

/// W7f spec §3.3: the replacement a `CREATE_NEW_EXECUTION` retry starts copies
/// the original's own labels.
#[sqlx::test(migrations = "../../migrations")]
async fn a_create_new_execution_retry_copies_the_labels(pool: sqlx::PgPool) {
    let (_db, ws, _) = seed(&pool).await;
    let source =
        "id: cne-l\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n\
                  \x20   message: hi\n    retry:\n      behavior: CREATE_NEW_EXECUTION\n\
                  \x20     type: constant\n      interval: PT1S\n      maxAttempts: 2\n";
    let first = start(&pool, ws, source).await;
    let own = serde_json::json!([{"key": "reason", "value": "backfill"}]);
    sqlx::query("UPDATE workflow.executions SET labels = $2 WHERE id = $1")
        .bind(first)
        .bind(&own)
        .execute(&pool)
        .await
        .unwrap();
    drain(&pool, &Fails).await;
    pass_a_second(&pool).await;
    drain(&pool, &Fails).await;
    let copied: Option<Value> = sqlx::query_scalar(
        "SELECT labels FROM workflow.executions WHERE parent_execution_id = $1 AND parent_run_ix IS NULL",
    )
    .bind(first)
    .fetch_one(&pool)
    .await
    .expect("the replacement exists");
    assert_eq!(copied, Some(own));
}

/// Package 7d D6 (S6): the replacement carries the flag with the list.
#[sqlx::test(migrations = "../../migrations")]
async fn a_create_new_execution_retry_copies_labels_replaced(pool: sqlx::PgPool) {
    let (_db, ws, _) = seed(&pool).await;
    let source =
        "id: cne-lr\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n\
                  \x20   message: hi\n    retry:\n      behavior: CREATE_NEW_EXECUTION\n\
                  \x20     type: constant\n      interval: PT1S\n      maxAttempts: 2\n";
    let first = start(&pool, ws, source).await;
    let own = serde_json::json!([{"key": "reason", "value": "edit"}]);
    sqlx::query("UPDATE workflow.executions SET labels = $2, labels_replaced = true WHERE id = $1")
        .bind(first)
        .bind(&own)
        .execute(&pool)
        .await
        .unwrap();
    drain(&pool, &Fails).await;
    pass_a_second(&pool).await;
    drain(&pool, &Fails).await;
    let replaced: bool = sqlx::query_scalar(
        "SELECT labels_replaced FROM workflow.executions WHERE parent_execution_id = $1 AND parent_run_ix IS NULL",
    )
    .bind(first)
    .fetch_one(&pool)
    .await
    .expect("the replacement exists");
    assert!(replaced);
}

// ── 5. Pause and resume ─────────────────────────────────────────────────────

/// The paused run keeps its start across the resume; the execution has no
/// end while paused; both end once it finishes.
#[sqlx::test(migrations = "../../migrations")]
async fn a_resumed_pause_keeps_its_start_and_ends_once(pool: sqlx::PgPool) {
    let (_db, ws, _) = seed(&pool).await;
    let source = "id: pause\nnamespace: t\ntasks:\n  \
                  - id: wait\n    type: io.kestra.plugin.core.flow.Pause\n    pauseDuration: PT1S\n  \
                  - id: after\n    type: io.kestra.plugin.core.log.Log\n    message: after\n";
    let ex = start(&pool, ws, source).await;
    drain(&pool, &Succeeds).await;

    let paused = run(&pool, ex, "wait").await;
    assert_eq!(paused.state, "PAUSED", "{paused:?}");
    let paused_at = paused.started_at.expect("the pause started");
    assert_eq!(paused.ended_at, None, "{paused:?}");
    let e_paused = execution(&pool, ex).await;
    let e_start = e_paused.started_at.expect("the execution started");
    assert_eq!(e_paused.ended_at, None, "no end while paused: {e_paused:?}");

    assert_eq!(pass_a_second(&pool).await, vec![ex]);
    drain(&pool, &Succeeds).await;

    let resumed = run(&pool, ex, "wait").await;
    let e = execution(&pool, ex).await;
    assert_eq!(
        (resumed.state.as_str(), e.state.as_str()),
        ("SUCCESS", "SUCCESS")
    );
    assert_eq!(
        resumed.started_at,
        Some(paused_at),
        "a resume does not move the start"
    );
    assert_eq!(e.started_at, Some(e_start), "nor the execution's");
    let end = resumed.ended_at.expect("the pause ended");
    assert!(
        end - paused_at >= chrono::Duration::milliseconds(1_000),
        "{resumed:?}"
    );
    let after = run(&pool, ex, "after").await;
    assert!(after.started_at.unwrap() >= end, "{after:?}");
    assert!(e.ended_at.unwrap() >= after.ended_at.unwrap(), "{e:?}");
    assert_invariants(&pool, true).await;
}

// ── 6. Subflow ──────────────────────────────────────────────────────────────

fn subflow_parent(id: &str, extra: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: sub\n    \
         type: io.kestra.plugin.core.flow.Subflow\n    namespace: t\n    \
         flowId: child\n{extra}"
    )
}

async fn the_child(pool: &sqlx::PgPool, parent: Uuid) -> Uuid {
    sqlx::query_scalar("SELECT id FROM workflow.executions WHERE trigger_execution_id = $1")
        .bind(parent)
        .fetch_one(pool)
        .await
        .expect("one subflow child")
}

/// `wait: true` (the default): the parent's run ends no earlier than the
/// child, and has no end while the child runs.
#[sqlx::test(migrations = "../../migrations")]
async fn a_waiting_subflow_run_ends_after_its_child(pool: sqlx::PgPool) {
    let (db, ws, user) = seed(&pool).await;
    file_flow(&db, ws, user, &one_log("child")).await;
    let parent = start(&pool, ws, &subflow_parent("waits", "")).await;

    let child = the_child(&pool, parent).await;
    assert_eq!(
        run(&pool, parent, "sub").await.ended_at,
        None,
        "the child has not run, so the waiting run has no end"
    );
    drain(&pool, &Succeeds).await;

    let (c, sub) = (
        execution(&pool, child).await,
        run(&pool, parent, "sub").await,
    );
    assert_eq!(
        (c.state.as_str(), sub.state.as_str()),
        ("SUCCESS", "SUCCESS")
    );
    let c_end = c.ended_at.expect("the child ended");
    assert!(
        sub.ended_at.expect("the run ended") >= c_end,
        "{sub:?} vs {c:?}"
    );
    assert!(
        sub.started_at.unwrap() <= c.started_at.unwrap(),
        "{sub:?} vs {c:?}"
    );
    assert!(execution(&pool, parent).await.ended_at.unwrap() >= c_end);
    assert_invariants(&pool, true).await;
}

/// `wait: false`: the parent's run ends at once — while the child has not
/// even run.
#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_that_does_not_wait_ends_at_once(pool: sqlx::PgPool) {
    let (db, ws, user) = seed(&pool).await;
    file_flow(&db, ws, user, &one_log("child")).await;
    let parent = start(&pool, ws, &subflow_parent("nowait", "    wait: false\n")).await;

    let child = the_child(&pool, parent).await;
    let sub = run(&pool, parent, "sub").await;
    let c = execution(&pool, child).await;
    assert_eq!(sub.state, "SUCCESS", "{sub:?}");
    assert!(sub.ended_at.is_some(), "the run ended at spawn: {sub:?}");
    assert_eq!(c.ended_at, None, "while the child has not run: {c:?}");

    drain(&pool, &Succeeds).await;
    let c = execution(&pool, child).await;
    assert!(sub.ended_at.unwrap() <= c.ended_at.expect("the child ended later"));
    assert_eq!(
        run(&pool, parent, "sub").await.ended_at,
        sub.ended_at,
        "unmoved"
    );
    assert_invariants(&pool, true).await;
}

// ── 7. Loop iterations ──────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn every_loop_iteration_has_its_own_times(pool: sqlx::PgPool) {
    let (_db, ws, _) = seed(&pool).await;
    let source = "id: loop\nnamespace: t\ntasks:\n  - id: l\n    \
                  type: io.kestra.plugin.core.flow.Loop\n    values: '[\"x\",\"y\",\"z\"]'\n    \
                  tasks:\n      - id: body\n        type: io.kestra.plugin.core.log.Log\n        \
                  message: hi\n";
    let root = start(&pool, ws, source).await;
    drain(&pool, &Succeeds).await;

    let iterations: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE parent_execution_id = $1 \
            AND loop_index IS NOT NULL ORDER BY loop_index",
    )
    .bind(root)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(iterations.len(), 3);
    let l = run(&pool, root, "l").await;
    assert_eq!(l.state, "SUCCESS", "{l:?}");
    let mut spans = Vec::new();
    for it in iterations {
        let e = execution(&pool, it).await;
        let (s, end) = (
            e.started_at.expect("own start"),
            e.ended_at.expect("own end"),
        );
        assert!(
            s <= end && s >= l.started_at.unwrap() && end <= l.ended_at.unwrap(),
            "{e:?} in {l:?}"
        );
        let body = run(&pool, it, "body").await;
        assert!(
            body.started_at.unwrap() >= s && body.ended_at.unwrap() <= end,
            "{body:?}"
        );
        spans.push((s, end));
    }
    spans.dedup();
    assert_eq!(
        spans.len(),
        3,
        "three iterations, three distinct spans: {spans:?}"
    );
    assert_invariants(&pool, true).await;
}

// ── 8. SLA cancel ───────────────────────────────────────────────────────────

/// An `sla:` that cancels at creation ends the execution, which never ran.
#[sqlx::test(migrations = "../../migrations")]
async fn an_sla_cancel_ends_the_execution(pool: sqlx::PgPool) {
    let (_db, ws, _) = seed(&pool).await;
    let source = "id: sla\nnamespace: t\nsla:\n  - id: never\n    type: EXECUTION_ASSERTION\n    \
                  behavior: CANCEL\n    assert: \"{{ false }}\"\ntasks:\n  - id: a\n    \
                  type: io.kestra.plugin.core.log.Log\n    message: hi\n";
    let ex = start(&pool, ws, source).await;
    drain(&pool, &Succeeds).await;

    let e = execution(&pool, ex).await;
    assert_eq!(e.state, "CANCELLED", "{e:?}");
    assert!(
        e.ended_at.is_some(),
        "a cancelled execution has an end: {e:?}"
    );
    assert_eq!(
        e.started_at, None,
        "it never entered RUNNING ({:?}), so it has no start",
        e.history
    );
    assert_invariants(&pool, true).await;
}

// ── 10. SKIPPED by runIf ────────────────────────────────────────────────────

#[sqlx::test(migrations = "../../migrations")]
async fn a_run_skipped_by_run_if_has_an_end_and_no_start(pool: sqlx::PgPool) {
    let (_db, ws, _) = seed(&pool).await;
    let source = "id: skip\nnamespace: t\ntasks:\n  - id: a\n    \
                  type: io.kestra.plugin.core.log.Log\n    message: hi\n    runIf: \"false\"\n  \
                  - id: b\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n";
    let ex = start(&pool, ws, source).await;
    drain(&pool, &Succeeds).await;

    let a = run(&pool, ex, "a").await;
    assert_eq!(a.state, "SKIPPED", "{a:?}");
    assert_eq!(a.started_at, None, "a skipped run never ran: {a:?}");
    assert!(a.ended_at.is_some(), "but it did end: {a:?}");
    assert!(
        attempt_ticks(&a)[0].1.is_some(),
        "and so did its one attempt: {a:?}"
    );
    assert_eq!(execution(&pool, ex).await.state, "SUCCESS");
    assert_invariants(&pool, true).await;
}

// ── Review minor 2: leave and re-enter a terminal state in one settle ─────

/// A `LoopUntil` descendant that `runIf` skips is SKIPPED on every
/// iteration. Each new iteration's settle empties it (`RestartLoop`),
/// rebuilds it (`RecreateRun`) and skips it again, so the stored row is
/// terminal before that settle and terminal after it. Its end must still be
/// the new iteration's, not the first one's. The old row-against-row rule
/// kept the first end.
#[sqlx::test(migrations = "../../migrations")]
async fn a_loop_until_descendant_skipped_again_gets_a_new_end(pool: sqlx::PgPool) {
    let (_db, ws, _) = seed(&pool).await;
    let source = "id: until\nnamespace: t\ntasks:\n  - id: loop\n    \
                  type: io.kestra.plugin.core.flow.LoopUntil\n    \
                  condition: \"{{ false }}\"\n    \
                  checkFrequency:\n      interval: PT0.2S\n      maxIterations: 3\n    \
                  tasks:\n      \
                  - id: skipped\n        type: io.kestra.plugin.core.log.Log\n        \
                  message: never\n        runIf: \"false\"\n      \
                  - id: work\n        type: io.kestra.plugin.core.log.Log\n        \
                  message: hi\n";
    let ex = start(&pool, ws, source).await;
    drain(&pool, &Succeeds).await;
    let mut ends = vec![run(&pool, ex, "skipped").await];
    for _ in 0..4 {
        tokio::time::sleep(Duration::from_millis(300)).await;
        worker::wake(&pool, 16).await.expect("wake");
        drain(&pool, &Succeeds).await;
        ends.push(run(&pool, ex, "skipped").await);
    }
    let msgs: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT task_id, state, message FROM workflow.task_runs WHERE execution_id = $1 ORDER BY run_ix",
    )
    .bind(ex)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(execution(&pool, ex).await.state, "SUCCESS", "{msgs:?}");
    for r in &ends {
        assert_eq!((r.state.as_str(), r.started_at), ("SKIPPED", None), "{r:?}");
    }
    let mut distinct: Vec<DateTime<Utc>> = ends.iter().map(|r| r.ended_at.unwrap()).collect();
    distinct.dedup();
    assert!(
        distinct.len() >= 2 && distinct.windows(2).all(|w| w[0] < w[1]),
        "each iteration's skip has its own, later end: {ends:?}"
    );
    assert_invariants(&pool, true).await;
}

// ── 11. The invariant under later writes ────────────────────────────────────

/// Once terminal, a row's end does not move — not on an outputs write, not on
/// a rewrite in the same terminal state, not on a move to another terminal.
/// Leaving a terminal state clears it. Driven through `persist`'s OWN
/// statements (`worker::RUN_STATE_UPDATE`, `worker::EXECUTION_STATE_UPDATE`).
#[sqlx::test(migrations = "../../migrations")]
async fn a_terminal_rows_end_does_not_move_on_later_writes(pool: sqlx::PgPool) {
    let (_db, ws, _) = seed(&pool).await;
    let ex = start(&pool, ws, &one_log("later")).await;
    drain(&pool, &Succeeds).await;
    let e0 = execution(&pool, ex).await;
    let r0 = run(&pool, ex, "a").await;
    let (e_end, r_end) = (e0.ended_at.unwrap(), r0.ended_at.unwrap());
    tokio::time::sleep(Duration::from_millis(20)).await;

    // An outputs and `updated_at` bump, the write a finished run does receive.
    sqlx::query(
        "UPDATE workflow.task_runs SET outputs = '{\"x\":1}', updated_at = now() \
          WHERE execution_id = $1",
    )
    .bind(ex)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(run(&pool, ex, "a").await.ended_at, Some(r_end));

    // `persist`'s run write, re-sent with the row's own values (what a lease
    // release on an unchanged run writes), then as a terminal-to-terminal move.
    for (state, extra) in [("SUCCESS", None), ("WARNING", Some("WARNING"))] {
        rewrite_run(&pool, ex, 0, state, extra).await;
        let r = run(&pool, ex, "a").await;
        assert_eq!(
            (r.started_at, r.ended_at),
            (r0.started_at, Some(r_end)),
            "{state}: {r:?}"
        );
    }
    for (state, extra) in [("SUCCESS", None), ("WARNING", Some("WARNING"))] {
        rewrite_execution(&pool, ex, state, extra).await;
        let e = execution(&pool, ex).await;
        assert_eq!(
            (e.started_at, e.ended_at),
            (e0.started_at, Some(e_end)),
            "{state}: {e:?}"
        );
    }

    // Leaving a terminal state clears the end and keeps the start; entering
    // one again stamps a NEW end.
    rewrite_run(&pool, ex, 0, "CREATED", Some("CREATED")).await;
    let reopened = run(&pool, ex, "a").await;
    assert_eq!(
        (reopened.started_at, reopened.ended_at),
        (r0.started_at, None)
    );
    rewrite_execution(&pool, ex, "RUNNING", Some("RUNNING")).await;
    assert_eq!(execution(&pool, ex).await.ended_at, None);
    rewrite_execution(&pool, ex, "FAILED", Some("FAILED")).await;
    assert!(execution(&pool, ex).await.ended_at.unwrap() > e_end);
    rewrite_run(&pool, ex, 0, "FAILED", Some("FAILED")).await;
    let failed_end = run(&pool, ex, "a").await.ended_at.unwrap();
    assert!(failed_end > r_end);

    // Review minor 2: leave a terminal state and re-enter one inside ONE
    // write. The stored row is terminal before and after; the end must still
    // be the new one, not the stale one.
    tokio::time::sleep(Duration::from_millis(20)).await;
    rewrite_run(
        &pool,
        ex,
        0,
        "FAILED",
        Some("RETRYING,CREATED,RUNNING,FAILED"),
    )
    .await;
    let again = run(&pool, ex, "a").await;
    assert!(again.ended_at.unwrap() > failed_end, "{again:?}");
    assert_eq!(
        again.started_at, r0.started_at,
        "a retry keeps the first start"
    );
    assert_invariants(&pool, true).await;
}

/// A row from BEFORE migration 0183 -- terminal or running, with no times --
/// is not given a start or an end it did not have. Only a transition it
/// actually makes afterwards is stamped.
#[sqlx::test(migrations = "../../migrations")]
async fn a_row_from_before_the_migration_is_not_guessed(pool: sqlx::PgPool) {
    let (_db, ws, _) = seed(&pool).await;
    let ex = start(&pool, ws, &two_logs("legacy")).await;
    drain(&pool, &Succeeds).await;
    // Make it look like a pre-0183 row: same states, no times.
    sqlx::query("UPDATE workflow.executions SET started_at = NULL, ended_at = NULL WHERE id = $1")
        .bind(ex)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE workflow.task_runs SET started_at = NULL, ended_at = NULL WHERE execution_id = $1",
    )
    .bind(ex)
    .execute(&pool)
    .await
    .unwrap();

    rewrite_run(&pool, ex, 0, "SUCCESS", None).await;
    rewrite_execution(&pool, ex, "SUCCESS", None).await;
    assert_eq!(
        (
            run(&pool, ex, "a").await.started_at,
            run(&pool, ex, "a").await.ended_at
        ),
        (None, None),
        "a terminal row rewritten is not given an end"
    );
    assert_eq!(execution(&pool, ex).await.ended_at, None);

    // A legacy row that was RUNNING when the migration ran and then really
    // ends: the end is real and is written; the start stays unknown.
    rewrite_run(&pool, ex, 1, "RUNNING", Some("RUNNING")).await;
    sqlx::query(
        "UPDATE workflow.task_runs SET started_at = NULL WHERE execution_id = $1 AND run_ix = 1",
    )
    .bind(ex)
    .execute(&pool)
    .await
    .unwrap();
    rewrite_run(&pool, ex, 1, "SUCCESS", Some("SUCCESS")).await;
    let b = run(&pool, ex, "b").await;
    assert_eq!(b.started_at, None, "{b:?}");
    assert!(b.ended_at.is_some(), "{b:?}");
    assert_invariants(&pool, false).await;
}

/// `history` with `extra`'s comma-separated states appended.
fn extended(history: &Value, extra: Option<&str>) -> Value {
    let mut h = history.clone();
    for s in extra.into_iter().flat_map(|e| e.split(',')) {
        h.as_array_mut().unwrap().push(Value::from(s));
    }
    h
}

/// What `persist` would bind for this write: the production rule,
/// `worker::stamps_entered`, over the loaded and the written history.
fn entered(loaded: &Value, written: &Value) -> (bool, bool) {
    let names = |v: &Value| -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_string())
            .collect()
    };
    let (a, b) = (names(loaded), names(written));
    let a: Vec<&str> = a.iter().map(String::as_str).collect();
    let b: Vec<&str> = b.iter().map(String::as_str).collect();
    worker::stamps_entered(&a, &b)
}

/// The columns of a run that `persist`'s run `UPDATE` re-sends.
#[derive(sqlx::FromRow)]
struct RunColumns {
    history: Value,
    attempts: Value,
    rendered: Option<String>,
    rendered_run_if: Option<String>,
    not_before_ms: Option<i64>,
    ended_seq: Option<i64>,
    resume_with: Option<String>,
}

/// Re-send `persist`'s run `UPDATE` with the row's own values and `state`,
/// appending `extra` to its history.
async fn rewrite_run(pool: &sqlx::PgPool, ex: Uuid, run_ix: i32, state: &str, extra: Option<&str>) {
    let row: RunColumns = sqlx::query_as(
        "SELECT history, attempts, rendered, rendered_run_if, not_before_ms, ended_seq, \
                resume_with \
           FROM workflow.task_runs WHERE execution_id = $1 AND run_ix = $2",
    )
    .bind(ex)
    .bind(run_ix)
    .fetch_one(pool)
    .await
    .unwrap();
    let history = extended(&row.history, extra);
    let (running, terminal) = entered(&row.history, &history);
    let hit = sqlx::query(worker::RUN_STATE_UPDATE)
        .bind(state)
        .bind(&history)
        .bind(&row.attempts)
        .bind(row.rendered)
        .bind(row.rendered_run_if)
        .bind(row.not_before_ms)
        .bind(false)
        .bind(ex)
        .bind(run_ix)
        .bind(row.ended_seq)
        .bind(None::<DateTime<Utc>>)
        .bind(None::<i64>)
        .bind(row.resume_with)
        .bind(running)
        .bind(terminal)
        .bind(None::<String>)
        .execute(pool)
        .await
        .expect("persist's run UPDATE");
    assert_eq!(hit.rows_affected(), 1);
}

async fn rewrite_execution(pool: &sqlx::PgPool, ex: Uuid, state: &str, extra: Option<&str>) {
    let loaded = execution(pool, ex).await.history;
    let history = extended(&loaded, extra);
    let (running, terminal) = entered(&loaded, &history);
    let hit = sqlx::query(worker::EXECUTION_STATE_UPDATE)
        .bind(state)
        .bind(&history)
        .bind(ex)
        .bind(running)
        .bind(terminal)
        .execute(pool)
        .await
        .expect("persist's execution UPDATE");
    assert_eq!(hit.rows_affected(), 1);
}

// ── The migration ───────────────────────────────────────────────────────────

/// Every `models::State`, all 17. The `match` makes an 18th variant a compile
/// error here rather than a state this pin silently skips.
fn all_model_states() -> Vec<egret_workflow::models::State> {
    use egret_workflow::models::State::{self, *};
    fn _exhaustive(s: State) {
        match s {
            Created | Submitted | Running | Paused | Restarted | Killing | Success | Warning
            | Failed | Killed | Cancelled | Queued | Retrying | Retried | Skipped | Breakpoint
            | Resubmitted => {}
        }
    }
    vec![
        Created,
        Submitted,
        Running,
        Paused,
        Restarted,
        Killing,
        Success,
        Warning,
        Failed,
        Killed,
        Cancelled,
        Queued,
        Retrying,
        Retried,
        Skipped,
        Breakpoint,
        Resubmitted,
    ]
}

/// `workflow.state_is_terminal`, the one SQL copy of the terminal list used
/// by the stamp helper and by 0183's CHECKs, is EXACTLY
/// `models::State::is_terminated` (spec §3.2's authority) on all 17 states.
/// It agrees with `RunState::is_terminal` on every variant, and answers false
/// for a spelling it does not know.
#[sqlx::test(migrations = "../../migrations")]
async fn the_sql_terminal_list_is_the_models_terminated_list(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let sql_terminal = |wire: String| {
        let pool = pool.clone();
        async move {
            let t: bool = sqlx::query_scalar("SELECT workflow.state_is_terminal($1)")
                .bind(&wire)
                .fetch_one(&pool)
                .await
                .unwrap();
            t
        }
    };
    let states = all_model_states();
    assert_eq!(states.len(), 17);
    for s in states {
        let wire = serde_json::to_value(s)
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(
            sql_terminal(wire.clone()).await,
            s.is_terminated(),
            "{wire}"
        );
    }
    for s in egret_workflow::executor::RunState::ALL {
        assert_eq!(
            sql_terminal(s.wire().to_string()).await,
            s.is_terminal(),
            "{s:?}"
        );
    }
    assert!(!sql_terminal("NOPE".to_string()).await);
    // The constant this file uses is `RunState`'s list.
    let mut want: Vec<&str> = egret_workflow::executor::RunState::ALL
        .iter()
        .filter(|s| s.is_terminal())
        .map(|s| s.wire())
        .collect();
    let mut have = TERMINAL.to_vec();
    want.sort_unstable();
    have.sort_unstable();
    assert_eq!(have, want);
}

/// The CHECKs refuse what a writer bypassing the helper would produce.
#[sqlx::test(migrations = "../../migrations")]
async fn the_schema_refuses_an_end_on_a_live_row_and_an_end_before_the_start(pool: sqlx::PgPool) {
    let (_db, ws, _) = seed(&pool).await;
    let ex = start(&pool, ws, &one_log("checks")).await;
    let live = sqlx::query("UPDATE workflow.executions SET ended_at = now() WHERE id = $1")
        .bind(ex)
        .execute(&pool)
        .await
        .expect_err("a RUNNING execution cannot carry an end");
    assert!(
        live.to_string()
            .contains("workflow_executions_ended_only_when_terminal"),
        "{live}"
    );
    let backwards = sqlx::query(
        "UPDATE workflow.task_runs SET state = 'SUCCESS', ended_at = started_at - interval '1 second' \
          WHERE execution_id = $1",
    )
    .bind(ex)
    .execute(&pool)
    .await
    .expect_err("an end before the start");
    assert!(
        backwards
            .to_string()
            .contains("workflow_task_runs_ends_after_it_starts"),
        "{backwards}"
    );
}

/// **`KILLING` needs no migration**: no CHECK lists the state vocabulary, so
/// both tables take it on a live row, and the one CHECK that reads the state
/// (`*_ended_only_when_terminal`, through `workflow.state_is_terminal`) treats
/// it as live -- an end on a `KILLING` row is refused like one on `RUNNING`.
#[sqlx::test(migrations = "../../migrations")]
async fn the_schema_holds_killing_as_a_live_state(pool: sqlx::PgPool) {
    let (_db, ws, _) = seed(&pool).await;
    let ex = start(&pool, ws, &one_log("killing")).await;
    let tables: [(&str, &'static str, &'static str); 2] = [
        (
            "workflow.executions",
            "UPDATE workflow.executions SET state = 'KILLING', ended_at = NULL WHERE id = $1",
            "UPDATE workflow.executions SET ended_at = now() WHERE id = $1",
        ),
        (
            "workflow.task_runs",
            "UPDATE workflow.task_runs SET state = 'KILLING', ended_at = NULL WHERE execution_id = $1",
            "UPDATE workflow.task_runs SET ended_at = now() WHERE execution_id = $1",
        ),
    ];
    for (table, live, end) in tables {
        let n = sqlx::query(live)
            .bind(ex)
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("{table} refused a live KILLING row: {e}"))
            .rows_affected();
        assert!(n >= 1, "{table}: the fixture wrote no KILLING row");
        let ended = sqlx::query(end)
            .bind(ex)
            .execute(&pool)
            .await
            .expect_err("a KILLING row cannot carry an end");
        assert!(
            ended.to_string().contains("_ended_only_when_terminal"),
            "{table}: {ended}"
        );
    }
}

// ── 14b. An `Exit` (kill-and-exit Task 3, spec §6) ──────────────────────────

/// The corpus shape (`valids/exit`): an `Exit` in an `If`'s `else`, and a task
/// after the `If` that is never created. `state:` here is `{{ inputs.s }}`,
/// rendered by the worker's render pass at `RUNNING`.
fn exit_flow(state: &str) -> String {
    format!(
        "id: exit\nnamespace: t\ninputs:\n  - id: s\n    type: STRING\n    defaults: CANCELLED\n\
         tasks:\n  - id: if\n    type: io.kestra.plugin.core.flow.If\n    condition: \"{{{{ false }}}}\"\n    \
         then:\n      - id: hello\n        type: io.kestra.plugin.core.log.Log\n        message: hi\n    \
         else:\n      - id: exit\n        type: io.kestra.plugin.core.execution.Exit\n        state: {state}\n  \
         - id: end\n    type: io.kestra.plugin.core.log.Log\n    message: end\n"
    )
}

/// Run end to end through the worker: the `Exit` run, its `If` and the
/// execution all end in the rendered state, each with a start and an end, and
/// nothing after the `If` is created. No worker claims the `Exit` run.
#[sqlx::test(migrations = "../../migrations")]
async fn an_exit_ends_its_run_its_parent_and_the_execution(pool: sqlx::PgPool) {
    let (_db, ws, _) = seed(&pool).await;
    let ex = start(&pool, ws, &exit_flow("\"{{ inputs.s }}\"")).await;
    drain(&pool, &Succeeds).await;

    let e = execution(&pool, ex).await;
    assert_eq!(e.state, "CANCELLED", "{e:?}");
    assert_eq!(
        e.history,
        serde_json::json!(["CREATED", "RUNNING", "CANCELLED"])
    );
    let all = runs(&pool, ex).await;
    assert_eq!(
        all.iter().map(|r| r.task_id.as_str()).collect::<Vec<_>>(),
        ["if", "exit"],
        "`end` is never created"
    );
    for r in &all {
        assert_eq!(r.state, "CANCELLED", "{r:?}");
        assert!(r.started_at.is_some() && r.ended_at.is_some(), "{r:?}");
    }
    let type_id: Option<String> = sqlx::query_scalar(
        "SELECT type_id FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'exit'",
    )
    .bind(ex)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        type_id, None,
        "no `type_id`, so no worker claim can take it"
    );
    assert_invariants(&pool, true).await;
}

/// `state: NOPE` is not one of the five: the run fails with a message naming
/// the value, and the failure propagates as any run failure does.
#[sqlx::test(migrations = "../../migrations")]
async fn an_exit_state_outside_the_five_fails_the_run_naming_it(pool: sqlx::PgPool) {
    let (_db, ws, _) = seed(&pool).await;
    let ex = start(&pool, ws, &exit_flow("NOPE")).await;
    drain(&pool, &Succeeds).await;

    assert_eq!(execution(&pool, ex).await.state, "FAILED");
    let (state, message): (String, Option<String>) = sqlx::query_as(
        "SELECT state, message FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'exit'",
    )
    .bind(ex)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, "FAILED");
    let message = message.expect("the run carries why it failed");
    assert!(message.contains("NOPE"), "{message}");
    assert_invariants(&pool, true).await;
}

// ── 15. The parent= index ───────────────────────────────────────────────────

/// `EXPLAIN` of the list's OWN statement (`repo::EXECUTION_LIST_SQL`) with
/// `parent=` set is answered from the two partial indexes that lead with the
/// link columns -- `workflow_executions_loop_run_idx` (0135/0138) and
/// `workflow_executions_trigger_idx` (0138) -- and never by a scan.
///
/// **This is why migration 0183 builds no index.** The spec asked for
/// `(workspace_id, parent_execution_id)` and `(workspace_id,
/// trigger_execution_id)`; with both built, this same `EXPLAIN` chose neither
/// (the first run of this test, recorded in the W7b Task 1 report).
#[sqlx::test(migrations = "../../migrations")]
async fn the_parent_filter_is_served_by_the_partial_indexes(pool: sqlx::PgPool) {
    let (_db, ws, _) = seed(&pool).await;
    // 3 000 roots, and one root with children of both kinds, so a scan of the
    // workspace is far costlier than the two partial indexes.
    sqlx::query(
        "INSERT INTO workflow.executions (workspace_id, namespace, flow_key, source, state, history) \
         SELECT $1, 't', 'f' || i, 's', 'SUCCESS', '[\"CREATED\",\"RUNNING\",\"SUCCESS\"]' \
           FROM generate_series(1, 3000) i",
    )
    .bind(ws)
    .execute(&pool)
    .await
    .unwrap();
    let root: Uuid =
        sqlx::query_scalar("SELECT id FROM workflow.executions WHERE workspace_id = $1 LIMIT 1")
            .bind(ws)
            .fetch_one(&pool)
            .await
            .unwrap();
    for (parent, trigger) in [(Some((root, 0, 0, "v")), None), (None, Some((root, 1)))] {
        insert_child(&pool, ws, parent, trigger).await;
    }
    sqlx::query("ANALYZE workflow.executions")
        .execute(&pool)
        .await
        .unwrap();

    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("SET plan_cache_mode = force_custom_plan")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "PREPARE list_q(uuid, uuid, text, text, text[], timestamptz, timestamptz, boolean, uuid, \
         text, bigint, bigint) AS {}",
        egret_workflow::repo::EXECUTION_LIST_SQL
    )))
    .execute(&mut *conn)
    .await
    .unwrap();
    let plan: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "EXPLAIN EXECUTE list_q('{ws}', NULL, NULL, NULL, NULL, NULL, NULL, false, '{root}', NULL, 50, 0)"
    )))
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    let plan = plan.join("\n");
    println!("{plan}");
    assert!(plan.contains("workflow_executions_loop_run_idx"), "{plan}");
    assert!(plan.contains("workflow_executions_trigger_idx"), "{plan}");
    assert!(!plan.contains("Seq Scan"), "{plan}");
    // The control: without `parent`, the same statement over the same rows is
    // NOT an index lookup on the link columns, so the assertion above is about
    // the filter and not about a plan this table always gets.
    let unfiltered: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "EXPLAIN EXECUTE list_q('{ws}', NULL, NULL, NULL, NULL, NULL, NULL, false, NULL, NULL, 50, 0)"
    )))
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    let unfiltered = unfiltered.join("\n");
    assert!(
        !unfiltered.contains("workflow_executions_loop_run_idx")
            && !unfiltered.contains("workflow_executions_trigger_idx"),
        "{unfiltered}"
    );
}

/// A child row of either kind, in the shape migration 0138's CHECKs admit.
async fn insert_child(
    pool: &sqlx::PgPool,
    ws: Uuid,
    parent: Option<(Uuid, i32, i32, &str)>,
    trigger: Option<(Uuid, i32)>,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO workflow.executions \
           (workspace_id, namespace, flow_key, source, state, history, \
            parent_execution_id, parent_run_ix, loop_index, loop_value, \
            trigger_execution_id, trigger_run_ix) \
         VALUES ($1, 't', 'child', 's', 'SUCCESS', '[\"CREATED\",\"RUNNING\",\"SUCCESS\"]', \
                 $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(ws)
    .bind(parent.map(|p| p.0))
    .bind(parent.map(|p| p.1))
    .bind(parent.map(|p| p.2))
    .bind(parent.map(|p| p.3))
    .bind(trigger.map(|t| t.0))
    .bind(trigger.map(|t| t.1))
    .fetch_one(pool)
    .await
    .expect("insert child execution")
}

// ── 12. The coverage guard ──────────────────────────────────────────────────

/// One SQL statement in the source that writes to `workflow.executions` or
/// `workflow.task_runs`.
#[derive(Debug)]
struct Write {
    at: String,
    text: String,
    /// The statement is the argument of a `format!`. Its SQL is assembled at
    /// runtime, so this scan cannot read it.
    formatted: bool,
}

/// Every `UPDATE`/`INSERT INTO` of the two tables in `text`, each with its
/// statement text. The text runs from the keyword to the first `.bind(`,
/// `.execute(`, `.fetch…` or `);` after it, which is the whole string (or
/// `concat!`) the statement is built from, macro invocations included.
fn writes_in(file: &str, text: &str) -> Vec<Write> {
    let table = regex::Regex::new(
        r#"(?i)\b(UPDATE|INSERT\s+INTO)[\s\\"]+workflow\s*\.\s*(executions|task_runs)\b"#,
    )
    .unwrap();
    table
        .find_iter(text)
        .map(|m| {
            let rest = &text[m.start()..];
            let end = [".bind(", ".execute(", ".fetch", ");"]
                .iter()
                .filter_map(|e| rest.find(e))
                .min()
                .unwrap_or(rest.len());
            let line = 1 + text[..m.start()].matches('\n').count();
            // The opening of the call this literal sits in: a `format!(` up
            // to one line before the keyword.
            let lead = &text[text[..m.start()]
                .rfind('\n')
                .map_or(0, |i| text[..i].rfind('\n').map_or(0, |j| j + 1))
                ..m.start()];
            Write {
                at: format!("{file}:{line}"),
                text: rest[..end].to_string(),
                formatted: lead.contains("format!("),
            }
        })
        .collect()
}

/// `text` with every string literal's CONTENTS and every `//` comment removed,
/// so what remains is the Rust around the SQL. `text` is a [`Write`]'s, which
/// begins inside a string literal. A macro name counts only as a
/// call that the compiler will expand, not as words inside the SQL or inside
/// a comment.
fn code_only(text: &str) -> String {
    let strings = regex::Regex::new(r#"(?s)"(?:[^"\\]|\\.)*""#).unwrap();
    let comments = regex::Regex::new(r"//[^\n]*").unwrap();
    // A statement's text starts INSIDE the literal that holds its keyword,
    // so the opening quote is supplied.
    let quoted = format!("\"{text}");
    comments
        .replace_all(&strings.replace_all(&quoted, "\"\""), "")
        .into_owned()
}

/// The `SET` list of an `UPDATE`, which runs from `SET` to `WHERE`.
fn set_list(t: &str) -> &str {
    let set = regex::Regex::new(r"(?i)\bSET\b").unwrap();
    let start = set.find(t).map_or(0, |m| m.end());
    let stop = regex::Regex::new(r"(?i)\bWHERE\b")
        .unwrap()
        .find(&t[start..])
        .map_or(t.len(), |m| start + m.start());
    &t[start..stop]
}

/// Does this `SET` list write the `state` column: `state =` in it, or
/// `state` in a row-constructor target `(…, state, …) = (…)`?
fn set_writes_state(set: &str) -> bool {
    let assign = regex::Regex::new(r"(?i)\bstate\s*=").unwrap();
    let tuple = regex::Regex::new(r"(?is)\([^()]*\bstate\b[^()]*\)\s*=").unwrap();
    assign.is_match(set) || tuple.is_match(set)
}

/// An `INSERT`'s `ON CONFLICT … DO UPDATE` clause, from after `DO UPDATE`
/// on (still inside the statement's SQL, so [`code_only`] reads it). `None`
/// when there is no upsert.
fn upsert_clause(t: &str) -> Option<&str> {
    regex::Regex::new(r"(?is)\bON\s+CONFLICT\b.*?\bDO\s+UPDATE\b")
        .unwrap()
        .find(t)
        .map(|m| &t[m.end()..])
}

/// Does this statement write the `state` column?
///
/// - An `UPDATE` writes it when its `SET` list does ([`set_writes_state`]).
///   A `WHERE … state = 'RUNNING'` only reads it.
/// - An `INSERT` writes it when `state` is in its column list, or when its
///   `ON CONFLICT … DO UPDATE SET` list writes it (final review M4 a).
fn writes_state(w: &Write) -> bool {
    let t = &w.text;
    if t.trim_start().to_ascii_uppercase().starts_with("UPDATE") {
        set_writes_state(set_list(t))
    } else {
        let open = t.find('(').unwrap_or(0);
        let close = t[open..].find(')').map(|i| open + i).unwrap_or(t.len());
        regex::Regex::new(r"(?i)\bstate\b")
            .unwrap()
            .is_match(&t[open..close])
            || upsert_clause(t).is_some_and(|c| set_writes_state(set_list(c)))
    }
}

/// Does this `UPDATE` (or `DO UPDATE`) clause stamp through `stamp_set!`?
/// The macro must be a CALL in the code around the SQL, not inside a string
/// or a comment (review mutation M4), and its first two arguments must be
/// the placeholders that `state =` and `history =` bind (review mutation
/// M5). `clause` begins inside the SQL literal.
fn set_stamped(clause: &str) -> bool {
    if !code_only(clause).contains("stamp_set!(") {
        return false;
    }
    let set = set_list(clause);
    let placeholder = |col: &str| {
        regex::Regex::new(&format!(r"(?i)\b{col}\s*=\s*(\$\d+)"))
            .unwrap()
            .captures(set)
            .map(|c| c[1].to_string())
    };
    let args = regex::Regex::new(r#"stamp_set!\(\s*"(\$\d+)"\s*,\s*"(\$\d+)""#)
        .unwrap()
        .captures(clause)
        .map(|c| (c[1].to_string(), c[2].to_string()));
    match (placeholder("state"), placeholder("history"), args) {
        (Some(s), Some(h), Some((a, b))) => s == a && h == b,
        _ => false,
    }
}

/// An `INSERT`'s upsert is an `UPDATE`: when its `DO UPDATE SET` writes
/// `state`, it must stamp like one (final review M4 a). True when there is
/// no upsert or it leaves `state` alone.
fn upsert_stamped(w: &Write) -> bool {
    match upsert_clause(&w.text) {
        Some(c) if set_writes_state(set_list(c)) => set_stamped(c),
        _ => true,
    }
}

/// Does it stamp the timestamps through `worker::stamps`?
///
/// - An `UPDATE` through `stamp_set!` ([`set_stamped`]).
/// - An `INSERT` through `stamp_columns!` and `stamp_values!`, and its
///   upsert, if it writes `state`, through `stamp_set!` ([`upsert_stamped`]).
fn stamps(w: &Write) -> bool {
    if w.text
        .trim_start()
        .to_ascii_uppercase()
        .starts_with("UPDATE")
    {
        set_stamped(&w.text)
    } else {
        let code = code_only(&w.text);
        code.contains("stamp_columns!(") && code.contains("stamp_values!(") && upsert_stamped(w)
    }
}

/// The SQL of the string literal a [`Write`]'s text begins inside: up to its
/// first unescaped `"`, with Rust's escapes undone (`\` + newline swallows the
/// newline and the next line's leading whitespace). `None` when the literal
/// does not close, or uses an escape this scan does not read.
fn first_literal(text: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                '\n' => {
                    while chars.peek().is_some_and(|c| c.is_whitespace()) {
                        chars.next();
                    }
                }
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                'n' => out.push('\n'),
                _ => return None,
            },
            c => out.push(c),
        }
    }
    None
}

/// `list` split at its top-level commas (not inside `(…)` or `'…'`), each
/// item trimmed.
fn top_level_items(list: &str) -> Vec<String> {
    let (mut items, mut cur, mut depth, mut quoted) = (Vec::new(), String::new(), 0i32, false);
    for c in list.chars() {
        match c {
            '\'' => quoted = !quoted,
            '(' if !quoted => depth += 1,
            ')' if !quoted => depth -= 1,
            ',' if !quoted && depth == 0 => {
                items.push(cur.trim().to_string());
                cur.clear();
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    items.push(cur.trim().to_string());
    items
}

/// The byte offset of the first top-level (not parenthesised, not quoted)
/// `FROM` keyword in `sql`.
fn top_level_from(sql: &str) -> Option<usize> {
    let from = regex::Regex::new(r"(?i)\bFROM\b").unwrap();
    let at = from.find_iter(sql).map(|m| m.start()).find(|&at| {
        let before = &sql[..at];
        let depth = before.matches('(').count() as i64 - before.matches(')').count() as i64;
        depth == 0 && before.matches('\'').count().is_multiple_of(2)
    });
    at
}

/// The four columns the stamps rule ties together.
const STAMPED: [&str; 4] = ["state", "history", "started_at", "ended_at"];

/// **A verbatim copy of a row of the same table**, the one `INSERT` that
/// writes `state` without the stamp macros and is still allowed (package 7b,
/// `worker/copy.rs`' `INSERT_COPIED_RUNS`). A copy of a row whose
/// timestamps were written under the rule carries them; stamping `now()`
/// would invent a start and an end the run never had (Kestra copies a task
/// run with its state history).
///
/// Recognised only when ALL of these hold, read from the ONE string literal
/// the statement starts in:
///
/// - it is `INSERT INTO workflow.<t> (cols…) SELECT items… FROM …`, with as
///   many items as columns, the select list ending at a top-level `FROM`;
/// - `state`, `history`, `started_at` and `ended_at` are ALL in the columns,
///   and each one's item is exactly `<alias>.<that same column>`;
/// - the four aliases are one alias;
/// - the `FROM` binds that alias DIRECTLY to `workflow.<t>`, the table
///   inserted into: `FROM workflow.<t> o` or `JOIN workflow.<t> o` at the
///   top level, not inside a derived table `(SELECT … ) o` (final review
///   M4 b);
/// - an `ON CONFLICT … DO UPDATE SET` that writes `state` stamps it like any
///   `UPDATE` ([`upsert_stamped`], final review M4 a).
///
/// A state from a bind, a literal, another alias, an alias that is not that
/// table (an `unnest` of bound arrays), a swapped column, or a missing
/// timestamp column is not a verbatim copy, and is refused as any unstamped
/// write is.
fn copies_verbatim(w: &Write) -> bool {
    let Some(sql) = first_literal(&w.text) else {
        return false;
    };
    let head =
        regex::Regex::new(r"(?is)^\s*INSERT\s+INTO\s+workflow\s*\.\s*(executions|task_runs)\s*\(")
            .unwrap();
    let Some(h) = head.captures(&sql) else {
        return false;
    };
    let table = h[1].to_string();
    let open = h.get(0).unwrap().end();
    let Some(close) = sql[open..].find(')').map(|i| open + i) else {
        return false;
    };
    let columns: Vec<String> = top_level_items(&sql[open..close])
        .into_iter()
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let rest = &sql[close + 1..];
    let Some(select) = regex::Regex::new(r"(?is)^\s*SELECT\b").unwrap().find(rest) else {
        return false;
    };
    let rest = &rest[select.end()..];
    let Some(from) = top_level_from(rest) else {
        return false;
    };
    let items = top_level_items(&rest[..from]);
    if items.len() != columns.len() {
        return false;
    }
    let item = regex::Regex::new(r"(?i)^([a-z_][a-z0-9_]*)\s*\.\s*([a-z_][a-z0-9_]*)$").unwrap();
    let mut alias: Option<String> = None;
    for col in STAMPED {
        let Some(ix) = columns.iter().position(|c| c == col) else {
            return false;
        };
        let Some(m) = item.captures(&items[ix]) else {
            return false;
        };
        if !m[2].eq_ignore_ascii_case(col) {
            return false;
        }
        let a = m[1].to_ascii_lowercase();
        if *alias.get_or_insert_with(|| a.clone()) != a {
            return false;
        }
    }
    let alias = alias.expect("four columns were read");
    let source = &rest[from..];
    let bound = regex::Regex::new(&format!(
        r"(?i)\b(?:FROM|JOIN)\s+workflow\s*\.\s*{table}\s+(?:AS\s+)?{alias}\b"
    ))
    .unwrap()
    .find_iter(source)
    .any(|m| {
        let before = &source[..m.start()];
        before.matches('(').count() == before.matches(')').count()
            && before.matches('\'').count().is_multiple_of(2)
    });
    bound && upsert_stamped(w)
}

fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// **Test 12: a statement that writes `state` on either table and does not
/// stamp the timestamps fails here, by file and line.**
///
/// It scans the `src/` of EVERY crate in the workspace, because a writer of
/// these tables outside this crate would bypass the rule just as well. Its own
/// reach is checked twice, so a scanner that silently finds nothing cannot
/// pass:
///
/// - the seven writers enumerated in the W7b Task 1 report must each be
///   found and classified as state writers;
/// - the classifier is run on hand-written statements, including every
///   bypass the Task 1 review built, and must flag each one.
///
/// # What it refuses
///
/// - `SET state =` with any spacing, and a row-constructor
///   `SET (…, state, …) = (…)`, without `stamp_set!`;
/// - a `stamp_set!` that appears only inside the SQL string or a comment;
/// - a `stamp_set!` whose first two arguments are not the placeholders that
///   `state =` and `history =` bind;
/// - an `INSERT` with `state` in its columns but without both
///   `stamp_columns!` and `stamp_values!`, unless it is a VERBATIM copy
///   ([`copies_verbatim`]): an `INSERT … SELECT` whose `state`, `history`,
///   `started_at` and `ended_at` are each the same-named column of ONE alias
///   of the table inserted into. That is the one allowance, and it is
///   structural, not a file or line: a copy that takes `state` or `history`
///   from a bind, a literal or another alias while carrying the old
///   timestamps is refused, and so is one that leaves a timestamp column
///   out. A copy that CHANGES a copied row's state must do it with a later
///   stamped write (package 7b: `restart::copy_job`'s settle, through
///   `persist`'s `stamp_set!`);
/// - an `INSERT … ON CONFLICT … DO UPDATE SET` that writes `state` without
///   `stamp_set!` on its placeholders, stamped `INSERT` and verbatim copy
///   alike (final review M4 a), and a "copy" whose alias is a derived table
///   `(SELECT … ) o` rather than the table itself (final review M4 b);
/// - ANY `UPDATE`/`INSERT` of these tables built with `format!`, and any
///   `UPDATE {…}`/`INSERT INTO {…}` whose table name is interpolated, in
///   every crate. None exists today; a new one is refused rather than
///   trusted.
///
/// # What it cannot see (honest limits)
///
/// It is LEXICAL. It does not see:
/// - SQL that reaches these tables some other way: a table name held in a
///   `const` or a variable, a view, a function, a trigger, or `sqlx::query!`
///   with an external file;
/// - whether `stamp_set!`'s two booleans are bound to `stamps::Entered`'s
///   values (the binds are Rust, not SQL);
/// - whether an `INSERT`'s `stamp_values!` arguments are the placeholders
///   of its `state`/`history` values;
/// - an `UPDATE` whose state write it cannot recognise as one;
/// - whether the row a verbatim copy reads was itself written under the rule
///   (a pre-0183 row is copied with its NULLs, as it should be), or whether
///   a later stamped write does follow a copy whose state must change.
///   `tests/replay.rs::a_copy_keeps_the_stamps_rule` measures the one copy.
///
/// Migration 0183's CHECKs catch only part of a bypass at runtime: an end on
/// a live row, or an end before its start. A bypass that writes a terminal
/// state and leaves `ended_at` NULL passes both, and looks like a pre-0183
/// row. The scenario tests in this file are what catch a wrong VALUE on the
/// paths that exist.
#[test]
fn every_state_write_stamps_the_timestamps() {
    // The classifier's own controls: the review's five bypasses (M1..M5)
    // and the forms that must pass.
    let probe = |text: &str| Write {
        at: "probe".into(),
        text: text.into(),
        formatted: false,
    };
    let refused = [
        // M1: a bare SET state.
        "UPDATE workflow.executions SET state = 'KILLED' WHERE id = $1",
        "UPDATE workflow.task_runs\n    SET   state=$1, updated_at = now() WHERE id = $2",
        // M2: a row constructor.
        "UPDATE workflow.task_runs SET (state, history) = ($1, $2) WHERE id = $3",
        // M4: the macro name only inside the SQL, or inside a comment.
        "UPDATE workflow.task_runs SET state = $1, history = $2 /* stamp_set!( */ WHERE id = $3\"",
        "UPDATE workflow.task_runs SET state = $1, history = $2, \",\n // stamp_set!(\"$1\", \"$2\", \"$4\", \"$5\"),\n \" WHERE id = $3\"",
        // M5: swapped arguments.
        "UPDATE workflow.executions SET state = $1, history = $2, \", stamp_set!(\"$2\", \"$1\", \"$4\", \"$5\"), \" WHERE id = $3\"",
        // An INSERT with state and no stamps.
        "INSERT INTO workflow.executions (id, state) VALUES ($1, 'CREATED')",
    ];
    for text in refused {
        let w = probe(text);
        assert!(writes_state(&w) && !stamps(&w), "must be refused: {text}");
    }
    let read = probe("UPDATE workflow.task_runs SET claimed_at = NULL WHERE state = 'RUNNING'");
    assert!(
        !writes_state(&read),
        "a WHERE that reads `state` is not a write"
    );
    let good = probe(
        "UPDATE workflow.executions SET state = $1, history = $2, \", \
         stamp_set!(\"$1\", \"$2\", \"$4\", \"$5\"), \" WHERE id = $3\"",
    );
    assert!(writes_state(&good) && stamps(&good));
    // The verbatim-copy allowance: the shape `worker/copy.rs` writes passes,
    // and every copy that takes a stamped column from anywhere but the copied
    // row of the same table is refused. Each probe is a Write's text as the
    // scan sees it: it starts inside the literal and the literal closes.
    let copy = |cols: &str, items: &str, from: &str| {
        probe(&format!(
            "INSERT INTO workflow.task_runs \\\n  ({cols}) \\\nSELECT {items} \\\n  FROM {from}\";"
        ))
    };
    let cols = "execution_id, run_ix, state, history, attempts, started_at, ended_at";
    let unnest = "unnest($3::int[], $4::int[]) AS m(old_ix, new_ix) \
                  JOIN workflow.task_runs o ON o.execution_id = $2 AND o.run_ix = m.old_ix";
    let verbatim = copy(
        cols,
        "$1, m.new_ix, o.state, o.history, m.attempts, o.started_at, \
         CASE WHEN o.x IS NOT NULL THEN COALESCE(o.y, now()) END, o.ended_at",
        unnest,
    );
    // (One item too many above: a miscount is refused.)
    assert!(writes_state(&verbatim) && !copies_verbatim(&verbatim));
    let verbatim = copy(
        cols,
        "$1, m.new_ix, o.state, o.history, m.attempts, o.started_at, o.ended_at",
        unnest,
    );
    assert!(
        writes_state(&verbatim) && copies_verbatim(&verbatim),
        "a verbatim copy is allowed"
    );
    let not_verbatim = [
        // `state` from a bind, the old timestamps carried.
        copy(
            cols,
            "$1, m.new_ix, $5, o.history, m.attempts, o.started_at, o.ended_at",
            unnest,
        ),
        // `state` and `history` as literals: a reset written by the copy.
        copy(
            cols,
            "$1, m.new_ix, 'CREATED', '[\"CREATED\"]', m.attempts, o.started_at, o.ended_at",
            unnest,
        ),
        // `state` from another alias.
        copy(
            cols,
            "$1, m.new_ix, m.state, o.history, m.attempts, o.started_at, o.ended_at",
            unnest,
        ),
        // All four from an alias that is not the table: bound arrays.
        copy(
            cols,
            "$1, m.new_ix, m.state, m.history, m.attempts, m.started_at, m.ended_at",
            "unnest($3::text[], $4::jsonb[], $5::jsonb[], $6::timestamptz[], $7::timestamptz[]) \
             AS m(state, history, attempts, started_at, ended_at)",
        ),
        // The timestamps swapped.
        copy(
            cols,
            "$1, m.new_ix, o.state, o.history, m.attempts, o.ended_at, o.started_at",
            unnest,
        ),
        // A timestamp invented beside a copied state.
        copy(
            cols,
            "$1, m.new_ix, o.state, o.history, m.attempts, o.started_at, NULL",
            unnest,
        ),
        // `ended_at` left out: a copied terminal state with no end.
        copy(
            "execution_id, run_ix, state, history, attempts, started_at",
            "$1, m.new_ix, o.state, o.history, m.attempts, o.started_at",
            unnest,
        ),
        // The alias is the OTHER table.
        copy(
            cols,
            "$1, m.new_ix, o.state, o.history, m.attempts, o.started_at, o.ended_at",
            "unnest($3::int[]) AS m(new_ix) JOIN workflow.executions o ON o.id = $2",
        ),
        // A VALUES insert is never a copy.
        probe(
            "INSERT INTO workflow.task_runs (state, history, started_at, ended_at) \
             VALUES (o.state, o.history, o.started_at, o.ended_at)\";",
        ),
        // Final review M4 (a): a verbatim copy whose upsert rewrites `state`
        // with no `stamp_set!`.
        copy(
            cols,
            "$1, m.new_ix, o.state, o.history, m.attempts, o.started_at, o.ended_at",
            &format!("{unnest} ON CONFLICT (execution_id, run_ix) DO UPDATE SET state = $9"),
        ),
        // Final review M4 (b): the alias is a derived table that only READS
        // the same table; its `state` is a bind.
        copy(
            cols,
            "$1, m.new_ix, o.state, o.history, m.attempts, o.started_at, o.ended_at",
            "unnest($3::int[], $4::int[]) AS m(old_ix, new_ix) \
             JOIN (SELECT $5 AS state, t.history, t.started_at, t.ended_at, t.run_ix \
                     FROM workflow.task_runs o JOIN workflow.task_runs t ON t.id = o.id \
                    WHERE o.execution_id = $2) o ON o.run_ix = m.old_ix",
        ),
        copy(
            cols,
            "$1, m.new_ix, o.state, o.history, m.attempts, o.started_at, o.ended_at",
            "(SELECT $5 AS state, o.history, o.started_at, o.ended_at \
                FROM workflow.task_runs o WHERE o.execution_id = $2) o, \
             unnest($3::int[], $4::int[]) AS m(old_ix, new_ix)",
        ),
    ];
    for w in &not_verbatim {
        assert!(
            writes_state(w) && !stamps(w) && !copies_verbatim(w),
            "must be refused: {}",
            w.text
        );
    }
    // Final review M4 (a), on stamped INSERTs too: an `ON CONFLICT … DO
    // UPDATE SET` that writes `state` is an UPDATE and needs `stamp_set!`
    // with the placeholders it binds, whatever the INSERT's own columns.
    let stamped_insert = "INSERT INTO workflow.executions (id, state, history, \", \
         stamp_columns!(), \") VALUES ($1, $2, $3, \", stamp_values!(\"$2\", \"$3\"), \")";
    for upsert in [
        format!("{stamped_insert} ON CONFLICT (id) DO UPDATE SET state = $4\""),
        format!("{stamped_insert} ON CONFLICT (id) DO UPDATE SET (state, history) = ($4, $5)\""),
        format!(
            "{stamped_insert} ON CONFLICT (id) DO UPDATE SET state = $4, history = $5, \", \
             stamp_set!(\"$5\", \"$4\", \"$6\", \"$7\"), \"\""
        ),
        // `state` not among the columns: only the upsert writes it.
        "INSERT INTO workflow.task_runs (id, message) VALUES ($1, $2) \
         ON CONFLICT (id) DO UPDATE SET state = 'KILLED'\""
            .to_string(),
    ] {
        let w = probe(&upsert);
        assert!(
            writes_state(&w) && !stamps(&w) && !copies_verbatim(&w),
            "must be refused: {upsert}"
        );
    }
    let stamped_upsert = probe(&format!(
        "{stamped_insert} ON CONFLICT (id) DO UPDATE SET state = $4, history = $5, \", \
         stamp_set!(\"$4\", \"$5\", \"$6\", \"$7\"), \"\""
    ));
    assert!(
        writes_state(&stamped_upsert) && stamps(&stamped_upsert),
        "an upsert that stamps its state write is allowed"
    );
    let quiet_upsert = probe(&format!(
        "{stamped_insert} ON CONFLICT (id) DO UPDATE SET message = $4\""
    ));
    assert!(
        stamps(&quiet_upsert),
        "an upsert that does not touch `state` needs no `stamp_set!`"
    );

    // M3: `format!` is recognised as such.
    let formatted = writes_in(
        "probe",
        "let q = format!(\n    \"UPDATE workflow.task_runs SET {col} = $1\");",
    );
    assert!(
        formatted.len() == 1 && formatted[0].formatted,
        "{formatted:?}"
    );

    // The scan.
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for krate in std::fs::read_dir(&crates).unwrap() {
        let src = krate.unwrap().path().join("src");
        if src.is_dir() {
            rust_files(&src, &mut files);
        }
    }
    let interpolated = regex::Regex::new(r#"(?i)\b(UPDATE|INSERT\s+INTO)[\s\\"]+\{"#).unwrap();
    let mut state_writes = Vec::new();
    let mut dynamic = Vec::new();
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap();
        let name = f.strip_prefix(&crates).unwrap().display().to_string();
        for m in interpolated.find_iter(&text) {
            let line = 1 + text[..m.start()].matches('\n').count();
            dynamic.push(format!(
                "{name}:{line}: an UPDATE/INSERT whose table is interpolated"
            ));
        }
        for w in writes_in(&name, &text) {
            if w.formatted {
                dynamic.push(format!(
                    "{}: a write to these tables built with format!",
                    w.at
                ));
            } else if writes_state(&w) {
                state_writes.push(w);
            }
        }
    }
    assert!(
        dynamic.is_empty(),
        "SQL this guard cannot read; build it with `concat!` and the `stamps` macros: {dynamic:#?}"
    );

    let unstamped: Vec<&Write> = state_writes
        .iter()
        .filter(|w| !stamps(w) && !copies_verbatim(w))
        .collect();
    assert!(
        unstamped.is_empty(),
        "these statements write `state` without the timestamps; build them with \
         `worker::stamps`' `stamp_set!` (UPDATE) or `stamp_columns!`/`stamp_values!` \
         (INSERT):\n{unstamped:#?}"
    );

    // The scanner's reach: every writer the report enumerates is found.
    let found: Vec<&str> = state_writes.iter().map(|w| w.at.as_str()).collect();
    for (file, kind) in [
        (
            "egret-workflow/src/worker/claim.rs",
            "UPDATE workflow.task_runs",
        ),
        (
            "egret-workflow/src/worker/claim.rs",
            "INSERT INTO workflow.task_runs",
        ),
        (
            "egret-workflow/src/worker/claim.rs",
            "UPDATE workflow.executions",
        ),
        (
            "egret-workflow/src/worker/claim.rs",
            "INSERT INTO workflow.executions",
        ),
        (
            "egret-workflow/src/worker/spawn.rs",
            "INSERT INTO workflow.executions",
        ),
        (
            "egret-workflow/src/worker/subflow.rs",
            "INSERT INTO workflow.executions",
        ),
        (
            "egret-workflow/src/worker/replay.rs",
            "INSERT INTO workflow.executions",
        ),
    ] {
        assert!(
            state_writes
                .iter()
                .any(|w| w.at.starts_with(file) && w.text.starts_with(kind)),
            "the scan did not find `{kind}` in {file}; it found {found:?}"
        );
    }
    // And the one verbatim copy is found, and is allowed AS a copy (not by
    // a stamp macro the scan mistook for one).
    assert!(
        state_writes.iter().any(|w| {
            w.at.starts_with("egret-workflow/src/worker/copy.rs")
                && w.text.starts_with("INSERT INTO workflow.task_runs")
                && !stamps(w)
                && copies_verbatim(w)
        }),
        "the scan did not find package 7b's verbatim copy in worker/copy.rs; it found {found:?}"
    );
}
