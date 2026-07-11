//! Per-object authorization for `dashboards/{id}` handlers (Effort B / B2-1a Task 7).
//! DB-gated; self-skips without DATABASE_URL. Builds a REAL `CedarPdp` AFTER
//! `migrate()` so it snapshots the `ws_viewer`/`ws_editor` BI perms.
//!
//! Proves the dashboard per-item handlers are now object-scoped (`require_object`)
//! AND that the edit model is ROLE-BASED (owner gate removed — Metabase curate):
//! an OUTSIDE user (not a workspace member) with a `ws_viewer` object grant on a
//! dashboard can READ it (200) but not a sibling (403) and cannot UPDATE it (403,
//! viewer lacks manage); an OUTSIDE user with a `ws_editor` object grant can
//! UPDATE the granted dashboard (2xx) but not a sibling (403); a workspace
//! `ws_editor` member reads + updates ANY dashboard, including one they do NOT
//! own (role-based edit, not owner-based).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_bi::{
    dashboard_router, BiState, ChartRepo, DashboardRepo, DatasetRepo, SavedQueryRepo, SchemaService,
};
use egret_metastore::Db;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

fn build_state(db: Db, pdp: Arc<dyn egret_core::authz::Pdp>) -> BiState {
    BiState {
        db: db.clone(),
        saved: SavedQueryRepo::new(db.clone()),
        schema: SchemaService::new(),
        databases: egret_query::DatabaseRepo::new(
            db.clone(),
            egret_secrets::SecretStore::new(db.clone(), None),
            egret_secrets::WorkspaceSecretStore::new(
                db.clone(),
                egret_secrets::SecretStore::new(db.clone(), None),
            ),
        ),
        sessions: egret_auth::session::SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        workspaces: egret_workspace::workspaces::WorkspaceRepo::new(db.clone()),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        pdp,
        charts: ChartRepo::new(db.clone()),
        datasets: DatasetRepo::new(db.clone()),
        dashboards: DashboardRepo::new(db.clone()),
        folders: egret_bi::folder::FolderRepo::new(db),
    }
}

async fn seed_user(db: &Db) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
        .bind(format!("u_{}@egret.local", Uuid::new_v4().simple()))
        .fetch_one(&db.pool)
        .await
        .expect("insert user")
}

async fn session_for(sessions: &egret_auth::session::SessionService, uid: Uuid) -> String {
    sessions
        .create(egret_core::ids::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
}

async fn add_member(db: &Db, ws: Uuid, uid: Uuid, role: &str) {
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $2, r.id, 'workspace', $1 FROM roles r \
         WHERE r.scope = 'workspace' AND r.name = $3 ON CONFLICT DO NOTHING",
    )
    .bind(ws)
    .bind(uid)
    .bind(role)
    .execute(&db.pool)
    .await
    .expect("add member");
}

/// Seed a direct object grant of workspace `role` to `uid` on dashboard `db_id`.
async fn grant_object(db: &Db, uid: Uuid, dash: Uuid, role: &str, granted_by: Uuid) {
    let role_id: Uuid =
        sqlx::query_scalar("SELECT id FROM roles WHERE scope = 'workspace' AND name = $1")
            .bind(role)
            .fetch_one(&db.pool)
            .await
            .expect("role id");
    sqlx::query(
        "INSERT INTO access_grants \
           (principal_type, principal_id, role_id, scope_type, object_type, scope_id, granted_by) \
         VALUES ('user', $1, $2, 'object', 'dashboard', $3, $4) ON CONFLICT DO NOTHING",
    )
    .bind(uid)
    .bind(role_id)
    .bind(dash)
    .bind(granted_by)
    .execute(&db.pool)
    .await
    .expect("seed object grant");
}

