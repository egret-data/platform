//! AX-1 workspace-member integration tests: list enrichment (name / provenance /
//! platform-admin flag), custom-role acceptance, one-role-per-member replace, and
//! the last-admin guard. DB-gated; self-skip without DATABASE_URL — matching the
//! sibling `workspace_db.rs` harness (same `connect`/`uniq`/`ws_state`/`login_cookie`
//! setup, same repo-direct + router/oneshot styles).

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::password::hash_password;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_core::UserId;
use egret_metastore::Db;
use egret_workspace::members::MemberRepo;
use egret_workspace::rbac::Authorizer;
use egret_workspace::routes::{ws_member_router, WorkspaceState};
use egret_workspace::workspaces::WorkspaceRepo;
use tower::ServiceExt;
use uuid::Uuid;

async fn connect() -> Option<Db> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn uniq(tag: &str) -> String {
    format!("{tag}-{}", uuid::Uuid::new_v4().simple())
}

/// Seed a local user with a caller-known (lowercase) email so `principal_name` /
/// `granted_by_name` equality assertions are exact.
async fn seed_user_email(db: &Db, email: &str) -> UserId {
    UserRepo::new(db.clone())
        .insert_local(email, "U", &hash_password("pw").unwrap())
        .await
        .unwrap()
}

async fn seed_group(db: &Db, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO groups (name, source) VALUES ($1, 'local') RETURNING id")
        .bind(name)
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

// ── Test 1: list enrichment ───────────────────────────────────────────────────

/// `MemberRepo::list` resolves the display name (user email / group name), the
/// adder's email as provenance, a populated `added_at`, and the platform-admin
/// flag per principal.
#[tokio::test]
async fn list_enriches_names_provenance_and_platform_admin() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let t0 = chrono::Utc::now() - chrono::Duration::minutes(5);

    let adder_email = format!("adder-{}@t.local", uuid::Uuid::new_v4().simple());
    let member_email = format!("member-{}@t.local", uuid::Uuid::new_v4().simple());
    let pa_email = format!("pa-{}@t.local", uuid::Uuid::new_v4().simple());
    let group_name = uniq("Analytics Team");

    let adder = seed_user_email(&db, &adder_email).await;
    let plain = seed_user_email(&db, &member_email).await;
    let pa_user = seed_user_email(&db, &pa_email).await;
    let gid = seed_group(&db, &group_name).await;

    let wrepo = WorkspaceRepo::new(db.clone());
    let mrepo = MemberRepo::new(db.clone());
    let authz = Authorizer::new(db.clone());
    let ws = wrepo.create(&uniq("ws"), "W", adder).await.unwrap();

    // A plain user member, a member who ALSO holds platform_admin, and a group.
    mrepo
        .add(ws.id, "user", plain.0, "ws_editor", adder)
        .await
        .unwrap();
    mrepo
        .add(ws.id, "user", pa_user.0, "ws_viewer", adder)
        .await
        .unwrap();
    mrepo
        .add(ws.id, "group", gid, "ws_viewer", adder)
        .await
        .unwrap();
    authz
        .replace_platform_roles(pa_user, &["platform_admin".into()])
        .await
        .unwrap();

    let members = mrepo.list(ws.id).await.unwrap();
    assert_eq!(members.len(), 3, "three members expected");

    let find_user = |uid: Uuid| {
        members
            .iter()
            .find(|m| m.principal_type == "user" && m.principal_id == uid)
            .unwrap_or_else(|| panic!("member {uid} missing"))
    };

    // Plain user: name == its email, provenance == adder's email, populated
    // added_at, NOT a platform admin.
    let m = find_user(plain.0);
    assert_eq!(m.principal_name.as_deref(), Some(member_email.as_str()));
    assert_eq!(m.granted_by_name.as_deref(), Some(adder_email.as_str()));
    assert!(m.added_at > t0, "added_at must be populated & recent");
    assert!(!m.is_platform_admin, "plain member is not a platform admin");

    // The platform-admin holder is flagged true.
    let pa = find_user(pa_user.0);
    assert!(
        pa.is_platform_admin,
        "member holding platform_admin must be flagged"
    );

    // Group member: name == group name.
    let g = members
        .iter()
        .find(|m| m.principal_type == "group" && m.principal_id == gid)
        .expect("group member missing");
    assert_eq!(g.principal_name.as_deref(), Some(group_name.as_str()));
    assert_eq!(g.role, "ws_viewer");
}

// ── Test 2: custom role accepted, bad roles rejected ──────────────────────────

/// `add` accepts ANY workspace-scoped role (including a custom one) and rejects a
/// platform-scoped role name and an unknown role with `InvalidInput`.
#[tokio::test]
async fn add_accepts_custom_ws_role_rejects_bad_role() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let by = seed_user_email(&db, &format!("by-{}@t.local", uuid::Uuid::new_v4().simple())).await;
    let member =
        seed_user_email(&db, &format!("cm-{}@t.local", uuid::Uuid::new_v4().simple())).await;
    let wrepo = WorkspaceRepo::new(db.clone());
    let mrepo = MemberRepo::new(db.clone());
    let ws = wrepo.create(&uniq("ws"), "W", by).await.unwrap();

    // A custom workspace-scoped role.
    let custom = uniq("Report Builder");
    sqlx::query("INSERT INTO roles (scope, name, description) VALUES ('workspace', $1, 'custom')")
        .bind(&custom)
        .execute(&db.pool)
        .await
        .unwrap();

    // Custom role → Ok, and list shows the member with that role.
    mrepo
        .add(ws.id, "user", member.0, &custom, by)
        .await
        .expect("custom ws role accepted");
    let members = mrepo.list(ws.id).await.unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].role, custom);

    // A platform-scoped role name → InvalidInput (not a workspace role).
    let plat = mrepo.add(ws.id, "user", member.0, "platform_admin", by).await;
    assert!(
        matches!(plat, Err(egret_core::Error::InvalidInput(_))),
        "platform_admin must be rejected as a workspace role, got {plat:?}"
    );

    // A totally unknown role → InvalidInput.
    let bad = mrepo.add(ws.id, "user", member.0, "nope", by).await;
    assert!(
        matches!(bad, Err(egret_core::Error::InvalidInput(_))),
        "unknown role must be rejected, got {bad:?}"
    );
}

