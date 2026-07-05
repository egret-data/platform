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

#[tokio::test]
async fn create_list_delete_engine_catalogs_cascade() {
    let Some(store) = common::store().await else {
        eprintln!("DATABASE_URL unset — skipping");
        return;
    };
    let ws = common::seed_workspace(&store).await;

    let sc = store
        .create_storage_conn(&StorageConnInput {
            name: format!("cat-store-{}", Uuid::new_v4().simple()),
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

    let wh_name = format!("cat-wh-{}", Uuid::new_v4().simple());
    let wh = store.create_warehouse(&wh_name, ws, sc.id).await.unwrap();

    let eng_name = format!("cat-eng-{}", Uuid::new_v4().simple());
    let spec = serde_json::json!({ "coordinatorReplicas": 1, "workerReplicas": 2 });
    let engine = store
        .create_engine(&eng_name, "trino", "egret-engines", wh.id, &spec)
        .await
        .unwrap();

    // Insert the extra (non-primary) catalog first, then the primary, to prove the
    // ordering comes from `is_primary DESC` and not insertion order.
    let extra = store
        .create_engine_catalog(
            engine.id,
            "tpch",
            "tpch",
            &serde_json::json!({}),
            None,
            false,
        )
        .await
        .unwrap();
    let primary = store
        .create_engine_catalog(
            engine.id,
            "egret",
            "egret-warehouse",
            &serde_json::json!({ "warehouse": wh_name }),
            None,
            true,
        )
        .await
        .unwrap();

    // list → primary first
    let list = store.list_engine_catalogs(engine.id).await.unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].id, primary.id);
    assert!(list[0].is_primary);
    assert_eq!(list[1].id, extra.id);

    // get by id
    let got = store.get_engine_catalog(extra.id).await.unwrap().unwrap();
    assert_eq!(got.name, "tpch");
    assert_eq!(got.kind, "tpch");

    // delete the extra → only primary left
    store.delete_engine_catalog(extra.id).await.unwrap();
    let list = store.list_engine_catalogs(engine.id).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, primary.id);

    // deleting the engine cascades to its catalogs
    store.delete_engine(engine.id).await.unwrap();
    assert!(store
        .list_engine_catalogs(engine.id)
        .await
        .unwrap()
        .is_empty());
    assert!(store
        .get_engine_catalog(primary.id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn update_engine_catalog_reflects_in_get() {
    let Some(store) = common::store().await else {
        eprintln!("DATABASE_URL unset — skipping");
        return;
    };
    let ws = common::seed_workspace(&store).await;

    let sc = store
        .create_storage_conn(&StorageConnInput {
            name: format!("upd-store-{}", Uuid::new_v4().simple()),
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

    let wh_name = format!("upd-wh-{}", Uuid::new_v4().simple());
    let wh = store.create_warehouse(&wh_name, ws, sc.id).await.unwrap();

    let eng_name = format!("upd-eng-{}", Uuid::new_v4().simple());
    let spec = serde_json::json!({ "coordinatorReplicas": 1, "workerReplicas": 2 });
    let engine = store
        .create_engine(&eng_name, "trino", "egret-engines", wh.id, &spec)
        .await
        .unwrap();

    let catalog = store
        .create_engine_catalog(
            engine.id,
            "tpch",
            "tpch",
            &serde_json::json!({}),
            None,
            false,
        )
        .await
        .unwrap();
    assert_eq!(catalog.config, serde_json::json!({}));
    assert_eq!(catalog.secret_ref, None);

    let new_config = serde_json::json!({ "connector.name": "tpch" });
    let new_secret_ref = Uuid::new_v4();
    let updated = store
        .update_engine_catalog(catalog.id, &new_config, Some(new_secret_ref))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.config, new_config);
    assert_eq!(updated.secret_ref, Some(new_secret_ref));

    let got = store.get_engine_catalog(catalog.id).await.unwrap().unwrap();
    assert_eq!(got.config, new_config);
    assert_eq!(got.secret_ref, Some(new_secret_ref));

    // updating a non-existent id returns None rather than erroring
    assert!(store
        .update_engine_catalog(Uuid::new_v4(), &new_config, None)
        .await
        .unwrap()
        .is_none());
}
