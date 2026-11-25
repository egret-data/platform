//! Package 10a Task 2: `secret()` in a task's `runIf:`, through the REAL
//! worker. Requires DATABASE_URL.
//!
//! `runIf` renders synchronously inside a settle's transaction, so the value
//! is read through the per-attempt prefetch (`storage::prefetch`): the first
//! attempt records the read and is rolled back, the owner fetches it with no
//! transaction open, and the next attempt renders with it. The value gates
//! the task and is stored nowhere (not in `rendered_run_if`, a log line, a
//! run row or the history).

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use egret_workflow::runners::secrets::{self, SecretError, SecretSource};
use egret_workflow::worker::{start_execution, tick, Handlers};
use uuid::Uuid;

const SENTINEL: &str = "on-SENTINEL-7f";

struct Flag;

#[async_trait::async_trait]
impl SecretSource for Flag {
    async fn get(
        &self,
        _workspace: Uuid,
        name: &str,
    ) -> Result<BTreeMap<String, String>, SecretError> {
        if name == "FLAG" {
            Ok(BTreeMap::from([(
                "value".to_string(),
                SENTINEL.to_string(),
            )]))
        } else {
            Err(SecretError::NotFound { name: name.into() })
        }
    }
}

const FLOW: &str = r#"id: f
namespace: a.b
tasks:
  - id: "on"
    type: io.kestra.plugin.core.log.Log
    message: hi
    runIf: "{{ secret('FLAG') == 'on-SENTINEL-7f' }}"
  - id: off
    type: io.kestra.plugin.core.log.Log
    message: hi
    runIf: "{{ secret('FLAG') == 'something-else' }}"
  - id: bare
    type: io.kestra.plugin.core.log.Log
    message: hi
    runIf: "{{ secret('FLAG') }}"
  - id: bad
    type: io.kestra.plugin.core.log.Log
    message: hi
    runIf: "{{ secret('FLAG') | number }}"
"#;

#[tokio::test]
async fn a_run_if_resolves_secret_and_no_stored_row_holds_the_value() {
    let Some(db) = common::db().await else { return };
    let _ = secrets::install(Arc::new(Flag));
    let pool = db.pool.clone();
    let (ws, _user) = common::seed_workspace(&db).await;
    let ex = start_execution(&pool, ws, None, FLOW, serde_json::json!({}))
        .await
        .expect("start");

    let reg = Handlers::with_core_tasks(None);
    for _ in 0..64 {
        if tick(&pool, 16, &reg).await.expect("tick") == 0 {
            break;
        }
    }

    let state_of = |task: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT state FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2",
            )
            .bind(ex)
            .bind(task)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(state_of("on").await, "SUCCESS", "a matching compare runs");
    assert_eq!(state_of("off").await, "SKIPPED", "a mismatch is skipped");
    assert_eq!(state_of("bare").await, "SUCCESS", "a bare value is truthy");
    // A runIf that errors on the value fails its run; the error carries no value.
    let bad = state_of("bad").await;
    assert_eq!(bad, "FAILED", "a runIf that errors fails its run");
    // The miss -> rollback -> fetch -> re-run cycle ran (each owner call
    // has its own cache, so the count is per call, not per execution).
    assert!(
        egret_workflow::storage::prefetch::fetch_rounds_so_far() >= 1,
        "the secret was fetched by a prefetch round"
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
    // (`executions.source` holds the author's literal, so it is not read.)
    assert_eq!(leaked, 0, "no stored row holds the value");
}