async fn get_dashboard(app: &axum::Router, slug: &str, dash: Uuid, tok: &str) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(format!("/ws/{slug}/dashboards/{dash}"))
                .header("Cookie", format!("egret_session={tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// PUT a dashboard update (rename with an empty layout). Empty tabs/cards is a
/// valid layout, so repeated renames don't collide.
async fn update_dashboard(
    app: &axum::Router,
    slug: &str,
    dash: Uuid,
    tok: &str,
    name: &str,
) -> StatusCode {
    let body = format!(
        r#"{{"name":"{name}","description":"","parameters":[],"width":"fixed","tabs":[],"cards":[]}}"#
    );
    app.clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/ws/{slug}/dashboards/{dash}"))
                .header("Cookie", format!("egret_session={tok}"))
                .header("Content-Type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// POST a dashboard duplicate. Duplicating creates new workspace content, so it
/// is gated on dashboard:manage (security review F1), matching db_create.
async fn duplicate_dashboard(app: &axum::Router, slug: &str, dash: Uuid, tok: &str) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/ws/{slug}/dashboards/{dash}/duplicate"))
                .header("Cookie", format!("egret_session={tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn dashboard_per_object_handlers_are_object_scoped() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    // Sanity: ws_viewer holds dashboard:read but NOT dashboard:manage; ws_editor
    // holds both. This underpins read-yes / manage-no for viewers.
    let viewer_read: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM role_permissions rp JOIN roles r ON r.id = rp.role_id \
         WHERE r.scope = 'workspace' AND r.name = 'ws_viewer' AND rp.permission = 'dashboard:read'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(viewer_read, 1, "ws_viewer must have dashboard:read");
    let viewer_manage: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM role_permissions rp JOIN roles r ON r.id = rp.role_id \
         WHERE r.scope = 'workspace' AND r.name = 'ws_viewer' AND rp.permission = 'dashboard:manage'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(viewer_manage, 0, "ws_viewer must NOT have dashboard:manage");

    let owner = seed_user(&db).await;
    let slug = format!("dbobj-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'DB Obj Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");

    // Principals: a ws_editor member (does everything), and TWO outside users
    // (not members) — one gets a Viewer object grant, one an Editor object grant.
    let editor_member = seed_user(&db).await;
    add_member(&db, ws, editor_member, "ws_editor").await;
    let outside_viewer = seed_user(&db).await;
    let outside_editor = seed_user(&db).await;

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = build_state(db.clone(), pdp);
    let sessions = st.sessions.clone();

    let editor_tok = session_for(&sessions, editor_member).await;
    let outside_viewer_tok = session_for(&sessions, outside_viewer).await;
    let outside_editor_tok = session_for(&sessions, outside_editor).await;

    // Two dashboards in the workspace, BOTH owned by `owner`: DB (shared) and DB2
    // (sibling, not shared). DB2 is used to prove a non-owner member can edit a
    // dashboard they don't own (role-based, not owner-based).
    let dash = st
        .dashboards
        .create(ws, "DB", "", Some(owner))
        .await
        .unwrap()
        .id;
    let dash2 = st
        .dashboards
        .create(ws, "DB2", "", Some(owner))
        .await
        .unwrap()
        .id;

    // Direct object grants (seeded straight into access_grants).
    grant_object(&db, outside_viewer, dash, "ws_viewer", owner).await;
    grant_object(&db, outside_editor, dash, "ws_editor", owner).await;

    let app = dashboard_router().with_state(st);

    // 1. Outside VIEWER: reads DB via object grant (200), denied sibling DB2 (403),
    //    denied update (viewer lacks dashboard:manage → 403).
    let read_db = get_dashboard(&app, &slug, dash, &outside_viewer_tok).await;
    assert_eq!(
        read_db,
        StatusCode::OK,
        "outside ws_viewer object-grant must READ its granted dashboard DB (200)"
    );
    let read_sibling = get_dashboard(&app, &slug, dash2, &outside_viewer_tok).await;
    assert_eq!(
        read_sibling,
        StatusCode::FORBIDDEN,
        "outside ws_viewer must NOT read ungranted sibling DB2 (403)"
    );
    let viewer_update = update_dashboard(&app, &slug, dash, &outside_viewer_tok, "v_rename").await;
    assert_eq!(
        viewer_update,
        StatusCode::FORBIDDEN,
        "outside ws_viewer must NOT update DB (viewer lacks manage → 403)"
    );
    // Duplicating creates new content → dashboard:manage (F1). A read-only guest is denied.
    let viewer_dup = duplicate_dashboard(&app, &slug, dash, &outside_viewer_tok).await;
    assert_eq!(
        viewer_dup,
        StatusCode::FORBIDDEN,
        "outside ws_viewer must NOT duplicate DB (duplicate needs manage → 403, F1)"
    );

    // 2. Outside EDITOR: updates the granted DB (2xx) but not the sibling DB2 (403).
    let editor_update_db =
        update_dashboard(&app, &slug, dash, &outside_editor_tok, "e_rename").await;
    assert!(
        editor_update_db.is_success(),
        "outside ws_editor object-grant must UPDATE its granted dashboard DB (2xx); got {editor_update_db}"
    );
    let editor_update_sibling =
        update_dashboard(&app, &slug, dash2, &outside_editor_tok, "e_rename2").await;
    assert_eq!(
        editor_update_sibling,
        StatusCode::FORBIDDEN,
        "outside ws_editor must NOT update ungranted sibling DB2 (403)"
    );
    // An Editor object-grant holds dashboard:manage on DB → may duplicate it (2xx, F1).
    let editor_dup = duplicate_dashboard(&app, &slug, dash, &outside_editor_tok).await;
    assert!(
        editor_dup.is_success(),
        "outside ws_editor object-grant must duplicate its granted DB (2xx, F1); got {editor_dup}"
    );

    // 3. Workspace ws_editor member reads + updates ANY dashboard. DB2 is owned by
    //    `owner`, NOT the member — a successful update here locks in the confirmed
    //    ROLE-BASED edit model (owner gate removed).
    let member_read = get_dashboard(&app, &slug, dash, &editor_tok).await;
    assert_eq!(
        member_read,
        StatusCode::OK,
        "ws_editor member must read DB (200)"
    );
    let member_update = update_dashboard(&app, &slug, dash2, &editor_tok, "m_rename").await;
    assert!(
        member_update.is_success(),
        "ws_editor member must update a dashboard they do NOT own (role-based, 2xx); got {member_update}"
    );
}
