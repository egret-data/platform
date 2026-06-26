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

/// Seed a workspace + its default catalog and return the catalog id.
async fn seed_catalog(s: &CatalogStore) -> uuid::Uuid {
    let ws = uuid::Uuid::new_v4();
    let slug = format!("ws_{}", ws.simple());
    sqlx::query("INSERT INTO workspaces (id, slug, name, status) VALUES ($1, $2, $3, 'active')")
        .bind(ws)
        .bind(&slug)
        .bind(&slug)
        .execute(s.pool())
        .await
        .expect("seed workspace");
    s.create_catalog(ws, "default", "s3://wh")
        .await
        .expect("seed catalog")
        .id
}

fn minimal_metadata(location: &str, id: uuid::Uuid) -> iceberg::spec::TableMetadata {
    egret_catalog::service::test_support::minimal_table_metadata(location, id)
}

/// Build table metadata carrying one snapshot on a `main` branch.
///
/// Exercises the snapshot + ref + snapshot_log + current_snapshot_id paths of
/// shred/assemble (the hard parts the minimal metadata never touches). Returns
/// `(metadata, snapshot_id, timestamp_ms)`.
fn metadata_with_snapshot(
    location: &str,
    id: uuid::Uuid,
) -> (iceberg::spec::TableMetadata, i64, i64) {
    use iceberg::spec::{Operation, Snapshot, Summary, TableMetadataBuilder};
    let base = minimal_metadata(location, id);
    let snapshot_id = 3_001i64;
    let ts = chrono::Utc::now().timestamp_millis();
    let snap = Snapshot::builder()
        .with_snapshot_id(snapshot_id)
        .with_sequence_number(1)
        .with_schema_id(0)
        .with_manifest_list(format!("{location}/metadata/snap-{snapshot_id}.avro"))
        .with_summary(Summary {
            operation: Operation::Append,
            additional_properties: std::collections::HashMap::from([(
                "added-records".to_string(),
                "1".to_string(),
            )]),
        })
        .with_timestamp_ms(ts)
        .build();
    // set_branch_snapshot adds the snapshot, creates the `main` branch ref, and
    // keeps current_snapshot_id in agreement with the ref (validated by
    // try_normalize on assemble).
    let md = TableMetadataBuilder::new_from_metadata(base, None)
        .set_branch_snapshot(snap, "main")
        .unwrap()
        .build()
        .unwrap()
        .metadata;
    (md, snapshot_id, ts)
}

#[tokio::test]
async fn insert_load_drop_table() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let cat = seed_catalog(&s).await;
    let levels = vec![format!("t_{}", uuid::Uuid::new_v4().simple())];
    let ns = s
        .create_namespace(cat, &levels, &HashMap::new())
        .await
        .unwrap();
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
    s.drop_namespace(cat, &levels).await.unwrap();
}

#[tokio::test]
async fn insert_then_load_roundtrips_metadata() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let cid = seed_catalog(&s).await;
    let levels = vec![format!("t_{}", uuid::Uuid::new_v4().simple())];
    let ns = s
        .create_namespace(cid, &levels, &std::collections::HashMap::new())
        .await
        .unwrap();
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
    assert_eq!(row.metadata.uuid(), md.uuid());
    assert_eq!(row.metadata.format_version(), md.format_version());
    assert_eq!(row.metadata.location(), md.location());
    assert_eq!(
        row.metadata.current_schema().schema_id(),
        md.current_schema().schema_id()
    );
    assert_eq!(row.metadata.current_snapshot_id(), md.current_snapshot_id());
    assert_eq!(
        row.metadata.default_partition_spec().spec_id(),
        md.default_partition_spec().spec_id()
    );
    s.drop_table(ns.id, "orders").await.unwrap();
    s.drop_namespace(cid, &levels).await.unwrap();
}

