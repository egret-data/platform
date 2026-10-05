//! W9b Task 4: a database outage during a namespace lookup at render is
//! egret's, not the author's (W9b spec §11). Requires DATABASE_URL.
//!
//! Its own test binary because the namespace-file connect options are
//! process-wide (`nsfiles::install_db`, set once): this binary installs
//! options that reach a closed port, while the worker's own pool -- claims,
//! outcomes, the execution -- uses the real database. So every namespace
//! lookup here fails to connect, and nothing else does.

mod common;

use egret_workflow::storage;
use egret_workflow::storage::nsfiles;
use egret_workflow::worker::{start_execution, tick, Handlers};
use serde_json::json;
use uuid::Uuid;

async fn attempts(pool: &sqlx::PgPool, execution: Uuid, task: &str) -> i32 {
    sqlx::query_scalar(
        "SELECT jsonb_array_length(attempts) FROM workflow.task_runs \
          WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(execution)
    .bind(task)
    .fetch_one(pool)
    .await
    .expect("attempts")
}

async fn state_of(pool: &sqlx::PgPool, execution: Uuid, task: &str) -> Option<String> {
    sqlx::query_scalar(
        "SELECT state FROM workflow.task_runs WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(execution)
    .bind(task)
    .fetch_optional(pool)
    .await
    .unwrap()
}

/// A port nothing listens on: bound, then released.
fn closed_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

/// The handler path: `r`'s `fileExists` cannot reach the database. Not
/// `false` (which a missing file answers) and not a failed task: the tick is
/// an `Err`, the run keeps its lease and records nothing, no attempt is
/// spent. Then the settle path: `b`'s `runIf` renders inside the transaction
/// that finishes `a`, and the outage rolls that transaction back whole.
#[tokio::test(flavor = "multi_thread")]
async fn a_database_outage_during_a_namespace_read_re_claims_with_no_attempt_spent() {
    let Some(db) = common::db().await else {
        return;
    };
    let unreachable = (*db.pool.connect_options())
        .clone()
        .host("127.0.0.1")
        .port(closed_port());
    nsfiles::install_db(unreachable).expect("the first install in this binary");
    storage::install_mem_for_tests();
    let (ws, _) = common::seed_workspace(&db).await;
    let reg = Handlers::with_core_tasks(storage::current());

    // The handler.
    let source = "id: outage\nnamespace: w9b.outage\ntasks:\n\
         \x20 - id: r\n    type: io.kestra.plugin.core.debug.Return\n    \
         format: \"{{ fileExists('cfg/x.txt') }}\"\n";
    let ex = start_execution(&db.pool, ws, None, source, json!({}))
        .await
        .unwrap();
    let before = attempts(&db.pool, ex, "r").await;
    let err = tick(&db.pool, 16, &reg)
        .await
        .expect_err("an outage is the worker's error, not the task's");
    assert!(
        err.to_string().contains("'/cfg/x.txt'") && err.to_string().contains("database"),
        "{err}"
    );
    let (state, outputs, message): (String, Option<serde_json::Value>, Option<String>) =
        sqlx::query_as(
            "SELECT state, outputs, message FROM workflow.task_runs \
              WHERE execution_id = $1 AND task_id = 'r'",
        )
        .bind(ex)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!((state.as_str(), outputs, message), ("RUNNING", None, None));
    assert_eq!(
        attempts(&db.pool, ex, "r").await,
        before,
        "no attempt spent"
    );

    // The settle.
    let source = "id: outage_settle\nnamespace: w9b.outage\ntasks:\n\
         \x20 - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: a\n\
         \x20 - id: b\n    type: io.kestra.plugin.core.log.Log\n    \
         runIf: \"{{ fileExists('cfg/x.txt') }}\"\n    message: b\n";
    let ex = start_execution(&db.pool, ws, None, source, json!({}))
        .await
        .unwrap();
    let before = attempts(&db.pool, ex, "a").await;
    let err = tick(&db.pool, 16, &reg)
        .await
        .expect_err("an outage in settle is the worker's error");
    assert!(err.to_string().contains("'/cfg/x.txt'"), "{err}");
    assert_eq!(
        state_of(&db.pool, ex, "a").await.as_deref(),
        Some("RUNNING"),
        "a's outcome rolled back"
    );
    assert_eq!(
        state_of(&db.pool, ex, "b").await,
        None,
        "b was never created"
    );
    assert_eq!(
        attempts(&db.pool, ex, "a").await,
        before,
        "no attempt spent"
    );
}
