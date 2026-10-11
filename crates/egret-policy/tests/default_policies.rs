//! DB-gated tests for `seed_default_trino_policies`. Self-skips without `DATABASE_URL` ONLY
//! under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS — do NOT point this at the live demo
//! Postgres on :5432.
use egret_metastore::Db;
use egret_policy::PolicyStore;
use serde_json::json;

async fn store() -> Option<PolicyStore> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(PolicyStore::new(db))
}
fn uniq(p: &str) -> String {
    format!("{p}_{}", uuid::Uuid::new_v4().simple())
}

#[tokio::test]
async fn seed_default_trino_policies_is_idempotent_and_faithful() {
    let Some(s) = store().await else {
        eprintln!("skip");
        return;
    };
    // The real `trino` def: seed-defaults refuses any other type (SP4d-3
    // final review I-1), and a fresh service per run keeps it isolated.
    egret_policy::seed_trino(&s).await.unwrap();
    let svc = s
        .create_service(&uniq("svc"), "trino", &json!({}))
        .await
        .unwrap();

    let created = s
        .seed_default_trino_policies(svc.id, &svc.name, "admin")
        .await
        .unwrap();
    assert!(created > 0, "should create at least one default policy");
    assert_eq!(
        created, 13,
        "expected one default policy per Trino resource hierarchy"
    );

    // Idempotent: calling again on a service that already has policies is a no-op.
    let again = s
        .seed_default_trino_policies(svc.id, &svc.name, "admin")
        .await
        .unwrap();
    assert_eq!(
        again, 0,
        "re-seeding an already-seeded service must be a no-op"
    );

    let rows = s.list_policies(svc.id).await.unwrap();
    assert_eq!(rows.len(), created as usize);

    let trinouser_policy = rows
        .iter()
        .find(|r| r.name == "all - trinouser")
        .expect("expected a default policy named 'all - trinouser'");
    let accesses = trinouser_policy.policy_jsonb["policyItems"][0]["accesses"]
        .as_array()
        .unwrap();
    assert!(
        accesses
            .iter()
            .any(|a| a["type"] == "impersonate" && a["isAllowed"] == true),
        "trinouser default policy should grant impersonate"
    );
    assert_eq!(
        trinouser_policy.policy_jsonb["policyItems"][0]["groups"][0],
        "admin"
    );

    s.delete_service(svc.id).await.unwrap();
}
