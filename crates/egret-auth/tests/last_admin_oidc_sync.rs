//! KI-284: the OIDC group sync (every login, and the stale-membership sweep
//! that replays it with no claim) never removes the last active platform
//! admin. It keeps the membership that carries `platform_admin` and goes on;
//! it cannot refuse, because refusing would fail the login or retry forever.
//! DB-gated; self-skips without `DATABASE_URL` ONLY under
//! `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.
//!
//! Its own test binary, so its own database (`egret_test_db`: one per test
//! process) and a single test: the guard counts the GLOBAL set of active
//! platform admins.

use egret_auth::groups::GroupRepo;
use egret_auth::users::UserRepo;
use egret_metastore::Db;

#[path = "support/recorder.rs"]
mod recorder;
use recorder::Recorder;

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

async fn group_names_of(db: &Db, user: uuid::Uuid) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT g.name FROM user_groups ug JOIN groups g ON g.id = ug.group_id \
          WHERE ug.user_id = $1 ORDER BY g.name",
    )
    .bind(user)
    .fetch_all(&db.pool)
    .await
    .unwrap()
}

async fn grant_platform_admin(db: &Db, principal_type: &str, principal_id: uuid::Uuid) {
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT $1, $2, id, 'platform', NULL FROM roles \
          WHERE scope = 'platform' AND name = 'platform_admin'",
    )
    .bind(principal_type)
    .bind(principal_id)
    .execute(&db.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn the_oidc_sync_keeps_the_last_admins_group_membership() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    // Arrange: `admin` is this database's only platform admin, through the
    // OIDC group `admins`; `other` is an OIDC group with no role.
    let users = UserRepo::new(db.clone());
    let groups = GroupRepo::new(db.clone());
    let rc = Recorder::default();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let admins = format!("admins-{suffix}");
    let other = format!("other-{suffix}");
    let admins_id = groups.upsert_oidc_group(&admins).await.unwrap();
    groups.upsert_oidc_group(&other).await.unwrap();
    grant_platform_admin(&db, "group", admins_id).await;
    let admin = users
        .insert_local(&format!("admin-{suffix}@idp.local"), "Admin", "x")
        .await
        .unwrap();
    groups
        .sync_user_oidc_groups(admin, &[admins.clone(), other.clone()], &rc)
        .await
        .unwrap();

    // Act: a login whose claim no longer names either group.
    groups.sync_user_oidc_groups(admin, &[], &rc).await.unwrap();

    // Assert: the role-less membership is dropped, the admin one is kept.
    assert_eq!(group_names_of(&db, admin.0).await, vec![admins.clone()]);

    // Act: the stale-membership sweep replays the same empty claim.
    sqlx::query("UPDATE users SET last_login_at = now() - interval '8 days' WHERE id = $1")
        .bind(admin.0)
        .execute(&db.pool)
        .await
        .unwrap();
    groups
        .drop_stale_oidc_memberships(std::time::Duration::from_secs(7 * 24 * 3600), &rc)
        .await
        .unwrap();

    // Assert: still kept.
    assert_eq!(group_names_of(&db, admin.0).await, vec![admins.clone()]);

    // A second admin breaks the tie: the next sync drops the membership.
    let second = users
        .insert_local(&format!("second-{suffix}@idp.local"), "Second", "x")
        .await
        .unwrap();
    grant_platform_admin(&db, "user", second.0).await;
    groups.sync_user_oidc_groups(admin, &[], &rc).await.unwrap();
    assert!(group_names_of(&db, admin.0).await.is_empty());
}
