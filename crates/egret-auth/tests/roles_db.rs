//! DB-gated `RoleRepo` tests. Self-skips without `DATABASE_URL` ONLY under
//! `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS; CI / `just up` provides Postgres.

use egret_auth::groups::GroupRepo;
use egret_auth::roles::RoleRepo;
use egret_auth::users::UserRepo;
use egret_metastore::Db;

#[path = "support/recorder.rs"]
mod recorder;

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn uniq(tag: &str) -> String {
    format!("{tag}-{}", uuid::Uuid::new_v4().simple())
}

#[tokio::test]
async fn full_role_lifecycle() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let roles = RoleRepo::new(db.clone(), egret_events::EventBus::new());
    let users = UserRepo::new(db.clone());

    // Create a workspace-scoped role with a description.
    let name = uniq("t_role");
    let role_id = roles
        .create("workspace", &name, "temp test role", None)
        .await
        .unwrap();

    let fetched = roles.get(role_id).await.unwrap().expect("role exists");
    assert_eq!(fetched.name, name);
    assert_eq!(fetched.scope, "workspace");
    assert_eq!(fetched.description, "temp test role");

    // It should show up in `list()`.
    assert!(roles.list().await.unwrap().iter().any(|r| r.id == role_id));

    // set_permissions replaces the full set.
    roles
        .set_permissions(
            role_id,
            &["engine:read".to_string(), "engine:create".to_string()],
        )
        .await
        .unwrap();
    let mut perms = roles.permissions_of(role_id).await.unwrap();
    perms.sort();
    assert_eq!(perms, vec!["engine:create", "engine:read"]);

    // Replacing again drops the old set (not additive).
    roles
        .set_permissions(role_id, &["engine:read".to_string()])
        .await
        .unwrap();
    assert_eq!(
        roles.permissions_of(role_id).await.unwrap(),
        vec!["engine:read".to_string()]
    );

    // Assign to a user and a group.
    let user_id = users
        .insert_local(&format!("{}@test.local", uniq("user")), "Tester", "x")
        .await
        .unwrap();
    let group_id: uuid::Uuid =
        sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
            .bind(uniq("group"))
            .fetch_one(&db.pool)
            .await
            .unwrap();

    roles.assign("user", user_id.0, role_id).await.unwrap();
    roles.assign("group", group_id, role_id).await.unwrap();
    // Idempotent: re-assigning does not error or duplicate.
    roles.assign("user", user_id.0, role_id).await.unwrap();

    let mut assignees = roles.assignees_of(role_id).await.unwrap();
    assignees.sort();
    let mut expected = vec![
        ("group".to_string(), group_id),
        ("user".to_string(), user_id.0),
    ];
    expected.sort();
    assert_eq!(assignees, expected);

    let direct = roles.roles_of("user", user_id.0).await.unwrap();
    assert!(direct.iter().any(|r| r.id == role_id));

    // count_platform_admins: assign platform_admin to a temp user and check it counts.
    //
    // Asserted against THIS user, not as a global before/after delta. The two
    // sibling tests in this binary each grant AND revoke platform_admin, and
    // `#[tokio::test]` functions run concurrently, so a revoke landing between
    // the two reads made `after > before` false. That failed only under
    // `cargo test --workspace` — where the extra load changes the interleaving
    // — and passed under `-p egret-auth`, which is the shape of a bug that
    // looks like it belongs to whatever change happened to be in flight.
    let admin_role = roles
        .list()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.name == "platform_admin")
        .expect("platform_admin seeded by migration 0003");
    roles
        .assign("user", user_id.0, admin_role.id)
        .await
        .unwrap();
    assert!(
        roles
            .platform_admin_user_ids()
            .await
            .unwrap()
            .contains(&user_id),
        "the granted user must be counted as a platform admin"
    );
    // The counting query itself still has to work; it just cannot be asserted
    // as an exact delta from a concurrently-mutated global.
    assert!(roles.count_platform_admins().await.unwrap() >= 1);

    // Update description.
    roles
        .update_description(role_id, "updated description")
        .await
        .unwrap();
    assert_eq!(
        roles.get(role_id).await.unwrap().unwrap().description,
        "updated description"
    );

    // Unassign then delete; both are idempotent/cascading.
    roles.unassign("user", user_id.0, role_id).await.unwrap();
    roles.unassign("group", group_id, role_id).await.unwrap();
    assert!(roles.assignees_of(role_id).await.unwrap().is_empty());

    roles
        .unassign("user", user_id.0, admin_role.id)
        .await
        .unwrap();

    roles.delete(role_id).await.unwrap();
    assert!(roles.get(role_id).await.unwrap().is_none());
}

