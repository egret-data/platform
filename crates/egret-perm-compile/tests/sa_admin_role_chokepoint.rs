//! SA4: a service account can never hold `platform_admin` — directly or through
//! a group — enforced at the role-assignment chokepoint. DB-gated.

use egret_metastore::Db;
use egret_perm::CompileError;
use egret_perm_compile::{
    assign_platform_role_and_recompile, unassign_platform_role_and_recompile,
};
use sqlx::PgPool;
use uuid::Uuid;

async fn pool() -> Option<PgPool> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db.pool)
}

async fn user(pool: &PgPool, provider: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ($1, $2, 'U') RETURNING id",
    )
    .bind(provider)
    .bind(format!("u-{}@t.test", Uuid::new_v4().simple()))
    .fetch_one(pool)
    .await
    .expect("user")
}

async fn group(pool: &PgPool, member: Option<Uuid>) -> Uuid {
    let g: Uuid =
        sqlx::query_scalar("INSERT INTO groups (name, source) VALUES ($1, 'local') RETURNING id")
            .bind(format!("g-{}", Uuid::new_v4().simple()))
            .fetch_one(pool)
            .await
            .expect("group");
    if let Some(u) = member {
        sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
            .bind(u)
            .bind(g)
            .execute(pool)
            .await
            .expect("member");
    }
    g
}

async fn role(pool: &PgPool, name: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM roles WHERE name = $1 AND scope = 'platform'")
        .bind(name)
        .fetch_one(pool)
        .await
        .expect("role")
}

async fn holds(pool: &PgPool, ptype: &str, pid: Uuid, role_id: Uuid) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM access_grants WHERE principal_type = $1 \
         AND principal_id = $2 AND role_id = $3 AND scope_type = 'platform')",
    )
    .bind(ptype)
    .bind(pid)
    .bind(role_id)
    .fetch_one(pool)
    .await
    .expect("holds")
}

#[tokio::test]
async fn platform_admin_refused_for_service_users_and_service_groups() {
    let Some(pool) = pool().await else { return };
    let admin = role(&pool, "platform_admin").await;
    let svc = user(&pool, "service").await;
    let human = user(&pool, "local").await;
    let svc_group = group(&pool, Some(svc)).await;
    let plain_group = group(&pool, Some(human)).await;

    let e = assign_platform_role_and_recompile(&pool, "user", svc, admin).await;
    assert!(matches!(e, Err(CompileError::Refused(_))), "{e:?}");
    let e = assign_platform_role_and_recompile(&pool, "group", svc_group, admin).await;
    assert!(matches!(e, Err(CompileError::Refused(_))), "{e:?}");
    assert!(!holds(&pool, "user", svc, admin).await);
    assert!(!holds(&pool, "group", svc_group, admin).await);

    assign_platform_role_and_recompile(&pool, "user", human, admin)
        .await
        .expect("human ok");
    assign_platform_role_and_recompile(&pool, "group", plain_group, admin)
        .await
        .expect("group without service member ok");
}

#[tokio::test]
async fn other_roles_may_go_to_a_service_user() {
    let Some(pool) = pool().await else { return };
    let svc = user(&pool, "service").await;
    let other: Uuid = sqlx::query_scalar(
        "SELECT id FROM roles WHERE scope = 'platform' AND name <> 'platform_admin' LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("another platform role");
    assign_platform_role_and_recompile(&pool, "user", svc, other)
        .await
        .expect("non-admin role ok");
}

#[tokio::test]
async fn unassign_platform_admin_from_a_service_user_stays_permitted() {
    let Some(pool) = pool().await else { return };
    let admin = role(&pool, "platform_admin").await;
    let svc = user(&pool, "service").await;
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'platform', NULL)",
    )
    .bind(svc)
    .bind(admin)
    .execute(&pool)
    .await
    .expect("seed holding row");
    unassign_platform_role_and_recompile(&pool, "user", svc, admin)
        .await
        .expect("revoke stays open");
    assert!(!holds(&pool, "user", svc, admin).await);
}
