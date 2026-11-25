//! Package 10a Task 1: `secret()` in a `Log` task's `message:`, through the
//! REAL worker and the REAL log writer. Requires DATABASE_URL.
//!
//! The value resolves, is logged masked (`******`), and appears in no stored
//! log line, task-run row, or execution row.

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use egret_workflow::logs::{spawn_writer, LogLimits, LogRepo, WriterConfig};
use egret_workflow::runners::secrets::{SecretError, SecretSource};
use egret_workflow::worker::handlers::log::Log;
use egret_workflow::worker::{start_execution, tick, Handlers};
use uuid::Uuid;

const SENTINEL: &str = "hunter2-SENTINEL";

struct OneSecret;

#[async_trait::async_trait]
impl SecretSource for OneSecret {
    async fn get(
        &self,
        _workspace: Uuid,
        name: &str,
    ) -> Result<BTreeMap<String, String>, SecretError> {
        if name == "DB_PASSWORD" {
            Ok(BTreeMap::from([(
                "value".to_string(),
                SENTINEL.to_string(),
            )]))
        } else {
            Err(SecretError::NotFound { name: name.into() })
        }
    }
}

const FLOW: &str = "id: f\nnamespace: a.b\ntasks:\n  - id: l\n    \
     type: io.kestra.plugin.core.log.Log\n    \
     message: \"pw={{ secret('DB_PASSWORD') }}\"\n";

#[tokio::test]
async fn a_log_message_resolves_secret_and_no_stored_row_holds_the_value() {
    let Some(db) = common::db().await else { return };
    let pool = db.pool.clone();
    let (ws, _user) = common::seed_workspace(&db).await;
    let ex = start_execution(&pool, ws, None, FLOW, serde_json::json!({}))
        .await
        .expect("start");

    let cancel = tokio_util::sync::CancellationToken::new();
    let (sink, writer) = spawn_writer(
        LogRepo::new(pool.clone()),
        WriterConfig::default(),
        cancel.clone(),
    );
    let mut reg =
        Handlers::with_core_tasks(None).with_log_sink(Arc::new(sink), LogLimits::default());
    reg.add_publishable(Arc::new(Log::with_secrets(Arc::new(OneSecret))));
    for _ in 0..64 {
        if tick(&pool, 16, &reg).await.expect("tick") == 0 {
            break;
        }
    }
    drop(reg);
    cancel.cancel();
    writer.await.unwrap();

    let state: String = sqlx::query_scalar(
        "SELECT state FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'l'",
    )
    .bind(ex)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, "SUCCESS", "the Log task runs");

    let lines: Vec<String> = sqlx::query_scalar(
        "SELECT message FROM workflow.task_logs WHERE execution_id = $1 ORDER BY id",
    )
    .bind(ex)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(
        lines.iter().any(|l| l == "pw=******"),
        "the masked form is stored: {lines:?}"
    );
    let leaked: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM workflow.task_logs t \
                  WHERE t.execution_id = $1 AND t::text LIKE '%' || $2 || '%') \
              + (SELECT count(*) FROM workflow.task_runs t \
                  WHERE t.execution_id = $1 AND t::text LIKE '%' || $2 || '%') \
              + (SELECT count(*) FROM workflow.executions t \
                  WHERE t.id = $1 AND (t.history::text || coalesce(t.outputs::text, '')) \
                        LIKE '%' || $2 || '%')",
    )
    .bind(ex)
    .bind(SENTINEL)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(leaked, 0, "no stored log line, run row or history holds it");
}
