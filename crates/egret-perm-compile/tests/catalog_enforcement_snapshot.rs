//! Governance SP5c Task 1: `build_catalog_enforcement_snapshot` carries one
//! catalog's rules and withholds (no grants) and its classified objects, so
//! `Snapshot::table_read_restricted` answers whether a table's read is
//! withheld from a user.
//!   DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
//!   cargo test -p egret-perm-compile --features harness --test catalog_enforcement_snapshot
include!("opa_support/fixtures_host.rs");
#[allow(dead_code)]
#[path = "golden/fixtures.rs"]
mod fixtures;

use egret_metastore::Db;
use egret_perm::{Action, Effect, Selector};
use fixtures::*;

static SEEDED: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

async fn db() -> Db {
    let dsn = egret_test_db::url().await.expect("a test database");
    let db = Db::connect(&dsn, 5).await.expect("connect");
    SEEDED
        .get_or_init(|| async {
            db.migrate().await.expect("migrate");
        })
        .await;
    db
}

#[tokio::test]
async fn the_snapshot_restricts_ruled_tables_and_only_them() {
    // Arrange: a catalog with a classification-selected mask (pii on
    // orders.ssn), a path row filter on ledger, bob exempt from both, and a
    // grant that must stay out of the snapshot.
    let db = db().await;
    let mut names = Names::default();
    let mut tx = db.pool.begin().await.expect("tx");
    let wh = warehouse(&mut tx, "ces", &mut names).await;
    let cat = catalog(&mut tx, "ces.lake", wh, &[], &mut names).await;
    let lake: String = sqlx::query_scalar("SELECT name FROM registry.catalogs WHERE id = $1")
        .bind(cat)
        .fetch_one(&mut *tx)
        .await
        .expect("catalog name");
    user(&mut tx, "ces.ann", Some("ces_ann"), &mut names).await;
    user(&mut tx, "ces.bob", Some("ces_bob"), &mut names).await;
    grant(
        &mut tx,
        "ces.read",
        Effect::Allow,
        &[Action::Read],
        scope(cat, None, None, None),
        set(&[user_ref("ces_ann")], &[]),
        &mut names,
    )
    .await;
    let pii = tag(&mut tx, "ces_pii", &mut names).await;
    tag_column(&mut tx, pii, wh, "s", "orders", "ssn").await;
    rule(
        &mut tx,
        "ces.mask",
        mask("MASK_NULL", None),
        by_tag("ces_pii"),
        set(&[user_ref("ces_bob")], &[]),
        &mut names,
    )
    .await;
    rule(
        &mut tx,
        "ces.filter",
        row_filter("region = 'eu'"),
        Selector::Path(scope(cat, Some("s"), Some("ledger"), None)),
        set(&[user_ref("ces_bob")], &[]),
        &mut names,
    )
    .await;

    // Act
    let snap = egret_perm_compile::opa::build::build_catalog_enforcement_snapshot(&mut tx, cat)
        .await
        .expect("the snapshot builds")
        .expect("a registry catalog");

    // Assert
    assert!(snap.table_read_restricted("ces_ann", &lake, "s", "orders"));
    assert!(snap.table_read_restricted("ces_ann", &lake, "s", "ledger"));
    assert!(!snap.table_read_restricted("ces_ann", &lake, "s", "plain"));
    assert!(
        !snap.table_read_restricted("ces_bob", &lake, "s", "orders"),
        "exempt"
    );
    assert!(
        !snap.table_read_restricted("ces_bob", &lake, "s", "ledger"),
        "exempt"
    );
    tx.rollback().await.expect("rollback");
}

#[tokio::test]
async fn an_unknown_catalog_has_no_snapshot() {
    let db = db().await;
    let mut tx = db.pool.begin().await.expect("tx");
    let none = egret_perm_compile::opa::build::build_catalog_enforcement_snapshot(
        &mut tx,
        uuid::Uuid::from_u128(0xdead),
    )
    .await
    .expect("no error");
    assert!(none.is_none());
    tx.rollback().await.expect("rollback");
}
