//! The kill route (workflow kill spec
//! `docs/en/specs/2026-10-17-workflow-kill-and-exit-design.md` §3).
//!
//! ```text
//! POST /api/ws/{workspace}/workflows/executions/{id}/kill?cascade=true|false
//! ```
//!
//! Every test is `#[sqlx::test]`: the kill transaction itself touches only the
//! execution it names and its children, but several tests then drive the real
//! worker (`tick`, `wake`), which is global by construction, so each test owns
//! its database.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5452/egret_test
//!   cargo test -p egret-workflow --test kill_route

use egret_metastore::Db;
use serde_json::{json, Value};
use uuid::Uuid;

mod common;

// ── Fixtures ────────────────────────────────────────────────────────────────

fn one_log(id: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    )
}

fn one_pause(id: &str) -> String {
    format!("id: {id}\nnamespace: t\ntasks:\n  - id: pause\n    type: io.kestra.plugin.core.flow.Pause\n")
}

fn one_retrying(id: &str, interval: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n\
         \x20   message: hi\n    retry:\n      type: constant\n      interval: {interval}\n\
         \x20     maxAttempts: 2\n"
    )
}

/// A `Parallel` holding a two-value sequential `Loop` and a `Subflow` of
/// `t.child` ([`one_log`]): one loop child and one subflow child, both live
/// as soon as the first settle has run.
fn parent_with_children(id: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n\
         \x20     - id: loop\n        type: io.kestra.plugin.core.flow.Loop\n        values: '[\"1\", \"2\"]'\n\
         \x20       concurrencyLimit: 1\n        tasks:\n          - id: body\n\
         \x20           type: io.kestra.plugin.core.log.Log\n            message: hi\n\
         \x20     - id: sub\n        type: io.kestra.plugin.core.flow.Subflow\n        namespace: t\n\
         \x20       flowId: child\n"
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

async fn start(pool: &sqlx::PgPool, ws: Uuid, source: &str) -> Uuid {
    egret_workflow::worker::start_execution(pool, ws, None, source, json!({}))
        .await
        .expect("start execution")
}

/// A workspace, a `workflow:manage` holder, the router.
async fn setup(pool: &sqlx::PgPool) -> (Db, Uuid, egret_core::UserId, common::App) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage", "workflow:read"]).await;
    let app = common::app(&db).await;
    (db, ws, user, app)
}

