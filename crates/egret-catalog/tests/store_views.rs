//! DB-gated view-store tests. Self-skip without DATABASE_URL.
mod common;
use common::{seed_catalog, store};
use egret_catalog::service::view_repo;
use std::collections::HashMap;

fn minimal_view(location: &str, id: uuid::Uuid) -> iceberg::spec::ViewMetadata {
    egret_catalog::service::test_support::minimal_view_metadata(location, id)
}

#[tokio::test]
async fn insert_load_drop_view() {
    let Some(s) = store().await else {
        eprintln!("DATABASE_URL not set — skipping store_views test");
        return;
    };
    let cat = seed_catalog(&s).await;
    let levels = vec![format!("t_{}", uuid::Uuid::new_v4().simple())];
    let ns = s
        .create_namespace(cat, &levels, &HashMap::new())
        .await
        .unwrap();

    let view_uuid = uuid::Uuid::new_v4();
    let loc = "s3://wh/ns/v";
    let mloc = "s3://wh/ns/v/metadata/0.json";
    let view_md = minimal_view(loc, view_uuid);

    s.insert_view(ns.id, "v", &view_md, mloc, loc)
        .await
        .unwrap();

    let row = s.load_view(ns.id, "v").await.unwrap();
    assert_eq!(row.name, "v");
    assert_eq!(row.metadata_location, mloc);

    // The assembled ViewMetadata must round-trip the typed fields.
    let md = &row.metadata;
    assert_eq!(md.uuid(), view_uuid);
    assert_eq!(md.current_version_id(), 1);
    assert_eq!(md.location(), loc);

    let cur = md.current_version();
    assert_eq!(cur.schema_id(), 0);
    let reps = cur.representations();
    assert_eq!(reps.iter().len(), 1);
    let iceberg::spec::ViewRepresentation::Sql(sql_rep) = reps.iter().next().unwrap();
    assert_eq!(sql_rep.sql, "SELECT 1");
    assert_eq!(sql_rep.dialect, "trino");

    assert_eq!(s.list_views(ns.id).await.unwrap(), vec!["v".to_string()]);
    s.drop_view(ns.id, "v").await.unwrap();
    assert!(s.load_view(ns.id, "v").await.is_err());
    s.drop_namespace(cat, &levels).await.unwrap();
}

#[tokio::test]
async fn clear_view_metadata_removes_children_without_error() {
    let Some(s) = store().await else {
        eprintln!("DATABASE_URL not set — skipping");
        return;
    };
    let cat = seed_catalog(&s).await;
    let levels = vec![format!("t_{}", uuid::Uuid::new_v4().simple())];
    let ns = s
        .create_namespace(cat, &levels, &HashMap::new())
        .await
        .unwrap();

    let view_uuid = uuid::Uuid::new_v4();
    let loc = "s3://wh/ns/clear_test";
    let mloc = "s3://wh/ns/clear_test/metadata/0.json";
    let view_md = minimal_view(loc, view_uuid);
    s.insert_view(ns.id, "cv", &view_md, mloc, loc)
        .await
        .unwrap();

    // Get the internal row id for the view.
    let view_row = s.load_view(ns.id, "cv").await.unwrap();
    let view_id = view_row.id;

    // Run clear_view_metadata inside a transaction and commit.
    let mut tx = s.pool().begin().await.unwrap();
    view_repo::clear_view_metadata(&mut tx, view_id)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Every child table must now be empty for this view_id.
    let pool = s.pool();
    let schemas: i64 =
        sqlx::query_scalar("SELECT count(*) FROM catalog_view_schemas WHERE view_id = $1")
            .bind(view_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(schemas, 0, "catalog_view_schemas not cleared");

    let versions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM catalog_view_versions WHERE view_id = $1")
            .bind(view_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(versions, 0, "catalog_view_versions not cleared");

    let representations: i64 =
        sqlx::query_scalar("SELECT count(*) FROM catalog_view_representations WHERE view_id = $1")
            .bind(view_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        representations, 0,
        "catalog_view_representations not cleared"
    );

    let version_log: i64 =
        sqlx::query_scalar("SELECT count(*) FROM catalog_view_version_log WHERE view_id = $1")
            .bind(view_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(version_log, 0, "catalog_view_version_log not cleared");

    let properties: i64 =
        sqlx::query_scalar("SELECT count(*) FROM catalog_view_properties WHERE view_id = $1")
            .bind(view_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(properties, 0, "catalog_view_properties not cleared");

    // Cleanup — clear_view_metadata removed children but the root row still exists;
    // drop_view → load_view → assemble would fail on missing versions, so delete
    // the root row directly.
    sqlx::query("DELETE FROM catalog_views WHERE id = $1")
        .bind(view_id)
        .execute(pool)
        .await
        .unwrap();
    s.drop_namespace(cat, &levels).await.unwrap();
}
