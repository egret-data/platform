//! Integration test: applies migrations against a real Postgres.
//! Skips (passes) when `DATABASE_URL` is unset so `cargo test` stays green offline.
//! In CI / local dev, start Postgres via `deploy/docker-compose.dev.yml`.

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
