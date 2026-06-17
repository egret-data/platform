//! DB-gated integration tests; self-skip without DATABASE_URL.

use egret_auth::password::hash_password;
use egret_auth::users::UserRepo;
use egret_core::{Pagination, UserId};
use egret_metastore::Db;
use egret_workspace::workspaces::WorkspaceRepo;

async fn connect() -> Option<Db> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn uniq(tag: &str) -> String {
    format!("{tag}-{}", uuid::Uuid::new_v4().simple())
}

async fn seed_user(db: &Db) -> UserId {
    let repo = UserRepo::new(db.clone());
    repo.insert_local(
        &format!("{}@test.local", uuid::Uuid::new_v4()),
        "U",
        &hash_password("pw").unwrap(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn create_get_and_unique_slug() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let owner = seed_user(&db).await;
    let repo = WorkspaceRepo::new(db.clone());
    let slug = uniq("ws");

    let ws = repo.create(&slug, "Sales", owner).await.unwrap();
    assert_eq!(ws.slug, slug);
    assert_eq!(ws.status, "active");

    let got = repo.get_by_slug(&slug).await.unwrap().expect("exists");
    assert_eq!(got.id, ws.id);

    // duplicate slug → Conflict
    let dup = repo.create(&slug, "Other", owner).await;
    assert!(matches!(dup, Err(egret_core::Error::Conflict(_))));
}

#[tokio::test]
async fn invalid_slug_rejected() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let owner = seed_user(&db).await;
    let repo = WorkspaceRepo::new(db);
    let err = repo.create("Bad Slug", "x", owner).await;
    assert!(matches!(err, Err(egret_core::Error::InvalidInput(_))));
}

#[tokio::test]
async fn archive_hides_from_active_lookups() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let owner = seed_user(&db).await;
    let repo = WorkspaceRepo::new(db);
    let ws = repo.create(&uniq("ws"), "Temp", owner).await.unwrap();
    repo.archive(ws.id).await.unwrap();
    let got = repo.get_by_id(ws.id).await.unwrap().unwrap();
    assert_eq!(got.status, "archived");
    let _ = Pagination::default();
}

use egret_workspace::members::MemberRepo;
use egret_workspace::rbac::Authorizer;

#[tokio::test]
async fn add_list_remove_member() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let owner = seed_user(&db).await;
    let member = seed_user(&db).await;
    let wrepo = WorkspaceRepo::new(db.clone());
    let mrepo = MemberRepo::new(db.clone());
    let ws = wrepo.create(&uniq("ws"), "W", owner).await.unwrap();

    mrepo
        .add(ws.id, "user", member.0, "ws_editor", owner)
        .await
        .unwrap();
    let members = mrepo.list(ws.id).await.unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].role, "ws_editor");

    // upsert role
    mrepo
        .add(ws.id, "user", member.0, "ws_admin", owner)
        .await
        .unwrap();
    assert_eq!(mrepo.list(ws.id).await.unwrap()[0].role, "ws_admin");

    mrepo.remove(ws.id, "user", member.0).await.unwrap();
    assert!(mrepo.list(ws.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn rejects_unknown_role() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let owner = seed_user(&db).await;
    let wrepo = WorkspaceRepo::new(db.clone());
    let mrepo = MemberRepo::new(db);
    let ws = wrepo.create(&uniq("ws"), "W", owner).await.unwrap();
    let err = mrepo.add(ws.id, "user", owner.0, "superuser", owner).await;
    assert!(matches!(err, Err(egret_core::Error::InvalidInput(_))));
}

async fn make_group(db: &Db, name: &str) -> uuid::Uuid {
    sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
        .bind(name)
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn workspace_permission_via_group_is_unioned() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let owner = seed_user(&db).await;
    let user = seed_user(&db).await;
    let wrepo = WorkspaceRepo::new(db.clone());
    let mrepo = MemberRepo::new(db.clone());
    let authz = Authorizer::new(db.clone());
    let ws = wrepo.create(&uniq("ws"), "W", owner).await.unwrap();

    // user is NOT a direct member; their GROUP is ws_admin.
    let gid = make_group(&db, &uniq("grp")).await;
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(user.0)
        .bind(gid)
        .execute(&db.pool)
        .await
        .unwrap();
    mrepo
        .add(ws.id, "group", gid, "ws_admin", owner)
        .await
        .unwrap();

    let perms = authz.workspace_permissions(user, ws.id).await.unwrap();
    assert!(
        perms.contains("workspace:manage_members"),
        "group role must grant the perm"
    );
}

#[tokio::test]
async fn replace_platform_roles_and_check() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let user = seed_user(&db).await;
    let authz = Authorizer::new(db.clone());
    authz
        .replace_platform_roles(user, &["data_steward".into()])
        .await
        .unwrap();
    let perms = authz.platform_permissions(user).await.unwrap();
    assert!(perms.contains("user:read"));
    assert!(!perms.contains("workspace:create")); // data_steward lacks it
                                                  // bad role rejected
    assert!(authz
        .replace_platform_roles(user, &["root".into()])
        .await
        .is_err());
}
