//! SA4: `replace_platform_roles` (the `PUT /api/admin/users/{id}/platform-roles`
//! path) writes `access_grants` itself, so it must refuse `platform_admin` for
//! a service account just as the role-assignment chokepoint does. DB-gated;
//! self-skips without `DATABASE_URL` only under `EGRET_ALLOW_DB_SKIP`.

use egret_auth::password::hash_password;
use egret_auth::users::UserRepo;
use egret_core::{Error, UserId};
use egret_metastore::Db;
use egret_workspace::rbac::Authorizer;

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

async fn seed_service(db: &Db) -> UserId {
    let id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ('service', $1, 'SA') RETURNING id",
    )
    .bind(format!("svc-{}@sa.test", uuid::Uuid::new_v4()))
    .fetch_one(&db.pool)
    .await
    .expect("service user");
    UserId(id)
}

async fn roles_of(db: &Db, user: UserId) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT r.name FROM access_grants ag JOIN roles r ON r.id = ag.role_id \
          WHERE ag.principal_type = 'user' AND ag.principal_id = $1 \
            AND ag.scope_type = 'platform' ORDER BY r.name",
    )
    .bind(user.0)
    .fetch_all(&db.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_service_account_cannot_be_given_platform_admin_by_replace() {
    let Some(db) = connect().await else { return };
    let authz = Authorizer::new(db.clone());
    let sa = seed_service(&db).await;

    let err = authz
        .replace_platform_roles(sa, &["platform_admin".into()])
        .await
        .expect_err("refused");
    assert!(matches!(err, Error::Conflict(_)), "{err:?}");
    assert!(roles_of(&db, sa).await.is_empty(), "nothing written");

    // data_steward is fine for a service account.
    authz
        .replace_platform_roles(sa, &["data_steward".into()])
        .await
        .unwrap();
    assert_eq!(roles_of(&db, sa).await, vec!["data_steward"]);

    // A mixed set containing platform_admin is refused and leaves the set as it was.
    authz
        .replace_platform_roles(sa, &["data_steward".into(), "platform_admin".into()])
        .await
        .expect_err("refused");
    assert_eq!(roles_of(&db, sa).await, vec!["data_steward"]);

    // A human user can still be given platform_admin.
    let human = UserRepo::new(db.clone())
        .insert_local(
            &format!("{}@test.local", uuid::Uuid::new_v4()),
            "H",
            &hash_password("pw").unwrap(),
        )
        .await
        .unwrap();
    authz
        .replace_platform_roles(human, &["platform_admin".into()])
        .await
        .unwrap();
    assert_eq!(roles_of(&db, human).await, vec!["platform_admin"]);
}
