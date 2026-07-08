//! DB-gated AuthzStore service tests. Self-skip without DATABASE_URL.
use egret_metastore::Db;
use egret_policy::AuthzStore;
use serde_json::json;

async fn store() -> Option<AuthzStore> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(AuthzStore::new(db))
}
fn uniq(p: &str) -> String {
    format!("{p}_{}", uuid::Uuid::new_v4().simple())
}

#[tokio::test]
async fn seed_def_then_create_service() {
    let Some(s) = store().await else {
        eprintln!("skip");
        return;
    };
    let defname = uniq("trino");
    s.upsert_service_def(900900, &defname, &json!({"name": defname, "resources": []}))
        .await
        .unwrap();
    s.upsert_service_def(900900, &defname, &json!({"name": defname, "resources": []}))
        .await
        .unwrap(); // idempotent
    assert_eq!(s.get_service_def(&defname).await.unwrap().name, defname);

    let svcname = uniq("trino_prod");
    let svc = s
        .create_service(&svcname, &defname, &json!({}))
        .await
        .unwrap();
    assert_eq!(svc.policy_version, 1);
    assert!(matches!(
        s.create_service(&svcname, &defname, &json!({}))
            .await
            .unwrap_err(),
        egret_policy::AuthzError::AlreadyExists(_)
    ));
    s.delete_service(svc.id).await.unwrap();
    assert!(s.get_service(&svcname).await.is_err());
}