// ── Test 3: one role per member (replace, not duplicate) ──────────────────────

/// Re-adding a principal with a new role REPLACES the previous role; `list` shows
/// the member exactly once with the latest role.
#[tokio::test]
async fn re_add_replaces_role() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let by = seed_user_email(&db, &format!("by-{}@t.local", uuid::Uuid::new_v4().simple())).await;
    let u = seed_user_email(&db, &format!("u-{}@t.local", uuid::Uuid::new_v4().simple())).await;
    let wrepo = WorkspaceRepo::new(db.clone());
    let mrepo = MemberRepo::new(db.clone());
    let ws = wrepo.create(&uniq("ws"), "W", by).await.unwrap();

    mrepo
        .add(ws.id, "user", u.0, "ws_editor", by)
        .await
        .unwrap();
    mrepo
        .add(ws.id, "user", u.0, "ws_viewer", by)
        .await
        .unwrap();

    let members = mrepo.list(ws.id).await.unwrap();
    let mine: Vec<_> = members
        .iter()
        .filter(|m| m.principal_type == "user" && m.principal_id == u.0)
        .collect();
    assert_eq!(mine.len(), 1, "member must appear exactly once (replace)");
    assert_eq!(mine[0].role, "ws_viewer", "latest role wins");
}

// ── Test 4: last-admin guard ──────────────────────────────────────────────────

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
        members: MemberRepo::new(db.clone()),
        authz: Authorizer::new(db.clone()),
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

async fn login_cookie(user: UserId, sessions: &SessionService) -> String {
    let issued = sessions.create(user, "a", "").await.unwrap();
    format!("egret_session={}", issued.token)
}

/// The handler refuses to remove or demote a workspace's sole `ws_admin`. The guard
/// condition (`role_of == ws_admin` && `count_ws_admins <= 1`) is asserted directly,
/// and — because this harness drives the axum handlers — the actual `409 CONFLICT`
/// is exercised end-to-end through `DELETE`/`POST /api/ws/{ws}/members`.
#[tokio::test]
async fn last_admin_guard() {
    let Some(db) = connect().await else {
        eprintln!("skip");
        return;
    };
    let st = ws_state(&db).await;
    let admin =
        seed_user_email(&db, &format!("a1-{}@t.local", uuid::Uuid::new_v4().simple())).await;
    let ws = st.workspaces.create(&uniq("ws"), "W", admin).await.unwrap();

    // Sole ws_admin.
    st.members
        .add(ws.id, "user", admin.0, "ws_admin", admin)
        .await
        .unwrap();

    // Exactly the condition the handler checks before refusing.
    assert_eq!(
        st.members
            .role_of(ws.id, "user", admin.0)
            .await
            .unwrap()
            .as_deref(),
        Some("ws_admin"),
    );
    assert_eq!(st.members.count_ws_admins(ws.id).await.unwrap(), 1);

    let app = axum::Router::new()
        .nest("/api/ws", ws_member_router())
        .with_state(st.clone());
    let cookie = login_cookie(admin, &st.sessions).await;

    // Removing the sole admin → 409 CONFLICT.
    let resp = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("DELETE")
                .uri(format!("/api/ws/{}/members/user/{}", ws.slug, admin.0))
                .header("cookie", cookie.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "removing the sole ws_admin must 409"
    );

    // Demoting the sole admin via add (role != ws_admin) → 409 CONFLICT.
    let body = serde_json::to_vec(&serde_json::json!({
        "principal_type": "user", "principal_id": admin.0, "role": "ws_viewer"
    }))
    .unwrap();
    let resp = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri(format!("/api/ws/{}/members", ws.slug))
                .header("cookie", cookie.clone())
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "demoting the sole ws_admin must 409"
    );

    // Still the sole admin — the guard blocked both mutations.
    assert_eq!(
        st.members.role_of(ws.id, "user", admin.0).await.unwrap().as_deref(),
        Some("ws_admin"),
    );

    // Add a SECOND ws_admin: the guard no longer trips.
    let admin2 =
        seed_user_email(&db, &format!("a2-{}@t.local", uuid::Uuid::new_v4().simple())).await;
    st.members
        .add(ws.id, "user", admin2.0, "ws_admin", admin)
        .await
        .unwrap();
    assert_eq!(
        st.members.count_ws_admins(ws.id).await.unwrap(),
        2,
        "two admins after adding a second"
    );

    // With two admins the handler now allows demoting the first → 200 OK.
    let body = serde_json::to_vec(&serde_json::json!({
        "principal_type": "user", "principal_id": admin.0, "role": "ws_viewer"
    }))
    .unwrap();
    let resp = app
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri(format!("/api/ws/{}/members", ws.slug))
                .header("cookie", cookie)
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "with a second admin present, demoting the first is allowed"
    );
    assert_eq!(st.members.count_ws_admins(ws.id).await.unwrap(), 1);
}
