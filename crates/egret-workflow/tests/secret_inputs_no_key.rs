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
