//! Package #12b: a `Subflow`'s `scheduleDate` defers the child's start.
//!
//! Kestra renders `scheduleDate` in the PARENT context; a FUTURE instant parks
//! the child CREATED and starts it at the instant; a PAST/now one starts it
//! immediately with no clamp; an unrenderable/invalid one FAILS the Subflow run
//! (`ExecutableUtils.java:240`, `ExecutionEventMessageHandler.java:148`).
//!
//! **The tests that drive the worker take turns** ([`SERIAL`]): `worker::tick`
//! and `worker::wake` are GLOBAL (they claim every runnable run and wake every
//! due execution in the database), so two running at once would settle each
//! other's executions.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5442/egret_test
//!   cargo test -p egret-workflow --test subflow_schedule_date

use chrono::{DateTime, Duration, Utc};
use egret_metastore::Db;
use serde_json::json;
use uuid::Uuid;

mod common;

/// The worker-driving tests take turns: `tick`/`wake` are global.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const CHILD: &str = "id: child\nnamespace: t\ntasks:\n  - id: a\n    \
     type: io.kestra.plugin.core.log.Log\n    message: hi\n";

fn parent(schedule_date: &str, wait: bool) -> String {
    format!(
        "id: parent\nnamespace: t\ntasks:\n  - id: sub\n    \
         type: io.kestra.plugin.core.flow.Subflow\n    namespace: t\n    flowId: child\n    \
         wait: {wait}\n    scheduleDate: \"{schedule_date}\"\n"
    )
}

async fn file_child(db: &Db, ws: Uuid, by: egret_core::UserId) {
    let flow = egret_workflow::spec::parse(CHILD).expect("the child fixture parses");
    egret_workflow::FlowRepo::new(db.clone())
        .create(
            ws,
            by,
            &egret_workflow::CreateFlowInput {
                namespace: flow.namespace.clone(),
                flow_id: flow.id.clone(),
                source: CHILD.to_string(),
                plan: serde_json::to_value(&flow).expect("a parsed flow serialises"),
                folder_id: None,
            },
        )
        .await
        .expect("file the child");
}

async fn start(pool: &sqlx::PgPool, ws: Uuid, source: &str) -> Uuid {
    egret_workflow::worker::start_execution_with_trigger_labels(
        pool,
        ws,
        None,
        source,
        json!({}),
        None,
        vec![],
    )
    .await
    .expect("start the parent")
}

#[derive(sqlx::FromRow)]
struct Child {
    id: Uuid,
    state: String,
    scheduled_start: Option<DateTime<Utc>>,
}

/// The one child a parent spawned, if any.
async fn child_of(pool: &sqlx::PgPool, parent: Uuid) -> Option<Child> {
    sqlx::query_as(
        "SELECT id, state, scheduled_start FROM workflow.executions \
          WHERE trigger_execution_id = $1",
    )
    .bind(parent)
    .fetch_optional(pool)
    .await
    .expect("read the child")
}

async fn task_runs(pool: &sqlx::PgPool, execution: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM workflow.task_runs WHERE execution_id = $1")
        .bind(execution)
        .fetch_one(pool)
        .await
        .expect("count task runs")
}

async fn state_of(pool: &sqlx::PgPool, id: Uuid) -> String {
    sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("read the execution state")
}

/// (a) A future `scheduleDate` parks the child CREATED with `scheduled_start`
/// set and no first settle driven.
#[tokio::test]
async fn a_future_schedule_date_parks_the_child_created_with_no_task_runs() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    file_child(&db, ws, user).await;

    let when = (Utc::now() + Duration::hours(1)).to_rfc3339();
    let p = start(&db.pool, ws, &parent(&when, false)).await;

    let child = child_of(&db.pool, p).await.expect("a child row exists");
    assert_eq!(child.state, "CREATED", "a parked child stays CREATED");
    assert!(
        child.scheduled_start.is_some(),
        "the future instant is stored on scheduled_start"
    );
    assert_eq!(
        task_runs(&db.pool, child.id).await,
        0,
        "a parked child has no task runs -- its first settle was NOT driven"
    );
}