async fn kill(
    app: &common::App,
    ws: Uuid,
    id: Uuid,
    query: &str,
    user: egret_core::UserId,
) -> axum::response::Response {
    common::post_no_body(
        app,
        &format!("/api/ws/{ws}/workflows/executions/{id}/kill{query}"),
        user,
    )
    .await
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

/// One run's `(state, claimed_at, ended_at, attempts)`.
type RunRow = (
    String,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<chrono::DateTime<chrono::Utc>>,
    Value,
);

async fn run_row(pool: &sqlx::PgPool, execution: Uuid, task_id: &str) -> RunRow {
    sqlx::query_as(
        "SELECT state, claimed_at, ended_at, attempts FROM workflow.task_runs \
          WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(execution)
    .bind(task_id)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| panic!("run {task_id} of {execution}: {e}"))
}

/// Every run of `execution`, `task_id → (state, history)`, for before/after
/// comparisons.
async fn runs_of(pool: &sqlx::PgPool, execution: Uuid) -> Vec<(String, String, Value)> {
    sqlx::query_as(
        "SELECT task_id, state, history FROM workflow.task_runs \
          WHERE execution_id = $1 ORDER BY run_ix",
    )
    .bind(execution)
    .fetch_all(pool)
    .await
    .expect("runs")
}

async fn loop_children(pool: &sqlx::PgPool, parent: Uuid) -> Vec<Uuid> {
    sqlx::query_scalar(
        "SELECT id FROM workflow.executions \
          WHERE parent_execution_id = $1 AND parent_run_ix IS NOT NULL ORDER BY loop_index",
    )
    .bind(parent)
    .fetch_all(pool)
    .await
    .expect("loop children")
}

async fn subflow_child(pool: &sqlx::PgPool, parent: Uuid) -> Uuid {
    sqlx::query_scalar(
        "SELECT id FROM workflow.executions \
          WHERE trigger_execution_id = $1 AND trigger_run_ix IS NOT NULL",
    )
    .bind(parent)
    .fetch_one(pool)
    .await
    .expect("the subflow child")
}

/// The last state of the last attempt in a stored `attempts` array.
fn last_attempt_state(attempts: &Value) -> Option<String> {
    attempts
        .as_array()?
        .last()?
        .get("history")?
        .as_array()?
        .last()?
        .as_str()
        .map(str::to_string)
}

/// A registry that records each `cancel` by `(run, attempt)` and whether any
/// handler was run: what a kill-only claim must and must not do.
#[derive(Default)]
struct CancelRecorder {
    cancels: std::sync::Mutex<Vec<(Uuid, usize)>>,
    ran: std::sync::atomic::AtomicBool,
}

impl CancelRecorder {
    fn cancels(&self) -> Vec<(Uuid, usize)> {
        self.cancels.lock().unwrap().clone()
    }
    fn ran(&self) -> bool {
        self.ran.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl egret_workflow::worker::Registry for CancelRecorder {
    async fn run(
        &self,
        _task: &egret_workflow::worker::TaskInput,
    ) -> egret_core::Result<egret_workflow::worker::Outcome> {
        self.ran.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(egret_workflow::worker::Outcome::success())
    }
    async fn cancel(&self, task: &egret_workflow::worker::TaskInput) -> egret_core::Result<()> {
        self.cancels
            .lock()
            .unwrap()
            .push((task.run_id, task.attempt));
        Ok(())
    }
}

/// One worker tick whose only work is the kill-only claims of `KILLING`
/// leaves: it cancels each and runs no handler.
async fn finish_kills(pool: &sqlx::PgPool) {
    let reg = CancelRecorder::default();
    egret_workflow::worker::tick(pool, 10, &reg)
        .await
        .expect("the kill-only claims");
    assert!(!reg.ran(), "a tick that finishes kills runs no handler");
}

/// Lease `execution`'s run of `task_id`, as a worker's claim would, so a
/// tick does not take it.
async fn lease(pool: &sqlx::PgPool, execution: Uuid, task_id: &str) {
    sqlx::query(
        "UPDATE workflow.task_runs SET claimed_at = now() \
          WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(execution)
    .bind(task_id)
    .execute(pool)
    .await
    .expect("lease the run");
}

// ── Spec §3.2 step 1: a leased leaf ─────────────────────────────────────────

/// **A leaf a worker holds is moved to `KILLING` and keeps its lease**: the
/// worker that holds it stops the handler and writes `KILLED` (spec §4.2), so
/// the route neither clears `claimed_at` nor writes the terminal itself.
#[sqlx::test(migrations = "../../migrations")]
async fn a_leased_leaf_is_left_killing_under_its_lease(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = start(&pool, ws, &one_log("leased")).await;
    let claimed = egret_workflow::worker::claim(&pool, 10)
        .await
        .expect("claim");
    assert_eq!(claimed.len(), 1);

    let res = kill(&app, ws, ex, "", user).await;
    assert_eq!(res.status(), 202);
    let body = common::json(res).await;
    assert_eq!(body["state"], "KILLING", "{body}");

    assert_eq!(exec_state(&pool, ex).await, "KILLING");
    let (state, claimed_at, ended_at, attempts) = run_row(&pool, ex, "a").await;
    assert_eq!(state, "KILLING");
    assert_eq!(
        claimed_at,
        Some(claimed[0].claimed_at),
        "the lease is left exactly as it was"
    );
    assert_eq!(ended_at, None, "a KILLING run has not ended");
    assert_eq!(last_attempt_state(&attempts).as_deref(), Some("KILLING"));
}

// ── Spec §3.2 step 1: a running leaf nobody holds ───────────────────────────

/// **A running leaf nobody holds is `KILLING`, and one worker tick finishes
/// it through the kill-only claim** (final review I1, superseding Task 6's
/// "unleased RUNNING → KILLED"): no lease does not mean nothing is running --
/// a reaped worker's container may be -- so the claim cancels it by run and
/// attempt, without running the handler, and writes `KILLED`. Its open
/// attempt is then closed `KILLED` and the execution ends `KILLED`.
#[sqlx::test(migrations = "../../migrations")]
async fn a_waiting_leaf_is_killing_until_one_tick_kills_it(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = start(&pool, ws, &one_log("waiting")).await;

    let res = kill(&app, ws, ex, "", user).await;
    assert_eq!(res.status(), 202);
    let body = common::json(res).await;
    assert_eq!(body["state"], "KILLING", "{body}");
    let (state, claimed_at, ended_at, _) = run_row(&pool, ex, "a").await;
    assert_eq!(state, "KILLING");
    assert_eq!(claimed_at, None);
    assert_eq!(ended_at, None);

    let reg = CancelRecorder::default();
    egret_workflow::worker::tick(&pool, 10, &reg)
        .await
        .expect("the kill-only claim");
    assert!(!reg.ran(), "the handler of a killed run is never run");
    let (state, claimed_at, ended_at, attempts) = run_row(&pool, ex, "a").await;
    assert_eq!(state, "KILLED");
    assert_eq!(claimed_at, None);
    assert!(ended_at.is_some(), "a KILLED run has ended");
    assert_eq!(last_attempt_state(&attempts).as_deref(), Some("KILLED"));
    assert!(
        attempts[0].get("ended_at").is_some(),
        "the attempt's end is stamped: {attempts}"
    );
    let ended: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT ended_at FROM workflow.executions WHERE id = $1")
            .bind(ex)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(ended.is_some(), "the execution's end is stamped");
    assert_eq!(
        exec_history(&pool, ex).await,
        json!(["CREATED", "RUNNING", "KILLING", "KILLED"])
    );
    assert_eq!(exec_state(&pool, ex).await, "KILLED");
    // Nothing is left for a worker.
    assert!(egret_workflow::worker::claim(&pool, 10)
        .await
        .unwrap()
        .is_empty());
}

/// **Final review I1: a reaped run is cancelled, not just written `KILLED`.**
/// Its worker died and the reaper cleared the lease, so the run is `RUNNING`
/// with `claimed_at` NULL and an attempt that was started -- and its
/// container may still be running (W5a re-attaches to it on the next claim).
/// The route makes it `KILLING`; the next worker tick takes it kill-only and
/// asks the handler to stop exactly that `(run, attempt)`, without running
/// it, and the run and the execution end `KILLED`.
#[sqlx::test(migrations = "../../migrations")]
async fn a_reaped_run_is_cancelled_by_run_and_attempt_then_killed(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = start(&pool, ws, &one_log("reaped")).await;
    let claimed = egret_workflow::worker::claim(&pool, 10).await.unwrap();
    assert_eq!(claimed.len(), 1);
    let (run_id, attempt) = (claimed[0].id, claimed[0].attempt as usize);
    assert!(attempt > 0, "the attempt was started: {attempt}");
    // The worker dies; its lease expires and the reaper clears it.
    assert_eq!(
        egret_workflow::worker::reap(&pool, std::time::Duration::ZERO)
            .await
            .expect("reap"),
        1
    );
    let (state, claimed_at, _, _) = run_row(&pool, ex, "a").await;
    assert_eq!(
        (state.as_str(), claimed_at),
        ("RUNNING", None),
        "the fixture: reaped"
    );

    let res = kill(&app, ws, ex, "", user).await;
    assert_eq!(res.status(), 202);
    assert_eq!(common::json(res).await["state"], "KILLING");
    assert_eq!(run_row(&pool, ex, "a").await.0, "KILLING");

    let reg = CancelRecorder::default();
    egret_workflow::worker::tick(&pool, 10, &reg)
        .await
        .expect("the kill-only claim");
    assert_eq!(
        reg.cancels(),
        vec![(run_id, attempt)],
        "the reaped run's own attempt is cancelled, once"
    );
    assert!(!reg.ran(), "and its handler is not run again");
    let (state, claimed_at, ended_at, attempts) = run_row(&pool, ex, "a").await;
    assert_eq!(state, "KILLED");
    assert_eq!(claimed_at, None);
    assert!(ended_at.is_some());
    assert_eq!(last_attempt_state(&attempts).as_deref(), Some("KILLED"));
    assert_eq!(exec_state(&pool, ex).await, "KILLED");
}

/// **D2: a paused execution is killed without a worker** -- its `Pause` run
/// and the execution both end `KILLED`.
#[sqlx::test(migrations = "../../migrations")]
async fn a_paused_execution_is_killed_without_a_worker(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = start(&pool, ws, &one_pause("paused")).await;
    assert_eq!(exec_state(&pool, ex).await, "PAUSED");

    let res = kill(&app, ws, ex, "", user).await;
    assert_eq!(res.status(), 202);
    assert_eq!(common::json(res).await["state"], "KILLED");
    assert_eq!(exec_state(&pool, ex).await, "KILLED");
    let (state, _, ended_at, _) = run_row(&pool, ex, "pause").await;
    assert_eq!(state, "KILLED");
    assert!(ended_at.is_some());
}

/// **A RETRYING run is killed and nothing revives it** (decision 2): after
/// the kill its retry is due, and neither the waker (`due_retries` / `wake`)
/// nor a tick moves the run or the execution again.
#[sqlx::test(migrations = "../../migrations")]
async fn a_killed_retrying_run_is_not_revived_by_the_waker(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = start(&pool, ws, &one_retrying("retrying", "PT1S")).await;
    let claimed = egret_workflow::worker::claim(&pool, 10).await.unwrap();
    egret_workflow::worker::commit_outcome(
        &pool,
        &claimed[0],
        &egret_workflow::worker::Outcome::failed(),
    )
    .await
    .expect("fail the first attempt");
    assert_eq!(run_row(&pool, ex, "a").await.0, "RETRYING");

    let res = kill(&app, ws, ex, "", user).await;
    assert_eq!(res.status(), 202);
    assert_eq!(common::json(res).await["state"], "KILLED");
    let killed = run_row(&pool, ex, "a").await;
    assert_eq!(killed.0, "KILLED");
    let history = exec_history(&pool, ex).await;

    // The retry's delay has passed: the waker would reopen it now.
    tokio::time::sleep(std::time::Duration::from_millis(1300)).await;
    assert!(
        egret_workflow::worker::due_retries(&pool, 10)
            .await
            .unwrap()
            .is_empty(),
        "a killed run is not a due retry"
    );
    assert!(egret_workflow::worker::wake(&pool, 10)
        .await
        .unwrap()
        .is_empty());
    let ran = egret_workflow::worker::tick(&pool, 10, &common::Answering(true))
        .await
        .expect("tick");
    assert_eq!(ran, 0, "nothing is claimable");

    assert_eq!(
        run_row(&pool, ex, "a").await,
        killed,
        "the run did not move"
    );
    assert_eq!(exec_history(&pool, ex).await, history);
    assert_eq!(exec_state(&pool, ex).await, "KILLED");
}

// ── Refusals ────────────────────────────────────────────────────────────────

/// **409 `execution_terminated` for a terminal execution (D3); 404 for an
/// unknown id and for another workspace's; 403 without `workflow:manage`
/// (D4)** -- and none of them writes anything.
#[sqlx::test(migrations = "../../migrations")]
async fn the_refusals_write_nothing(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage", "workflow:read"]).await;
    let (theirs, other) = common::seed_workspace(&db).await;
    common::grant(&db, theirs, other, &["workflow:manage"]).await;
    let (_, reader) = common::seed_workspace(&db).await;
    common::grant(&db, ws, reader, &["workflow:read"]).await;
    // Built after every grant: the PDP reads them when it is built.
    let app = common::app(&db).await;

    // Terminal.
    let done = start(&pool, ws, &one_log("done")).await;
    assert_eq!(kill(&app, ws, done, "", user).await.status(), 202);
    finish_kills(&pool).await;
    assert_eq!(exec_state(&pool, done).await, "KILLED");
    let history = exec_history(&pool, done).await;
    let res = kill(&app, ws, done, "", user).await;
    assert_eq!(res.status(), 409);
    let body = common::json(res).await;
    assert_eq!(body["code"], "execution_terminated", "{body}");
    assert_eq!(exec_history(&pool, done).await, history);

    // Unknown.
    assert_eq!(kill(&app, ws, Uuid::new_v4(), "", user).await.status(), 404);

    // Another workspace's, live.
    let foreign = start(&pool, theirs, &one_log("foreign")).await;
    assert_eq!(kill(&app, ws, foreign, "", user).await.status(), 404);
    assert_eq!(exec_state(&pool, foreign).await, "RUNNING");

    // `workflow:read` alone.
    let live = start(&pool, ws, &one_log("live")).await;
    assert_eq!(kill(&app, ws, live, "", reader).await.status(), 403);
    assert_eq!(exec_state(&pool, live).await, "RUNNING");
    assert_eq!(run_row(&pool, live, "a").await.0, "RUNNING");

    // Controls: the owner kills the foreign one; the manager the live one.
    assert_eq!(kill(&app, theirs, foreign, "", other).await.status(), 202);
    assert_eq!(kill(&app, ws, live, "", user).await.status(), 202);
}

/// **The audit record** names the execution, the state it was killed from,
/// the cascade flag and the outcome.
#[sqlx::test(migrations = "../../migrations")]
async fn a_kill_writes_an_audit_record(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = start(&pool, ws, &one_log("audited")).await;
    common::audit_mutations(&app);
    assert_eq!(
        kill(&app, ws, ex, "?cascade=false", user).await.status(),
        202
    );
    let audit = common::audit_mutations(&app);
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert_eq!(audit[0].0, "workflow_execution:kill");
    assert_eq!(audit[0].1.as_deref(), Some(ex.to_string().as_str()));
    assert_eq!(
        audit[0].2,
        json!({"from_state": "RUNNING", "cascade": false, "outcome": "KILLING"})
    );

    // A refusal audits nothing.
    finish_kills(&pool).await;
    assert_eq!(kill(&app, ws, ex, "", user).await.status(), 409);
    assert!(common::audit_mutations(&app).is_empty());
}

// ── Cascade (D1) ────────────────────────────────────────────────────────────

/// **`cascade` (the default) kills a loop child and a subflow child**; the
/// children's leaves were leased, so they are `KILLING`, like their own
/// executions. The parent has no leaf of its own: its `Subflow` run is
/// `KILLED`, its `Loop` and `Parallel` are closed by `advance()`, and it
/// answers `KILLED`.
#[sqlx::test(migrations = "../../migrations")]
async fn cascade_kills_loop_and_subflow_children(pool: sqlx::PgPool) {
    let (db, ws, user, app) = setup(&pool).await;
    file_flow(&db, ws, user, &one_log("child")).await;
    let parent = start(&pool, ws, &parent_with_children("cascade")).await;
    let iteration = loop_children(&pool, parent).await;
    assert_eq!(iteration.len(), 1, "a sequential loop's first iteration");
    let child = subflow_child(&pool, parent).await;
    let claimed = egret_workflow::worker::claim(&pool, 10).await.unwrap();
    assert_eq!(
        claimed.len(),
        2,
        "the iteration's body and the child's task"
    );

    let res = kill(&app, ws, parent, "", user).await;
    assert_eq!(res.status(), 202);
    assert_eq!(common::json(res).await["state"], "KILLED");

    assert_eq!(exec_state(&pool, parent).await, "KILLED");
    assert_eq!(run_row(&pool, parent, "sub").await.0, "KILLED");
    assert_eq!(run_row(&pool, parent, "loop").await.0, "KILLED");
    assert_eq!(exec_state(&pool, iteration[0]).await, "KILLING");
    assert_eq!(run_row(&pool, iteration[0], "body").await.0, "KILLING");
    assert_eq!(exec_state(&pool, child).await, "KILLING");
    assert_eq!(run_row(&pool, child, "a").await.0, "KILLING");
    assert_eq!(
        loop_children(&pool, parent).await.len(),
        1,
        "no second iteration"
    );
}

/// **The same with nothing leased**: the parent, which has no leaf of its
/// own, ends `KILLED` in the route's own transaction; the children's running
/// leaves are `KILLING` (final review I1) and one tick's kill-only claims end
/// the whole tree `KILLED`, with no handler run.
#[sqlx::test(migrations = "../../migrations")]
async fn cascade_with_nothing_leased_kills_the_whole_tree_after_one_tick(pool: sqlx::PgPool) {
    let (db, ws, user, app) = setup(&pool).await;
    file_flow(&db, ws, user, &one_log("child")).await;
    let parent = start(&pool, ws, &parent_with_children("cascade-free")).await;
    let iteration = loop_children(&pool, parent).await;
    let child = subflow_child(&pool, parent).await;

    let res = kill(&app, ws, parent, "?cascade=true", user).await;
    assert_eq!(res.status(), 202);
    assert_eq!(exec_state(&pool, parent).await, "KILLED");
    for ex in [iteration[0], child] {
        assert_eq!(exec_state(&pool, ex).await, "KILLING", "{ex}");
    }
    let reg = CancelRecorder::default();
    egret_workflow::worker::tick(&pool, 10, &reg)
        .await
        .expect("the kill-only claims");
    assert!(!reg.ran(), "no handler is run");
    assert_eq!(reg.cancels().len(), 2, "one cancel per killed leaf");
    for ex in [parent, iteration[0], child] {
        assert_eq!(exec_state(&pool, ex).await, "KILLED", "{ex}");
    }
    assert_eq!(run_row(&pool, iteration[0], "body").await.0, "KILLED");
    assert_eq!(run_row(&pool, child, "a").await.0, "KILLED");
}

/// **`cascade=false` leaves the children running, and their later report into
/// the killed parent is a safe no-op** (decision 3, as Kestra): the children
/// finish under a real worker, the parent's runs and history do not move, the
/// sequential loop starts no second iteration, and nothing is left stuck.
#[sqlx::test(migrations = "../../migrations")]
async fn without_cascade_the_children_run_on_and_report_into_nothing(pool: sqlx::PgPool) {
    let (db, ws, user, app) = setup(&pool).await;
    file_flow(&db, ws, user, &one_log("child")).await;
    let parent = start(&pool, ws, &parent_with_children("no-cascade")).await;
    let iteration = loop_children(&pool, parent).await;
    let child = subflow_child(&pool, parent).await;

    let res = kill(&app, ws, parent, "?cascade=false", user).await;
    assert_eq!(res.status(), 202);
    assert_eq!(common::json(res).await["state"], "KILLED");
    assert_eq!(exec_state(&pool, iteration[0]).await, "RUNNING");
    assert_eq!(exec_state(&pool, child).await, "RUNNING");
    assert_eq!(run_row(&pool, child, "a").await.0, "RUNNING");
    let parent_runs = runs_of(&pool, parent).await;
    let parent_history = exec_history(&pool, parent).await;

    // The children finish; their terminals are reported to the parent.
    let reg = common::Answering(true);
    let mut rounds = 0;
    while egret_workflow::worker::tick(&pool, 10, &reg)
        .await
        .expect("a tick never errors on a report into a killed parent")
        > 0
    {
        rounds += 1;
        assert!(rounds < 20, "the ticks never ran dry");
    }
    assert_eq!(exec_state(&pool, iteration[0]).await, "SUCCESS");
    assert_eq!(exec_state(&pool, child).await, "SUCCESS");
    assert_eq!(
        runs_of(&pool, parent).await,
        parent_runs,
        "the parent's runs"
    );
    assert_eq!(exec_history(&pool, parent).await, parent_history);
    assert_eq!(
        loop_children(&pool, parent).await.len(),
        1,
        "the killed loop starts no second iteration"
    );
    assert!(egret_workflow::worker::wake(&pool, 10)
        .await
        .unwrap()
        .is_empty());
}

/// **A replay is not a child, and nor is an execution a Flow trigger fired**:
/// `parent_execution_id` with no `parent_run_ix`, and `trigger_execution_id`
/// with no `trigger_run_ix`. Cascade reaches neither.
#[sqlx::test(migrations = "../../migrations")]
async fn cascade_does_not_reach_a_replay_or_a_flow_trigger_execution(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let source = start(&pool, ws, &one_log("source")).await;
    let replay = start(&pool, ws, &one_log("replay")).await;
    let fired = start(&pool, ws, &one_log("fired")).await;
    sqlx::query("UPDATE workflow.executions SET parent_execution_id = $1 WHERE id = $2")
        .bind(source)
        .bind(replay)
        .execute(&pool)
        .await
        .expect("link the replay");
    sqlx::query("UPDATE workflow.executions SET trigger_execution_id = $1 WHERE id = $2")
        .bind(source)
        .bind(fired)
        .execute(&pool)
        .await
        .expect("link the fired execution");

    assert_eq!(
        kill(&app, ws, source, "?cascade=true", user).await.status(),
        202
    );
    assert_eq!(exec_state(&pool, source).await, "KILLING");
    assert_eq!(exec_state(&pool, replay).await, "RUNNING");
    assert_eq!(run_row(&pool, replay, "a").await.0, "RUNNING");
    assert_eq!(exec_state(&pool, fired).await, "RUNNING");
}

/// **Final review M2: the replacement of a DESCENDANT is killed, the root's
/// own replay is not.** A `CREATE_NEW_EXECUTION` retry of a subflow child
/// starts a replacement linked `parent_execution_id = <child>` with no
/// `parent_run_ix` (`worker::replay`); it is the child's continuation, so a
/// cascade over the parent must reach it. A replay of the execution being
/// killed is a separate execution the operator did not name, and the cascade
/// still leaves it alone.
#[sqlx::test(migrations = "../../migrations")]
async fn cascade_kills_a_descendants_replacement_and_not_the_roots_replay(pool: sqlx::PgPool) {
    let (db, ws, user, app) = setup(&pool).await;
    file_flow(&db, ws, user, &one_log("child")).await;
    let source = "id: tree\nnamespace: t\ntasks:\n  - id: sub\n    type: io.kestra.plugin.core.flow.Subflow\n    namespace: t\n    flowId: child\n";
    let parent = start(&pool, ws, source).await;
    let child = subflow_child(&pool, parent).await;
    let replacement = start(&pool, ws, &one_log("replacement")).await;
    let root_replay = start(&pool, ws, &one_log("root-replay")).await;
    for (of, link) in [(child, replacement), (parent, root_replay)] {
        sqlx::query("UPDATE workflow.executions SET parent_execution_id = $1 WHERE id = $2")
            .bind(of)
            .bind(link)
            .execute(&pool)
            .await
            .expect("link the replay");
    }
    // Leased, so the tick below does not run it; a kill would still reach it.
    lease(&pool, root_replay, "a").await;

    assert_eq!(
        kill(&app, ws, parent, "?cascade=true", user).await.status(),
        202
    );
    assert_eq!(exec_state(&pool, replacement).await, "KILLING");
    assert_eq!(run_row(&pool, replacement, "a").await.0, "KILLING");
    finish_kills(&pool).await;
    for ex in [parent, child, replacement] {
        assert_eq!(exec_state(&pool, ex).await, "KILLED", "{ex}");
    }
    assert_eq!(exec_state(&pool, root_replay).await, "RUNNING");
    assert_eq!(run_row(&pool, root_replay, "a").await.0, "RUNNING");
}

/// **Final review M1: a worker that loses a deadlock to a cascading kill
/// still finishes the kill.** A child's last run commits: the worker holds
/// the child (its execution row and its run row) and, as the child ends,
/// reports to the parent -- child, then parent. A cascading kill holds the
/// parent and wants the child -- the reverse. Here a test transaction plays
/// the kill: it holds the parent, then writes the child and its run
/// `KILLING`, which closes the cycle. Its `deadlock_timeout` is raised so
/// Postgres's detector fires on the worker's side, which is the case under
/// test: the worker is the `40P01` victim. Until the fix the worker returned
/// that error and the run stayed `KILLING` under the dead lease until the
/// reaper; now the outcome commit is run once more, its fence finds the run
/// `KILLING`, and the worker takes the kill path: one cancel, `KILLED`.
#[sqlx::test(migrations = "../../migrations")]
async fn a_worker_that_loses_a_deadlock_to_a_kill_still_finishes_it(pool: sqlx::PgPool) {
    let (db, ws, user, _app) = setup(&pool).await;
    file_flow(&db, ws, user, &one_log("child")).await;
    let source = "id: deadlocked\nnamespace: t\ntasks:\n  - id: sub\n    type: io.kestra.plugin.core.flow.Subflow\n    namespace: t\n    flowId: child\n";
    let parent = start(&pool, ws, source).await;
    let child = subflow_child(&pool, parent).await;

    // The "kill": hold the parent first.
    let mut kill_tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL deadlock_timeout = '60s'")
        .execute(&mut *kill_tx)
        .await
        .unwrap();
    let kill_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *kill_tx)
        .await
        .unwrap();
    sqlx::query("SELECT 1 FROM workflow.executions WHERE id = $1 FOR UPDATE")
        .bind(parent)
        .execute(&mut *kill_tx)
        .await
        .unwrap();

    // The worker runs the child's task and, committing, blocks on the parent.
    let reg = std::sync::Arc::new(CancelRecorder::default());
    let worker = {
        let (pool, reg) = (pool.clone(), reg.clone());
        tokio::spawn(async move { egret_workflow::worker::tick(&pool, 10, &*reg).await })
    };
    let waited = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let blocked: Option<i32> = sqlx::query_scalar(
                "SELECT pid FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid))",
            )
            .bind(kill_pid)
            .fetch_optional(&pool)
            .await
            .unwrap();
            if blocked.is_some() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(
        waited.is_ok(),
        "the worker's commit never waited on the parent"
    );

    // The "kill" now writes the child, which the worker holds: a deadlock,
    // and the worker is its victim.
    sqlx::query(
        "UPDATE workflow.executions SET state = 'KILLING', \
                history = history || '[\"KILLING\"]'::jsonb WHERE id = $1",
    )
    .bind(child)
    .execute(&mut *kill_tx)
    .await
    .expect("the kill's write goes through once the worker is aborted");
    sqlx::query(
        "UPDATE workflow.task_runs SET state = 'KILLING', \
                history = history || '[\"KILLING\"]'::jsonb, \
                attempts = jsonb_set(attempts, \
                    ARRAY[(jsonb_array_length(attempts) - 1)::text, 'history'], \
                    (attempts -> -1 -> 'history') || '[\"KILLING\"]'::jsonb) \
          WHERE execution_id = $1 AND task_id = 'a'",
    )
    .bind(child)
    .execute(&mut *kill_tx)
    .await
    .unwrap();
    kill_tx.commit().await.unwrap();

    let ticked = tokio::time::timeout(std::time::Duration::from_secs(30), worker)
        .await
        .expect("the worker returns")
        .expect("the worker task");
    assert!(
        ticked.is_ok(),
        "the deadlock victim must not give up the kill: {ticked:?}"
    );
    let (state, claimed_at, ended_at, _) = run_row(&pool, child, "a").await;
    assert_eq!(state, "KILLED");
    assert_eq!(claimed_at, None);
    assert!(ended_at.is_some());
    assert_eq!(
        reg.cancels().len(),
        1,
        "the kill path cancelled the task once"
    );
    assert_eq!(exec_state(&pool, child).await, "KILLED");
}

/// **Debt D5: a kill-only claim whose `KILLED` commit loses a deadlock still
/// finishes it** (`claim::commit_kill`'s retry, which no test drove: the test
/// above drives `run_one`'s). The child's run is `KILLING` with no lease, as a
/// reaped kill leaves it, so the next tick takes it kill-only: one cancel,
/// then `commit_kill`. That commit locks the child (`load`), then -- the child
/// ends, and `settle` reports to the subflow parent -- the parent. The test
/// transaction holds the parent, then locks the child: the reverse. Its
/// `deadlock_timeout` is raised so the worker is the `40P01` victim. The retry
/// waits for the test's commit, finds the fence still `KILLING` under its
/// lease, and commits.
///
/// **The positive control** is the database's own deadlock count
/// (`pg_stat_database.deadlocks`; `#[sqlx::test]` gives each test its own
/// database, so no other test's deadlock is counted): exactly one, the
/// worker's first attempt. Without it this test would pass on a schedule in
/// which no deadlock happened and the retry never ran.
#[sqlx::test(migrations = "../../migrations")]
async fn a_kill_only_claim_that_loses_a_deadlock_still_commits_killed(pool: sqlx::PgPool) {
    // Arrange: a parent waiting on a subflow child whose run was claimed,
    // reaped and killed.
    let (db, ws, user, app) = setup(&pool).await;
    file_flow(&db, ws, user, &one_log("child")).await;
    let source = "id: kdead\nnamespace: t\ntasks:\n  - id: sub\n    type: io.kestra.plugin.core.flow.Subflow\n    namespace: t\n    flowId: child\n";
    let parent = start(&pool, ws, source).await;
    let child = subflow_child(&pool, parent).await;
    let claimed = egret_workflow::worker::claim(&pool, 10).await.unwrap();
    assert_eq!(claimed.len(), 1, "the child's run");
    let (run_id, attempt) = (claimed[0].id, claimed[0].attempt as usize);
    egret_workflow::worker::reap(&pool, std::time::Duration::ZERO)
        .await
        .expect("reap");
    assert_eq!(
        kill(&app, ws, child, "?cascade=false", user).await.status(),
        202
    );
    assert_eq!(run_row(&pool, child, "a").await.0, "KILLING");
    assert_eq!(deadlocks(&pool).await, 0, "the fixture: no deadlock yet");

    let mut kill_tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL deadlock_timeout = '60s'")
        .execute(&mut *kill_tx)
        .await
        .unwrap();
    let kill_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *kill_tx)
        .await
        .unwrap();
    sqlx::query("SELECT 1 FROM workflow.executions WHERE id = $1 FOR UPDATE")
        .bind(parent)
        .execute(&mut *kill_tx)
        .await
        .unwrap();

    // Act: the kill-only claim commits KILLED and, reporting to the parent,
    // waits on it.
    let reg = std::sync::Arc::new(CancelRecorder::default());
    let worker = {
        let (pool, reg) = (pool.clone(), reg.clone());
        tokio::spawn(async move { egret_workflow::worker::tick(&pool, 10, &*reg).await })
    };
    let waited = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let blocked: Option<i32> = sqlx::query_scalar(
                "SELECT pid FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid))",
            )
            .bind(kill_pid)
            .fetch_optional(&pool)
            .await
            .unwrap();
            if blocked.is_some() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(
        waited.is_ok(),
        "the worker's KILLED commit never waited on the parent"
    );
    // Close the cycle: the worker is the victim. Waiting on the worker, this
    // lock is granted only once the worker's transaction is aborted.
    sqlx::query("SELECT 1 FROM workflow.executions WHERE id = $1 FOR UPDATE")
        .bind(child)
        .execute(&mut *kill_tx)
        .await
        .expect("the lock is granted once the worker is aborted");
    kill_tx.commit().await.unwrap();

    // Assert
    let ticked = tokio::time::timeout(std::time::Duration::from_secs(60), worker)
        .await
        .expect("the worker returns")
        .expect("the worker task");
    let counted = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let n = deadlocks(&pool).await;
            if n > 0 {
                return n;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the positive control: Postgres counted a deadlock");
    assert_eq!(counted, 1, "one deadlock: the worker's first attempt");
    assert!(
        ticked.is_ok(),
        "the deadlock victim must retry its KILLED commit: {ticked:?}"
    );
    let (state, claimed_at, ended_at, attempts) = run_row(&pool, child, "a").await;
    assert_eq!(state, "KILLED");
    assert_eq!(claimed_at, None);
    assert!(ended_at.is_some());
    assert_eq!(last_attempt_state(&attempts).as_deref(), Some("KILLED"));
    assert_eq!(reg.cancels(), vec![(run_id, attempt)], "cancelled once");
    assert!(!reg.ran(), "a kill-only claim runs no handler");
    assert_eq!(exec_state(&pool, child).await, "KILLED");
}

/// The deadlocks Postgres has detected in this test's own database. The
/// victim's backend reports it to the cumulative statistics, which a reader
/// sees after that backend flushes, so a caller polls.
async fn deadlocks(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT deadlocks FROM pg_stat_database WHERE datname = current_database()")
        .fetch_one(pool)
        .await
        .unwrap()
}

// ── The invariant (decision 4) ──────────────────────────────────────────────

/// **After the route returns, no live run of any execution in the killed tree
/// is in a state other than `KILLING`**: leased, unleased, `RETRYING`,
/// `PAUSED` and `CREATED` rows side by side, and a loop child.
#[sqlx::test(migrations = "../../migrations")]
async fn after_a_kill_every_live_run_in_the_tree_is_killing(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let source = "id: mixed\nnamespace: t\ntasks:\n  - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n\
         \x20     - id: retrying\n        type: io.kestra.plugin.core.log.Log\n        message: r\n\
         \x20       retry:\n          type: constant\n          interval: PT1H\n          maxAttempts: 2\n\
         \x20     - id: leased\n        type: io.kestra.plugin.core.log.Log\n        message: l\n\
         \x20     - id: waiting\n        type: io.kestra.plugin.core.log.Log\n        message: w\n\
         \x20     - id: created\n        type: io.kestra.plugin.core.log.Log\n        message: c\n\
         \x20     - id: pause\n        type: io.kestra.plugin.core.flow.Pause\n\
         \x20     - id: loop\n        type: io.kestra.plugin.core.flow.Loop\n        values: '[\"1\"]'\n\
         \x20       tasks:\n          - id: body\n            type: io.kestra.plugin.core.log.Log\n\
         \x20           message: b\n";
    let ex = start(&pool, ws, source).await;
    let iteration = loop_children(&pool, ex).await;
    assert_eq!(iteration.len(), 1);
    let claimed = egret_workflow::worker::claim(&pool, 20).await.unwrap();
    let retrying = claimed
        .iter()
        .find(|c| c.task_id == "retrying")
        .expect("claimed");
    egret_workflow::worker::commit_outcome(
        &pool,
        retrying,
        &egret_workflow::worker::Outcome::failed(),
    )
    .await
    .unwrap();
    // `waiting` and `created` are given back; `created` is put back before
    // its start, a row a worker has never seen.
    sqlx::query(
        "UPDATE workflow.task_runs SET claimed_at = NULL \
          WHERE execution_id = $1 AND task_id IN ('waiting', 'created')",
    )
    .bind(ex)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE workflow.task_runs SET state = 'CREATED', history = '[\"CREATED\"]', \
                attempts = '[]', started_at = NULL \
          WHERE execution_id = $1 AND task_id = 'created'",
    )
    .bind(ex)
    .execute(&pool)
    .await
    .unwrap();
    let before: Vec<(String, String)> = runs_of(&pool, ex)
        .await
        .into_iter()
        .map(|(t, s, _)| (t, s))
        .collect();
    for (task, state) in [
        ("retrying", "RETRYING"),
        ("leased", "RUNNING"),
        ("waiting", "RUNNING"),
        ("created", "CREATED"),
        ("pause", "PAUSED"),
    ] {
        assert!(
            before.contains(&(task.to_string(), state.to_string())),
            "fixture: {task} is {state}: {before:?}"
        );
    }

    assert_eq!(kill(&app, ws, ex, "", user).await.status(), 202);

    let tree = [ex, iteration[0]];
    let live: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT execution_id, task_id, state FROM workflow.task_runs \
          WHERE execution_id = ANY($1) AND NOT workflow.state_is_terminal(state)",
    )
    .bind(&tree[..])
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(
        live.iter().all(|(_, _, s)| s == "KILLING"),
        "every live run is KILLING: {live:?}"
    );
    let leased: Vec<&str> = live
        .iter()
        .filter(|(e, _, _)| *e == ex)
        .map(|(_, t, _)| t.as_str())
        .collect();
    assert!(leased.contains(&"leased"), "{live:?}");
    // Final review I1: running and unleased is KILLING too, for the claim.
    assert!(leased.contains(&"waiting"), "{live:?}");
    assert_eq!(run_row(&pool, ex, "created").await.0, "KILLED");
    assert_eq!(run_row(&pool, ex, "retrying").await.0, "KILLED");
    assert_eq!(run_row(&pool, ex, "pause").await.0, "KILLED");
    assert_eq!(run_row(&pool, iteration[0], "body").await.0, "KILLING");
    for e in tree {
        assert_eq!(exec_state(&pool, e).await, "KILLING");
    }
}

// ── Exit(KILLED) and its subflow child (decision 5) ─────────────────────────

/// **An `Exit` with `state: KILLED` kills its execution's subflow child too**,
/// as Kestra's kill event cascades. The parent ends `KILLED` at once (its
/// `Subflow` sibling is `KILLED` by the Exit); the child's leaf was leased,
/// so the child is `KILLING`.
#[sqlx::test(migrations = "../../migrations")]
async fn an_exit_killed_kills_its_subflow_child(pool: sqlx::PgPool) {
    let (db, ws, user, _app) = setup(&pool).await;
    file_flow(&db, ws, user, &one_log("child")).await;
    let source = "id: exiting\nnamespace: t\ntasks:\n  - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n\
         \x20     - id: sub\n        type: io.kestra.plugin.core.flow.Subflow\n        namespace: t\n\
         \x20       flowId: child\n\
         \x20     - id: seq\n        type: io.kestra.plugin.core.flow.Sequential\n        tasks:\n\
         \x20         - id: a\n            type: io.kestra.plugin.core.log.Log\n            message: hi\n\
         \x20         - id: exit\n            type: io.kestra.plugin.core.execution.Exit\n\
         \x20           state: KILLED\n";
    let parent = start(&pool, ws, source).await;
    let child = subflow_child(&pool, parent).await;
    let claimed = egret_workflow::worker::claim(&pool, 10).await.unwrap();
    assert_eq!(claimed.len(), 2);
    let a = claimed
        .iter()
        .find(|c| c.execution_id == parent)
        .expect("the parent's own task");
    egret_workflow::worker::commit_outcome(&pool, a, &egret_workflow::worker::Outcome::success())
        .await
        .expect("finish a");

    assert_eq!(exec_state(&pool, parent).await, "KILLED");
    assert_eq!(run_row(&pool, parent, "sub").await.0, "KILLED");
    assert_eq!(exec_state(&pool, child).await, "KILLING");
    assert_eq!(run_row(&pool, child, "a").await.0, "KILLING");
}

// ── Fix round 1: the cascade follows the Exit, not the state ───────────────

/// The `Subflow` child of `parent` started by its run of task `task_id`.
async fn child_of_task(pool: &sqlx::PgPool, parent: Uuid, task_id: &str) -> Uuid {
    sqlx::query_scalar(
        "SELECT e.id FROM workflow.executions e \
           JOIN workflow.task_runs r \
             ON r.execution_id = e.trigger_execution_id AND r.run_ix = e.trigger_run_ix \
          WHERE e.trigger_execution_id = $1 AND r.task_id = $2",
    )
    .bind(parent)
    .bind(task_id)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| panic!("the child of {task_id}: {e}"))
}

