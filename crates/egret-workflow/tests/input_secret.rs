//! Package 10a Task 3: `secret()` in a flow input's `defaults:`, through the
//! REAL worker. Requires DATABASE_URL.
//!
//! The default renders at START, with the value read through the attempt's
//! prefetch, and is stored SEALED like a `type: SECRET` input (decision D3:
//! Kestra keeps the plaintext of a non-SECRET input, egret does not).

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use egret_workflow::logs::{spawn_writer, LogLimits, LogRepo, WriterConfig};
use egret_workflow::runners::secrets::{self, SecretError, SecretSource};
use egret_workflow::worker::{start_execution, tick, Handlers};
use uuid::Uuid;

const SENTINEL: &str = "tok-SENTINEL-9a";

/// The tests tick the shared queue with different handlers, so one runs at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Token;

#[async_trait::async_trait]
impl SecretSource for Token {
    async fn get(
        &self,
        _workspace: Uuid,
        name: &str,
    ) -> Result<BTreeMap<String, String>, SecretError> {
        if name == "TOKEN" {
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
inputs:
  - id: x
    type: STRING
    defaults: "{{ secret('TOKEN') }}"
tasks:
  - id: real
    type: io.kestra.plugin.core.log.Log
    message: "got {{ inputs.x }}"
    runIf: "{{ inputs.x == 'tok-SENTINEL-9a' }}"
"#;

#[tokio::test]
async fn a_secret_default_is_resolved_at_start_sealed_and_never_stored_in_clear() {
    let _serial = SERIAL.lock().await;
    let Some(db) = common::db().await else { return };
    let _ = secrets::install(Arc::new(Token));
    egret_workflow::secret_inputs::install_for_tests();
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
    let reg = Handlers::with_core_tasks(None).with_log_sink(Arc::new(sink), LogLimits::default());
    for _ in 0..64 {
        if tick(&pool, 16, &reg).await.expect("tick") == 0 {
            break;
        }
    }
    drop(reg);
    cancel.cancel();
    writer.await.unwrap();

    // (a) sealed: the stored inputs read the mask.
    let inputs: serde_json::Value =
        sqlx::query_scalar("SELECT inputs FROM workflow.executions WHERE id = $1")
            .bind(ex)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(inputs["x"], egret_workflow::secret_inputs::MASK);
    let sealed: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT secret_inputs FROM workflow.executions WHERE id = $1")
            .bind(ex)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(sealed.is_some(), "the value is sealed");

    // (b) the downstream render sees the REAL value (the runIf compares it).
    let state: String = sqlx::query_scalar(
        "SELECT state FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'real'",
    )
    .bind(ex)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, "SUCCESS", "{{ inputs.x }} rendered the real value");

    // (c) no stored row holds it (the whole execution row but its source).
    let leaked: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM workflow.task_logs t \
                  WHERE t.execution_id = $1 AND t::text LIKE '%' || $2 || '%') \
              + (SELECT count(*) FROM workflow.task_runs t \
                  WHERE t.execution_id = $1 AND t::text LIKE '%' || $2 || '%') \
              + (SELECT count(*) FROM workflow.executions e \
                  WHERE e.id = $1 AND (to_jsonb(e) - 'source')::text LIKE '%' || $2 || '%')",
    )
    .bind(ex)
    .bind(SENTINEL)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(leaked, 0, "no stored row holds the value");
    let logs: Vec<String> = sqlx::query_scalar(
        "SELECT message FROM workflow.task_logs WHERE execution_id = $1 ORDER BY id",
    )
    .bind(ex)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(
        logs.iter()
            .any(|m| m.contains("got ") && m.contains("******")),
        "the Log ran, masked: {logs:?}"
    );
}

async fn settle_all(pool: &sqlx::PgPool) {
    let reg = Handlers::with_core_tasks(None);
    for _ in 0..64 {
        if tick(pool, 16, &reg).await.expect("tick") == 0 {
            break;
        }
    }
}

async fn leaks(pool: &sqlx::PgPool, ex: Uuid, needle: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM workflow.task_logs t \
                  WHERE t.execution_id = $1 AND t::text LIKE '%' || $2 || '%') \
              + (SELECT count(*) FROM workflow.task_runs t \
                  WHERE t.execution_id = $1 AND t::text LIKE '%' || $2 || '%') \
              + (SELECT count(*) FROM workflow.executions e \
                  WHERE e.id = $1 AND (to_jsonb(e) - 'source')::text LIKE '%' || $2 || '%')",
    )
    .bind(ex)
    .bind(needle)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// The property that justifies sealing from the DECLARATION: a replay that
/// reuses the stored inputs (revealed in clear, then split again) seals the
/// value again instead of writing it into the new row.
#[tokio::test]
async fn a_replay_with_reused_inputs_seals_the_secret_default_again() {
    let _serial = SERIAL.lock().await;
    let Some(db) = common::db().await else { return };
    let _ = secrets::install(Arc::new(Token));
    egret_workflow::secret_inputs::install_for_tests();
    let pool = db.pool.clone();
    let (ws, user) = common::seed_workspace(&db).await;
    let original = start_execution(&pool, ws, None, FLOW, serde_json::json!({}))
        .await
        .expect("start");
    settle_all(&pool).await;

    let replay = egret_workflow::worker::replay_execution(
        &pool,
        &egret_workflow::worker::ReplayRequest {
            workspace_id: ws,
            from: original,
            source: FLOW,
            kind: egret_workflow::worker::ReplayKind::Whole,
            inputs: egret_workflow::worker::ReplayInputs::Reuse,
            started_by: user.0,
            id: Uuid::new_v4(),
            input_files: Vec::new(),
        },
    )
    .await
    .expect("replay")
    .expect("not refused");
    settle_all(&pool).await;
    assert_ne!(replay, original);

    let inputs: serde_json::Value =
        sqlx::query_scalar("SELECT inputs FROM workflow.executions WHERE id = $1")
            .bind(replay)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(inputs["x"], egret_workflow::secret_inputs::MASK);
    let state: String = sqlx::query_scalar(
        "SELECT state FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'real'",
    )
    .bind(replay)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, "SUCCESS", "the replay still renders the real value");
    assert_eq!(
        leaks(&pool, replay, SENTINEL).await,
        0,
        "replayed row clean"
    );
}

/// A value POSTED for an input whose default calls `secret()` is sealed too
/// (the set is the declaration's): over-sealing, never under-sealing.
#[tokio::test]
async fn a_posted_value_for_such_an_input_is_sealed_too() {
    let _serial = SERIAL.lock().await;
    let Some(db) = common::db().await else { return };
    let _ = secrets::install(Arc::new(Token));
    egret_workflow::secret_inputs::install_for_tests();
    let pool = db.pool.clone();
    let (ws, _user) = common::seed_workspace(&db).await;
    let posted = "posted-SENTINEL-4c";
    let ex = start_execution(&pool, ws, None, FLOW, serde_json::json!({"x": posted}))
        .await
        .expect("start");
    settle_all(&pool).await;
    let inputs: serde_json::Value =
        sqlx::query_scalar("SELECT inputs FROM workflow.executions WHERE id = $1")
            .bind(ex)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(inputs["x"], egret_workflow::secret_inputs::MASK);
    assert_eq!(leaks(&pool, ex, posted).await, 0);
}
