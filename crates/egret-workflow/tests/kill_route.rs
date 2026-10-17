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

// ── Spec §3.2 step 2: a leaf nobody holds ───────────────────────────────────

/// **A leaf waiting for a worker is `KILLED` at once, with `ended_at`**, and
/// with nothing else running the execution answers `KILLED` (step 4's
/// `advance()` in the same transaction). Its open attempt is closed `KILLED`.
#[sqlx::test(migrations = "../../migrations")]
async fn a_waiting_leaf_is_killed_at_once_and_so_is_its_execution(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = start(&pool, ws, &one_log("waiting")).await;

    let res = kill(&app, ws, ex, "", user).await;
    assert_eq!(res.status(), 202);
    let body = common::json(res).await;
    assert_eq!(body["state"], "KILLED", "{body}");

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
    // Nothing is left for a worker.
    assert!(egret_workflow::worker::claim(&pool, 10)
        .await
        .unwrap()
        .is_empty());
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
        json!({"from_state": "RUNNING", "cascade": false, "outcome": "KILLED"})
    );

    // A refusal audits nothing.
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

/// **The same with nothing leased**: every execution in the tree ends
/// `KILLED` in the route's own transaction.
#[sqlx::test(migrations = "../../migrations")]
async fn cascade_with_nothing_leased_kills_the_whole_tree_at_once(pool: sqlx::PgPool) {
    let (db, ws, user, app) = setup(&pool).await;
    file_flow(&db, ws, user, &one_log("child")).await;
    let parent = start(&pool, ws, &parent_with_children("cascade-free")).await;
    let iteration = loop_children(&pool, parent).await;
    let child = subflow_child(&pool, parent).await;

    let res = kill(&app, ws, parent, "?cascade=true", user).await;
    assert_eq!(res.status(), 202);
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
    assert_eq!(exec_state(&pool, source).await, "KILLED");
    assert_eq!(exec_state(&pool, replay).await, "RUNNING");
    assert_eq!(run_row(&pool, replay, "a").await.0, "RUNNING");
    assert_eq!(exec_state(&pool, fired).await, "RUNNING");
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
    assert_eq!(run_row(&pool, ex, "waiting").await.0, "KILLED");
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
