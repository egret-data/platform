//! Package 11a Task 1: `{{ trigger.* }}` in a fired flow's task render
//! context, bound from `workflow.executions.execution_trigger`. Requires
//! DATABASE_URL.
//!
//! Executions are born with the trigger record the fire paths write
//! (`schedule_trigger_record`, `flow_trigger_record` shapes), and the render
//! context is read back through `execution_render_context`, the loader every
//! task render shares.

mod common;

use egret_metastore::Db;
use egret_workflow::render_str;
use egret_workflow::worker::{
    execution_render_context, replay_execution, start_execution, start_execution_in, NewExecution,
    Outcome, Registry, ReplayInputs, ReplayKind, ReplayRequest, TaskInput,
};
use serde_json::{json, Value};
use uuid::Uuid;

const SCHEDULE: &str = "io.kestra.plugin.core.trigger.Schedule";
const FLOW: &str = "io.kestra.plugin.core.trigger.Flow";
const SRC: &str = "id: f\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n";

fn schedule_record() -> Value {
    json!({
        "id": "daily", "type": SCHEDULE,
        "variables": {"date": "2026-11-25T07:00:00.000000+07:00", "next": null, "previous": null}
    })
}

fn flow_record() -> Value {
    json!({
        "id": "on-src", "type": FLOW,
        "variables": {
            "state": "SUCCESS", "flowId": "src", "namespace": "t",
            "executionId": "exec-123", "outputs": {"x": "ten"}
        }
    })
}

async fn setup(pool: &sqlx::PgPool) -> Uuid {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    common::seed_workspace(&db).await.0
}

async fn born(
    pool: &sqlx::PgPool,
    ws: Uuid,
    trigger: Option<Value>,
    scheduled: bool,
    event: Option<i64>,
) -> Uuid {
    let mut tx = pool.begin().await.unwrap();
    let id = start_execution_in(
        &mut tx,
        NewExecution {
            workspace_id: ws,
            flow_id: None,
            source: SRC,
            inputs: json!({}),
            trigger,
            scheduled_date: scheduled.then(chrono::Utc::now),
            trigger_execution_id: None,
            execution_depth: 0,
            flow_trigger_event_id: event,
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
    .expect("start_execution_in");
    tx.commit().await.unwrap();
    id
}

async fn render(pool: &sqlx::PgPool, ex: Uuid, tpl: &str) -> Result<String, String> {
    let mut conn = pool.acquire().await.unwrap();
    let cx = execution_render_context(&mut conn, ex).await.unwrap();
    render_str(tpl, &cx).map_err(|e| e.to_string())
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_schedule_fire_binds_date_and_context(pool: sqlx::PgPool) {
    let ws = setup(&pool).await;
    let ex = born(&pool, ws, Some(schedule_record()), true, None).await;
    assert_eq!(
        render(&pool, ex, "{{ trigger.date }}").await.unwrap(),
        "2026-11-25T07:00:00.000000+07:00"
    );
    assert_eq!(
        render(&pool, ex, "{{ trigger._context.type }}")
            .await
            .unwrap(),
        SCHEDULE
    );
    assert_eq!(
        render(&pool, ex, "{{ trigger._context.id }}")
            .await
            .unwrap(),
        "daily"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_flow_fire_binds_the_source_values(pool: sqlx::PgPool) {
    let ws = setup(&pool).await;
    let ex = born(&pool, ws, Some(flow_record()), false, Some(1)).await;
    assert_eq!(
        render(&pool, ex, "{{ trigger.outputs.x }}").await.unwrap(),
        "ten"
    );
    assert_eq!(
        render(&pool, ex, "{{ trigger.executionId }}")
            .await
            .unwrap(),
        "exec-123"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_standalone_execution_has_no_trigger_binding(pool: sqlx::PgPool) {
    let ws = setup(&pool).await;
    let ex = start_execution(&pool, ws, None, SRC, json!({}))
        .await
        .unwrap();
    let err = render(&pool, ex, "{{ trigger.x }}")
        .await
        .expect_err("no fire, no `trigger`");
    assert!(err.contains("undefined"), "{err}");
    // control: the same loader still renders what is bound.
    assert!(render(&pool, ex, "{{ flow.id }}").await.is_ok());
}

struct Ok_;
#[async_trait::async_trait]
impl Registry for Ok_ {
    async fn run(&self, _t: &TaskInput) -> egret_core::Result<Outcome> {
        Ok(Outcome::succeeded_with(json!({})))
    }
    async fn cancel(&self, _t: &TaskInput) -> egret_core::Result<()> {
        Ok(())
    }
}

/// Ignored: `worker::copy::replay_execution` builds its `NewExecution` with
/// `trigger: None` (copy.rs:219), so a replay does NOT carry the column and
/// `trigger.*` is unbound there. Needs a product decision (copy trigger +
/// scheduled_date + flow_trigger_event_id on replay?) -- out of Task 1 scope.
#[ignore = "replay does not copy execution_trigger today (copy.rs:219)"]
#[sqlx::test(migrations = "../../migrations")]
async fn a_replay_of_a_schedule_fire_keeps_the_binding(pool: sqlx::PgPool) {
    let ws = setup(&pool).await;
    let old = born(&pool, ws, Some(schedule_record()), true, None).await;
    for _ in 0..30 {
        egret_workflow::worker::tick(&pool, 64, &Ok_).await.unwrap();
    }
    let state: String = sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
        .bind(old)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state, "SUCCESS");
    let run: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'a'",
    )
    .bind(old)
    .fetch_one(&pool)
    .await
    .unwrap();
    let user: Uuid = sqlx::query_scalar("SELECT created_by FROM workspaces WHERE id = $1")
        .bind(ws)
        .fetch_one(&pool)
        .await
        .unwrap();
    let new = replay_execution(
        &pool,
        &ReplayRequest {
            workspace_id: ws,
            from: old,
            source: SRC,
            kind: ReplayKind::FromTaskRun(run),
            inputs: ReplayInputs::Reuse,
            started_by: user,
            id: Uuid::new_v4(),
            input_files: Vec::new(),
        },
    )
    .await
    .expect("replay")
    .expect("not refused");
    assert_ne!(new, old);
    assert_eq!(
        render(&pool, new, "{{ trigger.date }}").await.unwrap(),
        "2026-11-25T07:00:00.000000+07:00"
    );
}
