//! DB-gated `RoleRepo` tests. Self-skip (pass) without `DATABASE_URL` so the
//! suite stays green offline; CI / `just up` provides Postgres.

use egret_auth::roles::RoleRepo;
use egret_auth::users::UserRepo;
use egret_metastore::Db;

async fn connect() -> Option<Db> {
    let url = std::env::var("DATABASE_URL").ok()?;
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
        eprintln!("DATABASE_URL unset — skipping");
        return;
    };
    let roles = RoleRepo::new(db.clone());
    let users = UserRepo::new(db.clone());

    // Create a workspace-scoped role with a description.
    let name = uniq("t_role");
    let role_id = roles
        .create("workspace", &name, "temp test role")
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
    let before = roles.count_platform_admins().await.unwrap();
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
    let after = roles.count_platform_admins().await.unwrap();
    assert!(after > before);

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

#[test]
fn seeded_role_names_are_recognized() {
    assert!(egret_auth::roles::is_seeded("platform_admin"));
    assert!(egret_auth::roles::is_seeded("ws_viewer"));
    assert!(!egret_auth::roles::is_seeded("not_a_role"));
}