/// (b) A parked child whose `scheduled_start` is due: one `wake` starts it, it
/// runs to terminal and the `wait: true` parent completes; a second `wake` does
/// not double-start it.
#[tokio::test]
async fn a_due_parked_child_is_woken_once_and_the_waiting_parent_completes() {
    let Some(db) = common::db().await else { return };
    let _turn = SERIAL.lock().await;
    let (ws, user) = common::seed_workspace(&db).await;
    file_child(&db, ws, user).await;

    let when = (Utc::now() + Duration::hours(1)).to_rfc3339();
    let p = start(&db.pool, ws, &parent(&when, true)).await;

    let child = child_of(&db.pool, p).await.expect("a child row exists");
    assert_eq!(child.state, "CREATED", "parked");
    assert_eq!(
        task_runs(&db.pool, child.id).await,
        0,
        "parked: no runs yet"
    );
    assert_eq!(
        state_of(&db.pool, p).await,
        "RUNNING",
        "the wait:true parent is still running while the child is parked"
    );

    // Make it due (the waker's clock is wall-clock `now()`).
    sqlx::query(
        "UPDATE workflow.executions SET scheduled_start = now() - interval '1 minute' \
          WHERE id = $1",
    )
    .bind(child.id)
    .execute(&db.pool)
    .await
    .expect("make the child due");

    // One wake starts it.
    let woken = egret_workflow::worker::wake(&db.pool, 16)
        .await
        .expect("wake");
    assert!(
        woken.contains(&child.id),
        "the due child was started by the waker: {woken:?}"
    );
    assert_ne!(
        state_of(&db.pool, child.id).await,
        "CREATED",
        "the child left CREATED on its first settle"
    );
    assert!(
        task_runs(&db.pool, child.id).await > 0,
        "the first settle created the child's runs"
    );

    // It runs to terminal and the waiting parent completes.
    common::drive_to(&db, child.id, "SUCCESS").await;
    common::drive_to(&db, p, "SUCCESS").await;

    // A second wake does not double-start it.
    let runs_before = task_runs(&db.pool, child.id).await;
    let again = egret_workflow::worker::wake(&db.pool, 16)
        .await
        .expect("a second wake");
    assert!(
        !again.contains(&child.id),
        "a started child is not re-woken: {again:?}"
    );
    assert_eq!(
        task_runs(&db.pool, child.id).await,
        runs_before,
        "the second wake created no new runs"
    );
}

/// (c) A past `scheduleDate` starts the child immediately (no park, no clamp):
/// `scheduled_start` stays NULL and the child runs to terminal.
#[tokio::test]
async fn a_past_schedule_date_starts_the_child_immediately() {
    let Some(db) = common::db().await else { return };
    let _turn = SERIAL.lock().await;
    let (ws, user) = common::seed_workspace(&db).await;
    file_child(&db, ws, user).await;

    let when = (Utc::now() - Duration::hours(1)).to_rfc3339();
    let p = start(&db.pool, ws, &parent(&when, false)).await;

    let child = child_of(&db.pool, p).await.expect("a child row exists");
    assert!(
        child.scheduled_start.is_none(),
        "a past scheduleDate is never parked (no clamp): {:?}",
        child.scheduled_start
    );
    // Its first settle was driven inline at spawn, so the ordinary claim loop --
    // not the waker -- carries it to terminal.
    common::drive_to(&db, child.id, "SUCCESS").await;
}

/// (d) An unrenderable `scheduleDate` fails the Subflow run and spawns nothing.
#[tokio::test]
async fn an_unrenderable_schedule_date_fails_the_subflow_run() {
    let Some(db) = common::db().await else { return };
    let (ws, user) = common::seed_workspace(&db).await;
    file_child(&db, ws, user).await;

    // `{{ nope.x }}` is an attribute of an undefined variable -- a render error,
    // the same shape `subflow_labels` relies on.
    let p = start(&db.pool, ws, &parent("{{ nope.x }}", false)).await;

    assert!(
        child_of(&db.pool, p).await.is_none(),
        "no child is spawned when scheduleDate fails to render"
    );
    assert_eq!(
        state_of(&db.pool, p).await,
        "FAILED",
        "the Subflow run fails, as every other Subflow render error does"
    );
}
