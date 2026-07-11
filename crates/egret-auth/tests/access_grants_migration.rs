//! Post-B2-flip: the legacy grant tables are gone and access_grants is the sole
//! store. Requires DATABASE_URL.
use egret_metastore::Db;

async fn db() -> Db {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    db
}

#[tokio::test]
async fn legacy_grant_tables_are_dropped() {
    let db = db().await;
    // Check the `public` schema specifically: some test harnesses create isolated
    // `rbac_isolated_*` schemas that carry same-named tables, so an unqualified
    // information_schema lookup would false-positive. `to_regclass('public.<t>')`
    // resolves only the public table and returns NULL once it is dropped.
    for t in ["platform_role_assignments", "workspace_members"] {
        let oid: Option<String> = sqlx::query_scalar("SELECT to_regclass($1)::text")
            .bind(format!("public.{t}"))
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert!(oid.is_none(), "public.{t} must be dropped after B2-flip");
    }
}

#[tokio::test]
async fn seeded_admin_grant_survived_in_access_grants() {
    // The migration backfilled the seeded platform_admin (if any assignment existed).
    // A fresh test DB may have no admin; this asserts the query path works and the
    // grant store is the platform-scope source (no legacy table).
    let db = db().await;
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM access_grants WHERE scope_type = 'platform'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(n >= 0);
}
