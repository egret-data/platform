//! KI-322 and KI-323: the platform-role reads that the engine-admin set, the
//! last-admin guard and the session use honour a grant row's `expires_at` and
//! `effect` the way the PDP and the console's access resolver do — a lapsed
//! grant counts for nothing, and a `deny` grant never confers the role.
//!
//! No API writes such rows today (both KIs are latent), so the rows are
//! inserted directly. DB-gated; self-skips without `DATABASE_URL` ONLY under
//! `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.

use egret_auth::session::SessionService;
use egret_core::UserId;
use egret_metastore::Db;
use uuid::Uuid;

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

async fn active_user(db: &Db) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ('local', $1, 'T') RETURNING id",
    )
    .bind(format!("{}@test.local", Uuid::new_v4()))
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

async fn group_with(db: &Db, member: Uuid) -> Uuid {
    let gid: Uuid =
        sqlx::query_scalar("INSERT INTO groups (name, source) VALUES ($1, 'local') RETURNING id")
            .bind(format!("ki322-{}", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(member)
        .bind(gid)
        .execute(&db.pool)
        .await
        .unwrap();
    gid
}

/// A platform-scope grant of `role` to `(ptype, pid)` with the given effect,
/// expiring `expires_in_secs` from now (negative: already lapsed; `None`:
/// never).
async fn platform_grant(
    db: &Db,
    ptype: &str,
    pid: Uuid,
    role: &str,
    effect: &str,
    expires_in_secs: Option<i64>,
) {
    sqlx::query(
        "INSERT INTO access_grants \
           (principal_type, principal_id, role_id, scope_type, scope_id, effect, expires_at) \
         SELECT $1, $2, id, 'platform', NULL, $4, \
                CASE WHEN $5::bigint IS NULL THEN NULL \
                     ELSE now() + make_interval(secs => $5::bigint) END \
           FROM roles WHERE scope = 'platform' AND name = $3",
    )
    .bind(ptype)
    .bind(pid)
    .bind(role)
    .bind(effect)
    .bind(expires_in_secs)
    .execute(&db.pool)
    .await
    .unwrap();
}

async fn engine_admins(db: &Db) -> Vec<Uuid> {
    let mut conn = db.pool.acquire().await.unwrap();
    egret_auth::roles::engine_admin_user_ids_via(&mut conn)
        .await
        .unwrap()
        .into_iter()
        .map(|u| u.0)
        .collect()
}

/// KI-322: a lapsed `platform_admin` grant, direct or through a group, does not
/// make an engine admin; a live one does (the control, so the set is read at
/// all). KI-323's half for the same set: a `deny` grant does not either.
#[tokio::test]
async fn the_engine_admin_set_counts_only_live_allow_grants() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    // Arrange
    let live = active_user(&db).await;
    platform_grant(&db, "user", live, "platform_admin", "allow", Some(3600)).await;
    let lapsed_direct = active_user(&db).await;
    platform_grant(
        &db,
        "user",
        lapsed_direct,
        "platform_admin",
        "allow",
        Some(-60),
    )
    .await;
    let lapsed_via_group = active_user(&db).await;
    let g = group_with(&db, lapsed_via_group).await;
    platform_grant(&db, "group", g, "platform_admin", "allow", Some(-60)).await;
    let denied = active_user(&db).await;
    platform_grant(&db, "user", denied, "platform_admin", "deny", None).await;

    // Act
    let admins = engine_admins(&db).await;

    // Assert
    assert!(admins.contains(&live), "a live grant makes an engine admin");
    assert!(
        !admins.contains(&lapsed_direct),
        "a lapsed direct platform_admin grant must not keep engine ALL access"
    );
    assert!(
        !admins.contains(&lapsed_via_group),
        "a lapsed group platform_admin grant must not keep engine ALL access"
    );
    assert!(
        !admins.contains(&denied),
        "a deny platform_admin grant must not make an engine admin"
    );
}

/// KI-323: a platform-scope `deny` grant of a role does not put the role in
/// the session's platform roles (nor its permissions in the session's
/// permission list), directly or through a group; an `allow` grant of another
/// role still does (the control).
#[tokio::test]
async fn a_deny_grant_gives_the_session_no_platform_role() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    // Arrange
    let user = active_user(&db).await;
    platform_grant(&db, "user", user, "platform_admin", "deny", None).await;
    let g = group_with(&db, user).await;
    platform_grant(&db, "group", g, "data_steward", "deny", None).await;
    let other: String = sqlx::query_scalar(
        "INSERT INTO roles (scope, name, description) VALUES ('platform', $1, 'KI-323 control') \
         RETURNING name",
    )
    .bind(format!("ki323_{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    platform_grant(&db, "user", user, &other, "allow", None).await;
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(1),
        chrono::Duration::minutes(30),
    );

    // Act
    let roles = sessions.platform_roles_of(UserId(user)).await.unwrap();
    let token = sessions
        .create(UserId(user), "test", "127.0.0.1")
        .await
        .unwrap()
        .token;
    let p = sessions
        .validate(&token)
        .await
        .unwrap()
        .expect("live session");

    // Assert
    assert_eq!(roles, vec![other.clone()], "only the allow grant");
    assert_eq!(p.platform_roles, vec![other]);
    assert!(
        !p.is_platform_admin(),
        "a deny of platform_admin is not admin"
    );
    assert!(
        !p.permissions.iter().any(|k| k.starts_with("policy:")),
        "a denied data_steward lends no policy:* permission: {:?}",
        p.permissions
    );
}

/// A workspace-scope grant of `role` on `ws` to `(ptype, pid)`, with the given
/// effect and expiry (as [`platform_grant`]).
async fn workspace_grant(
    db: &Db,
    ptype: &str,
    pid: Uuid,
    ws: Uuid,
    role: &str,
    effect: &str,
    expires_in_secs: Option<i64>,
) {
    sqlx::query(
        "INSERT INTO access_grants \
           (principal_type, principal_id, role_id, scope_type, scope_id, effect, expires_at) \
         SELECT $1, $2, id, 'workspace', $6, $4, \
                CASE WHEN $5::bigint IS NULL THEN NULL \
                     ELSE now() + make_interval(secs => $5::bigint) END \
           FROM roles WHERE scope = 'workspace' AND name = $3",
    )
    .bind(ptype)
    .bind(pid)
    .bind(role)
    .bind(effect)
    .bind(expires_in_secs)
    .bind(ws)
    .execute(&db.pool)
    .await
    .unwrap();
}

fn sessions(db: &Db) -> SessionService {
    SessionService::new(
        db.clone(),
        chrono::Duration::hours(1),
        chrono::Duration::minutes(30),
    )
}

/// P5b row 2, the workspace form of KI-323: a `deny` `ws_admin` grant, direct
/// or through a group, is no `ws_admin` for the OIDC step-up re-check
/// (`ws_admin_of`) and no workspace role for the impersonation-target rule
/// (`workspace_roles_of`). A lapsed grant is neither (already true on main;
/// kept here so the rule is pinned in one place). A live allow grant is (the
/// control).
#[tokio::test]
async fn a_deny_or_lapsed_workspace_grant_gives_no_workspace_role() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    // Arrange
    let ws = Uuid::new_v4();
    let denied = active_user(&db).await;
    workspace_grant(&db, "user", denied, ws, "ws_admin", "deny", None).await;
    let denied_via_group = active_user(&db).await;
    let g = group_with(&db, denied_via_group).await;
    workspace_grant(&db, "group", g, ws, "ws_admin", "deny", None).await;
    let lapsed = active_user(&db).await;
    workspace_grant(&db, "user", lapsed, ws, "ws_admin", "allow", Some(-60)).await;
    let live = active_user(&db).await;
    workspace_grant(&db, "user", live, ws, "ws_admin", "allow", None).await;
    let s = sessions(&db);
    let w = egret_core::WorkspaceId(ws);

    // Act + Assert
    for (who, user) in [
        ("a direct deny", denied),
        ("a deny through a group", denied_via_group),
        ("a lapsed grant", lapsed),
    ] {
        assert!(
            !s.ws_admin_of(UserId(user), w).await.unwrap(),
            "{who} of ws_admin is no ws_admin"
        );
        assert_eq!(
            s.workspace_roles_of(UserId(user), w).await.unwrap(),
            Vec::<String>::new(),
            "{who} of ws_admin is no workspace role"
        );
    }
    assert!(s.ws_admin_of(UserId(live), w).await.unwrap());
    assert_eq!(
        s.workspace_roles_of(UserId(live), w).await.unwrap(),
        vec!["ws_admin".to_string()]
    );
}

