//! Embedded-catalog RBAC store: warehouse → role → Catalog Client create + verify.
mod common;

use egret_catalog::service::embedded::NewClient;
use egret_catalog::service::StorageConnInput;
use uuid::Uuid;

#[tokio::test]
async fn create_client_then_verify() {
    let Some(store) = common::store().await else {
        eprintln!("DATABASE_URL unset — skipping");
        return;
    };
    let ws = common::seed_workspace(&store).await;

    let sc = store
        .create_storage_conn(&StorageConnInput {
            name: format!("eb-store-{}", Uuid::new_v4().simple()),
            purpose: None,
            provider: "s3".into(),
            endpoint: Some("http://localhost:9000".into()),
            region: "us-east-1".into(),
            bucket: "warehouse".into(),
            key_prefix: None,
            access_key: "egret".into(),
            secret_ref: None,
            path_style: true,
        })
        .await
        .unwrap();

    let wh_name = format!("eb-wh-{}", Uuid::new_v4().simple());
    let wh = store.create_warehouse(&wh_name, ws, sc.id).await.unwrap();
    let role = store
        .create_role(wh.id, "engine", "engine", None)
        .await
        .unwrap();

    let created = store
        .create_client(&NewClient {
            name: "trino-eng".into(),
            consumer_type: "engine".into(),
            workspace_id: ws,
            warehouse_id: wh.id,
            catalog_role_id: role.id,
        })
        .await
        .unwrap();
    assert!(created.client_id.starts_with("egret_"));
    assert!(!created.client_secret.is_empty());

    // Correct secret verifies + returns (warehouse name, role name).
    let vc = store
        .verify_client(&created.client_id, &created.client_secret)
        .await
        .unwrap();
    assert_eq!(vc, Some((wh_name.clone(), "engine".to_string())));

    // Wrong secret → None.
    let bad = store
        .verify_client(&created.client_id, "WRONG")
        .await
        .unwrap();
    assert_eq!(bad, None);
}

#[tokio::test]
async fn create_list_get_delete_engine() {
    let Some(store) = common::store().await else {
        eprintln!("DATABASE_URL unset — skipping");
        return;
    };
    let ws = common::seed_workspace(&store).await;

    let sc = store
        .create_storage_conn(&StorageConnInput {
            name: format!("eng-store-{}", Uuid::new_v4().simple()),
            purpose: None,
            provider: "s3".into(),
            endpoint: Some("http://localhost:9000".into()),
            region: "us-east-1".into(),
            bucket: "warehouse".into(),
            key_prefix: None,
            access_key: "egret".into(),
            secret_ref: None,
            path_style: true,
        })
        .await
        .unwrap();

    let wh_name = format!("eng-wh-{}", Uuid::new_v4().simple());
    let wh = store.create_warehouse(&wh_name, ws, sc.id).await.unwrap();

    let eng_name = format!("eng-{}", Uuid::new_v4().simple());
    let spec = serde_json::json!({ "coordinatorReplicas": 1, "workerReplicas": 2 });
    let created = store
        .create_engine(&eng_name, "trino", "egret-engines", wh.id, &spec)
        .await
        .unwrap();
    assert_eq!(created.name, eng_name);
    assert_eq!(created.r#type, "trino");
    assert_eq!(created.warehouse_id, wh.id);
    assert_eq!(created.status, "provisioning");

    // list includes it
    let list = store.list_engines().await.unwrap();
    assert!(list.iter().any(|e| e.id == created.id));

    // get by id
    let got = store.get_engine(created.id).await.unwrap().unwrap();
    assert_eq!(got.id, created.id);
    assert_eq!(got.spec, spec);

    // delete → gone
    store.delete_engine(created.id).await.unwrap();
    assert!(store.get_engine(created.id).await.unwrap().is_none());
}
