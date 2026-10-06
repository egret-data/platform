//! DB-gated test for the PROD bootstrap path (`ensure_bootstrapped`): proves
//! it seeds lakekeeper's default project using a stable admin principal built
//! from public, non-`test-utils` APIs — and that it is idempotent.
//!
//! Unlike `roundtrip.rs` this needs NO RustFS: `bootstrap` only touches the
//! catalog DB (it seeds the default project + registers the system user); there
//! is no warehouse creation and thus no live S3 `validate_access`.
//!
//! Self-skips when `DATABASE_URL` is unset ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise
//! FAILS. Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-catalog-engine --test bootstrap

use egret_catalog_engine::{build_state, ensure_bootstrapped, run_lakekeeper_migrations};
use sqlx::ConnectOptions as _;
use sqlx::Executor as _;

const SYSTEM_USER: &str = "egret-system";

#[tokio::test]
async fn ensure_bootstrapped_is_idempotent_and_seeds_default_project() {
    let Some(dsn) = egret_test_db::url().await else {
        eprintln!("DATABASE_URL not set — skipping ensure_bootstrapped_is_idempotent");
        return;
    };

    // Migrate lakekeeper's schema first (ensure_bootstrapped requires it).
    run_lakekeeper_migrations(&dsn)
        .await
        .expect("lakekeeper migrations apply");

    let (state, _registry) = build_state(&dsn, SYSTEM_USER)
        .await
        .expect("build engine state");

    // First call bootstraps (or no-ops if a previous run already did — both Ok).
    ensure_bootstrapped(state.clone(), SYSTEM_USER)
        .await
        .expect("first ensure_bootstrapped succeeds");

    // Idempotency: a second call returns Ok, tolerating the
    // already-bootstrapped error internally.
    ensure_bootstrapped(state, SYSTEM_USER)
        .await
        .expect("second ensure_bootstrapped is idempotent (Ok twice)");

    // The default project must now exist in lakekeeper's schema. Query through a
    // connection pinned to the lakekeeper search_path (the tables are unqualified
    // there), exactly like roundtrip.rs verifies tabular rows.
    let mut conn = dsn
        .parse::<sqlx::postgres::PgConnectOptions>()
        .expect("parse dsn")
        .connect()
        .await
        .expect("verification connection");
    conn.execute("SET search_path = lakekeeper, public")
        .await
        .expect("set search_path");

    let project_count: i64 = sqlx::query_scalar("SELECT count(*) FROM project")
        .fetch_one(&mut conn)
        .await
        .expect("count projects");
    assert!(
        project_count >= 1,
        "bootstrap should have seeded the default project, got {project_count} projects"
    );

    // The system user must be registered as the (global-admin) bootstrap user.
    let user_count: i64 = sqlx::query_scalar(r#"SELECT count(*) FROM users"#)
        .fetch_one(&mut conn)
        .await
        .expect("count users");
    assert!(
        user_count >= 1,
        "bootstrap should have registered the system user, got {user_count} users"
    );
}
