//! Catalog-resource tag-target DB harness (Access P3.2). DB-gated; self-skips without
//! `DATABASE_URL` ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS. Covers
//! `CatalogTagRepo` assign/unassign + the resource/warehouse listing round-trips against
//! migration 0081's `catalog_tag_assignments` table.
//!
//! METADATA ONLY: no authorization decision reads these tables in this phase.

use egret_bi::catalog_tags::CatalogTagRepo;
use egret_bi::TagRepo;
use egret_metastore::Db;
use uuid::Uuid;

async fn seed_user(db: &Db) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
        .bind(format!("u_{}@egret.local", Uuid::new_v4().simple()))
        .fetch_one(&db.pool)
        .await
        .expect("insert user")
}

#[tokio::test]
async fn catalog_tag_assign_list_and_cascade() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let by = seed_user(&db).await;
    let tags = TagRepo::new(db.clone());
    let repo = CatalogTagRepo::new(db.clone());

    // Two tags: one for the table, one for a column.
    let t_key = format!("pii-{}", Uuid::new_v4().simple());
    let c_key = format!("secret-{}", Uuid::new_v4().simple());
    let tbl_tag = tags
        .create(&t_key, "PII", "Personal data", "#ff0000", Some(by))
        .await
        .expect("create table tag");
    let col_tag = tags
        .create(&c_key, "Secret", "Secret column", "#00ff00", Some(by))
        .await
        .expect("create column tag");

    let wh = Uuid::new_v4();
    let (schema, table) = ("analytics", "orders");

    // Assign a table-level tag (column = "") and a column-level tag.
    repo.assign_catalog(tbl_tag.id, wh, schema, table, "", Some(by))
        .await
        .expect("assign table tag");
    // Idempotent: a re-assign is a no-op.
    repo.assign_catalog(tbl_tag.id, wh, schema, table, "", Some(by))
        .await
        .expect("assign table tag again (idempotent)");
    repo.assign_catalog(col_tag.id, wh, schema, table, "email", Some(by))
        .await
        .expect("assign column tag");

    // tags_for_resource distinguishes table-level from column-level.
    let for_table = repo
        .tags_for_resource(wh, schema, table, "")
        .await
        .expect("tags_for_resource table");
    assert!(
        for_table.iter().any(|t| t.id == tbl_tag.id),
        "table resource carries the table tag"
    );
    assert!(
        !for_table.iter().any(|t| t.id == col_tag.id),
        "table resource must NOT carry the column tag"
    );

    let for_col = repo
        .tags_for_resource(wh, schema, table, "email")
        .await
        .expect("tags_for_resource column");
    assert!(
        for_col.iter().any(|t| t.id == col_tag.id),
        "column resource carries the column tag"
    );
    assert!(
        !for_col.iter().any(|t| t.id == tbl_tag.id),
        "column resource must NOT carry the table tag"
    );

    // list_for_warehouse returns BOTH rows, tagged with their (schema, table, column).
    let all = repo
        .list_for_warehouse(wh)
        .await
        .expect("list_for_warehouse");
    let table_row = all
        .iter()
        .find(|r| r.tag.id == tbl_tag.id)
        .expect("table row present");
    assert_eq!(table_row.schema, schema);
    assert_eq!(table_row.table, table);
    assert_eq!(table_row.column, "", "table-level row has empty column");
    let col_row = all
        .iter()
        .find(|r| r.tag.id == col_tag.id)
        .expect("column row present");
    assert_eq!(col_row.column, "email");

    // Another warehouse is isolated.
    let other_wh = Uuid::new_v4();
    assert!(
        repo.list_for_warehouse(other_wh)
            .await
            .expect("list other warehouse")
            .is_empty(),
        "a different warehouse sees no assignments"
    );

    // unassign the column tag -> resource loses it.
    repo.unassign_catalog(col_tag.id, wh, schema, table, "email")
        .await
        .expect("unassign column tag");
    let for_col = repo
        .tags_for_resource(wh, schema, table, "email")
        .await
        .expect("tags_for_resource column after unassign");
    assert!(
        !for_col.iter().any(|t| t.id == col_tag.id),
        "column resource lost the column tag"
    );

    // Deleting the `tags` row CASCADEs the catalog assignment.
    tags.delete(tbl_tag.id).await.expect("delete table tag");
    let for_table = repo
        .tags_for_resource(wh, schema, table, "")
        .await
        .expect("tags_for_resource table after tag delete");
    assert!(
        !for_table.iter().any(|t| t.id == tbl_tag.id),
        "deleting the tag row cascades the catalog assignment"
    );
    assert!(
        repo.list_for_warehouse(wh)
            .await
            .expect("list after cascade")
            .is_empty(),
        "no assignments remain after cascade + unassign"
    );
}
