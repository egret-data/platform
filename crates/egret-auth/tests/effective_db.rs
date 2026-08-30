//! DB-gated integration tests for the AX-3 effective-access resolver. Skip (pass)
//! when DATABASE_URL is unset so the suite stays green offline; CI / `just up`
//! provides Postgres.
//!
//! Object-grant target choice: these tests use BARE uuids for `scope_id` on object
//! grants (no matching `analytics.charts` row). The resolver tolerates a missing
//! target row and yields `name: None`, which is exactly what the objects/denies
//! assertions check — so we avoid seeding a real chart and its FK chain.

use egret_auth::effective::AccessResolver;
use egret_auth::password::hash_password;
use egret_auth::users::UserRepo;
use egret_core::UserId;
use egret_metastore::Db;
use uuid::Uuid;

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn uniq(tag: &str) -> String {
    format!("{tag}-{}", Uuid::new_v4().simple())
}

async fn seed_user(db: &Db) -> UserId {
    let repo = UserRepo::new(db.clone());
    let hash = hash_password("pw").unwrap();
    repo.insert_local(&format!("{}@test.local", uniq("eff")), "E", &hash)
        .await
        .unwrap()
}

async fn seed_group(db: &Db) -> (Uuid, String) {
    let name = uniq("g");
    let id: Uuid = sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
        .bind(&name)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    (id, name)
}

async fn add_user_to_group(db: &Db, uid: UserId, gid: Uuid) {
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(uid.0)
        .bind(gid)
        .execute(&db.pool)
        .await
        .unwrap();
}

/// Create a platform-scoped role, optionally with a parent. Returns its id.
async fn seed_platform_role(db: &Db, name: &str, parent: Option<Uuid>) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO roles (scope, name, parent_role_id) VALUES ('platform', $1, $2) RETURNING id",
    )
    .bind(name)
    .bind(parent)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

async fn add_perm(db: &Db, role_id: Uuid, perm: &str) {
    sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2)")
        .bind(role_id)
        .bind(perm)
        .execute(&db.pool)
        .await
        .unwrap();
}

async fn role_id_by_name(db: &Db, scope: &str, name: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM roles WHERE scope = $1 AND name = $2")
        .bind(scope)
        .bind(name)
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

async fn seed_workspace(db: &Db) -> (Uuid, String) {
    let slug = uniq("ws");
    let id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1, $2) RETURNING id")
            .bind(&slug)
            .bind(format!("WS {slug}"))
            .fetch_one(&db.pool)
            .await
            .unwrap();
    (id, slug)
}

#[allow(clippy::too_many_arguments)]
async fn grant(
    db: &Db,
    ptype: &str,
    pid: Uuid,
    role_id: Uuid,
    scope_type: &str,
    scope_id: Option<Uuid>,
    object_type: Option<&str>,
    effect: &str,
) {
    sqlx::query(
        "INSERT INTO access_grants \
         (principal_type, principal_id, role_id, scope_type, scope_id, object_type, effect) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(ptype)
    .bind(pid)
    .bind(role_id)
    .bind(scope_type)
    .bind(scope_id)
    .bind(object_type)
    .bind(effect)
    .execute(&db.pool)
    .await
    .unwrap();
}

// 1. Direct + group platform roles, each tagged with the correct source, and both
//    roles' permissions surface.
#[tokio::test]
async fn resolves_direct_and_group_roles_with_source() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let r1 = uniq("r1");
    let r2 = uniq("r2");
    let r1_id = seed_platform_role(&db, &r1, None).await;
    let r2_id = seed_platform_role(&db, &r2, None).await;
    add_perm(&db, r1_id, "a:read").await;
    add_perm(&db, r2_id, "b:read").await;

    let uid = seed_user(&db).await;
    let (gid, gname) = seed_group(&db).await;
    add_user_to_group(&db, uid, gid).await;

    // R1 direct to the user; R2 to the group.
    grant(&db, "user", uid.0, r1_id, "platform", None, None, "allow").await;
    grant(&db, "group", gid, r2_id, "platform", None, None, "allow").await;

    let eff = AccessResolver::new(db.clone())
        .resolve("user", uid.0)
        .await
        .unwrap();

    let r1_src = eff
        .platform_roles
        .iter()
        .find(|rs| rs.name == r1)
        .expect("R1 present");
    assert_eq!(r1_src.source, "direct");
    let r2_src = eff
        .platform_roles
        .iter()
        .find(|rs| rs.name == r2)
        .expect("R2 present");
    assert_eq!(r2_src.source, format!("group:{gname}"));

    let keys: Vec<&str> = eff.permissions.iter().map(|p| p.key.as_str()).collect();
    assert!(keys.contains(&"a:read"), "R1 perm present: {keys:?}");
    assert!(keys.contains(&"b:read"), "R2 perm present: {keys:?}");
}

