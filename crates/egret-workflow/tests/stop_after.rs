//! Package 11b Task 5: a Schedule's `stopAfter` switches the trigger off when
//! an execution it started ends in one of the listed states. The off switch is
//! the manual one (`workflow.disabled_triggers`), so a later `sync` keeps it.
//!
//! Every test is `#[sqlx::test]`: each owns its database. Executions end
//! through the REAL worker tick, so through the real `settle`.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5474/egret_test
//!   export EGRET_DATABASE__URL=$DATABASE_URL
//!   cargo test -p egret-workflow --test stop_after

use chrono::Utc;
use egret_metastore::Db;
use serde_json::json;
use uuid::Uuid;

mod common;

const SCHEDULE: &str = "io.kestra.plugin.core.trigger.Schedule";

fn flow_src(stop_after: &str) -> String {
    format!(
        "id: sa\nnamespace: t\ntriggers:\n  - id: s\n    type: {SCHEDULE}\n    cron: \"*/1 * * * *\"\n    allowConcurrent: true\n{stop_after}tasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n"
    )
}

async fn setup(pool: &sqlx::PgPool, src: &str) -> (Db, Uuid, Uuid) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    let flow = egret_workflow::spec::parse(src).expect("parses");
    let input = egret_workflow::CreateFlowInput {
        namespace: flow.namespace.clone(),
        flow_id: flow.id.clone(),
        source: src.to_string(),
        plan: serde_json::to_value(&flow).expect("serialises"),
        folder_id: None,
    };
    let flow_id = egret_workflow::FlowRepo::new(db.clone())
        .create(ws, user, &input)
        .await
        .expect("file the flow")
        .id;
    (db, ws, flow_id)
}

/// Start an execution the way a fire does, or with no trigger at all.
async fn start(pool: &sqlx::PgPool, ws: Uuid, flow: Uuid, src: &str, trigger: bool) -> Uuid {
    let mut tx = pool.begin().await.unwrap();
    let id = egret_workflow::worker::start_execution_in(
        &mut tx,
        egret_workflow::worker::NewExecution {
            workspace_id: ws,
            flow_id: Some(flow),
            source: src,
            inputs: json!({}),
            trigger: trigger.then(|| json!({"id": "s", "type": SCHEDULE, "variables": {}})),
            scheduled_date: trigger.then(Utc::now),
            trigger_execution_id: None,
            execution_depth: 0,
            flow_trigger_event_id: None,
            born_failed: false,
            replayed_from_execution_id: None,
            id: None,
            input_files: Vec::new(),
            labels: None,
            backfill_id: None,
            variables: None,
            started_by: None,
        },
    )
    .await
    .expect("start");
    tx.commit().await.unwrap();
    id
}

async fn tick(pool: &sqlx::PgPool, ok: bool) {
    egret_workflow::worker::tick(pool, 3, &common::Answering(ok))
        .await
        .expect("tick");
}

