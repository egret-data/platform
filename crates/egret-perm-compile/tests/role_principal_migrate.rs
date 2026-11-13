//! SP3c-1: role principals -> managed `role:<name> holders` groups.

use egret_metastore::Db;
use uuid::Uuid;

#[tokio::test]
async fn a_managed_group_source_is_accepted() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let id = Uuid::new_v4();
    let r = sqlx::query("INSERT INTO public.groups (id, name, source) VALUES ($1,$2,'managed')")
        .bind(id)
        .bind(format!("role:data_steward holders {id}"))
        .execute(&db.pool)
        .await;
    assert!(r.is_ok(), "managed source must be accepted: {r:?}");
}

use egret_perm_compile::role_holder_group::{role_holder_group_name, sync_role_holder_group_in_tx};

async fn seed_user(pool: &sqlx::PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO public.users (id, provider, email, name) VALUES ($1,'local',$2,'T')")
        .bind(id)
        .bind(format!("rh_{id}@example.test"))
        .execute(pool)
        .await
        .expect("insert user");
    id
}

async fn grant_role(pool: &sqlx::PgPool, role_id: Uuid, user: Uuid) {
    sqlx::query(
        "INSERT INTO public.access_grants (principal_type, principal_id, role_id, scope_type) \
         VALUES ('user',$1,$2,'platform')",
    )
    .bind(user)
    .bind(role_id)
    .execute(pool)
    .await
    .expect("insert grant");
}

/// A fresh platform role with one direct user holder: (role name, role id, user id).
async fn seed_role_with_one_user_holder(pool: &sqlx::PgPool) -> (String, Uuid, Uuid) {
    let role_id = Uuid::new_v4();
    let role = format!("rh_{}", &role_id.simple().to_string()[..12]);
    sqlx::query("INSERT INTO public.roles (id, scope, name) VALUES ($1,'platform',$2)")
        .bind(role_id)
        .bind(&role)
        .execute(pool)
        .await
        .expect("insert role");
    let u1 = seed_user(pool).await;
    grant_role(pool, role_id, u1).await;
    (role, role_id, u1)
}

/// Revoke `u1`'s grant and grant the role to a new user; returns the new user.
async fn revoke_u1_grant_u2(pool: &sqlx::PgPool, role_id: Uuid, u1: Uuid) -> Uuid {
    sqlx::query("DELETE FROM public.access_grants WHERE principal_id=$1 AND role_id=$2")
        .bind(u1)
        .bind(role_id)
        .execute(pool)
        .await
        .expect("revoke");
    let u2 = seed_user(pool).await;
    grant_role(pool, role_id, u2).await;
    u2
}

#[tokio::test]
async fn the_holder_group_mirrors_the_roles_holders() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let (role, _role_id, user_id) = seed_role_with_one_user_holder(&pool).await;

    let mut tx = pool.begin().await.unwrap();
    let gid = sync_role_holder_group_in_tx(&mut tx, &role)
        .await
        .expect("sync");
    tx.commit().await.unwrap();

    let (name, source): (String, String) =
        sqlx::query_as("SELECT name, source FROM public.groups WHERE id=$1")
            .bind(gid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(name, role_holder_group_name(&role));
    assert_eq!(source, "managed");
    let members: Vec<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM public.user_groups WHERE group_id=$1")
            .bind(gid)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(members, vec![user_id]);
}

#[tokio::test]
async fn re_syncing_reconciles_added_and_removed_holders() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let (role, role_id, u1) = seed_role_with_one_user_holder(&pool).await;

    let mut tx = pool.begin().await.unwrap();
    let gid = sync_role_holder_group_in_tx(&mut tx, &role).await.unwrap();
    tx.commit().await.unwrap();

    let u2 = revoke_u1_grant_u2(&pool, role_id, u1).await;
    let mut tx = pool.begin().await.unwrap();
    let gid2 = sync_role_holder_group_in_tx(&mut tx, &role).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(gid, gid2, "idempotent: same group");
    let members: Vec<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM public.user_groups WHERE group_id=$1")
            .bind(gid)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(members, vec![u2]);
}

#[tokio::test]
async fn a_holder_change_resyncs_the_managed_group_via_the_chokepoint() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let (role, role_id, _u1) = seed_role_with_one_user_holder(&pool).await;

    let mut tx = pool.begin().await.unwrap();
    sync_role_holder_group_in_tx(&mut tx, &role).await.unwrap();
    tx.commit().await.unwrap();

    let u2 = seed_user(&pool).await;
    grant_role(&pool, role_id, u2).await;
    egret_perm_compile::role_membership::recompile_role_referencing_profiles(&pool)
        .await
        .unwrap();

    let present: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM public.user_groups ug \
         JOIN public.groups g ON g.id = ug.group_id \
         WHERE g.name = $1 AND ug.user_id = $2)",
    )
    .bind(role_holder_group_name(&role))
    .bind(u2)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(present, "chokepoint must resync the holder group");
}