/// **An execution that ends `KILLED` by aggregation kills no child**: its
/// `Subflow` child was killed on its own, the report made the `Subflow` run
/// `KILLED` and the `Parallel` over it `KILLED`, and the parent ended
/// `KILLED` -- with no `Exit` and no kill of the parent. Its other child, a
/// `wait: false` one nobody asked to stop, runs on (Kestra cascades a kill
/// event, not a natural `KILLED` end).
#[sqlx::test(migrations = "../../migrations")]
async fn a_parent_killed_by_aggregation_kills_no_other_child(pool: sqlx::PgPool) {
    let (db, ws, user, app) = setup(&pool).await;
    file_flow(&db, ws, user, &one_log("child")).await;
    file_flow(&db, ws, user, &one_log("detached")).await;
    let source = "id: agg\nnamespace: t\ntasks:\n  - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n\
         \x20     - id: waits\n        type: io.kestra.plugin.core.flow.Subflow\n        namespace: t\n\
         \x20       flowId: child\n\
         \x20     - id: fire\n        type: io.kestra.plugin.core.flow.Subflow\n        namespace: t\n\
         \x20       flowId: detached\n        wait: false\n";
    let parent = start(&pool, ws, source).await;
    let waited = child_of_task(&pool, parent, "waits").await;
    let detached = child_of_task(&pool, parent, "fire").await;
    assert_eq!(run_row(&pool, parent, "fire").await.0, "SUCCESS");

    // The detached child's leaf is leased, so the tick below finishes only
    // the kill (final review I1: the killed leaf is KILLING until a claim
    // cancels it) and does not run the detached one.
    lease(&pool, detached, "a").await;
    assert_eq!(kill(&app, ws, waited, "", user).await.status(), 202);
    finish_kills(&pool).await;
    assert_eq!(exec_state(&pool, waited).await, "KILLED");
    // The scenario: the parent reached KILLED by aggregation alone.
    assert_eq!(run_row(&pool, parent, "waits").await.0, "KILLED");
    assert_eq!(exec_state(&pool, parent).await, "KILLED");

    assert_eq!(exec_state(&pool, detached).await, "RUNNING");
    assert_eq!(run_row(&pool, detached, "a").await.0, "RUNNING");
}