async fn state(pool: &sqlx::PgPool, id: Uuid) -> String {
    sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// `(disabled column, next_evaluation_date is set, switch rows)`.
async fn trigger(pool: &sqlx::PgPool, flow: Uuid) -> (bool, bool, i64) {
    let (disabled, queued): (bool, bool) = sqlx::query_as(
        "SELECT disabled, next_evaluation_date IS NOT NULL FROM workflow.triggers \
         WHERE flow_id = $1 AND trigger_id = 's'",
    )
    .bind(flow)
    .fetch_one(pool)
    .await
    .unwrap();
    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.disabled_triggers WHERE flow_id = $1 AND trigger_id = 's'",
    )
    .bind(flow)
    .fetch_one(pool)
    .await
    .unwrap();
    (disabled, queued, rows)
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_matching_terminal_disables_the_trigger(pool: sqlx::PgPool) {
    // Arrange
    let src = flow_src("    stopAfter: [FAILED]\n");
    let (_db, ws, flow) = setup(&pool, &src).await;
    assert_eq!(trigger(&pool, flow).await, (false, true, 0));
    let id = start(&pool, ws, flow, &src, true).await;

    // Act
    tick(&pool, false).await;

    // Assert
    assert_eq!(state(&pool, id).await, "FAILED");
    assert_eq!(trigger(&pool, flow).await, (true, false, 1));
    let by: Option<Uuid> =
        sqlx::query_scalar("SELECT disabled_by FROM workflow.disabled_triggers WHERE flow_id = $1")
            .bind(flow)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(by, None, "a system disable has no user");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_terminal_outside_stop_after_leaves_it_enabled(pool: sqlx::PgPool) {
    let src = flow_src("    stopAfter: [FAILED]\n");
    let (_db, ws, flow) = setup(&pool, &src).await;
    let id = start(&pool, ws, flow, &src, true).await;

    tick(&pool, true).await;

    assert_eq!(state(&pool, id).await, "SUCCESS");
    assert_eq!(trigger(&pool, flow).await, (false, true, 0));
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_execution_no_trigger_started_disables_nothing(pool: sqlx::PgPool) {
    let src = flow_src("    stopAfter: [FAILED]\n");
    let (_db, ws, flow) = setup(&pool, &src).await;
    let id = start(&pool, ws, flow, &src, false).await;

    tick(&pool, false).await;

    assert_eq!(state(&pool, id).await, "FAILED");
    assert_eq!(trigger(&pool, flow).await, (false, true, 0));
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_absent_stop_after_never_disables(pool: sqlx::PgPool) {
    let src = flow_src("");
    let (_db, ws, flow) = setup(&pool, &src).await;
    let id = start(&pool, ws, flow, &src, true).await;

    tick(&pool, false).await;

    assert_eq!(state(&pool, id).await, "FAILED");
    assert_eq!(trigger(&pool, flow).await, (false, true, 0));
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_disable_survives_a_sync(pool: sqlx::PgPool) {
    // Arrange
    let src = flow_src("    stopAfter: [FAILED, WARNING]\n");
    let (_db, ws, flow) = setup(&pool, &src).await;
    start(&pool, ws, flow, &src, true).await;
    tick(&pool, false).await;
    assert_eq!(trigger(&pool, flow).await, (true, false, 1));

    // Act: the reconcile pass re-derives every trigger from the source.
    egret_workflow::scheduler::reconcile(&pool)
        .await
        .expect("reconcile");

    // Assert
    let (disabled, queued, rows) = trigger(&pool, flow).await;
    assert!(
        disabled && rows == 1,
        "still off after sync: {:?}",
        (disabled, queued, rows)
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_killed_execution_disables_a_trigger_that_stops_after_killed(pool: sqlx::PgPool) {
    // Arrange
    let src = flow_src("    stopAfter: [KILLED]\n");
    let (db, ws, flow) = setup(&pool, &src).await;
    let user: Uuid = sqlx::query_scalar("SELECT id FROM users LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    let user = egret_core::UserId(user);
    common::grant(&db, ws, user, &["workflow:manage", "workflow:read"]).await;
    let app = common::app(&db).await;
    let id = start(&pool, ws, flow, &src, true).await;

    // Act: the kill route, then the worker tick that finishes the kill.
    let res = common::post_no_body(
        &app,
        &format!("/api/ws/{ws}/workflows/executions/{id}/kill"),
        user,
    )
    .await;
    assert_eq!(res.status(), 202);
    egret_workflow::worker::tick(&pool, 3, &CancelOnly)
        .await
        .expect("tick");

    // Assert
    assert_eq!(state(&pool, id).await, "KILLED");
    assert_eq!(trigger(&pool, flow).await, (true, false, 1));
}

/// A registry whose only work is the kill-only claim's cancel.
struct CancelOnly;

#[async_trait::async_trait]
impl egret_workflow::worker::Registry for CancelOnly {
    async fn run(
        &self,
        _task: &egret_workflow::worker::TaskInput,
    ) -> egret_core::Result<egret_workflow::worker::Outcome> {
        panic!("a kill runs no handler");
    }
    async fn cancel(&self, _task: &egret_workflow::worker::TaskInput) -> egret_core::Result<()> {
        Ok(())
    }
}
