//! `ServiceAccountRepo` against a real database (migration 0229).
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, otherwise FAILS.

#[path = "support/recorder.rs"]
mod recorder;

use egret_auth::service_accounts::{SaScope, ServiceAccountRepo};
use egret_auth::users::UserRepo;
use egret_core::UserId;
use egret_metastore::Db;
use recorder::Recorder;
use uuid::Uuid;

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

async fn workspace(db: &Db) -> Uuid {
    sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1, 'W') RETURNING id")
        .bind(format!("w-{}", Uuid::new_v4().simple()))
        .fetch_one(&db.pool)
        .await
        .expect("workspace")
}

async fn creator(db: &Db) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ('local', $1, 'C') RETURNING id",
    )
    .bind(format!("c-{}@t.test", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("creator")
}

fn uniq(p: &str) -> String {
    format!("{p}-{}", &Uuid::new_v4().simple().to_string()[..8])
}

async fn assert_service_user(db: &Db, id: Uuid) {
    let (provider, pw_null, status): (String, bool, String) =
        sqlx::query_as("SELECT provider, password_hash IS NULL, status FROM users WHERE id = $1")
            .bind(id)
            .fetch_one(&db.pool)
            .await
            .expect("user row");
    assert_eq!(provider, "service");
    assert!(pw_null);
    assert_eq!(status, "active");
    let username = UserRepo::new(db.clone())
        .ensure_username(UserId(id))
        .await
        .expect("username");
    assert!(username.starts_with("svc-"), "username {username}");
}

#[tokio::test]
async fn creates_workspace_and_platform_accounts() {
    let Some(db) = connect().await else { return };
    let repo = ServiceAccountRepo::new(db.pool.clone());
    let (ws, by) = (workspace(&db).await, creator(&db).await);

    let w = repo
        .create(SaScope::Workspace, Some(ws), &uniq("wsa"), "d", by)
        .await
        .expect("workspace sa");
    assert_eq!(w.scope, SaScope::Workspace);
    assert_eq!(w.workspace_id, Some(ws));
    assert_eq!(w.status, "active");
    assert_service_user(&db, w.user_id).await;

    let p = repo
        .create(SaScope::Platform, None, &uniq("psa"), "d", by)
        .await
        .expect("platform sa");
    assert_eq!(p.scope, SaScope::Platform);
    assert_eq!(p.workspace_id, None);
    assert_service_user(&db, p.user_id).await;

    let stored: (String, Option<Uuid>) =
        sqlx::query_as("SELECT scope, workspace_id FROM service_accounts WHERE user_id = $1")
            .bind(p.user_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(stored, ("platform".to_string(), None));
}

#[tokio::test]
async fn scope_workspace_mismatch_is_refused_and_duplicates_conflict() {
    let Some(db) = connect().await else { return };
    let repo = ServiceAccountRepo::new(db.pool.clone());
    let (ws, by) = (workspace(&db).await, creator(&db).await);

    let e = repo
        .create(SaScope::Platform, Some(ws), &uniq("x"), "", by)
        .await
        .unwrap_err();
    assert!(matches!(e, egret_core::Error::InvalidInput(_)), "{e:?}");
    let e = repo
        .create(SaScope::Workspace, None, &uniq("x"), "", by)
        .await
        .unwrap_err();
    assert!(matches!(e, egret_core::Error::InvalidInput(_)), "{e:?}");

    let name = uniq("dup");
    repo.create(SaScope::Workspace, Some(ws), &name, "", by)
        .await
        .unwrap();
    let e = repo
        .create(SaScope::Workspace, Some(ws), &name, "", by)
        .await
        .unwrap_err();
    assert!(matches!(e, egret_core::Error::Conflict(_)), "{e:?}");
}

#[tokio::test]
async fn list_filters_by_scope_and_workspace() {
    let Some(db) = connect().await else { return };
    let repo = ServiceAccountRepo::new(db.pool.clone());
    let (ws1, ws2, by) = (
        workspace(&db).await,
        workspace(&db).await,
        creator(&db).await,
    );
    let a = repo
        .create(SaScope::Workspace, Some(ws1), &uniq("a"), "", by)
        .await
        .unwrap();
    let b = repo
        .create(SaScope::Workspace, Some(ws2), &uniq("b"), "", by)
        .await
        .unwrap();
    let p = repo
        .create(SaScope::Platform, None, &uniq("p"), "", by)
        .await
        .unwrap();

    let l = repo
        .list(Some(SaScope::Workspace), Some(ws1))
        .await
        .unwrap();
    assert_eq!(
        l.iter().map(|s| s.user_id).collect::<Vec<_>>(),
        vec![a.user_id]
    );
    let l = repo.list(Some(SaScope::Platform), None).await.unwrap();
    assert!(l.iter().all(|s| s.scope == SaScope::Platform));
    assert!(l.iter().any(|s| s.user_id == p.user_id));
    assert!(!l.iter().any(|s| s.user_id == b.user_id));
}

#[tokio::test]
async fn disable_enable_and_delete() {
    let Some(db) = connect().await else { return };
    let repo = ServiceAccountRepo::new(db.pool.clone());
    let by = creator(&db).await;
    let sa = repo
        .create(SaScope::Platform, None, &uniq("life"), "", by)
        .await
        .unwrap();

    repo.set_status_without_recompile_or_revoke_for_tests(sa.user_id, "disabled")
        .await
        .unwrap();
    assert_ne!(
        repo.get(sa.user_id).await.unwrap().unwrap().status,
        "active"
    );
    repo.set_status_without_recompile_or_revoke_for_tests(sa.user_id, "active")
        .await
        .unwrap();
    assert_eq!(
        repo.get(sa.user_id).await.unwrap().unwrap().status,
        "active"
    );

    let role: Uuid = sqlx::query_scalar(
        "SELECT id FROM roles WHERE scope = 'platform' AND name = 'data_steward'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'platform', NULL)",
    )
    .bind(sa.user_id)
    .bind(role)
    .execute(&db.pool)
    .await
    .unwrap();

    assert!(repo
        .delete(sa.user_id, &actor(), &Recorder::default())
        .await
        .unwrap());
    let grants: i64 =
        sqlx::query_scalar("SELECT count(*) FROM access_grants WHERE principal_id = $1")
            .bind(sa.user_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(grants, 0, "no orphan grants");
    assert!(repo.get(sa.user_id).await.unwrap().is_none());
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE id = $1")
        .bind(sa.user_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

fn actor() -> egret_core::Principal {
    egret_core::Principal {
        user_id: UserId(Uuid::nil()),
        email: "actor@test.local".into(),
        platform_roles: vec![],
        permissions: vec![],
        group_ids: vec![],
        impersonator: None,
    }
}

#[tokio::test]
async fn delete_of_a_missing_or_non_service_id_returns_false() {
    let Some(db) = connect().await else { return };
    let repo = ServiceAccountRepo::new(db.pool.clone());
    let human = creator(&db).await;
    for id in [Uuid::new_v4(), human] {
        assert!(!repo
            .delete(id, &actor(), &Recorder::default())
            .await
            .unwrap());
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE id = $1")
        .bind(human)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(n, 1, "a human user is never deleted through the SA repo");
}