/// A registry whose handlers succeed and whose `cancel` stops nothing and
/// says so: what finishing a claimed `KILLING` run needs.
struct StopsCleanly;

#[async_trait::async_trait]
impl egret_workflow::worker::Registry for StopsCleanly {
    async fn run(
        &self,
        _task: &egret_workflow::worker::TaskInput,
    ) -> egret_core::Result<egret_workflow::worker::Outcome> {
        Ok(egret_workflow::worker::Outcome::success())
    }
    async fn cancel(&self, _task: &egret_workflow::worker::TaskInput) -> egret_core::Result<()> {
        Ok(())
    }
}

/// **The settle that closes an already-`KILLING` execution does not
/// cascade**: a `cascade=false` kill leaves the parent `KILLING` over a leased
/// leaf and its `wait: false` child running; the claim of the abandoned
/// `KILLING` run finishes it and the parent ends `KILLED`, and the child is
/// still running.
#[sqlx::test(migrations = "../../migrations")]
async fn closing_a_killing_execution_does_not_cascade(pool: sqlx::PgPool) {
    let (db, ws, user, app) = setup(&pool).await;
    file_flow(&db, ws, user, &one_log("detached")).await;
    let source = "id: closing\nnamespace: t\ntasks:\n  - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n\
         \x20     - id: a\n        type: io.kestra.plugin.core.log.Log\n        message: hi\n\
         \x20     - id: fire\n        type: io.kestra.plugin.core.flow.Subflow\n        namespace: t\n\
         \x20       flowId: detached\n        wait: false\n";
    let parent = start(&pool, ws, source).await;
    let detached = child_of_task(&pool, parent, "fire").await;
    // Both leaves leased: the parent's `a` and the child's `a`.
    assert_eq!(
        egret_workflow::worker::claim(&pool, 10)
            .await
            .unwrap()
            .len(),
        2
    );

    assert_eq!(
        kill(&app, ws, parent, "?cascade=false", user)
            .await
            .status(),
        202
    );
    assert_eq!(exec_state(&pool, parent).await, "KILLING");
    // Its worker is gone: the lease is given back, as the reaper would.
    sqlx::query(
        "UPDATE workflow.task_runs SET claimed_at = NULL \
          WHERE execution_id = $1 AND task_id = 'a'",
    )
    .bind(parent)
    .execute(&pool)
    .await
    .unwrap();
    egret_workflow::worker::tick(&pool, 10, &StopsCleanly)
        .await
        .expect("the claim finishes the kill");

    assert_eq!(run_row(&pool, parent, "a").await.0, "KILLED");
    assert_eq!(exec_state(&pool, parent).await, "KILLED");
    assert_eq!(exec_state(&pool, detached).await, "RUNNING");
    assert_eq!(run_row(&pool, detached, "a").await.0, "RUNNING");
}

