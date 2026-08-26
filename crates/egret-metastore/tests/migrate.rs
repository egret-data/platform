//! Integration test: applies migrations against a real Postgres.
//! Skips (passes) when `DATABASE_URL` is unset so `cargo test` stays green offline.
//! In CI / local dev, start Postgres via `deploy/docker-compose.dev.yml`.
//!
//! # The one DB-gated test that deliberately does NOT use `egret-test-db`
//!
//! Every other crate's tests now take a private clone of a pre-migrated template.
//! This one must not: a clone is a database the migrator has *already* run on, so
//! `db.migrate()` there is a no-op and the assertion below would pass without the
//! migrations ever being exercised. What it checks is that the migrator applies to
//! whatever `DATABASE_URL` names, which is also the shape of the CI smoke test.
//!
//! It is also what builds `egret-test-db`'s template, transitively — that crate
//! depends on this one for `Db::migrate`, so wiring it back in here would be a
//! dev-dependency cycle in service of a weaker test.

use egret_metastore::Db;

#[tokio::test]
async fn applies_migrations_and_seeds_meta() {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL unset — skipping DB integration test");
        return;
    };

    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM egret_meta")
        .fetch_one(&db.pool)
        .await
        .expect("query egret_meta");

    assert!(count >= 1, "baseline row should be present");
}
