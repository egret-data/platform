//! KI-112: a deployment without `secrets.master_key` installs no cipher, and
//! a start that would store a SECRET value is refused rather than stored in
//! clear. Its own test binary: the cipher is process-wide, and
//! `tests/secret_inputs.rs` installs one.

mod common;

use egret_metastore::Db;
use serde_json::json;

#[sqlx::test(migrations = "../../migrations")]
async fn a_secret_value_without_a_master_key_is_refused_and_nothing_is_stored(pool: sqlx::PgPool) {
    // Arrange
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:manage", "workflow:read"]).await;
    let source = "id: nokey\nnamespace: ki112\ninputs:\n  - id: pw\n    type: SECRET\n\
                  tasks:\n  - id: say\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n";
    let flow = common::seed_flow(&db, ws, user, source, "draft").await;
    let app = common::app(&db).await;

    // Act
    let res = common::post(
        &app,
        &format!("/api/ws/{ws}/workflows/flows/{flow}/executions"),
        user,
        json!({ "inputs": { "pw": "hunter2-ki112" } }),
    )
    .await;

    // Assert
    assert_eq!(res.status(), 400);
    let text = common::body_text(res).await;
    assert!(text.contains("secrets.master_key"), "{text}");
    assert!(!text.contains("hunter2-ki112"), "{text}");
    assert_eq!(common::execution_count(&db, ws).await, 0);
}

/// Package 7c (S7, spec Q6): a SECRET `onResume` value is refused as a SECRET
/// start input is, naming `secrets.master_key`, and nothing is written.
#[sqlx::test(migrations = "../../migrations")]
async fn a_secret_on_resume_value_without_a_master_key_is_refused(pool: sqlx::PgPool) {
    // Arrange
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    let source = "id: nokeyresume\nnamespace: t\ntasks:\n  - id: p\n    \
                  type: io.kestra.plugin.core.flow.Pause\n    \
                  onResume:\n      - id: tok\n        type: SECRET\n  \
                  - id: after\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n";
    let flow = common::seed_flow(&db, ws, user, source, "draft").await;
    let ex = egret_workflow::worker::start_execution(&pool, ws, Some(flow), source, json!({}))
        .await
        .expect("start");
    let reg = egret_workflow::worker::Handlers::with_core_tasks(egret_workflow::storage::current());
    for _ in 0..16 {
        if egret_workflow::worker::tick(&pool, 16, &reg)
            .await
            .expect("tick")
            == 0
        {
            break;
        }
    }
    let state = |pool: sqlx::PgPool| async move {
        sqlx::query_scalar::<_, String>("SELECT state FROM workflow.executions WHERE id = $1")
            .bind(ex)
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    assert_eq!(state(pool.clone()).await, "PAUSED");
    let inputs = match json!({ "tok": "hunter2-7c" }) {
        serde_json::Value::Object(m) => m,
        _ => unreachable!(),
    };

    // The waiting Pause run, which the route passes with any inputs (a
    // resume with inputs and no expected run is `Moved`, T5 review C1).
    let run: uuid::Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'p'",
    )
    .bind(ex)
    .fetch_one(&pool)
    .await
    .unwrap();

    // Act
    let resumed = egret_workflow::worker::resume_with(
        &pool,
        &egret_workflow::worker::ResumeRequest {
            id: ex,
            inputs: Some(&inputs),
            files: &[],
            expected: Some(run),
        },
    )
    .await
    .expect("resume answers");

    // Assert
    match &resumed {
        egret_workflow::worker::Resumed::Refused(
            egret_workflow::worker::ResumeRefused::Invalid(m),
        ) => {
            assert!(m.contains("secrets.master_key"), "{m}");
            assert!(m.contains("tok"), "{m}");
            assert!(!m.contains("hunter2-7c"), "{m}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(state(pool.clone()).await, "PAUSED");
    let outputs: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT outputs FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'p'",
    )
    .bind(ex)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(outputs, None);
}
