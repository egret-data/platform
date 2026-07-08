//! DB-gated tests for the `PolicyPort` (contract crate) impl over `AuthzStore`.
//! Exercised through `&dyn PolicyPort` so it is the trait method under test, not
//! the same-named inherent method it shadows. Self-skip without DATABASE_URL.
use egret_authz::AuthzStore;
use egret_metastore::Db;
use egret_policy_api::{PolicyError, PolicyPort};
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
async fn ensure_trino_def_is_idempotent_and_create_service_returns_policyservice() {
    let Some(s) = store().await else {
        eprintln!("skip: no DATABASE_URL");
        return;
    };
    let port: &dyn PolicyPort = &s;

    // ensure_trino_def seeds the built-in def and is idempotent.
    port.ensure_trino_def().await.unwrap();
    port.ensure_trino_def().await.unwrap();

    // create_service returns the boundary DTO with the right projection.
    let name = uniq("trino_prod");
    let svc = port
        .create_service(&name, "trino", &json!({}))
        .await
        .unwrap();
    assert_eq!(svc.name, name);
    assert_eq!(svc.r#type, "trino");
    assert!(svc.id > 0);

    // list_services surfaces it as a PolicyService (keyed by id).
    let listed = port.list_services().await.unwrap();
    assert!(listed.iter().any(|p| p.id == svc.id && p.name == name));

    // Duplicate name maps AuthzError::AlreadyExists -> PolicyError::AlreadyExists.
    let err = port
        .create_service(&name, "trino", &json!({}))
        .await
        .unwrap_err();
    assert!(matches!(err, PolicyError::AlreadyExists(_)), "got {err:?}");

    // cleanup (inherent method — fine, not part of the port).
    s.delete_service(svc.id).await.unwrap();
}
