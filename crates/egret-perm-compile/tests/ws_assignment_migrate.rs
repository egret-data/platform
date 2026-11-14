//! SP3c-3: workspace members -> managed `ws:<slug> members` groups.

use egret_metastore::Db;
use egret_perm_compile::ws_members_group::{sync_ws_members_group_in_tx, ws_members_group_name};
use uuid::Uuid;

/// The suite shares one database; serialise so each test's own workspace is
/// the only thing it asserts on.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn seed_user(pool: &sqlx::PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO public.users (id, provider, email, name) VALUES ($1,'local',$2,'T')")
        .bind(id)
        .bind(format!("wm_{id}@example.test"))
        .execute(pool)
        .await
        .expect("insert user");
    id
}

async fn seed_workspace(pool: &sqlx::PgPool) -> (Uuid, String) {
    let slug = format!("wm-{}", &Uuid::new_v4().simple().to_string()[..12]);
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO public.workspaces (slug, name) VALUES ($1,$1) RETURNING id",
    )
    .bind(&slug)
    .fetch_one(pool)
    .await
    .expect("insert workspace");
    (id, slug)
}

async fn add_member(pool: &sqlx::PgPool, ws: Uuid, user: Uuid) {
    sqlx::query(
        "INSERT INTO public.access_grants (principal_type, principal_id, scope_type, scope_id, effect) \
         VALUES ('user',$1,'workspace',$2,'allow')",
    )
    .bind(user)
    .bind(ws)
    .execute(pool)
    .await
    .expect("insert member grant");
}

async fn group_members(pool: &sqlx::PgPool, name: &str) -> (String, Vec<Uuid>) {
    let (gid, source): (Uuid, String) =
        sqlx::query_as("SELECT id, source FROM public.groups WHERE name = $1")
            .bind(name)
            .fetch_one(pool)
            .await
            .expect("group exists");
    let mut users: Vec<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM public.user_groups WHERE group_id = $1")
            .bind(gid)
            .fetch_all(pool)
            .await
            .expect("members");
    users.sort();
    (source, users)
}

async fn sync(pool: &sqlx::PgPool, ws: Uuid) {
    let mut tx = pool.begin().await.expect("begin");
    sync_ws_members_group_in_tx(&mut tx, ws)
        .await
        .expect("sync");
    tx.commit().await.expect("commit");
}

#[tokio::test]
async fn the_ws_members_group_mirrors_the_workspace_members() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let _g = SERIAL.lock().await;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let (ws, slug) = seed_workspace(&db.pool).await;
    let u = seed_user(&db.pool).await;
    add_member(&db.pool, ws, u).await;

    sync(&db.pool, ws).await;

    let (source, users) = group_members(&db.pool, &ws_members_group_name(&slug)).await;
    assert_eq!(source, "managed");
    assert_eq!(users, vec![u]);
}

#[tokio::test]
async fn re_syncing_reconciles_added_and_removed_members() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let _g = SERIAL.lock().await;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let (ws, slug) = seed_workspace(&db.pool).await;
    let u1 = seed_user(&db.pool).await;
    let u2 = seed_user(&db.pool).await;
    add_member(&db.pool, ws, u1).await;
    sync(&db.pool, ws).await;
    let name = ws_members_group_name(&slug);
    assert_eq!(group_members(&db.pool, &name).await.1, vec![u1]);

    // Add u2, remove u1, then sync again (twice, to prove idempotence).
    add_member(&db.pool, ws, u2).await;
    sqlx::query(
        "DELETE FROM public.access_grants WHERE principal_id = $1 AND scope_type = 'workspace' AND scope_id = $2",
    )
    .bind(u1)
    .bind(ws)
    .execute(&db.pool)
    .await
    .expect("remove member");
    sync(&db.pool, ws).await;
    sync(&db.pool, ws).await;

    assert_eq!(group_members(&db.pool, &name).await.1, vec![u2]);
}

/// A user joining or leaving a group that HOLDS the workspace changes the
/// workspace's members; the live group-membership path (`PermGroupRecompiler`)
/// must re-sync the managed `ws:<slug> members` group, in both directions.
#[tokio::test]
async fn a_group_membership_change_resyncs_the_ws_members_group_via_the_live_path() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let _g = SERIAL.lock().await;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let (ws, slug) = seed_workspace(&db.pool).await;
    let u = seed_user(&db.pool).await;
    let name = ws_members_group_name(&slug);

    // A local group that holds the workspace.
    let repo = egret_auth::groups::GroupRepo::new(db.clone());
    let holder_name = format!("wm-holder-{}", &Uuid::new_v4().simple().to_string()[..10]);
    let holder = repo
        .create_local(&holder_name, &egret_perm_compile::PermGroupRecompiler)
        .await
        .expect("create group");
    sqlx::query(
        "INSERT INTO public.access_grants (principal_type, principal_id, role_id, scope_type, scope_id, effect) \
         SELECT 'group', $1, r.id, 'workspace', $2, 'allow' FROM roles r \
          WHERE r.scope = 'workspace' AND r.name = 'ws_viewer'",
    )
    .bind(holder)
    .bind(ws)
    .execute(&db.pool)
    .await
    .expect("group holds the workspace");
    sync(&db.pool, ws).await;
    assert!(group_members(&db.pool, &name).await.1.is_empty());

    repo.add_member(
        holder,
        egret_core::UserId(u),
        &egret_perm_compile::PermGroupRecompiler,
    )
    .await
    .expect("add member");
    assert_eq!(group_members(&db.pool, &name).await.1, vec![u]);

    repo.remove_member(
        holder,
        egret_core::UserId(u),
        &egret_perm_compile::PermGroupRecompiler,
    )
    .await
    .expect("remove member");
    assert!(
        group_members(&db.pool, &name).await.1.is_empty(),
        "a user leaving the holding group must leave the managed group (fail-closed)"
    );
}
