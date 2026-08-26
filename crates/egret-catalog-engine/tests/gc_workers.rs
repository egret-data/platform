//! DB-gated test proving `build_state` registers the 3 built-in GC queues
//! into the returned `TaskQueueRegistry` (CE-6).
//!
//! Self-skips when `DATABASE_URL` is unset. Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-catalog-engine --test gc_workers

use egret_catalog_engine::{build_state, run_lakekeeper_migrations};

const SYSTEM_USER: &str = "egret-system";

#[tokio::test]
async fn build_state_registers_builtin_gc_queues() {
    let Some(dsn) = egret_test_db::url().await else {
        eprintln!("DATABASE_URL not set — skipping build_state_registers_builtin_gc_queues");
        return;
    };

    // Migrate lakekeeper's schema first (build_state requires a migrated DB).
    run_lakekeeper_migrations(&dsn)
        .await
        .expect("lakekeeper migrations apply");

    let (_state, registry) = build_state(&dsn, SYSTEM_USER)
        .await
        .expect("build engine state");

    assert!(
        !registry.is_empty().await,
        "built-in GC queues must be registered (registry was empty)"
    );
}
