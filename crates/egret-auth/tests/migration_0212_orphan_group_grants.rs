//! Migration `0212_drop_orphan_group_grants.sql` (KI-324): an `access_grants`
//! row whose group principal no longer exists is deleted; a live group's
//! grant and a user's grant are not. The database carries 0212 already, so
//! its body is re-run against rows seeded to look like the orphans it exists
//! for (it is idempotent: one `DELETE`).
//!
//! DB-gated; self-skips without `DATABASE_URL` ONLY under
//! `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.

use egret_metastore::{Db, MIGRATOR};
use uuid::Uuid;

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

async fn platform_grant(db: &Db, principal_type: &str, principal: Uuid) {
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
         SELECT $1, $2, id, 'platform' FROM roles \
          WHERE scope = 'platform' AND name <> 'platform_admin' ORDER BY name LIMIT 1",
    )
    .bind(principal_type)
    .bind(principal)
    .execute(&db.pool)
    .await
    .expect("grant");
}

async fn grants_of(db: &Db, principal: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM access_grants WHERE principal_id = $1")
        .bind(principal)
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn migration_0212_deletes_only_grants_of_groups_that_are_gone() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    // Arrange: an orphan (a group id no group has), a live group's grant, and
    // a user's grant (a user id is no group id either, and must stay).
    let orphan = Uuid::new_v4();
    platform_grant(&db, "group", orphan).await;
    let live: Uuid =
        sqlx::query_scalar("INSERT INTO groups (name, source) VALUES ($1, 'local') RETURNING id")
            .bind(format!("m0212-{}", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .expect("group");
    platform_grant(&db, "group", live).await;
    let user: Uuid =
        sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
            .bind(format!("m0212-{}@test.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .expect("user");
    platform_grant(&db, "user", user).await;
    assert_eq!(grants_of(&db, orphan).await, 1, "positive control");

    // Act
    let m = MIGRATOR
        .migrations
        .iter()
        .find(|m| m.version == 212)
        .expect("0212 is embedded");
    sqlx::raw_sql(m.sql.clone())
        .execute(&db.pool)
        .await
        .expect("migration 0212 must apply cleanly");

    // Assert
    assert_eq!(grants_of(&db, orphan).await, 0, "the orphan must go");
    assert_eq!(grants_of(&db, live).await, 1, "a live group's grant stays");
    assert_eq!(grants_of(&db, user).await, 1, "a user's grant stays");
}
