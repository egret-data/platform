//! DB-gated integration tests; self-skip without DATABASE_URL.

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::password::hash_password;
use egret_auth::routes::AuthState;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_config::Config;
use egret_core::{Pagination, UserId};
use egret_metastore::Db;
use egret_workspace::routes::{
    admin_user_router, admin_workspace_router, me_router, ws_member_router, WorkspaceState,
};
use egret_workspace::workspaces::WorkspaceRepo;
use http_body_util::BodyExt;
use tower::ServiceExt;

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
async fn create_does_not_provision_legacy_catalogs_row() {
    // CE-2 T4: warehouse provisioning moved out of `WorkspaceRepo::create` (it no
    // longer writes the legacy `catalogs` table). The workspace's catalog is now a
    // lakekeeper warehouse, provisioned from the server/handler layer via the
    // `WarehouseProvisioner` hook. `create` is a plain workspace-row insert.
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let owner = seed_user(&db).await;
    let repo = WorkspaceRepo::new(db.clone());
    let slug = uniq("ws");

    let ws = repo.create(&slug, "Sales", owner).await.unwrap();
    assert_eq!(ws.slug, slug);

    // The legacy `catalogs` metadata table was dropped in migration 0026 once
    // lakekeeper became the engine, so `create` provably cannot write such a row:
    // the relation no longer exists.
    let legacy_table: Option<String> = sqlx::query_scalar("SELECT to_regclass('catalogs')::text")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(
        legacy_table.is_none(),
        "legacy `catalogs` table must no longer exist (dropped in 0026)"
    );
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
    // Verify the assignment landed (enforcement/permission-union now lives in the PDP,
    // so assert the durable `access_grants` platform-scope effect directly).
    let (assigned,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM access_grants ag \
         JOIN roles r ON r.id = ag.role_id \
         WHERE ag.principal_type = 'user' AND ag.principal_id = $1 \
           AND ag.scope_type = 'platform' AND r.name = 'data_steward'",
    )
    .bind(user.0)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(assigned, 1, "data_steward must be assigned");
    // bad role rejected
    assert!(authz
        .replace_platform_roles(user, &["root".into()])
        .await
        .is_err());
}

/// B2-2 Task 1: a user whose ONLY grant is an object grant on a notebook in the
/// workspace is admitted as an object-guest (`has_object_grant_in_workspace` →
/// true), matching the folder/dashboard/chart/dataset arms. A user with no grant
/// is NOT admitted.
#[tokio::test]
async fn notebook_object_grant_admits_guest() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let owner = seed_user(&db).await;
    let guest = seed_user(&db).await;
    let stranger = seed_user(&db).await;
    let authz = Authorizer::new(db.clone());
    let ws = WorkspaceRepo::new(db.clone())
        .create(&uniq("nb-ws"), "NB", owner)
        .await
        .unwrap();

    // A notebook living in this workspace.
    let notebook_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO analytics.notebooks (workspace_id, owner_user_id, name) VALUES ($1, $2, 'NB') RETURNING id",
    )
    .bind(ws.id.0)
    .bind(owner.0)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    // Grant the guest an object-scoped grant on the notebook (any workspace role).
    let role_id: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM roles WHERE scope = 'workspace' AND name = 'ws_viewer'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO access_grants \
           (principal_type, principal_id, role_id, scope_type, object_type, scope_id, granted_by) \
         VALUES ('user', $1, $2, 'object', 'notebook', $3, $1)",
    )
    .bind(guest.0)
    .bind(role_id)
    .bind(notebook_id)
    .execute(&db.pool)
    .await
    .unwrap();

    assert!(
        authz
            .has_object_grant_in_workspace(guest, ws.id)
            .await
            .unwrap(),
        "notebook object grant must admit the guest"
    );
    assert!(
        !authz
            .has_object_grant_in_workspace(stranger, ws.id)
            .await
            .unwrap(),
        "a user with no notebook grant must NOT be admitted"
    );
}