#[tokio::test]
async fn assign_writes_platform_access_grant() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let repo = RoleRepo::new(db.clone(), egret_events::EventBus::new());
    let uid = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, provider, email, name, status) VALUES ($1,'local',$2,'T','active')",
    )
    .bind(uid)
    .bind(format!("flip-{uid}@t.local"))
    .execute(&db.pool)
    .await
    .unwrap();
    let role: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM roles WHERE scope='platform' AND name='data_steward'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    repo.assign("user", uid, role).await.unwrap();
    // A platform-scope access_grant now exists (not via a legacy trigger — direct).
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM access_grants WHERE principal_type='user' AND principal_id=$1 AND role_id=$2 AND scope_type='platform'")
        .bind(uid).bind(role).fetch_one(&db.pool).await.unwrap();
    assert_eq!(n, 1, "assign must write a platform access_grant");
    repo.unassign("user", uid, role).await.unwrap();
    let n2: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM access_grants WHERE principal_type='user' AND principal_id=$1 AND role_id=$2 AND scope_type='platform'")
        .bind(uid).bind(role).fetch_one(&db.pool).await.unwrap();
    assert_eq!(n2, 0, "unassign must remove the platform access_grant");
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(uid)
        .execute(&db.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn platform_admin_user_ids_lists_direct_user_grants() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let roles = RoleRepo::new(db.clone(), egret_events::EventBus::new());
    let users = UserRepo::new(db.clone());

    // Seed a fresh user and grant them platform_admin at platform scope.
    let user_id = users
        .insert_local(&format!("{}@test.local", uniq("admin")), "Admin", "x")
        .await
        .unwrap();
    let admin_role = roles
        .list()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.name == "platform_admin")
        .expect("platform_admin seeded by migration 0003");
    roles
        .assign("user", user_id.0, admin_role.id)
        .await
        .unwrap();

    let ids = roles.platform_admin_user_ids().await.unwrap();
    assert!(
        ids.contains(&user_id),
        "platform_admin_user_ids must include the granted user"
    );

    // Cleanup.
    roles
        .unassign("user", user_id.0, admin_role.id)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(user_id.0)
        .execute(&db.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn platform_admin_user_ids_expands_group_granted_admins() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let roles = RoleRepo::new(db.clone(), egret_events::EventBus::new());
    let users = UserRepo::new(db.clone());
    let groups = GroupRepo::new(db.clone());

    // A user who holds platform_admin ONLY via group membership (not directly).
    let user_id = users
        .insert_local(&format!("{}@test.local", uniq("gadmin")), "GAdmin", "x")
        .await
        .unwrap();
    let group_id = groups.create_local(&uniq("admins-grp")).await.unwrap();
    groups
        .add_member(group_id, user_id, &crate::recorder::Recorder::default())
        .await
        .unwrap();
    let admin_role = roles
        .list()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.name == "platform_admin")
        .expect("platform_admin seeded");
    // Grant platform_admin to the GROUP, not the user.
    roles
        .assign("group", group_id, admin_role.id)
        .await
        .unwrap();

    let ids = roles.platform_admin_user_ids().await.unwrap();
    assert!(
        ids.contains(&user_id),
        "a group-granted platform admin must be expanded to its member users"
    );

    // Cleanup.
    roles
        .unassign("group", group_id, admin_role.id)
        .await
        .unwrap();
    sqlx::query("DELETE FROM user_groups WHERE user_id=$1")
        .bind(user_id.0)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM groups WHERE id=$1")
        .bind(group_id)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(user_id.0)
        .execute(&db.pool)
        .await
        .unwrap();
}

#[test]
fn seeded_role_names_are_recognized() {
    assert!(egret_auth::roles::is_seeded("platform_admin"));
    assert!(egret_auth::roles::is_seeded("ws_viewer"));
    assert!(!egret_auth::roles::is_seeded("not_a_role"));
}
