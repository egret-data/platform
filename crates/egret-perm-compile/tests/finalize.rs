//! SP3d Package C: finalize's installation-wide provenance delete, on the three
//! tables that carry no profile scope. Runs in its own binary, serialised, so
//! wiping them cannot race another suite's seeded rows within it.

use egret_metastore::Db;
use egret_perm_compile::finalize::{delete_legacy_inputs_in_tx, Scope};

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn the_global_provenance_tables_are_emptied_and_a_second_run_deletes_nothing() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let _g = SERIAL.lock().await;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = &db.pool;

    sqlx::query(
        "INSERT INTO perm.clearance_migration (tags, rules, rule_ids, exemption_hashes, original, migrated_by) \
         VALUES ('{}', '[]', '{}', '{}', '[]', 'test')",
    )
    .execute(pool)
    .await
    .expect("clearance row");
    sqlx::query(
        "INSERT INTO perm.role_principal_migration (roles, rewrites, row_hashes, migrated_by) \
         VALUES ('{}', '[]', '{}', 'test')",
    )
    .execute(pool)
    .await
    .expect("role row");
    sqlx::query(
        "INSERT INTO perm.ws_assignment_migration (catalogs, grant_ids, principal_hashes, original, migrated_by) \
         VALUES ('{}', '{}', '{}', '[]', 'test')",
    )
    .execute(pool)
    .await
    .expect("ws row");

    let mut tx = pool.begin().await.expect("begin");
    let s = delete_legacy_inputs_in_tx(&mut tx, Scope::All)
        .await
        .expect("delete");
    tx.commit().await.expect("commit");
    assert!(s.clearance_migrations_deleted >= 1, "{s:?}");
    assert!(s.role_principal_migrations_deleted >= 1, "{s:?}");
    assert!(s.ws_assignment_migrations_deleted >= 1, "{s:?}");

    for sql in [
        "SELECT count(*) FROM perm.migrated_from",
        "SELECT count(*) FROM perm.clearance_migration",
        "SELECT count(*) FROM perm.role_principal_migration",
        "SELECT count(*) FROM perm.ws_assignment_migration",
    ] {
        let n: i64 = sqlx::query_scalar(sql)
            .fetch_one(pool)
            .await
            .expect("count");
        assert_eq!(n, 0, "{sql}");
    }

    let mut tx = pool.begin().await.expect("begin");
    let again = delete_legacy_inputs_in_tx(&mut tx, Scope::All)
        .await
        .expect("second delete");
    tx.commit().await.expect("commit");
    assert_eq!(again.policy_migrations_deleted, 0);
    assert_eq!(again.clearance_migrations_deleted, 0);
    assert_eq!(again.role_principal_migrations_deleted, 0);
    assert_eq!(again.ws_assignment_migrations_deleted, 0);
    assert_eq!(again.original_policies_deleted, 0);
}