/// B2-flip Task 3: `MemberRepo::add` is one-role-per-member — adding a second
/// role for the same principal in the same workspace REPLACES the first, leaving
/// exactly one workspace grant (the delete-then-insert-in-a-tx invariant).
#[tokio::test]
async fn member_add_replaces_role_one_row() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let members = egret_workspace::members::MemberRepo::new(db.clone());
    let authz = egret_workspace::rbac::Authorizer::new(db.clone());
    // seed a workspace + a user + an adder
    let ws = egret_core::WorkspaceId::new();
    sqlx::query("INSERT INTO workspaces (id, slug, name) VALUES ($1,$2,'W')")
        .bind(ws.0)
        .bind(format!("flip-ws-{}", ws.0))
        .execute(&db.pool)
        .await
        .unwrap();
    let uid = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, provider, email, name, status) VALUES ($1,'local',$2,'M','active')",
    )
    .bind(uid)
    .bind(format!("flipm-{uid}@t.local"))
    .execute(&db.pool)
    .await
    .unwrap();
    let by = egret_core::UserId(uid);
    // add ws_viewer, then re-add as ws_editor → exactly one workspace grant remains, = ws_editor
    members.add(ws, "user", uid, "ws_viewer", by).await.unwrap();
    members.add(ws, "user", uid, "ws_editor", by).await.unwrap();
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM access_grants WHERE principal_type='user' AND principal_id=$1 AND scope_type='workspace' AND scope_id=$2")
        .bind(uid).bind(ws.0).fetch_one(&db.pool).await.unwrap();
    assert_eq!(n, 1, "one workspace role per member");
    let roles = authz
        .workspace_roles_for(egret_core::UserId(uid), ws)
        .await
        .unwrap();
    assert_eq!(roles, vec!["ws_editor".to_string()]);
    // cleanup
    sqlx::query("DELETE FROM access_grants WHERE principal_id=$1")
        .bind(uid)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(uid)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM workspaces WHERE id=$1")
        .bind(ws.0)
        .execute(&db.pool)
        .await
        .unwrap();
}

async fn login_cookie(db: &Db, user: UserId, sessions: &SessionService) -> String {
    let issued = sessions.create(user, "a", "").await.unwrap();
    let _ = db;
    format!("egret_session={}", issued.token)
}

/// Build a real `CedarPdp` snapshotted against `db`'s already-migrated
/// `roles`/`role_permissions` (the platform + workspace role catalog seeded by
/// migrations). The migrated `require()` gates in the workspace seam resolve
/// through `WorkspaceState.pdp`, so the 403/200 authz tests need the real PDP —
/// an allow-all stub would falsely 200 the denied cases. Role *assignments*
/// (access_grants) are resolved live at authorize time, so seeding them after
/// `build` is fine.
async fn build_pdp(db: &Db) -> std::sync::Arc<dyn egret_core::authz::Pdp> {
    std::sync::Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    )
}

async fn ws_state(db: &Db) -> WorkspaceState {
    WorkspaceState {
        workspaces: WorkspaceRepo::new(db.clone()),
        members: egret_workspace::members::MemberRepo::new(db.clone()),
        access_views: egret_workspace::access_views::AccessViewsRepo::new(db.clone()),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        pdp: build_pdp(db).await,
        users: UserRepo::new(db.clone()),
        sessions: SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        audit: egret_audit::AuditSink::new(16).0,
    }
}

#[tokio::test]
async fn create_workspace_requires_platform_permission() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let plain = seed_user(&db).await; // no platform role
    let st = ws_state(&db).await;
    let cookie = login_cookie(&db, plain, &st.sessions).await;
    let app = axum::Router::new()
        .nest("/api/admin", admin_workspace_router())
        .with_state(st);

    let body = serde_json::to_vec(&serde_json::json!({"slug": uniq("ws"), "name": "X"})).unwrap();
    let resp = app
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/api/admin/workspaces")
                .header("cookie", cookie)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn platform_admin_can_create_workspace() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let admin = seed_user(&db).await;
    let st = ws_state(&db).await;
    st.authz
        .replace_platform_roles(admin, &["platform_admin".into()])
        .await
        .unwrap();
    let cookie = login_cookie(&db, admin, &st.sessions).await;
    let app = axum::Router::new()
        .nest("/api/admin", admin_workspace_router())
        .with_state(st.clone());

    let body = serde_json::to_vec(&serde_json::json!({"slug": uniq("ws"), "name": "X"})).unwrap();
    let resp = app
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/api/admin/workspaces")
                .header("cookie", cookie)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let _ = AuthState {
        users: UserRepo::new(db.clone()),
        sessions: SessionService::new(
            db.clone(),
            chrono::Duration::hours(1),
            chrono::Duration::minutes(1),
        ),
        config: Config::default(),
        rate_limiter: egret_auth::ratelimit::RateLimiter::new(5, 300, 300),
    };
}

// ── Task 8: Admin user routes ─────────────────────────────────────────────────