#[tokio::test]
async fn insert_then_load_roundtrips_metadata_with_snapshot() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let cid = seed_catalog(&s).await;
    let levels = vec![format!("t_{}", uuid::Uuid::new_v4().simple())];
    let ns = s
        .create_namespace(cid, &levels, &HashMap::new())
        .await
        .unwrap();
    let tid = uuid::Uuid::new_v4();
    let loc = format!("s3://wh/{}/snaps-{tid}", levels[0]);
    let (md, snap_id, snap_ts) = metadata_with_snapshot(&loc, tid);
    let mloc = format!(
        "{loc}/metadata/00001-{}.metadata.json",
        uuid::Uuid::new_v4()
    );

    // Precondition: this metadata genuinely carries a snapshot + main ref.
    assert_eq!(md.current_snapshot_id(), Some(snap_id));
    assert!(md.refs().contains_key("main"));

    s.insert_table(ns.id, "snaps", tid, &md, &mloc, &loc)
        .await
        .unwrap();
    let row = s.load_table(ns.id, "snaps").await.unwrap();

    // current_snapshot_id is now a real Some(id) and must survive.
    assert_eq!(row.metadata.current_snapshot_id(), md.current_snapshot_id());
    assert_eq!(row.metadata.current_snapshot_id(), Some(snap_id));

    // The snapshot itself round-trips with all stored fields.
    let loaded_snap = row
        .metadata
        .snapshot_by_id(snap_id)
        .expect("snapshot present after reload");
    let orig_snap = md.snapshot_by_id(snap_id).unwrap();
    assert_eq!(loaded_snap.snapshot_id(), snap_id);
    assert_eq!(loaded_snap.sequence_number(), orig_snap.sequence_number());
    assert_eq!(loaded_snap.manifest_list(), orig_snap.manifest_list());
    assert_eq!(loaded_snap.timestamp_ms(), snap_ts);
    assert_eq!(loaded_snap.parent_snapshot_id(), None);
    // nullable schema_id round-trips as Some(0).
    assert_eq!(loaded_snap.schema_id(), orig_snap.schema_id());
    assert_eq!(loaded_snap.schema_id(), Some(0));

    // The `main` ref round-trips with the correct retention variant.
    let main_ref = row.metadata.refs().get("main").expect("main ref present");
    assert_eq!(main_ref.snapshot_id, snap_id);
    assert!(
        matches!(
            main_ref.retention,
            iceberg::spec::SnapshotRetention::Branch { .. }
        ),
        "main must be a branch, got: {:?}",
        main_ref.retention
    );

    // The snapshot log (history) round-trips.
    assert_eq!(row.metadata.history(), md.history());
    assert_eq!(
        row.metadata.history().last().map(|l| l.snapshot_id),
        Some(snap_id)
    );

    s.drop_table(ns.id, "snaps").await.unwrap();
    s.drop_namespace(cid, &levels).await.unwrap();
}

#[tokio::test]
async fn list_tables_multiple() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let cat = seed_catalog(&s).await;
    let levels = vec![format!("t_{}", uuid::Uuid::new_v4().simple())];
    let ns = s
        .create_namespace(cat, &levels, &HashMap::new())
        .await
        .unwrap();

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
    s.drop_namespace(cat, &levels).await.unwrap();
}

#[tokio::test]
async fn cas_commit_rejects_stale_version() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let cat = seed_catalog(&s).await;
    let levels = vec![format!("t_{}", uuid::Uuid::new_v4().simple())];
    let ns = s
        .create_namespace(cat, &levels, &HashMap::new())
        .await
        .unwrap();
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
    s.drop_namespace(cat, &levels).await.unwrap();
}

#[tokio::test]
async fn duplicate_table_is_already_exists() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let cat = seed_catalog(&s).await;
    let levels = vec![format!("t_{}", uuid::Uuid::new_v4().simple())];
    let ns = s
        .create_namespace(cat, &levels, &HashMap::new())
        .await
        .unwrap();
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
    s.drop_namespace(cat, &levels).await.unwrap();
}