// 2. Permission provenance: a child role's own perm has inherited_from=None; a perm
//    from its parent has inherited_from=Some(parentName).
#[tokio::test]
async fn permission_provenance_marks_inherited() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let parent = uniq("parent");
    let child = uniq("child");
    let parent_id = seed_platform_role(&db, &parent, None).await;
    let child_id = seed_platform_role(&db, &child, Some(parent_id)).await;
    add_perm(&db, parent_id, "x:read").await;
    add_perm(&db, child_id, "y:read").await;

    let uid = seed_user(&db).await;
    grant(
        &db, "user", uid.0, child_id, "platform", None, None, "allow",
    )
    .await;

    let eff = AccessResolver::new(db.clone())
        .resolve("user", uid.0)
        .await
        .unwrap();

    let y = eff
        .permissions
        .iter()
        .find(|p| p.key == "y:read")
        .expect("y:read present");
    assert!(
        y.via
            .iter()
            .any(|v| v.role == child && v.inherited_from.is_none()),
        "y:read is child's own perm: {:?}",
        y.via
    );

    let x = eff
        .permissions
        .iter()
        .find(|p| p.key == "x:read")
        .expect("x:read present");
    assert!(
        x.via
            .iter()
            .any(|v| v.role == child && v.inherited_from.as_deref() == Some(parent.as_str())),
        "x:read inherited by child from parent: {:?}",
        x.via
    );
}

// 3. Workspace memberships: one direct, one via a group; both resolve with the right
//    slug/role/source.
#[tokio::test]
async fn resolves_workspace_memberships() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let (ws_a, slug_a) = seed_workspace(&db).await;
    let (ws_b, slug_b) = seed_workspace(&db).await;
    let editor = role_id_by_name(&db, "workspace", "ws_editor").await;
    let viewer = role_id_by_name(&db, "workspace", "ws_viewer").await;

    let uid = seed_user(&db).await;
    let (gid, gname) = seed_group(&db).await;
    add_user_to_group(&db, uid, gid).await;

    // A: ws_editor direct; B: ws_viewer via the group.
    grant(
        &db,
        "user",
        uid.0,
        editor,
        "workspace",
        Some(ws_a),
        None,
        "allow",
    )
    .await;
    grant(
        &db,
        "group",
        gid,
        viewer,
        "workspace",
        Some(ws_b),
        None,
        "allow",
    )
    .await;

    let eff = AccessResolver::new(db.clone())
        .resolve("user", uid.0)
        .await
        .unwrap();

    let a = eff
        .workspaces
        .iter()
        .find(|w| w.workspace_slug == slug_a)
        .expect("workspace A present");
    assert_eq!(a.role, "ws_editor");
    assert_eq!(a.source, "direct");

    let b = eff
        .workspaces
        .iter()
        .find(|w| w.workspace_slug == slug_b)
        .expect("workspace B present");
    assert_eq!(b.role, "ws_viewer");
    assert_eq!(b.source, format!("group:{gname}"));
}

// 4. Object grants: an allow object grant appears in `objects`; a deny object grant
//    appears in `denies` with scope "object". Bare uuids → name is None.
#[tokio::test]
async fn resolves_objects_and_denies() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let viewer = role_id_by_name(&db, "workspace", "ws_viewer").await;
    let uid = seed_user(&db).await;

    let allow_chart = Uuid::new_v4();
    let deny_chart = Uuid::new_v4();
    grant(
        &db,
        "user",
        uid.0,
        viewer,
        "object",
        Some(allow_chart),
        Some("chart"),
        "allow",
    )
    .await;
    grant(
        &db,
        "user",
        uid.0,
        viewer,
        "object",
        Some(deny_chart),
        Some("chart"),
        "deny",
    )
    .await;

    let eff = AccessResolver::new(db.clone())
        .resolve("user", uid.0)
        .await
        .unwrap();

    let allow = eff
        .objects
        .iter()
        .find(|o| o.id == allow_chart.to_string())
        .expect("allow object grant present");
    assert_eq!(allow.kind, "chart");
    assert_eq!(allow.effect, "allow");
    assert!(allow.name.is_none(), "bare-uuid target has no name");
    assert_eq!(allow.role, "ws_viewer");

    let deny = eff
        .denies
        .iter()
        .find(|d| d.target == format!("chart:{deny_chart}"))
        .expect("deny entry present");
    assert_eq!(deny.scope, "object");
    assert_eq!(deny.role, "ws_viewer");
}

// 5. (a) A GROUP principal holding a role directly resolves with source "direct" and
//    no "group:" layer. (b) A user granted the seeded platform_admin is a superuser:
//    is_platform_admin=true and permissions empty.
#[tokio::test]
async fn group_principal_and_platform_admin() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };

    // (a) Group principal.
    let (gid, _gname) = seed_group(&db).await;
    let gr = uniq("gr");
    let gr_id = seed_platform_role(&db, &gr, None).await;
    add_perm(&db, gr_id, "z:read").await;
    grant(&db, "group", gid, gr_id, "platform", None, None, "allow").await;

    let eff = AccessResolver::new(db.clone())
        .resolve("group", gid)
        .await
        .unwrap();
    assert_eq!(eff.principal.r#type, "group");
    let rs = eff
        .platform_roles
        .iter()
        .find(|r| r.name == gr)
        .expect("group role present");
    assert_eq!(rs.source, "direct");
    assert!(
        eff.platform_roles.iter().all(|r| r.source == "direct"),
        "group resolution adds no via-group layer"
    );

    // (b) Platform-admin user.
    let admin_id = role_id_by_name(&db, "platform", "platform_admin").await;
    let uid = seed_user(&db).await;
    grant(
        &db, "user", uid.0, admin_id, "platform", None, None, "allow",
    )
    .await;

    let eff = AccessResolver::new(db.clone())
        .resolve("user", uid.0)
        .await
        .unwrap();
    assert!(eff.principal.is_platform_admin, "superuser flag set");
    assert!(
        eff.permissions.is_empty(),
        "platform_admin enumerates no permissions"
    );
}