// ── Debt D4: force-close an execution a kill cannot load ────────────────────

/// Corrupt the stored source so `claim::load` fails `Unplannable`, the
/// row-content fault a kill used to loop on (debt §2.2).
async fn make_unloadable(pool: &sqlx::PgPool, ex: Uuid) {
    sqlx::query("UPDATE workflow.executions SET source = 'tasks: [' WHERE id = $1")
        .bind(ex)
        .execute(pool)
        .await
        .expect("corrupt the source");
}

/// **Debt D4: the route force-closes an execution it cannot load.** A reaped
/// run (no lease, no handler can be built) ends `KILLED` with the force-close
/// message, the execution is `KILLED`, the audit says `forced`, and nothing
/// is left for a worker to claim.
#[sqlx::test(migrations = "../../migrations")]
async fn the_route_force_closes_an_execution_it_cannot_load(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = start(&pool, ws, &one_log("unloadable")).await;
    egret_workflow::worker::claim(&pool, 10).await.unwrap();
    egret_workflow::worker::reap(&pool, std::time::Duration::ZERO)
        .await
        .expect("reap");
    make_unloadable(&pool, ex).await;
    common::audit_mutations(&app);
    // Act
    let res = kill(&app, ws, ex, "", user).await;
    // Assert
    assert_eq!(res.status(), 202, "no longer a 500");
    assert_eq!(common::json(res).await["state"], "KILLED");
    let (state, claimed_at, ended_at, attempts) = run_row(&pool, ex, "a").await;
    assert_eq!((state.as_str(), claimed_at), ("KILLED", None));
    assert!(ended_at.is_some());
    assert_eq!(last_attempt_state(&attempts).as_deref(), Some("KILLED"));
    let message: Option<String> = sqlx::query_scalar(
        "SELECT message FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'a'",
    )
    .bind(ex)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        message.as_deref().is_some_and(|m| m
            .starts_with("egret could not load this execution (")
            && m.ends_with(
                "so it was killed without running its flow: no errors:, finally: or Flow trigger ran"
            )),
        "{message:?}"
    );
    assert_eq!(exec_state(&pool, ex).await, "KILLED");
    let audit = common::audit_mutations(&app);
    assert_eq!(audit[0].2["forced"], true, "{audit:?}");
    // And nothing loops: nothing is left to claim.
    let again = egret_workflow::worker::claim(&pool, 10).await.unwrap();
    assert!(again.is_empty(), "nothing is left to claim: {again:?}");
}

