//! DB-gated view-store tests. Self-skip without DATABASE_URL.
mod common;
use common::{seed_catalog, store};
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
