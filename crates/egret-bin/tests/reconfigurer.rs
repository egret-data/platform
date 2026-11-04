//! Governance SP2b Task 15: the reconfigurer `main` wires into the policy
//! admin, before the engine seam's state is in its cell.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.

use std::sync::Arc;

use egret_bin::reconfigurer::{ReconfigurerImpl, NOT_WIRED};
use egret_metastore::Db;
use egret_policy::ProfileEngineReconfigurer;

#[tokio::test]
async fn an_empty_cell_reports_every_bound_engine_failed() {
    // Arrange
    let Some(dsn) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let policy = egret_policy::PolicyStore::new(db.clone());
    egret_policy::seed_trino(&policy).await.expect("trino def");
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let profile = policy
        .create_service(&format!("reconf_{suffix}"), "trino", &serde_json::json!({}))
        .await
        .expect("profile");
    let names = [
        format!("b-reconf-{}", &suffix[..10]),
        format!("a-reconf-{}", &suffix[..10]),
        format!("c-reconf-{}", &suffix[..10]),
    ];
    for (name, status) in names.iter().zip(["running", "running", "deleting"]) {
        sqlx::query(
            "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, status, authz_service_id) \
             VALUES ($1, 'trino', 'egret', '{\"imageVersion\":\"483\"}'::jsonb, $2, $3)",
        )
        .bind(name)
        .bind(status)
        .bind(profile.id)
        .execute(&db.pool)
        .await
        .expect("engine");
    }
    let r = ReconfigurerImpl {
        cp: Arc::default(),
        store: egret_engine_mgmt::EngineMgmtStore::new(db.clone()),
    };

    // Act
    let report = r.reconfigure_bound_engines(profile.id).await;

    // Assert: by name, never the one being deleted.
    assert!(report.applied.is_empty(), "{report:?}");
    assert_eq!(
        report.failed,
        vec![
            (names[1].clone(), NOT_WIRED.to_string()),
            (names[0].clone(), NOT_WIRED.to_string()),
        ]
    );
}
