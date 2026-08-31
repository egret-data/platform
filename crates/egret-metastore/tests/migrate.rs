//! Integration test: applies migrations against a real Postgres.
//! Skips ONLY when `EGRET_ALLOW_DB_SKIP` is set; without it a run with no
//! `DATABASE_URL` FAILS rather than reporting `ok` having asserted nothing.
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
    // The same rule `egret_test_db::url` enforces for every other DB-gated suite,
    // restated rather than called: this crate is `egret-test-db`'s dependency, so
    // importing it here would be a dev-dependency cycle (see the header). Ten lines
    // of duplication is the price of the one test that cannot use the harness.
    let Ok(url) = std::env::var("DATABASE_URL") else {
        assert!(
            std::env::var_os("EGRET_ALLOW_DB_SKIP").is_some(),
            "DATABASE_URL is not set, so the migrator never ran and this test asserted \
             nothing. It used to report `ok` anyway. Either\n  \
             just up && export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test\n\
             or skip deliberately and visibly:\n  \
             EGRET_ALLOW_DB_SKIP=1 cargo test --workspace"
        );
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
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
