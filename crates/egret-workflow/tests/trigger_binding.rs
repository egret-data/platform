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
const WEBHOOK: &str = "io.kestra.plugin.core.trigger.Webhook";
const FLOW: &str = "io.kestra.plugin.core.trigger.Flow";
const SRC: &str = "id: f\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n";

fn src_reading(tpl: &str) -> String {
    format!("id: f\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.debug.Return\n    format: \"{tpl}\"\n")
}

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
    source: &str,
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
            source,
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
async fn a_schedule_fired_task_renders_date_and_context(pool: sqlx::PgPool) {
    let ws = setup(&pool).await;
    for (tpl, want) in [
        ("{{ trigger.date }}", "2026-11-25T07:00:00.000000+07:00"),
        ("{{ trigger._context.type }}", SCHEDULE),
        ("{{ trigger._context.id }}", "daily"),
    ] {
        let ex = born(
            &pool,
            ws,
            &src_reading(tpl),
            Some(schedule_record()),
            true,
            None,
        )
        .await;
        let (msg, state) = task_message(&pool, ex, tpl).await;
        assert_eq!(msg.as_deref(), Some(want), "{tpl} (state {state})");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_flow_fired_task_renders_the_source_values(pool: sqlx::PgPool) {
    let ws = setup(&pool).await;
    for (i, (tpl, want)) in [
        ("{{ trigger.outputs.x }}", "ten"),
        ("{{ trigger.executionId }}", "exec-123"),
    ]
    .into_iter()
    .enumerate()
    {
        let ex = born(
            &pool,
            ws,
            &src_reading(tpl),
            Some(flow_record()),
            false,
            Some(i as i64 + 1),
        )
        .await;
        let (msg, state) = task_message(&pool, ex, tpl).await;
        assert_eq!(msg.as_deref(), Some(want), "{tpl} (state {state})");
    }
}

/// The record the webhook route stores (`ExecutionTrigger` serialised): `body`
/// only when the request had one. Both coupled columns stay NULL.
fn webhook_record(body: Option<Value>) -> Value {
    let mut v = json!({
        "headers": {"x": ["hdr"]},
        "parameters": {"x": ["a", "b"]},
    });
    if let Some(b) = body {
        v["body"] = b;
    }
    json!({"id": "hook", "type": WEBHOOK, "variables": v})
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_webhook_fired_task_renders_body_headers_parameters(pool: sqlx::PgPool) {
    let ws = setup(&pool).await;
    for (tpl, want) in [
        ("{{ trigger.body.hello }}", "world"),
        ("{{ trigger.headers.x[0] }}", "hdr"),
        ("{{ trigger.parameters.x[1] }}", "b"),
        ("{{ trigger._context.type }}", WEBHOOK),
        ("{{ trigger._context.id }}", "hook"),
    ] {
        let ex = born(
            &pool,
            ws,
            &src_reading(tpl),
            Some(webhook_record(Some(json!({"hello": "world"})))),
            false,
            None,
        )
        .await;
        let (msg, state) = task_message(&pool, ex, tpl).await;
        assert_eq!(msg.as_deref(), Some(want), "{tpl} (state {state})");
    }
}

/// A GET has no `body` key (Kestra parity): reading it fails to render.
#[sqlx::test(migrations = "../../migrations")]
async fn a_bodyless_webhook_fire_has_no_trigger_body(pool: sqlx::PgPool) {
    let ws = setup(&pool).await;
    let tpl = "{{ trigger.body }}";
    let ex = born(
        &pool,
        ws,
        &src_reading(tpl),
        Some(webhook_record(None)),
        false,
        None,
    )
    .await;
    let (msg, state) = task_message(&pool, ex, tpl).await;
    assert_eq!(msg, None, "no body key: the task must not render");
    assert_eq!(state, "FAILED");
}

/// Guard 2: the Flow trigger's OWN `when`/`inputs` context
/// (`execution_render_context`) has no `trigger`, even when the SOURCE was
/// itself trigger-fired.
#[sqlx::test(migrations = "../../migrations")]
async fn a_flow_triggers_own_context_has_no_trigger_even_for_a_fired_source(pool: sqlx::PgPool) {
    let ws = setup(&pool).await;
    let ex = born(&pool, ws, SRC, Some(schedule_record()), true, None).await;
    let err = render(&pool, ex, "{{ trigger.executionId }}")
        .await
        .expect_err("the Flow trigger's when/inputs context must not bind `trigger`");
    assert!(err.contains("undefined"), "{err}");
    assert!(render(&pool, ex, "{{ flow.id }}").await.is_ok(), "control");
}

/// Guard 1: a standalone execution's task cannot read `trigger.*`.
#[sqlx::test(migrations = "../../migrations")]
async fn a_standalone_execution_has_no_trigger_binding(pool: sqlx::PgPool) {
    let ws = setup(&pool).await;
    let src = src_reading("{{ trigger.x }}");
    let ex = start_execution(&pool, ws, None, &src, json!({}))
        .await
        .unwrap();
    let (msg, state) = task_message(&pool, ex, "").await;
    assert_eq!(msg, None, "the task must not render");
    assert_eq!(state, "FAILED");
}

/// Born with a trigger, the task `a` is driven by the real core handlers;
/// returns what it rendered (`Return.outputs.value`) and the task's state.
async fn task_message(pool: &sqlx::PgPool, ex: Uuid, _tpl: &str) -> (Option<String>, String) {
    let reg = egret_workflow::worker::Handlers::with_core_tasks(None);
    for _ in 0..30 {
        egret_workflow::worker::tick(pool, 64, &reg).await.unwrap();
    }
    let (state, outputs): (String, Option<Value>) = sqlx::query_as(
        "SELECT state, outputs FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'a'",
    )
    .bind(ex)
    .fetch_one(pool)
    .await
    .unwrap();
    let msg = outputs.and_then(|o| o["value"].as_str().map(String::from));
    (msg, state)
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

/// Settles `old` and replays it from its task `a`; returns the replay's id.
/// The original gets a real `flow_id` first, so the replay (which copies it)
/// would collide with the original on the one-per-fire unique indexes if they
/// did not exclude replays.
async fn replay_of(pool: &sqlx::PgPool, ws: Uuid, old: Uuid, source: &str) -> Uuid {
    let flow: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow.flows (workspace_id, namespace, flow_id, source, plan) \
         VALUES ($1, 't', gen_random_uuid()::text, $2, '{}') RETURNING id",
    )
    .bind(ws)
    .bind(source)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("UPDATE workflow.executions SET flow_id = $2 WHERE id = $1")
        .bind(old)
        .bind(flow)
        .execute(pool)
        .await
        .unwrap();
    for _ in 0..30 {
        egret_workflow::worker::tick(pool, 64, &Ok_).await.unwrap();
    }
    let state: String = sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
        .bind(old)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(state, "SUCCESS");
    let run: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'a'",
    )
    .bind(old)
    .fetch_one(pool)
    .await
    .unwrap();
    let user: Uuid = sqlx::query_scalar("SELECT created_by FROM workspaces WHERE id = $1")
        .bind(ws)
        .fetch_one(pool)
        .await
        .unwrap();
    let new = replay_execution(
        pool,
        &ReplayRequest {
            workspace_id: ws,
            from: old,
            source,
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
    new
}

/// A replay carries the trigger record and its coupled columns verbatim, and
/// coexists with the original despite the one-per-fire unique indexes.
#[sqlx::test(migrations = "../../migrations")]
async fn a_replay_of_a_schedule_fire_keeps_the_binding(pool: sqlx::PgPool) {
    let ws = setup(&pool).await;
    let src = src_reading("{{ trigger.date }}");
    let old = born(&pool, ws, &src, Some(schedule_record()), true, None).await;
    let new = replay_of(&pool, ws, old, &src).await;
    let (msg, state) = task_message(&pool, new, "").await;
    assert_eq!(
        msg.as_deref(),
        Some("2026-11-25T07:00:00.000000+07:00"),
        "state {state}"
    );
    let same: bool = sqlx::query_scalar(
        "SELECT n.execution_trigger = o.execution_trigger \
            AND n.scheduled_date = o.scheduled_date AND n.flow_trigger_event_id IS NULL \
            AND n.flow_id = o.flow_id \
           FROM workflow.executions n, workflow.executions o WHERE n.id = $1 AND o.id = $2",
    )
    .bind(new)
    .bind(old)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(same);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_replay_of_a_flow_fire_keeps_the_binding(pool: sqlx::PgPool) {
    let ws = setup(&pool).await;
    for (tpl, want) in [
        ("{{ trigger.outputs.x }}", "ten"),
        ("{{ trigger.executionId }}", "exec-123"),
    ] {
        let src = src_reading(tpl);
        let old = born(&pool, ws, &src, Some(flow_record()), false, Some(7)).await;
        let new = replay_of(&pool, ws, old, &src).await;
        let (msg, state) = task_message(&pool, new, tpl).await;
        assert_eq!(msg.as_deref(), Some(want), "{tpl} (state {state})");
        let ev: Option<i64> = sqlx::query_scalar(
            "SELECT flow_trigger_event_id FROM workflow.executions WHERE id = $1",
        )
        .bind(new)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(ev, Some(7));
    }
}
/// A replay of a non-triggered execution stays NULL on all three columns
/// (no trigger is fabricated), and its `trigger.*` still does not render.
#[sqlx::test(migrations = "../../migrations")]
async fn a_replay_of_a_standalone_execution_has_no_trigger(pool: sqlx::PgPool) {
    let ws = setup(&pool).await;
    let src = src_reading("{{ trigger.x }}");
    let old = born(&pool, ws, &src, None, false, None).await;
    let new = replay_of(&pool, ws, old, &src).await;
    let nulls: bool = sqlx::query_scalar(
        "SELECT execution_trigger IS NULL AND scheduled_date IS NULL \
            AND flow_trigger_event_id IS NULL FROM workflow.executions WHERE id = $1",
    )
    .bind(new)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(nulls);
    let (msg, state) = task_message(&pool, new, "").await;
    assert_eq!(msg, None, "no trigger fabricated");
    assert_eq!(state, "FAILED");
}
