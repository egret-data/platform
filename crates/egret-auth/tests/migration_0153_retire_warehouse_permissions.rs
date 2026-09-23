//! Migration `0153_retire_warehouse_permissions.sql` (SP3c-2 §4 step 4).
//!
//! 0148 granted every platform role holding `warehouse:*` its
//! `catalog_registry:*` counterpart — once, as a one-time `INSERT … SELECT`
//! over the roles that existed at the moment 0148 ran. Any platform role
//! created AFTER that point holds `warehouse:*` alone, with no
//! `catalog_registry:*` counterpart, until 0153 re-runs the same grant (this
//! test's `full_role_lifecycle`-style DB carries 0153 already applied to
//! HEAD, so the migration under test is re-executed directly against a role
//! seeded to look like that population — `warehouse:read` and nothing else).
//!
//! Both halves are asserted: the role gains `catalog_registry:read`, AND it
//! holds no permission beginning `warehouse:`. A test that only checked the
//! new key would pass even if 0153's `DELETE` never ran.
//!
//! DB-gated; self-skips without `DATABASE_URL` ONLY under
//! `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.

use egret_auth::roles::RoleRepo;
use egret_metastore::{Db, MIGRATOR};

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn uniq(tag: &str) -> String {
    format!("{tag}-{}", uuid::Uuid::new_v4().simple())
}

#[tokio::test]
async fn migration_0153_regrants_then_deletes_warehouse_permissions() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let roles = RoleRepo::new(db.clone(), egret_events::EventBus::new());

    // Arrange: a platform role holding ONLY `warehouse:read`, seeded fresh —
    // i.e. created AFTER 0148 ran (0148 is long applied on this database, and
    // its one-time INSERT … SELECT cannot have reached a role that did not
    // exist yet). `set_permissions` writes the raw string with no
    // `rbac::is_known` gate, so a retired key seeds exactly as it would have
    // in production before 0153 existed.
    let role_id = roles
        .create(
            "platform",
            &uniq("m0153_role"),
            "post-0148 role holding only warehouse:read",
            None,
        )
        .await
        .expect("create platform role");
    roles
        .set_permissions(role_id, &["warehouse:read".to_string()])
        .await
        .expect("seed warehouse:read");

    // Positive control: the seed produced exactly the population 0153 exists
    // for — `warehouse:read` present, `catalog_registry:read` absent.
    let seeded = roles
        .permissions_of(role_id)
        .await
        .expect("read seeded perms");
    assert_eq!(
        seeded,
        vec!["warehouse:read".to_string()],
        "positive control: role must hold only warehouse:read before 0153 runs"
    );

    // Act: run 0153's body directly. It is idempotent (INSERT … ON CONFLICT DO
    // NOTHING, then an unconditional DELETE), so re-running it against a
    // database where it is already part of history is safe — it touches only
    // this test's fresh role and any leftover `warehouse:*` rows, of which
    // there are none left to re-delete.
    let m0153 = MIGRATOR
        .migrations
        .iter()
        .find(|m| m.version == 153)
        .expect("0153 is embedded — every migration on disk is embedded (build.rs)");
    sqlx::raw_sql(m0153.sql.clone())
        .execute(&db.pool)
        .await
        .expect("migration 0153 must apply cleanly");

    // Assert, both halves.
    let after = roles
        .permissions_of(role_id)
        .await
        .expect("read post-0153 perms");
    assert!(
        after.iter().any(|p| p == "catalog_registry:read"),
        "the role must gain catalog_registry:read; got {after:?}"
    );
    assert!(
        !after.iter().any(|p| p.starts_with("warehouse:")),
        "the role must hold no permission beginning `warehouse:` after 0153; got {after:?}"
    );
}