/// **Debt D4, the worker's side: a kill-only claim of a run whose execution
/// cannot be loaded commits `KILLED` once, instead of being reaped and
/// claimed again every `working_timeout` for ever.**
#[sqlx::test(migrations = "../../migrations")]
async fn a_kill_only_claim_of_an_unloadable_execution_closes_it(pool: sqlx::PgPool) {
    // Arrange: killed (KILLING, unleased) while loadable, then corrupted.
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = start(&pool, ws, &one_log("unloadable-claim")).await;
    egret_workflow::worker::claim(&pool, 10).await.unwrap();
    egret_workflow::worker::reap(&pool, std::time::Duration::ZERO)
        .await
        .expect("reap");
    assert_eq!(kill(&app, ws, ex, "", user).await.status(), 202);
    make_unloadable(&pool, ex).await;
    // Act
    let reg = CancelRecorder::default();
    let ticked = egret_workflow::worker::tick(&pool, 10, &reg).await;
    // Assert
    assert!(ticked.is_ok(), "{ticked:?}");
    assert_eq!(run_row(&pool, ex, "a").await.0, "KILLED");
    assert_eq!(exec_state(&pool, ex).await, "KILLED");
    assert!(
        reg.cancels().is_empty(),
        "no handler can be built, so nothing is cancelled"
    );
    let again = egret_workflow::worker::claim(&pool, 10).await.unwrap();
    assert!(again.is_empty(), "nothing is left to claim: {again:?}");
}

