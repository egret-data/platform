//! DB-gated table-store tests. Self-skip without DATABASE_URL.
use egret_catalog::service::CatalogStore;
use egret_catalog::CatalogError;
use egret_metastore::Db;
use std::collections::HashMap;

async fn store() -> Option<CatalogStore> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(CatalogStore::new(db))
}

fn minimal_metadata(location: &str, id: uuid::Uuid) -> iceberg::spec::TableMetadata {
    egret_catalog::service::test_support::minimal_table_metadata(location, id)
}

#[tokio::test]
async fn insert_load_drop_table() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let levels = vec![format!("t_{}", uuid::Uuid::new_v4().simple())];
    let ns = s.create_namespace(&levels, &HashMap::new()).await.unwrap();
    let tid = uuid::Uuid::new_v4();
    let loc = format!("s3://wh/{}/orders-{tid}", levels[0]);
    let md = minimal_metadata(&loc, tid);
    let mloc = format!(
        "{loc}/metadata/00000-{}.metadata.json",
        uuid::Uuid::new_v4()
    );
    s.insert_table(ns.id, "orders", tid, &md, &mloc, &loc)
        .await
        .unwrap();
    let row = s.load_table(ns.id, "orders").await.unwrap();
    assert_eq!(row.current_version, 0);
    assert_eq!(row.name, "orders");
    assert_eq!(row.table_uuid, tid);
    assert_eq!(
        s.list_tables(ns.id).await.unwrap(),
        vec!["orders".to_string()]
    );
    s.drop_table(ns.id, "orders").await.unwrap();
    assert!(s.load_table(ns.id, "orders").await.is_err());
    s.drop_namespace(&levels).await.unwrap();
}

#[tokio::test]
async fn list_tables_multiple() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let levels = vec![format!("t_{}", uuid::Uuid::new_v4().simple())];
    let ns = s.create_namespace(&levels, &HashMap::new()).await.unwrap();

    for name in ["alpha", "beta", "gamma"] {
        let tid = uuid::Uuid::new_v4();
        let loc = format!("s3://wh/{}/{}", levels[0], name);
        let md = minimal_metadata(&loc, tid);
        let mloc = format!("{loc}/metadata/00000-x.metadata.json");
        s.insert_table(ns.id, name, tid, &md, &mloc, &loc)
            .await
            .unwrap();
    }

    let names = s.list_tables(ns.id).await.unwrap();
    assert_eq!(names, vec!["alpha", "beta", "gamma"]);

    // Clean up
    for name in ["alpha", "beta", "gamma"] {
        s.drop_table(ns.id, name).await.unwrap();
    }
    s.drop_namespace(&levels).await.unwrap();
}

#[tokio::test]
async fn cas_commit_rejects_stale_version() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let levels = vec![format!("t_{}", uuid::Uuid::new_v4().simple())];
    let ns = s.create_namespace(&levels, &HashMap::new()).await.unwrap();
    let tid = uuid::Uuid::new_v4();
    let loc = format!("s3://wh/{}/t-{tid}", levels[0]);
    let md = minimal_metadata(&loc, tid);
    let mloc = format!("{loc}/metadata/00000-x.metadata.json");
    s.insert_table(ns.id, "t", tid, &md, &mloc, &loc)
        .await
        .unwrap();
    let row = s.load_table(ns.id, "t").await.unwrap();
    // First commit at version 0 succeeds.
    s.cas_commit(
        row.id,
        0,
        &md,
        &format!("{loc}/metadata/00001-y.metadata.json"),
    )
    .await
    .unwrap();
    // Second commit still using stale version 0 must conflict.
    let err = s
        .cas_commit(
            row.id,
            0,
            &md,
            &format!("{loc}/metadata/00002-z.metadata.json"),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, CatalogError::Conflict(_)),
        "expected Conflict, got: {err:?}"
    );
    s.drop_table(ns.id, "t").await.unwrap();
    s.drop_namespace(&levels).await.unwrap();
}

#[tokio::test]
async fn duplicate_table_is_already_exists() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let levels = vec![format!("t_{}", uuid::Uuid::new_v4().simple())];
    let ns = s.create_namespace(&levels, &HashMap::new()).await.unwrap();
    let tid = uuid::Uuid::new_v4();
    let loc = format!("s3://wh/{}/dup-{tid}", levels[0]);
    let md = minimal_metadata(&loc, tid);
    let mloc = format!("{loc}/metadata/00000-x.metadata.json");
    s.insert_table(ns.id, "dup", tid, &md, &mloc, &loc)
        .await
        .unwrap();
    let tid2 = uuid::Uuid::new_v4();
    let err = s
        .insert_table(ns.id, "dup", tid2, &md, &mloc, &loc)
        .await
        .unwrap_err();
    assert!(
        matches!(err, CatalogError::AlreadyExists(_)),
        "expected AlreadyExists, got: {err:?}"
    );
    s.drop_table(ns.id, "dup").await.unwrap();
    s.drop_namespace(&levels).await.unwrap();
}
