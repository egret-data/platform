//! The flow's `afterExecution:` in the worker (end hooks package #2b, spec
//! `docs/en/specs/2026-10-26-workflow-end-hooks-design.md` D8, D10, D11 and
//! the user's answer to Q3, 2026-10-27).
//!
//! `advance` runs the phase once the execution is terminal (Task B1). These
//! tests drive the real worker: an after-run is claimed and run like any
//! run, its settle leaves the execution's state, history and `ended_at`
//! alone, and a `Subflow` parent hears its child only once the child is
//! *terminated* (D10).
//!
//! Every test is `#[sqlx::test]`: the worker's `claim` and `tick` are global,
//! so each test owns its database.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5459/egret_test
//!   cargo test -p egret-workflow --test end_hooks

use egret_metastore::Db;
use serde_json::{json, Value};
use uuid::Uuid;

mod common;

// ── Fixtures ────────────────────────────────────────────────────────────────

const LOG: &str = "io.kestra.plugin.core.log.Log";

/// One `Log` task `a`, then `afterExecution: [end]`.
fn one_after(id: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: a\n    type: {LOG}\n    message: hi\n\
         afterExecution:\n  - id: end\n    type: {LOG}\n    message: after\n"
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

/// The execution's own `(state, history, ended_at)`: what the phase must
/// never move (D8).
type ExecRow = (String, Value, Option<chrono::DateTime<chrono::Utc>>);

async fn exec_row(pool: &sqlx::PgPool, id: Uuid) -> ExecRow {
    sqlx::query_as("SELECT state, history, ended_at FROM workflow.executions WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("the execution row")
}

/// The state of `execution`'s run of `task_id`, or `None` when it has none.
async fn run_state(pool: &sqlx::PgPool, execution: Uuid, task_id: &str) -> Option<String> {
    sqlx::query_scalar(
        "SELECT state FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(execution)
    .bind(task_id)
    .fetch_optional(pool)
    .await
    .expect("read the run")
}

/// Claim exactly the runs of `task_id` (of any execution) that the worker
/// offers now, and commit each `SUCCESS`. Panics when the claim offers a
/// run of another task: these tests step the worker one task at a time.
async fn finish(pool: &sqlx::PgPool, task_id: &str) {
    let claimed = egret_workflow::worker::claim(pool, 10)
        .await
        .expect("claim");
    assert!(
        !claimed.is_empty(),
        "the worker offers no run; expected one of {task_id}"
    );
    for c in &claimed {
        assert_eq!(c.task_id, task_id, "the worker offered {}", c.task_id);
        egret_workflow::worker::commit_outcome(
            pool,
            c,
            &egret_workflow::worker::Outcome::success(),
        )
        .await
        .expect("commit the outcome");
    }
}

/// Worker ticks, every task answering `SUCCESS`, until `done` holds.
async fn tick_until<F, Fut>(pool: &sqlx::PgPool, what: &str, mut done: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let reg = common::Answering(true);
    for _ in 0..50 {
        if done().await {
            return;
        }
        egret_workflow::worker::tick(pool, 64, &reg)
            .await
            .expect("a worker tick");
    }
    panic!("{what} did not happen in 50 ticks");
}

// ── Part 1: the worker runs the phase (D8) ──────────────────────────────────

/// **An after-run is claimed and run once the execution has ended, and its
/// settle leaves the execution alone** (D8): the state, the history and the
/// `ended_at` the execution ended with are byte-identical after the
/// after-run's own settle, and the after-run reaches `SUCCESS`.
#[sqlx::test(migrations = "../../migrations")]
async fn an_after_execution_leaf_is_claimed_and_run_after_the_execution_ended(pool: sqlx::PgPool) {
    let (_db, ws, _user, _app) = setup(&pool).await;
    let ex = start(&pool, ws, &one_after("after")).await;

    finish(&pool, "a").await;
    let ended = exec_row(&pool, ex).await;
    assert_eq!(ended.0, "SUCCESS");
    assert_eq!(ended.1, json!(["CREATED", "RUNNING", "SUCCESS"]));
    assert!(ended.2.is_some(), "the execution's end is stamped");
    assert_eq!(
        run_state(&pool, ex, "end").await.as_deref(),
        Some("RUNNING"),
        "the after-run is started by the settle that ended the execution"
    );

    finish(&pool, "end").await;
    assert_eq!(
        run_state(&pool, ex, "end").await.as_deref(),
        Some("SUCCESS")
    );
    assert_eq!(
        exec_row(&pool, ex).await,
        ended,
        "the after-run's settle moved the execution"
    );
    assert!(
        egret_workflow::worker::claim(&pool, 10)
            .await
            .unwrap()
            .is_empty(),
        "nothing is left for a worker"
    );
}

/// **The Flow-trigger outbox hears the terminal state once** (D10: events
/// stay per execution state change): one event for `SUCCESS`, and the
/// after-run's settle, which appends no execution state, writes none.
#[sqlx::test(migrations = "../../migrations")]
async fn the_flow_trigger_event_is_written_once_at_the_terminal_state(pool: sqlx::PgPool) {
    let (db, ws, user, _app) = setup(&pool).await;
    // A listener in the workspace, so the outbox writes an event at all.
    common::seed_flow(
        &db,
        ws,
        user,
        "id: listener\nnamespace: t\ntriggers:\n  - id: on_done\n    \
         type: io.kestra.plugin.core.trigger.Flow\n    states: [SUCCESS, RUNNING]\n\
         tasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n",
        "draft",
    )
    .await;
    let ex = start(&pool, ws, &one_after("source")).await;
    finish(&pool, "a").await;
    let at_the_end: Vec<(Uuid, String)> = common::events(&db)
        .await
        .into_iter()
        .filter(|(id, _)| *id == ex)
        .collect();
    assert_eq!(
        at_the_end.iter().filter(|(_, s)| s == "SUCCESS").count(),
        1,
        "one event for the terminal state: {at_the_end:?}"
    );

    finish(&pool, "end").await;
    let after: Vec<(Uuid, String)> = common::events(&db)
        .await
        .into_iter()
        .filter(|(id, _)| *id == ex)
        .collect();
    assert_eq!(after, at_the_end, "the after-run's settle wrote an event");
}

/// **A `Subflow` parent hears its child only once the child is terminated**
/// (D10, Kestra `DE:752-759`): the child is `SUCCESS` with its after-run
/// still live, and the parent's `Subflow` run stays `RUNNING` until the
/// after-run has ended. Then it is told once, with the child's own state.
#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_parent_hears_the_child_only_after_its_after_execution(pool: sqlx::PgPool) {
    let (db, ws, user, _app) = setup(&pool).await;
    file_flow(
        &db,
        ws,
        user,
        "id: child\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    \
         message: hi\nafterExecution:\n  - id: nap\n    type: io.kestra.plugin.core.flow.Sleep\n    \
         duration: PT0S\n",
    )
    .await;
    let parent = start(
        &pool,
        ws,
        "id: parent\nnamespace: t\ntasks:\n  - id: sub\n    type: io.kestra.plugin.core.flow.Subflow\n    \
         namespace: t\n    flowId: child\n",
    )
    .await;
    let child: Uuid =
        sqlx::query_scalar("SELECT id FROM workflow.executions WHERE trigger_execution_id = $1")
            .bind(parent)
            .fetch_one(&pool)
            .await
            .expect("the subflow child");

    finish(&pool, "a").await;
    assert_eq!(exec_row(&pool, child).await.0, "SUCCESS");
    assert_eq!(
        run_state(&pool, child, "nap").await.as_deref(),
        Some("RUNNING")
    );
    assert_eq!(
        run_state(&pool, parent, "sub").await.as_deref(),
        Some("RUNNING"),
        "the parent heard a child whose afterExecution is still running"
    );
    assert_eq!(exec_row(&pool, parent).await.0, "RUNNING");

    finish(&pool, "nap").await;
    assert_eq!(
        run_state(&pool, parent, "sub").await.as_deref(),
        Some("SUCCESS")
    );
    assert_eq!(exec_row(&pool, parent).await.0, "SUCCESS");
}

/// **An executor update inside `afterExecution:` runs** (spec §5: the
/// `render_updates` gate is widened to `drives_forward || drives_after`):
/// the `SetVariables` run is computed and completed, its value is written,
/// and the next after-task runs.
#[sqlx::test(migrations = "../../migrations")]
async fn a_set_variables_inside_after_execution_runs(pool: sqlx::PgPool) {
    let (_db, ws, _user, _app) = setup(&pool).await;
    let ex = start(
        &pool,
        ws,
        "id: setvars\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    \
         message: hi\nafterExecution:\n  - id: set\n    \
         type: io.kestra.plugin.core.execution.SetVariables\n    variables:\n      x: one\n\
         \x20 - id: end\n    type: io.kestra.plugin.core.log.Log\n    message: after\n",
    )
    .await;
    tick_until(&pool, "the after-run `end` ending", || async {
        run_state(&pool, ex, "end").await.as_deref() == Some("SUCCESS")
    })
    .await;
    assert_eq!(
        run_state(&pool, ex, "set").await.as_deref(),
        Some("SUCCESS")
    );
    let vars: Option<Value> =
        sqlx::query_scalar("SELECT variables FROM workflow.executions WHERE id = $1")
            .bind(ex)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(vars.as_ref().and_then(|v| v.get("x")), Some(&json!("one")));
    let row = exec_row(&pool, ex).await;
    assert_eq!(row.0, "SUCCESS");
    assert_eq!(row.1, json!(["CREATED", "RUNNING", "SUCCESS"]));
}

/// **An `If` inside `afterExecution:` renders its condition and runs its
/// branch** (golden `valids/after-execution-flowable`).
#[sqlx::test(migrations = "../../migrations")]
async fn an_if_inside_after_execution_renders_and_runs(pool: sqlx::PgPool) {
    let (_db, ws, _user, _app) = setup(&pool).await;
    let source = include_str!("../oracle-exec/corpus/valids/after-execution-flowable.yaml");
    let ex = start(&pool, ws, source).await;
    tick_until(&pool, "the after-run `after_end` ending", || async {
        run_state(&pool, ex, "after_end").await.as_deref() == Some("SUCCESS")
    })
    .await;
    assert_eq!(
        run_state(&pool, ex, "after_if").await.as_deref(),
        Some("SUCCESS")
    );
    assert_eq!(
        run_state(&pool, ex, "after_if_log").await.as_deref(),
        Some("SUCCESS")
    );
    let row = exec_row(&pool, ex).await;
    assert_eq!(row.0, "SUCCESS");
    assert_eq!(row.1, json!(["CREATED", "RUNNING", "SUCCESS"]));
}