/// P5b row 3: `RoleRepo::platform_admin_user_ids` lists only holders of a live
/// `allow` `platform_admin` grant, direct or through a group, like the
/// engine-admin set (KI-322) and the session (KI-323).
#[tokio::test]
async fn platform_admin_user_ids_lists_only_live_allow_grants() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    // Arrange
    let live = active_user(&db).await;
    platform_grant(&db, "user", live, "platform_admin", "allow", None).await;
    let lapsed = active_user(&db).await;
    platform_grant(&db, "user", lapsed, "platform_admin", "allow", Some(-60)).await;
    let lapsed_via_group = active_user(&db).await;
    let g = group_with(&db, lapsed_via_group).await;
    platform_grant(&db, "group", g, "platform_admin", "allow", Some(-60)).await;
    let denied = active_user(&db).await;
    platform_grant(&db, "user", denied, "platform_admin", "deny", None).await;
    let denied_via_group = active_user(&db).await;
    let g2 = group_with(&db, denied_via_group).await;
    platform_grant(&db, "group", g2, "platform_admin", "deny", None).await;

    // Act
    let ids: Vec<Uuid> =
        egret_auth::roles::RoleRepo::new(db.clone(), egret_events::EventBus::new())
            .platform_admin_user_ids()
            .await
            .unwrap()
            .into_iter()
            .map(|u| u.0)
            .collect();

    // Assert
    assert!(ids.contains(&live), "a live allow grant is an admin");
    for (who, user) in [
        ("a lapsed direct grant", lapsed),
        ("a lapsed group grant", lapsed_via_group),
        ("a direct deny", denied),
        ("a deny through a group", denied_via_group),
    ] {
        assert!(!ids.contains(&user), "{who} is no platform admin");
    }
}

/// P5b sweep: the console's per-user platform roles
/// (`RoleRepo::effective_platform_roles_of_users`, KI-260) are the session's
/// roles, so a `deny` grant, direct or through a group, lists no role there
/// either; an `allow` grant does (the control).
#[tokio::test]
async fn the_consoles_effective_platform_roles_skip_deny_grants() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    // Arrange
    let user = active_user(&db).await;
    platform_grant(&db, "user", user, "platform_admin", "deny", None).await;
    let g = group_with(&db, user).await;
    platform_grant(&db, "group", g, "platform_admin", "deny", None).await;
    platform_grant(&db, "user", user, "data_steward", "allow", None).await;

    // Act
    let rows = egret_auth::roles::RoleRepo::new(db.clone(), egret_events::EventBus::new())
        .effective_platform_roles_of_users(&[user])
        .await
        .unwrap();

    // Assert
    let names: Vec<&str> = rows.iter().map(|(_, n, _)| n.as_str()).collect();
    assert_eq!(names, vec!["data_steward"], "only the allow grant: {rows:?}");
}
