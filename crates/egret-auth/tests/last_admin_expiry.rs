//! KI-322, the last-admin guard's half: a user whose `platform_admin` grant has
//! lapsed is not an active admin, so removing the last LIVE admin is refused
//! even while the lapsed row is still in the table.
//!
//! Its own test binary, so its own database (`egret_test_db`: one per test
//! process) and a single test: the guard counts the GLOBAL set of active
//! platform admins. DB-gated; self-skips without `DATABASE_URL` ONLY under
//! `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.

use egret_auth::roles::LastAdminGuard;
use egret_metastore::Db;
use uuid::Uuid;

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

async fn admin_with_expiry(db: &Db, expires_in_secs: i64) -> Uuid {
    let uid: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ('local', $1, 'T') RETURNING id",
    )
    .bind(format!("{}@test.local", Uuid::new_v4()))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO access_grants \
           (principal_type, principal_id, role_id, scope_type, scope_id, expires_at) \
         SELECT 'user', $1, id, 'platform', NULL, now() + make_interval(secs => $2::bigint) \
           FROM roles WHERE scope = 'platform' AND name = 'platform_admin'",
    )
    .bind(uid)
    .bind(expires_in_secs)
    .execute(&db.pool)
    .await
    .unwrap();
    uid
}

#[tokio::test]
async fn a_lapsed_admin_does_not_keep_the_last_live_admin_removable() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    // Arrange: one live admin and one whose grant lapsed a minute ago.
    let live = admin_with_expiry(&db, 3600).await;
    let lapsed = admin_with_expiry(&db, -60).await;
    let mut conn = db.pool.acquire().await.unwrap();
    let before: Vec<Uuid> = egret_auth::roles::engine_admin_user_ids_via(&mut conn)
        .await
        .unwrap()
        .into_iter()
        .map(|u| u.0)
        .collect();
    assert!(
        before.contains(&live) && before.iter().all(|u| *u == live || *u == lapsed),
        "precondition: no admin in this database but the two seeded here: {before:?}"
    );
    drop(conn);

    // Act: remove the live admin's grant under the guard.
    let mut tx = db.pool.begin().await.unwrap();
    let guard = LastAdminGuard::lock_via(&mut tx).await.unwrap();
    sqlx::query("DELETE FROM access_grants WHERE principal_id = $1")
        .bind(live)
        .execute(&mut *tx)
        .await
        .unwrap();
    let refused = guard.refuse_if_emptied_via(&mut tx).await;
    tx.rollback().await.unwrap();

    // Assert
    assert!(
        matches!(&refused, Err(egret_core::Error::Conflict(m)) if m == egret_auth::roles::LAST_ADMIN_REFUSAL),
        "the lapsed admin must not count, so this removes the last active admin: {refused:?}"
    );
}