/// **A held leaf stays `KILLING` under its lease** (rule 2): its worker has
/// the input and can cancel; only the unleased sibling is closed now.
#[sqlx::test(migrations = "../../migrations")]
async fn a_force_close_leaves_a_held_leaf_to_its_worker(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let source = "id: held\nnamespace: t\ntasks:\n  - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n\
         \x20     - id: leased\n        type: io.kestra.plugin.core.log.Log\n        message: l\n\
         \x20     - id: waiting\n        type: io.kestra.plugin.core.log.Log\n        message: w\n";
    let ex = start(&pool, ws, source).await;
    let claimed = egret_workflow::worker::claim(&pool, 1).await.unwrap();
    let held = claimed[0].task_id.clone();
    make_unloadable(&pool, ex).await;
    let res = kill(&app, ws, ex, "", user).await;
    assert_eq!(res.status(), 202);
    assert_eq!(common::json(res).await["state"], "KILLING");
    let (state, claimed_at, _, _) = run_row(&pool, ex, &held).await;
    assert_eq!(state, "KILLING");
    assert!(claimed_at.is_some(), "the lease is kept");
    let other = if held == "leased" {
        "waiting"
    } else {
        "leased"
    };
    assert_eq!(run_row(&pool, ex, other).await.0, "KILLED");
}

/// **The cascade force-closes one descendant it cannot load and goes on**:
/// the parent's kill is not refused because its `Subflow` child's source no
/// longer plans. The child ends `KILLED` by force, and the parent `KILLED`.
#[sqlx::test(migrations = "../../migrations")]
async fn a_cascade_force_closes_an_unloadable_child_and_kills_the_parent(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user, app) = setup(&pool).await;
    file_flow(&db, ws, user, &one_log("child")).await;
    let parent = start(&pool, ws, &parent_with_children("cascade-unloadable")).await;
    let child = subflow_child(&pool, parent).await;
    make_unloadable(&pool, child).await;
    common::audit_mutations(&app);
    // Act
    let res = kill(&app, ws, parent, "", user).await;
    // Assert
    assert_eq!(res.status(), 202, "no longer a 500");
    assert_eq!(exec_state(&pool, child).await, "KILLED");
    assert_eq!(run_row(&pool, child, "a").await.0, "KILLED");
    assert_eq!(
        common::audit_mutations(&app)[0].2["forced"],
        true,
        "a descendant was closed by force"
    );
    finish_kills(&pool).await;
    assert_eq!(exec_state(&pool, parent).await, "KILLED");
}

/// **Rule 5: a child closed by force tells its parent.** The unloadable
/// `Subflow` child is killed on its own; its parent's `sub` run, which was
/// waiting on it, ends as a killed child's would, instead of waiting for
/// ever.
#[sqlx::test(migrations = "../../migrations")]
async fn a_force_closed_child_reports_to_its_parent(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user, app) = setup(&pool).await;
    file_flow(&db, ws, user, &one_log("child")).await;
    let parent = start(&pool, ws, &parent_with_children("report-unloadable")).await;
    let child = subflow_child(&pool, parent).await;
    assert_eq!(run_row(&pool, parent, "sub").await.0, "RUNNING");
    make_unloadable(&pool, child).await;
    // Act
    let res = kill(&app, ws, child, "", user).await;
    // Assert
    assert_eq!(res.status(), 202);
    assert_eq!(exec_state(&pool, child).await, "KILLED");
    assert_eq!(run_row(&pool, parent, "sub").await.0, "KILLED");
}

/// A sequential two-value `Loop` of one `Log` (`body`): one live iteration as
/// soon as the first settle has run. An iteration's source is cut from its
/// parent's (`spawn::sub_flow_source`), so a planner change that breaks one
/// breaks both.
fn one_loop(id: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: loop\n    type: io.kestra.plugin.core.flow.Loop\n\
         \x20   values: '[\"1\", \"2\"]'\n    concurrencyLimit: 1\n    tasks:\n      - id: body\n\
         \x20       type: io.kestra.plugin.core.log.Log\n        message: hi\n"
    )
}

/// **Fix round 1, I1: a `Loop` whose parent and iteration both fail to load
/// is killed, not refused.** The root is closed by force first, so the
/// iteration's report finds its parent terminal and is skipped instead of
/// loading it.
#[sqlx::test(migrations = "../../migrations")]
async fn a_cascade_force_closes_a_loop_whose_parent_and_iteration_cannot_load(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let parent = start(&pool, ws, &one_loop("loop-unloadable")).await;
    let iteration = loop_children(&pool, parent).await;
    assert_eq!(iteration.len(), 1);
    make_unloadable(&pool, parent).await;
    make_unloadable(&pool, iteration[0]).await;
    // Act
    let res = kill(&app, ws, parent, "", user).await;
    // Assert
    assert_eq!(res.status(), 202, "no longer a 500");
    assert_eq!(exec_state(&pool, parent).await, "KILLED");
    assert_eq!(exec_state(&pool, iteration[0]).await, "KILLED");
    assert_eq!(run_row(&pool, iteration[0], "body").await.0, "KILLED");
    let again = egret_workflow::worker::claim(&pool, 10).await.unwrap();
    assert!(again.is_empty(), "nothing is left to claim: {again:?}");
}

