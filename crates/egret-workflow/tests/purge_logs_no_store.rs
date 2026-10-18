//! Group C1 Task 4 fix round 1: `PurgeLogs` on a deployment with NO
//! `[storage]`. A binary of its own because `storage::install` is
//! process-wide and first-wins: `tests/purge_logs.rs` installs a store, this
//! one never does. The flow's namespace comes from the execution row, so the
//! task runs here; before the fix it came from the render scope, which only
//! a configured store builds, and every run FAILED. Requires DATABASE_URL.

mod common;

use chrono::{TimeZone, Utc};
use egret_workflow::logs::{LogLevel, LogLine, LogRepo, RunLog};
use egret_workflow::worker::handlers::purge_logs::{self, PurgeLogs};
use egret_workflow::worker::{start_execution, tick, Handlers, TaskHandler, TaskInput};
use serde_json::json;
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn seeded(pool: &sqlx::PgPool, ws: Uuid, ns: &str) -> Uuid {
    let ex: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow.executions \
           (workspace_id, namespace, flow_key, source, state, history) \
         VALUES ($1, $2, 'older', 'id: f\ntasks: []\n', 'SUCCESS', \
                 '[\"CREATED\",\"RUNNING\",\"SUCCESS\"]'::jsonb) \
         RETURNING id",
    )
    .bind(ws)
    .bind(ns)
    .fetch_one(pool)
    .await
    .unwrap();
    let lines: Vec<LogLine> = (0..4)
        .map(|i| LogLine {
            workspace_id: ws,
            execution_id: ex,
            task_run_id: None,
            task_id: None,
            attempt: None,
            level: LogLevel::Info,
            ts: Utc.with_ymd_and_hms(2026, 1, 1, i, 0, 0).unwrap(),
            message: format!("old {i}"),
        })
        .collect();
    assert_eq!(
        LogRepo::new(pool.clone())
            .insert_batch(&lines)
            .await
            .unwrap(),
        4
    );
    ex
}

async fn count(pool: &sqlx::PgPool, ex: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM workflow.task_logs WHERE execution_id = $1")
        .bind(ex)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Through the real worker, with no store installed: SUCCESS, and the scope
/// rule still holds (own namespace and child reached, sibling not).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn purge_logs_runs_with_no_file_store() {
    let _serial = SERIAL.lock().await;
    let Some(db) = common::db().await else { return };
    assert!(
        egret_workflow::storage::current().is_none(),
        "this binary must run with no store installed"
    );
    let p = &db.pool;
    let (ws, _) = common::seed_workspace(&db).await;
    let own = seeded(p, ws, "n.s").await;
    let child = seeded(p, ws, "n.s.c").await;
    let sibling = seeded(p, ws, "n.sx").await;

    let source = "id: purger\nnamespace: n.s\ntasks:\n  \
        - id: purge\n    type: io.kestra.plugin.core.log.PurgeLogs\n    \
          endDate: \"{{ now() | dateAdd(1, 'HOURS') }}\"\n";
    let ex = start_execution(p, ws, None, source, json!({}))
        .await
        .unwrap();
    let mut reg = Handlers::with_core_tasks(None);
    purge_logs::register(&mut reg, p.clone());
    for _ in 0..64 {
        if tick(p, 16, &reg).await.expect("a worker error") == 0 {
            break;
        }
    }
    let (state, outputs, message): (String, Option<serde_json::Value>, Option<String>) =
        sqlx::query_as(
            "SELECT state, outputs, message FROM workflow.task_runs \
              WHERE execution_id = $1 AND task_id = 'purge'",
        )
        .bind(ex)
        .fetch_one(p)
        .await
        .unwrap();
    assert_eq!(state, "SUCCESS", "{message:?}");
    assert_eq!(
        outputs,
        Some(json!({"count": 8, "executionLogsCount": 8, "nonExecutionLogsCount": 0}))
    );
    assert_eq!(
        (
            count(p, own).await,
            count(p, child).await,
            count(p, sibling).await
        ),
        (0, 0, 4)
    );
}

/// No execution row for the run (by id AND workspace): FAILED with the
/// "not a flow run" message, and nothing deleted.
#[tokio::test]
async fn a_run_with_no_execution_row_fails_and_deletes_nothing() {
    let _serial = SERIAL.lock().await;
    let Some(db) = common::db().await else { return };
    let p = &db.pool;
    let (ws, _) = common::seed_workspace(&db).await;
    let (other_ws, _) = common::seed_workspace(&db).await;
    let real = seeded(p, ws, "n.s").await;
    let handler = PurgeLogs::new(p.clone());
    // A random id, and the real execution's id under another workspace.
    for (workspace_id, execution_id) in [(ws, Uuid::new_v4()), (other_ws, real)] {
        let task = TaskInput {
            workspace_id,
            execution_id,
            run_id: Uuid::new_v4(),
            task_id: "purge".to_string(),
            type_id: purge_logs::TYPE_ID.to_string(),
            value: None,
            attempt: 1,
            spec: json!({"endDate": "2030-01-01T00:00:00Z"}),
            render: egret_workflow::runners::RenderContext::new(),
            timeout: None,
            working_dir: None,
            principal: None,
            claimed_at: Utc::now(),
            log: RunLog::discard(),
        };
        let out = handler.run(&task).await.unwrap();
        assert_eq!(out.state, egret_workflow::executor::RunState::Failed);
        assert!(
            out.message
                .as_deref()
                .unwrap_or_default()
                .contains("not a task run of a flow's execution"),
            "{:?}",
            out.message
        );
    }
    assert_eq!(count(p, real).await, 4);
}
