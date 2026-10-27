//! The flow's `afterExecution:` in the worker (end hooks package #2b, spec
//! `docs/en/specs/2026-10-26-workflow-end-hooks-design.md` D8, D10, D11 and
//! the user's answer to Q3, 2026-10-27).
//!
//! `advance` runs the phase once the execution is terminal (Task B1). These
//! tests drive the real worker: an after-run is claimed and run like any
//! run, its settle leaves the execution's state, history and `ended_at`
//! alone, a `Subflow` parent hears its child only once the child is
//! *terminated* (D10), and a kill stops the phase (Q3).
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

/// One `Log` task `a`, then two sequential after-tasks `end1` and `end2`.
fn two_after(id: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: a\n    type: {LOG}\n    message: hi\n\
         afterExecution:\n  - id: end1\n    type: {LOG}\n    message: one\n\
         \x20 - id: end2\n    type: {LOG}\n    message: two\n"
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

// ── Part 2: a kill stops the phase (Q3) ─────────────────────────────────────

async fn kill(
    app: &common::App,
    ws: Uuid,
    id: Uuid,
    user: egret_core::UserId,
) -> axum::response::Response {
    common::post_no_body(
        app,
        &format!("/api/ws/{ws}/workflows/executions/{id}/kill"),
        user,
    )
    .await
}

/// A registry for the kill-only claims: a handler that is run answers
/// `SUCCESS`, and each `cancel` is counted.
#[derive(Default)]
struct KillOnly {
    cancels: std::sync::atomic::AtomicUsize,
    ran: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl egret_workflow::worker::Registry for KillOnly {
    async fn run(
        &self,
        task: &egret_workflow::worker::TaskInput,
    ) -> egret_core::Result<egret_workflow::worker::Outcome> {
        self.ran.lock().unwrap().push(task.task_id.clone());
        Ok(egret_workflow::worker::Outcome::success())
    }
    async fn cancel(&self, _task: &egret_workflow::worker::TaskInput) -> egret_core::Result<()> {
        self.cancels
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

/// One worker tick; returns the registry so the caller can read what ran.
async fn one_tick(pool: &sqlx::PgPool) -> KillOnly {
    let reg = KillOnly::default();
    egret_workflow::worker::tick(pool, 64, &reg)
        .await
        .expect("a worker tick");
    reg
}

/// **A kill stops a running after-task and leaves the execution alone** (Q3,
/// user answer 2026-10-27; a security divergence from Kestra, which answers
/// 409): the kill of a `SUCCESS` execution whose after-run is `RUNNING` is
/// accepted, the after-run is `KILLING` and then, through the kill-only
/// claim, `KILLED`, and the execution's state, history and `ended_at` are
/// byte-identical throughout.
#[sqlx::test(migrations = "../../migrations")]
async fn a_kill_stops_a_running_after_execution_task(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = start(&pool, ws, &one_after("killed_after")).await;
    finish(&pool, "a").await;
    let ended = exec_row(&pool, ex).await;
    assert_eq!(ended.0, "SUCCESS");
    assert_eq!(
        run_state(&pool, ex, "end").await.as_deref(),
        Some("RUNNING")
    );

    let res = kill(&app, ws, ex, user).await;
    assert_eq!(res.status(), 202);
    let body = common::json(res).await;
    assert_eq!(body["state"], "SUCCESS", "{body}");
    assert_eq!(
        run_state(&pool, ex, "end").await.as_deref(),
        Some("KILLING")
    );
    assert_eq!(exec_row(&pool, ex).await, ended);

    let reg = one_tick(&pool).await;
    assert_eq!(reg.cancels.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(
        reg.ran.lock().unwrap().is_empty(),
        "the killed run never ran"
    );
    assert_eq!(run_state(&pool, ex, "end").await.as_deref(), Some("KILLED"));
    assert_eq!(exec_row(&pool, ex).await, ended);
}

/// **A terminal execution with nothing of its phase live is still a 409**
/// (spec D3, unchanged): once the after-run has ended, and for a flow with
/// no `afterExecution:` at all. Nothing is written.
#[sqlx::test(migrations = "../../migrations")]
async fn a_kill_of_a_terminal_execution_without_after_runs_is_still_409(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let with = start(&pool, ws, &one_after("with_after")).await;
    finish(&pool, "a").await;
    finish(&pool, "end").await;
    let without = start(
        &pool,
        ws,
        "id: without\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n",
    )
    .await;
    finish(&pool, "a").await;
    for ex in [with, without] {
        let row = exec_row(&pool, ex).await;
        assert_eq!(row.0, "SUCCESS");
        let res = kill(&app, ws, ex, user).await;
        assert_eq!(res.status(), 409);
        assert_eq!(common::json(res).await["code"], "execution_terminated");
        assert_eq!(exec_row(&pool, ex).await, row);
    }
}

/// **A killed phase is over** (Q3): with two sequential after-tasks and the
/// kill landing during the first, the second is never created, nothing is
/// left for a worker, and a second kill is a 409 because nothing of the
/// phase is live.
#[sqlx::test(migrations = "../../migrations")]
async fn a_killed_after_execution_phase_creates_no_more_after_runs(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = start(&pool, ws, &two_after("two_after")).await;
    finish(&pool, "a").await;
    let ended = exec_row(&pool, ex).await;
    assert_eq!(
        run_state(&pool, ex, "end1").await.as_deref(),
        Some("RUNNING")
    );

    assert_eq!(kill(&app, ws, ex, user).await.status(), 202);
    one_tick(&pool).await;
    assert_eq!(
        run_state(&pool, ex, "end1").await.as_deref(),
        Some("KILLED")
    );
    let reg = one_tick(&pool).await;
    assert!(reg.ran.lock().unwrap().is_empty(), "{:?}", reg.ran);
    assert_eq!(
        run_state(&pool, ex, "end2").await,
        None,
        "an after-task was created after the phase was killed"
    );
    assert!(egret_workflow::worker::claim(&pool, 10)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(exec_row(&pool, ex).await, ended);
    assert_eq!(kill(&app, ws, ex, user).await.status(), 409);
}

/// **A parent hears a child whose `afterExecution:` was killed** (D10 and
/// Q3): the kill lands during the first of two after-tasks, so the second is
/// never created and the child is terminated once the killed run is
/// terminal; its parent's `Subflow` run takes the child's own state,
/// `SUCCESS`.
#[sqlx::test(migrations = "../../migrations")]
async fn a_parent_hears_a_child_whose_after_execution_was_killed(pool: sqlx::PgPool) {
    let (db, ws, user, app) = setup(&pool).await;
    file_flow(
        &db,
        ws,
        user,
        "id: child\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    \
         message: hi\nafterExecution:\n  - id: nap\n    type: io.kestra.plugin.core.flow.Sleep\n    \
         duration: PT1H\n  - id: wake\n    type: io.kestra.plugin.core.log.Log\n    message: never\n",
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
    assert_eq!(
        run_state(&pool, parent, "sub").await.as_deref(),
        Some("RUNNING")
    );

    assert_eq!(kill(&app, ws, child, user).await.status(), 202);
    one_tick(&pool).await;
    assert_eq!(
        run_state(&pool, child, "nap").await.as_deref(),
        Some("KILLED")
    );
    assert_eq!(run_state(&pool, child, "wake").await, None);
    assert_eq!(exec_row(&pool, child).await.0, "SUCCESS");
    assert_eq!(
        run_state(&pool, parent, "sub").await.as_deref(),
        Some("SUCCESS")
    );
    assert_eq!(exec_row(&pool, parent).await.0, "SUCCESS");
}

/// **A phase kill cascades into the phase's own children only** (Q3): a
/// `Subflow` in `afterExecution:` is `KILLED` and its child execution killed,
/// while a `wait: false` child the flow's `tasks:` started runs on.
#[sqlx::test(migrations = "../../migrations")]
async fn a_phase_kill_cascades_into_the_phases_children_only(pool: sqlx::PgPool) {
    let (db, ws, user, app) = setup(&pool).await;
    for (id, task) in [("child", "c"), ("bgchild", "b")] {
        file_flow(
            &db,
            ws,
            user,
            &format!(
                "id: {id}\nnamespace: t\ntasks:\n  - id: {task}\n    \
                 type: io.kestra.plugin.core.log.Log\n    message: hi\n"
            ),
        )
        .await;
    }
    let ex = start(
        &pool,
        ws,
        "id: parent\nnamespace: t\ntasks:\n  - id: bg\n    type: io.kestra.plugin.core.flow.Subflow\n    \
         namespace: t\n    flowId: bgchild\n    wait: false\nafterExecution:\n  - id: sub\n    \
         type: io.kestra.plugin.core.flow.Subflow\n    namespace: t\n    flowId: child\n",
    )
    .await;
    let ended = exec_row(&pool, ex).await;
    assert_eq!(ended.0, "SUCCESS");
    assert_eq!(
        run_state(&pool, ex, "sub").await.as_deref(),
        Some("RUNNING")
    );
    let child_of = |task: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT c.id FROM workflow.executions c \
                   JOIN workflow.task_runs r \
                     ON r.execution_id = c.trigger_execution_id AND r.run_ix = c.trigger_run_ix \
                  WHERE c.trigger_execution_id = $1 AND r.task_id = $2",
            )
            .bind(ex)
            .bind(task)
            .fetch_one(&pool)
            .await
            .expect("the child")
        }
    };
    let child = child_of("sub").await;
    let bgchild = child_of("bg").await;

    assert_eq!(kill(&app, ws, ex, user).await.status(), 202);
    assert_eq!(run_state(&pool, ex, "sub").await.as_deref(), Some("KILLED"));
    assert_eq!(exec_row(&pool, child).await.0, "KILLING");
    assert_eq!(exec_row(&pool, bgchild).await.0, "RUNNING");
    assert_eq!(exec_row(&pool, ex).await, ended);

    one_tick(&pool).await;
    assert_eq!(exec_row(&pool, child).await.0, "KILLED");
    assert_eq!(exec_row(&pool, bgchild).await.0, "SUCCESS");
    assert_eq!(exec_row(&pool, ex).await, ended);
}

// ── Fix round 1 (B3 review) ─────────────────────────────────────────────────

/// **A phase kill that cannot apply now is a retryable 409, not an accepted
/// kill** (fix round 1, I1): the only live run of the phase is a root
/// `WorkingDirectory` block whose prepare claim is out, which must stay
/// `RUNNING` for its claim. The route answers 409 `kill_not_applied`,
/// distinct from `execution_terminated`, writes nothing and audits nothing.
#[sqlx::test(migrations = "../../migrations")]
async fn a_phase_kill_of_a_preparing_block_is_a_retryable_409(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = start(
        &pool,
        ws,
        "id: wdafter\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    \
         message: hi\nafterExecution:\n  - id: wd\n    \
         type: io.kestra.plugin.core.flow.WorkingDirectory\n    tasks:\n      - id: w\n        \
         type: io.kestra.plugin.core.log.Log\n        message: in\n",
    )
    .await;
    finish(&pool, "a").await;
    let block: (String, Option<String>) = sqlx::query_as(
        "SELECT state, working_dir FROM workflow.task_runs \
          WHERE execution_id = $1 AND task_id = 'wd'",
    )
    .bind(ex)
    .fetch_one(&pool)
    .await
    .expect("the block");
    assert_eq!(
        (block.0.as_str(), block.1.as_deref()),
        ("RUNNING", Some("PREPARING")),
        "the fixture: the block waits on its prepare claim"
    );
    let row = exec_row(&pool, ex).await;
    common::audit_mutations(&app);

    let res = kill(&app, ws, ex, user).await;
    assert_eq!(res.status(), 409);
    let body = common::json(res).await;
    assert_eq!(body["code"], "kill_not_applied", "{body}");
    assert!(
        body["error"].as_str().is_some_and(|e| e.contains("retry")),
        "{body}"
    );
    assert_eq!(exec_row(&pool, ex).await, row);
    let after: (String, Option<String>) = sqlx::query_as(
        "SELECT state, working_dir FROM workflow.task_runs \
          WHERE execution_id = $1 AND task_id = 'wd'",
    )
    .bind(ex)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(after, block, "the refusal wrote the block");
    assert!(
        common::audit_mutations(&app).is_empty(),
        "a refusal audits nothing"
    );
}

/// **An ancestor's kill reaches a terminal child's running after-task**
/// (fix round 1, I2; a recorded divergence from Kestra, whose cascade skips a
/// terminal child): the child is `SUCCESS` with its after-run live, so its
/// parent's `Subflow` run still waits (D10). Killing the parent kills the
/// child's after-run, and the child's state, history and `ended_at` stay
/// byte-identical.
#[sqlx::test(migrations = "../../migrations")]
async fn a_parent_kill_stops_a_terminal_childs_running_after_task(pool: sqlx::PgPool) {
    let (db, ws, user, app) = setup(&pool).await;
    file_flow(
        &db,
        ws,
        user,
        "id: child\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    \
         message: hi\nafterExecution:\n  - id: nap\n    type: io.kestra.plugin.core.flow.Sleep\n    \
         duration: PT1H\n",
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
    let ended = exec_row(&pool, child).await;
    assert_eq!(ended.0, "SUCCESS");
    assert_eq!(
        run_state(&pool, child, "nap").await.as_deref(),
        Some("RUNNING")
    );

    assert_eq!(kill(&app, ws, parent, user).await.status(), 202);
    assert_eq!(
        run_state(&pool, child, "nap").await.as_deref(),
        Some("KILLING")
    );
    assert_eq!(exec_row(&pool, child).await, ended);
    assert_eq!(
        run_state(&pool, parent, "sub").await.as_deref(),
        Some("KILLED")
    );

    one_tick(&pool).await;
    assert_eq!(
        run_state(&pool, child, "nap").await.as_deref(),
        Some("KILLED")
    );
    assert_eq!(exec_row(&pool, child).await, ended);
    assert_eq!(exec_row(&pool, parent).await.0, "KILLED");
}

/// **An ancestor's kill leaves a terminal child with no live phase alone**
/// (fix round 1, I2): the child ended `SUCCESS` through an `Exit` while its
/// `Parallel` sibling `long` still runs -- a live run that is not of an
/// `afterExecution:`. Killing the parent (kept live by its own `p`) does not
/// touch the child: `long` keeps running and the child's row is unchanged.
#[sqlx::test(migrations = "../../migrations")]
async fn a_parent_kill_leaves_a_terminal_child_without_a_live_phase_alone(pool: sqlx::PgPool) {
    let (db, ws, user, app) = setup(&pool).await;
    file_flow(
        &db,
        ws,
        user,
        "id: child\nnamespace: t\ntasks:\n  - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    \
         tasks:\n      - id: exit\n        type: io.kestra.plugin.core.execution.Exit\n        \
         state: SUCCESS\n      - id: long\n        type: io.kestra.plugin.core.log.Log\n        \
         message: hi\n",
    )
    .await;
    let parent = start(
        &pool,
        ws,
        "id: parent\nnamespace: t\ntasks:\n  - id: ppar\n    type: io.kestra.plugin.core.flow.Parallel\n    \
         tasks:\n      - id: sub\n        type: io.kestra.plugin.core.flow.Subflow\n        \
         namespace: t\n        flowId: child\n      - id: p\n        \
         type: io.kestra.plugin.core.log.Log\n        message: hi\n",
    )
    .await;
    let child: Uuid =
        sqlx::query_scalar("SELECT id FROM workflow.executions WHERE trigger_execution_id = $1")
            .bind(parent)
            .fetch_one(&pool)
            .await
            .expect("the subflow child");
    let ended = exec_row(&pool, child).await;
    assert_eq!(ended.0, "SUCCESS", "the fixture: the Exit ended the child");
    assert_eq!(
        run_state(&pool, child, "long").await.as_deref(),
        Some("RUNNING"),
        "the fixture: a live sibling of the Exit"
    );
    assert_eq!(exec_row(&pool, parent).await.0, "RUNNING");

    assert_eq!(kill(&app, ws, parent, user).await.status(), 202);
    assert_eq!(
        run_state(&pool, child, "long").await.as_deref(),
        Some("RUNNING")
    );
    assert_eq!(exec_row(&pool, child).await, ended);
}

/// **Pin: a phase `Subflow` with `transmitFailed: true` whose child was
/// killed by someone else stops the rest of the phase** (fix round 1, minor
/// 2). The child's `KILLED` reaches the `Subflow` run as `KILLED`
/// (`guess_state`), which reads as a killed phase (`phase_killed`): `end` is
/// never created, and the parent's row is unchanged.
#[sqlx::test(migrations = "../../migrations")]
async fn a_phase_subflow_whose_child_is_killed_elsewhere_stops_the_phase(pool: sqlx::PgPool) {
    let (db, ws, user, app) = setup(&pool).await;
    file_flow(
        &db,
        ws,
        user,
        "id: child\nnamespace: t\ntasks:\n  - id: c\n    type: io.kestra.plugin.core.log.Log\n    \
         message: hi\n",
    )
    .await;
    let parent = start(
        &pool,
        ws,
        "id: parent\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    \
         message: hi\nafterExecution:\n  - id: sub\n    type: io.kestra.plugin.core.flow.Subflow\n    \
         namespace: t\n    flowId: child\n    transmitFailed: true\n  - id: end\n    \
         type: io.kestra.plugin.core.log.Log\n    message: after\n",
    )
    .await;
    finish(&pool, "a").await;
    let ended = exec_row(&pool, parent).await;
    assert_eq!(ended.0, "SUCCESS");
    let child: Uuid =
        sqlx::query_scalar("SELECT id FROM workflow.executions WHERE trigger_execution_id = $1")
            .bind(parent)
            .fetch_one(&pool)
            .await
            .expect("the subflow child");

    assert_eq!(kill(&app, ws, child, user).await.status(), 202);
    one_tick(&pool).await;
    assert_eq!(exec_row(&pool, child).await.0, "KILLED");
    assert_eq!(
        run_state(&pool, parent, "sub").await.as_deref(),
        Some("KILLED")
    );
    let reg = one_tick(&pool).await;
    assert!(reg.ran.lock().unwrap().is_empty(), "{:?}", reg.ran);
    assert_eq!(run_state(&pool, parent, "end").await, None);
    assert_eq!(exec_row(&pool, parent).await, ended);
}

/// **A phase kill is scoped and needs `workflow:manage`** (fix round 1, minor
/// 5), now that the route's terminal pre-check is gone: another workspace's
/// terminal execution with a live after-run is a 404, `workflow:read` alone a
/// 403, and neither writes anything.
#[sqlx::test(migrations = "../../migrations")]
async fn a_phase_kill_is_scoped_and_needs_manage(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage", "workflow:read"]).await;
    let (theirs, _other) = common::seed_workspace(&db).await;
    let (_, reader) = common::seed_workspace(&db).await;
    common::grant(&db, ws, reader, &["workflow:read"]).await;
    let app = common::app(&db).await;

    let foreign = start(&pool, theirs, &one_after("foreign")).await;
    let mine = start(&pool, ws, &one_after("mine")).await;
    let claimed = egret_workflow::worker::claim(&pool, 10).await.unwrap();
    for c in &claimed {
        egret_workflow::worker::commit_outcome(
            &pool,
            c,
            &egret_workflow::worker::Outcome::success(),
        )
        .await
        .unwrap();
    }
    for ex in [foreign, mine] {
        assert_eq!(exec_row(&pool, ex).await.0, "SUCCESS");
        assert_eq!(
            run_state(&pool, ex, "end").await.as_deref(),
            Some("RUNNING")
        );
    }
    let rows = (exec_row(&pool, foreign).await, exec_row(&pool, mine).await);

    assert_eq!(kill(&app, ws, foreign, user).await.status(), 404);
    assert_eq!(kill(&app, ws, mine, reader).await.status(), 403);
    assert_eq!(
        run_state(&pool, foreign, "end").await.as_deref(),
        Some("RUNNING")
    );
    assert_eq!(
        run_state(&pool, mine, "end").await.as_deref(),
        Some("RUNNING")
    );
    assert_eq!(
        (exec_row(&pool, foreign).await, exec_row(&pool, mine).await),
        rows
    );
    // Control: the manager's own kill is accepted.
    assert_eq!(kill(&app, ws, mine, user).await.status(), 202);
}

// ── Part 3: the other flowables inside the phase (D12's measurement) ────────

/// One `Log` task `a`, then `afterExecution:` holding `after` (the YAML of
/// the list, already indented under `afterExecution:`).
fn with_after(id: &str, after: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: a\n    type: {LOG}\n    message: hi\n\
         afterExecution:\n{after}"
    )
}

/// Run `source` to the end of its phase and check what every kind must keep:
/// the top after-task `after` and every other run of the execution is
/// `SUCCESS`, and the execution's state, history and `ended_at` are what the
/// main run ended with. Returns the execution id.
async fn run_phase(pool: &sqlx::PgPool, ws: Uuid, source: &str) -> Uuid {
    run_phase_with(pool, ws, source, false).await
}

/// `run_phase`; with `real_tasks` the worker runs the real core-task handlers
/// over an in-memory object store (a `WorkingDirectory` block needs both).
async fn run_phase_with(pool: &sqlx::PgPool, ws: Uuid, source: &str, real_tasks: bool) -> Uuid {
    let ex = start(pool, ws, source).await;
    finish(pool, "a").await;
    let ended = exec_row(pool, ex).await;
    assert_eq!(ended.0, "SUCCESS");
    assert_eq!(ended.1, json!(["CREATED", "RUNNING", "SUCCESS"]));
    assert!(ended.2.is_some());

    if real_tasks {
        egret_workflow::storage::install_mem_for_tests();
        let reg =
            egret_workflow::worker::Handlers::with_core_tasks(egret_workflow::storage::current());
        let mut ended_phase = false;
        for _ in 0..50 {
            if run_state(pool, ex, "after").await.as_deref() == Some("SUCCESS") {
                ended_phase = true;
                break;
            }
            egret_workflow::worker::tick_with(
                pool,
                64,
                &reg,
                egret_workflow::worker::WorkerTiming::default(),
            )
            .await
            .expect("a worker tick");
        }
        assert!(
            ended_phase,
            "the after-task `after` did not end in 50 ticks"
        );
    } else {
        tick_until(pool, "the after-task `after` ending", || async {
            run_state(pool, ex, "after").await.as_deref() == Some("SUCCESS")
        })
        .await;
    }
    let states: Vec<(String, String)> =
        sqlx::query_as("SELECT task_id, state FROM workflow.task_runs WHERE execution_id = $1")
            .bind(ex)
            .fetch_all(pool)
            .await
            .unwrap();
    assert!(
        states.iter().all(|(_, s)| s == "SUCCESS"),
        "every run of the execution is SUCCESS: {states:?}"
    );
    assert_eq!(
        exec_row(pool, ex).await,
        ended,
        "the phase moved the execution"
    );
    ex
}

async fn runs_of(pool: &sqlx::PgPool, ex: Uuid, task_id: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2 AND state = 'SUCCESS'",
    )
    .bind(ex)
    .bind(task_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// **A `Loop` over `[1, 2]` inside `afterExecution:` runs both iterations.**
#[sqlx::test(migrations = "../../migrations")]
async fn a_loop_inside_after_execution_runs_to_the_end(pool: sqlx::PgPool) {
    let (_db, ws, _user, _app) = setup(&pool).await;
    let ex = run_phase(
        &pool,
        ws,
        &with_after(
            "after-loop",
            &format!(
                "  - id: after\n    type: io.kestra.plugin.core.flow.Loop\n    values: [1, 2]\n    \
                 tasks:\n      - id: it\n        type: {LOG}\n        message: \"{{{{ taskrun.value }}}}\"\n"
            ),
        ),
    )
    .await;
    // A `Loop` iterates in sub-executions, not in runs of its own execution.
    let iterations: Vec<(i32, String)> = sqlx::query_as(
        "SELECT loop_index, state FROM workflow.executions \
          WHERE parent_execution_id = $1 ORDER BY loop_index",
    )
    .bind(ex)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        iterations,
        vec![(0, "SUCCESS".to_string()), (1, "SUCCESS".to_string())],
        "one terminal sub-execution per value"
    );
}

/// **A `LoopUntil` inside `afterExecution:` is refused at start** (package
/// #9 lifts it). Measured 2026-10-27: it waits between iterations with
/// `Command::PauseRun` (even for `interval: PT0S`), which the post-terminal
/// phase cannot apply; unrefused, the worker panicked in
/// `after_execution.rs` ("the post-terminal phase emitted a command that
/// writes the execution").
#[sqlx::test(migrations = "../../migrations")]
async fn a_loop_until_inside_after_execution_is_refused_at_start(pool: sqlx::PgPool) {
    let (_db, ws, _user, _app) = setup(&pool).await;
    let source = with_after(
        "after-loopuntil",
        &format!(
            "  - id: after\n    type: io.kestra.plugin.core.flow.LoopUntil\n    \
             condition: \"{{{{ outputs.after.iterationCount >= 2 }}}}\"\n    \
             checkFrequency:\n      interval: PT0S\n    \
             tasks:\n      - id: it\n        type: {LOG}\n        message: x\n"
        ),
    );
    let err = egret_workflow::worker::start_execution(&pool, ws, None, &source, json!({}))
        .await
        .expect_err("the start is refused");
    let text = format!("{err:?}");
    assert!(
        text.contains("unsupported_after_execution_task") && text.contains("LoopUntil"),
        "{text}"
    );
}

/// **A `Subflow` inside `afterExecution:` runs its child execution to a
/// terminal state, and the phase ends.**
#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_inside_after_execution_runs_to_the_end(pool: sqlx::PgPool) {
    let (db, ws, user, _app) = setup(&pool).await;
    file_flow(
        &db,
        ws,
        user,
        &format!("id: kid\nnamespace: t\ntasks:\n  - id: k\n    type: {LOG}\n    message: kid\n"),
    )
    .await;
    let ex = run_phase(
        &pool,
        ws,
        &with_after(
            "after-subflow",
            "  - id: after\n    type: io.kestra.plugin.core.flow.Subflow\n    namespace: t\n    flowId: kid\n",
        ),
    )
    .await;
    let kid: (Uuid, String) =
        sqlx::query_as("SELECT id, state FROM workflow.executions WHERE trigger_execution_id = $1")
            .bind(ex)
            .fetch_one(&pool)
            .await
            .expect("the subflow child");
    assert_eq!(kid.1, "SUCCESS");
    assert_eq!(
        run_state(&pool, kid.0, "k").await.as_deref(),
        Some("SUCCESS")
    );
}

/// **A `WorkingDirectory` inside `afterExecution:` runs its `core.log` child
/// and ends.**
#[sqlx::test(migrations = "../../migrations")]
async fn a_working_directory_inside_after_execution_runs_to_the_end(pool: sqlx::PgPool) {
    let (_db, ws, _user, _app) = setup(&pool).await;
    let ex = run_phase_with(
        &pool,
        ws,
        &with_after(
            "after-wd",
            &format!(
                "  - id: after\n    type: io.kestra.plugin.core.flow.WorkingDirectory\n    \
                 tasks:\n      - id: it\n        type: {LOG}\n        message: x\n"
            ),
        ),
        true,
    )
    .await;
    assert_eq!(runs_of(&pool, ex, "it").await, 1, "the child ran");
}

// ── Final-review fix (2026-10-28): a STORED source with a refused kind ─────

/// **A flow stored before D12 cannot make `afterExecution:` write the ended
/// execution** (final review, Important 1). On main a `Pause`, `Exit` or
/// `LoopUntil` inside `afterExecution:` was accepted at publish and never
/// run; an execution of it that is running at deploy (D4) re-plans its own
/// stored source with `plan_stored`, which skips the start-time refusal.
///
/// Built here as exactly that: the execution starts from a source
/// `build_plan` accepts, and its stored source is then replaced -- index for
/// index -- by one whose first after-task is an `Exit` with `state: FAILED`.
/// The after-run is FAILED with the refusal's text without running; the
/// execution's state, history and `ended_at`, and its flow-trigger events,
/// do not move; the next after-task runs.
#[sqlx::test(migrations = "../../migrations")]
async fn a_stored_exit_in_after_execution_fails_and_leaves_the_execution_alone(pool: sqlx::PgPool) {
    let (db, ws, user, _app) = setup(&pool).await;
    common::seed_flow(
        &db,
        ws,
        user,
        "id: listener\nnamespace: t\ntriggers:\n  - id: on_done\n    \
         type: io.kestra.plugin.core.trigger.Flow\n    states: [SUCCESS, FAILED, KILLED, KILLING]\n\
         tasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n",
        "draft",
    )
    .await;
    let ex = start(&pool, ws, &two_after("stored")).await;
    let stored = format!(
        "id: stored\nnamespace: t\ntasks:\n  - id: a\n    type: {LOG}\n    message: hi\n\
         afterExecution:\n  - id: end1\n    type: io.kestra.plugin.core.execution.Exit\n    state: FAILED\n\
         \x20 - id: end2\n    type: {LOG}\n    message: two\n"
    );
    assert!(
        egret_workflow::executor::build_plan(&stored).is_err(),
        "the stored source must be one a new start refuses"
    );
    sqlx::query("UPDATE workflow.executions SET source = $2 WHERE id = $1")
        .bind(ex)
        .bind(&stored)
        .execute(&pool)
        .await
        .expect("store the pre-D12 source");

    finish(&pool, "a").await;
    let ended = exec_row(&pool, ex).await;
    assert_eq!(ended.0, "SUCCESS");
    assert_eq!(ended.1, json!(["CREATED", "RUNNING", "SUCCESS"]));
    assert!(ended.2.is_some());
    let events_at_end: Vec<(Uuid, String)> = common::events(&db)
        .await
        .into_iter()
        .filter(|(id, _)| *id == ex)
        .collect();

    let (state, history, message): (String, Value, Option<String>) = sqlx::query_as(
        "SELECT state, history, message FROM workflow.task_runs \
          WHERE execution_id = $1 AND task_id = 'end1'",
    )
    .bind(ex)
    .fetch_one(&pool)
    .await
    .expect("the Exit's after-run");
    assert_eq!(state, "FAILED");
    assert_eq!(history, json!(["CREATED", "FAILED"]), "it never started");
    let message = message.expect("the refusal is the run's message");
    assert!(
        message.starts_with("task 'end1' is an Exit inside afterExecution"),
        "{message}"
    );

    finish(&pool, "end2").await;
    assert_eq!(
        run_state(&pool, ex, "end2").await.as_deref(),
        Some("SUCCESS")
    );
    assert_eq!(
        exec_row(&pool, ex).await,
        ended,
        "the phase moved the execution"
    );
    let events_after: Vec<(Uuid, String)> = common::events(&db)
        .await
        .into_iter()
        .filter(|(id, _)| *id == ex)
        .collect();
    assert_eq!(
        events_after, events_at_end,
        "the phase wrote a flow-trigger event"
    );
    assert_eq!(
        events_after.iter().filter(|(_, s)| s == "SUCCESS").count(),
        1,
        "{events_after:?}"
    );
    assert!(
        events_after
            .iter()
            .all(|(_, s)| s != "FAILED" && s != "KILLING"),
        "{events_after:?}"
    );
    assert!(
        egret_workflow::worker::claim(&pool, 10)
            .await
            .unwrap()
            .is_empty(),
        "nothing is left for a worker"
    );
}