/// **Fix round 1, I1, the worker's side.** The iteration's leaf is held, so
/// the route leaves it `KILLING`; once reaped, the kill-only claim closes it
/// by force, and its report to the (already `KILLED`, unloadable) parent is
/// skipped rather than failing the commit for ever.
#[sqlx::test(migrations = "../../migrations")]
async fn a_held_iteration_of_an_unloadable_loop_is_closed_by_its_kill_only_claim(
    pool: sqlx::PgPool,
) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let parent = start(&pool, ws, &one_loop("loop-held")).await;
    let iteration = loop_children(&pool, parent).await[0];
    assert_eq!(
        egret_workflow::worker::claim(&pool, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    make_unloadable(&pool, parent).await;
    make_unloadable(&pool, iteration).await;
    assert_eq!(kill(&app, ws, parent, "", user).await.status(), 202);
    assert_eq!(exec_state(&pool, parent).await, "KILLED");
    assert_eq!(run_row(&pool, iteration, "body").await.0, "KILLING");
    egret_workflow::worker::reap(&pool, std::time::Duration::ZERO)
        .await
        .expect("reap");
    // Act
    let reg = CancelRecorder::default();
    let ticked = egret_workflow::worker::tick(&pool, 10, &reg).await;
    // Assert
    assert!(ticked.is_ok(), "{ticked:?}");
    assert_eq!(run_row(&pool, iteration, "body").await.0, "KILLED");
    assert_eq!(exec_state(&pool, iteration).await, "KILLED");
    let again = egret_workflow::worker::claim(&pool, 10).await.unwrap();
    assert!(again.is_empty(), "nothing is left to claim: {again:?}");
}

/// **Fix round 1, I1 (b): a live parent that cannot be loaded hears no
/// report.** Killing the iteration alone closes it; the report to its
/// unloadable parent is dropped with a warning, and the parent is closed by
/// its own kill.
#[sqlx::test(migrations = "../../migrations")]
async fn an_unloadable_iterations_report_to_an_unloadable_live_parent_is_dropped(
    pool: sqlx::PgPool,
) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let parent = start(&pool, ws, &one_loop("loop-live-parent")).await;
    let iteration = loop_children(&pool, parent).await[0];
    make_unloadable(&pool, parent).await;
    make_unloadable(&pool, iteration).await;
    // Act
    let res = kill(&app, ws, iteration, "", user).await;
    // Assert
    assert_eq!(res.status(), 202, "no longer a 500");
    assert_eq!(exec_state(&pool, iteration).await, "KILLED");
    assert_eq!(
        exec_state(&pool, parent).await,
        "RUNNING",
        "the parent was not loaded, so it did not move"
    );
    assert_eq!(kill(&app, ws, parent, "", user).await.status(), 202);
    assert_eq!(exec_state(&pool, parent).await, "KILLED");
}

/// **Fix round 1 (review M5): only a row-content fault is swallowed.** The
/// parent of an unloadable `Subflow` child is locked by another transaction,
/// and the route's connections give up on a lock after 300 ms: the report's
/// load of the parent fails with `55P03`, an I/O-class error, and the kill
/// answers 500 with nothing written, as before D4.
#[sqlx::test(migrations = "../../migrations")]
async fn a_report_that_cannot_lock_the_parent_still_fails_the_kill(pool: sqlx::PgPool) {
    // Arrange: every connection of this pool gives up a lock after 300 ms.
    let opts = (*pool.connect_options())
        .clone()
        .options([("lock_timeout", "300ms")]);
    let impatient = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect_with(opts)
        .await
        .expect("a pool with lock_timeout");
    let (db, ws, user, app) = setup(&impatient).await;
    file_flow(&db, ws, user, &one_log("child")).await;
    let parent = start(&impatient, ws, &parent_with_children("report-locked")).await;
    let child = subflow_child(&impatient, parent).await;
    make_unloadable(&impatient, child).await;
    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM workflow.executions WHERE id = $1 FOR NO KEY UPDATE")
        .bind(parent)
        .execute(&mut *holder)
        .await
        .unwrap();
    // Act
    let res = kill(&app, ws, child, "", user).await;
    // Assert
    assert_eq!(
        res.status(),
        500,
        "a lock timeout is not a row-content fault"
    );
    holder.rollback().await.unwrap();
    assert_eq!(
        exec_state(&pool, child).await,
        "RUNNING",
        "nothing was written"
    );
    drop(app);
    impatient.close().await;
}

/// A `WorkingDirectory` block of one `Log` child.
const ONE_BLOCK: &str = "id: wd-held\nnamespace: t\ntasks:\n  - id: block\n    type: io.kestra.plugin.core.flow.WorkingDirectory\n    tasks:\n      - id: a\n        type: io.kestra.plugin.core.log.Log\n        message: hi\n";

/// **Fix round 1, the held block: a force-close closes a leased block claim
/// at once.** Rule 2 leaves a held run to the worker that can cancel it, but
/// a block's claim is never killed (`Watch::Block`) and a `KILLING` block
/// outside `FINISHING` is never claimable again, so leaving it `KILLING`
/// would keep the execution `KILLING` for ever.
#[sqlx::test(migrations = "../../migrations")]
async fn a_force_close_closes_a_leased_block_claim_at_once(pool: sqlx::PgPool) {
    // Arrange: the block's prepare claim is leased.
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = start(&pool, ws, ONE_BLOCK).await;
    let claimed = egret_workflow::worker::claim(&pool, 10).await.unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].block_phase.as_deref(), Some("PREPARING"));
    make_unloadable(&pool, ex).await;
    // Act
    let res = kill(&app, ws, ex, "", user).await;
    // Assert
    assert_eq!(res.status(), 202);
    assert_eq!(common::json(res).await["state"], "KILLED");
    assert_eq!(run_row(&pool, ex, "block").await.0, "KILLED");
    egret_workflow::worker::reap(&pool, std::time::Duration::ZERO)
        .await
        .expect("reap");
    let again = egret_workflow::worker::claim(&pool, 10).await.unwrap();
    assert!(again.is_empty(), "nothing is left to claim: {again:?}");
}

/// **Fix round 1, I1 (a): a report to a parent that is already terminal is
/// skipped in plain SQL, without taking the parent's lock.** The parent was
/// killed without cascade, so its `Subflow` child runs on; the child is then
/// made unloadable and killed while another transaction holds the parent's
/// row. Loading the parent would wait out the 300 ms `lock_timeout` and
/// answer 500.
#[sqlx::test(migrations = "../../migrations")]
async fn a_report_to_a_terminal_parent_does_not_lock_it(pool: sqlx::PgPool) {
    // Arrange
    let opts = (*pool.connect_options())
        .clone()
        .options([("lock_timeout", "300ms")]);
    let impatient = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect_with(opts)
        .await
        .expect("a pool with lock_timeout");
    let (db, ws, user, app) = setup(&impatient).await;
    file_flow(&db, ws, user, &one_log("child")).await;
    let parent = start(&impatient, ws, &parent_with_children("report-terminal")).await;
    let child = subflow_child(&impatient, parent).await;
    assert_eq!(
        kill(&app, ws, parent, "?cascade=false", user)
            .await
            .status(),
        202
    );
    assert_eq!(exec_state(&pool, parent).await, "KILLED");
    assert_eq!(exec_state(&pool, child).await, "RUNNING");
    make_unloadable(&impatient, child).await;
    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM workflow.executions WHERE id = $1 FOR NO KEY UPDATE")
        .bind(parent)
        .execute(&mut *holder)
        .await
        .unwrap();
    // Act
    let res = kill(&app, ws, child, "", user).await;
    // Assert
    assert_eq!(res.status(), 202, "the terminal parent was not locked");
    holder.rollback().await.unwrap();
    assert_eq!(exec_state(&pool, child).await, "KILLED");
    drop(app);
    impatient.close().await;
}
