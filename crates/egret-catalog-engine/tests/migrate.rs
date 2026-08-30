//! DB-gated test proving egret's migrator and lakekeeper's migrator coexist in
//! one Postgres database without colliding, by isolating lakekeeper into its own
//! `lakekeeper` schema (tables AND its `_sqlx_migrations` tracker), while egret's
//! migration tracker (`_sqlx_migrations`) stays in `public` (egret's own tables now
//! live in per-seam schemas: `engine_mgmt`/`policy`/`storage`/`catalog`).
//!
//! Self-skips when `DATABASE_URL` is unset ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise
//! FAILS. Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-catalog-engine --test migrate

use egret_catalog_engine::run_lakekeeper_migrations;
use egret_metastore::Db;
use sqlx::Row;

#[tokio::test]
async fn migrate_isolates_schemas() {
    let Some(dsn) = egret_test_db::url().await else {
        eprintln!("DATABASE_URL not set — skipping migrate_isolates_schemas");
        return;
    };

    // Run BOTH migrators against the same database.
    // egret -> public.* + per-seam schemas, lakekeeper -> lakekeeper.*
    let db = Db::connect(&dsn, 5).await.expect("egret pool connects");
    db.migrate().await.expect("egret migrations apply");

    run_lakekeeper_migrations(&dsn)
        .await
        .expect("lakekeeper migrations apply into isolated schema");

    // Plain verification pool (default search_path).
    let pool = sqlx::PgPool::connect(&dsn)
        .await
        .expect("verification pool connects");

    // 1. lakekeeper's `table` lives in the lakekeeper schema.
    let lk_table: Option<String> =
        sqlx::query_scalar("SELECT to_regclass('lakekeeper.\"table\"')::text")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        lk_table.is_some(),
        "lakekeeper.\"table\" should exist in the lakekeeper schema"
    );

    // 2. egret's own tables stay in egret's schemas (not lakekeeper's). Assert
    //    against `warehouses`, an egret-owned table (migration 0053) that the
    //    Catalog seam moved into the `catalog` schema (migration 0065). (The old
    //    `catalog_tokens` this test used was dropped in migration 0048 when the
    //    embedded catalog was re-platformed.)
    let egret_table: Option<String> =
        sqlx::query_scalar("SELECT to_regclass('catalog.warehouses')::text")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        egret_table.is_some(),
        "catalog.warehouses should exist (egret's migrations untouched)"
    );

    // 2b. lakekeeper's `table` must NOT leak into public.
    let lk_in_public: Option<String> =
        sqlx::query_scalar("SELECT to_regclass('public.\"table\"')::text")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        lk_in_public.is_none(),
        "lakekeeper.\"table\" must not leak into public"
    );

    // 3. Two DISTINCT migration trackers, both populated.
    let lk_migrations: i64 = sqlx::query("SELECT count(*) AS c FROM lakekeeper._sqlx_migrations")
        .fetch_one(&pool)
        .await
        .expect("lakekeeper._sqlx_migrations exists")
        .get("c");
    let public_migrations: i64 = sqlx::query("SELECT count(*) AS c FROM public._sqlx_migrations")
        .fetch_one(&pool)
        .await
        .expect("public._sqlx_migrations exists")
        .get("c");

    assert!(
        lk_migrations > 0,
        "lakekeeper._sqlx_migrations should record applied migrations"
    );
    assert!(
        public_migrations > 0,
        "public._sqlx_migrations should record egret's applied migrations"
    );

    pool.close().await;
}
