//! SA4: a service account can never join a group holding `platform_admin`,
//! enforced at `GroupRepo::add_member`. DB-gated; FAILS without a database
//! unless `EGRET_ALLOW_DB_SKIP` is set.

#[path = "support/recorder.rs"]
mod recorder;

use egret_auth::groups::GroupRepo;
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

async fn user(db: &Db, provider: &str) -> UserId {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ($1, $2, 'U') RETURNING id",
    )
    .bind(provider)
    .bind(format!("u-{}@t.test", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("user");
    UserId(id)
}

async fn group(db: &Db, with_admin: bool) -> Uuid {
    let g = GroupRepo::new(db.clone())
        .create_local(
            &format!("g-{}", Uuid::new_v4().simple()),
            &Recorder::default(),
        )
        .await
        .expect("group");
    if with_admin {
        sqlx::query(
            "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
             SELECT 'group', $1, id, 'platform', NULL FROM roles WHERE name = 'platform_admin'",
        )
        .bind(g)
        .execute(&db.pool)
        .await
        .expect("grant");
    }
    g
}

#[tokio::test]
async fn service_account_cannot_join_a_platform_admin_group() {
    let Some(db) = connect().await else { return };
    let repo = GroupRepo::new(db.clone());
    let rc = Recorder::default();
    let g = group(&db, true).await;
    let svc = user(&db, "service").await;
    let err = repo.add_member(g, svc, &rc).await.expect_err("refused");
    assert!(matches!(err, egret_core::Error::Conflict(_)), "{err:?}");
    assert!(repo.members_of(g).await.unwrap().is_empty());
}

#[tokio::test]
async fn human_may_join_a_platform_admin_group() {
    let Some(db) = connect().await else { return };
    let repo = GroupRepo::new(db.clone());
    let g = group(&db, true).await;
    let human = user(&db, "local").await;
    repo.add_member(g, human, &Recorder::default())
        .await
        .expect("human ok");
}

#[tokio::test]
async fn service_account_may_join_a_group_without_platform_admin() {
    let Some(db) = connect().await else { return };
    let repo = GroupRepo::new(db.clone());
    let g = group(&db, false).await;
    let svc = user(&db, "service").await;
    repo.add_member(g, svc, &Recorder::default())
        .await
        .expect("service ok");
}