#[tokio::test]
async fn assign_platform_role_then_user_gains_access() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let admin = seed_user(&db).await;
    let target = seed_user(&db).await;
    let st = ws_state(&db).await;
    st.authz
        .replace_platform_roles(admin, &["platform_admin".into()])
        .await
        .unwrap();
    let cookie = login_cookie(&db, admin, &st.sessions).await;
    let app = axum::Router::new()
        .nest("/api/admin", admin_user_router())
        .with_state(st);

    let body = serde_json::to_vec(&serde_json::json!({"roles": ["data_steward"]})).unwrap();
    let resp = app
        .oneshot(
            HttpRequest::builder()
                .method("PUT")
                .uri(format!("/api/admin/users/{}/platform-roles", target.0))
                .header("cookie", cookie)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

/// Regression for the privilege-escalation this branch closes: `PUT
/// /users/{id}/platform-roles` replaces a user's platform role assignments
/// outright, so it must be `platform_admin`-only — not delegable via a
/// granular permission. Before the fix, a non-admin holding `user:assign_role`
/// (grantable through `role:assign_permission`, itself delegatable) could
/// call this route to assign themselves `platform_admin` directly, bypassing
/// `role_permissions` entirely.
#[tokio::test]
async fn set_platform_roles_rejects_delegated_permission_not_platform_admin() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let attacker = seed_user(&db).await;
    let st = ws_state(&db).await;

    // Grant the attacker exactly one delegated permission — `user:assign_role`
    // — via a fresh ad-hoc platform-scoped role, mirroring how a
    // non-admin could previously have been granted it through
    // `role:assign_permission`. No `platform_admin` role.
    let (role_id,): (uuid::Uuid,) = sqlx::query_as(
        "INSERT INTO roles (scope, name, description) VALUES ('platform', $1, 'test-only role') \
         RETURNING id",
    )
    .bind(uniq("attacker_role"))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO role_permissions (role_id, permission) VALUES ($1, 'user:assign_role')",
    )
    .bind(role_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'platform', NULL) ON CONFLICT DO NOTHING",
    )
    .bind(attacker.0)
    .bind(role_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let cookie = login_cookie(&db, attacker, &st.sessions).await;
    let app = axum::Router::new()
        .nest("/api/admin", admin_user_router())
        .with_state(st);

    // Attempt to grant itself `platform_admin` directly.
    let body = serde_json::to_vec(&serde_json::json!({"roles": ["platform_admin"]})).unwrap();
    let resp = app
        .oneshot(
            HttpRequest::builder()
                .method("PUT")
                .uri(format!("/api/admin/users/{}/platform-roles", attacker.0))
                .header("cookie", cookie)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a delegated user:assign_role permission must not grant platform roles"
    );
}

/// B1b-final Task 1: the `set_platform_roles` gate now folds into the
/// admin-graph `forbid` via `require(pdp, .., "user:assign_platform_role")`
/// instead of an inline `platform_roles.contains("platform_admin")` check.
/// This drives the migrated PDP path end-to-end through the real `CedarPdp`:
/// a `platform_admin` is allowed (200) while a legitimate non-admin holding
/// an unrelated platform role (`data_steward`) is denied (403), proving the
/// forbid covers the new admin-graph action.
#[tokio::test]
async fn set_platform_roles_gate_is_admin_graph_forbid() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let admin = seed_user(&db).await;
    let non_admin = seed_user(&db).await;
    let target = seed_user(&db).await;
    let st = ws_state(&db).await;
    st.authz
        .replace_platform_roles(admin, &["platform_admin".into()])
        .await
        .unwrap();
    // A real, non-adversarial non-admin: holds `data_steward` (an unrelated
    // platform role), not `platform_admin`.
    st.authz
        .replace_platform_roles(non_admin, &["data_steward".into()])
        .await
        .unwrap();
    let app = axum::Router::new()
        .nest("/api/admin", admin_user_router())
        .with_state(st.clone());

    let put = |actor: UserId, cookie: String| {
        let app = app.clone();
        async move {
            let body = serde_json::to_vec(&serde_json::json!({"roles": ["data_steward"]})).unwrap();
            let _ = actor;
            app.oneshot(
                HttpRequest::builder()
                    .method("PUT")
                    .uri(format!("/api/admin/users/{}/platform-roles", target.0))
                    .header("cookie", cookie)
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap()
        }
    };

    // platform_admin → allowed by the `unless is_platform_admin` clause.
    let admin_cookie = login_cookie(&db, admin, &st.sessions).await;
    let admin_resp = put(admin, admin_cookie).await;
    assert_eq!(
        admin_resp.status(),
        StatusCode::OK,
        "platform_admin must pass the admin-graph gate"
    );

    // non-admin (data_steward, not platform_admin) → forbidden.
    let non_admin_cookie = login_cookie(&db, non_admin, &st.sessions).await;
    let non_admin_resp = put(non_admin, non_admin_cookie).await;
    assert_eq!(
        non_admin_resp.status(),
        StatusCode::FORBIDDEN,
        "a non-platform_admin must be denied `user:assign_platform_role` by the forbid"
    );
}

// ── Task 9: Workspace member routes ──────────────────────────────────────────

#[tokio::test]
async fn member_routes_enforce_membership() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let owner = seed_user(&db).await;
    let admin_user = seed_user(&db).await;
    let outsider = seed_user(&db).await;
    let st = ws_state(&db).await;
    let ws = st.workspaces.create(&uniq("ws"), "W", owner).await.unwrap();
    st.members
        .add(ws.id, "user", admin_user.0, "ws_admin", owner)
        .await
        .unwrap();
    let app = axum::Router::new()
        .nest("/api/ws", ws_member_router())
        .with_state(st.clone());

    // ws_admin can list members
    let cookie = login_cookie(&db, admin_user, &st.sessions).await;
    let resp = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri(format!("/api/ws/{}/members", ws.slug))
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // outsider (no membership at all) gets 403 — rejected in the `WsContext`
    // extractor before the gate is even reached.
    let cookie2 = login_cookie(&db, outsider, &st.sessions).await;
    let resp = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri(format!("/api/ws/{}/members", ws.slug))
                .header("cookie", cookie2)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // A ws_viewer member is a legitimate member (passes the extractor) but its
    // role lacks `workspace:manage_members`, so the migrated PDP gate denies it
    // (403). This exercises the denial path of the migrated `require()` call
    // site itself, distinct from the outsider's extractor-level rejection above.
    let viewer = seed_user(&db).await;
    st.members
        .add(ws.id, "user", viewer.0, "ws_viewer", owner)
        .await
        .unwrap();
    let cookie3 = login_cookie(&db, viewer, &st.sessions).await;
    let resp = app
        .oneshot(
            HttpRequest::builder()
                .uri(format!("/api/ws/{}/members", ws.slug))
                .header("cookie", cookie3)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "ws_viewer lacks workspace:manage_members; PDP gate must deny"
    );
}

#[tokio::test]
async fn archived_workspace_is_404() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let owner = seed_user(&db).await;
    let st = ws_state(&db).await;
    let ws = st.workspaces.create(&uniq("ws"), "W", owner).await.unwrap();
    st.members
        .add(ws.id, "user", owner.0, "ws_admin", owner)
        .await
        .unwrap();
    st.workspaces.archive(ws.id).await.unwrap();
    let app = axum::Router::new()
        .nest("/api/ws", ws_member_router())
        .with_state(st.clone());
    let cookie = login_cookie(&db, owner, &st.sessions).await;
    let resp = app
        .oneshot(
            HttpRequest::builder()
                .uri(format!("/api/ws/{}/members", ws.slug))
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── Task 1 / Phase 0b: me_router tests ───────────────────────────────────────

#[tokio::test]
async fn my_workspaces_lists_membership_and_admin_sees_all() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let owner = seed_user(&db).await;
    let member = seed_user(&db).await;
    let st = ws_state(&db).await;
    let ws = st.workspaces.create(&uniq("ws"), "W", owner).await.unwrap();
    st.members
        .add(ws.id, "user", member.0, "ws_viewer", owner)
        .await
        .unwrap();
    let app = axum::Router::new()
        .nest("/api/me", me_router())
        .with_state(st.clone());

    // member sees their workspace
    let cookie = login_cookie(&db, member, &st.sessions).await;
    let resp = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri("/api/me/workspaces")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let arr: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(arr
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w["id"] == ws.id.to_string()));

    // a brand-new non-member sees none of this ws
    let outsider = seed_user(&db).await;
    let cookie2 = login_cookie(&db, outsider, &st.sessions).await;
    let resp = app
        .oneshot(
            HttpRequest::builder()
                .uri("/api/me/workspaces")
                .header("cookie", cookie2)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let arr: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(!arr
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w["id"] == ws.id.to_string()));
}
