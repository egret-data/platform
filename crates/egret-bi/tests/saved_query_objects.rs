//! Per-object authorization for saved-query (`sqllab/queries/{id}`) handlers
//! (saved-query object-scope). DB-gated; self-skips without DATABASE_URL. Builds
//! a REAL `CedarPdp` AFTER `migrate()` so it snapshots the `ws_viewer`/`ws_editor`
//! `saved_query:*` perms granted in migration 0077.
//!
//! Proves the four object-scope guarantees of this branch, end-to-end through the
//! axum handlers (same level as `chart_object_scope.rs` / `share_deny.rs`):
//!   1. `load` rejects a cross-workspace saved query (IDOR regression → 404).
//!   2. an OUTSIDE object-guest granted `ws_viewer` on ONE saved query reads that
//!      query (200) but not a sibling (403).
//!   3. a workspace `ws_editor` member edits a saved query they do NOT own
//!      (role-based edit, owner gate removed) and the change persists.
//!   4. `sq_set_folder` rejects a foreign-workspace folder id (`require_folder`
//!      guard → 404).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_bi::{
    sqllab_router, BiState, ChartRepo, DashboardRepo, DatasetRepo, SavedQueryRepo, SchemaService,
};
use egret_metastore::Db;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

/// Test stub: object-scope tests use non-delegated DBs, so mint is never called.
struct NoMinter;
#[async_trait::async_trait]
impl egret_analytics_api::engine_token::EngineTokenMinter for NoMinter {
    async fn mint(
        &self,
        _user: uuid::Uuid,
    ) -> Result<
        egret_analytics_api::engine_token::EngineToken,
        egret_analytics_api::engine_token::BrokerError,
    > {
        Err(egret_analytics_api::engine_token::BrokerError::NoRefreshToken)
    }
}

fn build_state(db: Db, pdp: Arc<dyn egret_core::authz::Pdp>) -> BiState {
    BiState {
        db: db.clone(),
        users: egret_auth::users::UserRepo::new(db.clone()),
        engine_minter: Arc::new(NoMinter),
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
        audit: egret_audit::AuditSink::new(16).0,
    }
}

async fn seed_user(db: &Db) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
        .bind(format!("u_{}@egret.local", Uuid::new_v4().simple()))
        .fetch_one(&db.pool)
        .await
        .expect("insert user")
}

async fn seed_ws(db: &Db, owner: Uuid, prefix: &str) -> (Uuid, String) {
    let slug = format!("{prefix}-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'SQ Obj Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");
    (ws, slug)
}

async fn session_for(sessions: &egret_auth::session::SessionService, uid: Uuid) -> String {
    sessions
        .create(egret_core::ids::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
}

/// Add `uid` to `ws` under workspace `role` (seeded straight into access_grants).
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

/// Seed a direct object grant of workspace `role` to `uid` on saved query `sq`.
async fn grant_object_sq(db: &Db, uid: Uuid, sq: Uuid, role: &str, granted_by: Uuid) {
    let role_id: Uuid =
        sqlx::query_scalar("SELECT id FROM roles WHERE scope = 'workspace' AND name = $1")
            .bind(role)
            .fetch_one(&db.pool)
            .await
            .expect("role id");
    sqlx::query(
        "INSERT INTO access_grants \
           (principal_type, principal_id, role_id, scope_type, object_type, scope_id, granted_by) \
         VALUES ('user', $1, $2, 'object', 'saved_query', $3, $4) ON CONFLICT DO NOTHING",
    )
    .bind(uid)
    .bind(role_id)
    .bind(sq)
    .bind(granted_by)
    .execute(&db.pool)
    .await
    .expect("seed object grant");
}

async fn get_query(app: &axum::Router, slug: &str, id: Uuid, tok: &str) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(format!("/ws/{slug}/sqllab/queries/{id}"))
                .header("Cookie", format!("egret_session={tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

async fn update_query(
    app: &axum::Router,
    slug: &str,
    id: Uuid,
    tok: &str,
    name: &str,
    sql: &str,
) -> StatusCode {
    let body = format!(r#"{{"database_id":null,"name":"{name}","sql":"{sql}","description":""}}"#);
    app.clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/ws/{slug}/sqllab/queries/{id}"))
                .header("Cookie", format!("egret_session={tok}"))
                .header("Content-Type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

async fn set_folder(
    app: &axum::Router,
    slug: &str,
    id: Uuid,
    tok: &str,
    folder_id: Uuid,
) -> StatusCode {
    let body = format!(r#"{{"folder_id":"{folder_id}"}}"#);
    app.clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/ws/{slug}/sqllab/queries/{id}/folder"))
                .header("Cookie", format!("egret_session={tok}"))
                .header("Content-Type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

async fn build_pdp(db: &Db) -> Arc<dyn egret_core::authz::Pdp> {
    Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    )
}

/// 1. IDOR regression: a ws_viewer of ws_a (holds saved_query:read there) that
///    asks for a saved query living in ws_b via the ws_a path must get 404 —
///    NOT the ws_b row. `resolve()` NotFounds on the workspace mismatch.
#[tokio::test]
async fn load_rejects_cross_workspace_saved_query() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let owner = seed_user(&db).await;
    let (ws_a, slug_a) = seed_ws(&db, owner, "sqidor-a").await;
    let (ws_b, _slug_b) = seed_ws(&db, owner, "sqidor-b").await;

    // Viewer is a member of ws_a ONLY (holds saved_query:read in ws_a).
    let viewer = seed_user(&db).await;
    add_member(&db, ws_a, viewer, "ws_viewer").await;

    let pdp = build_pdp(&db).await;
    let st = build_state(db.clone(), pdp);
    let sessions = st.sessions.clone();
    let viewer_tok = session_for(&sessions, viewer).await;

    // Saved query created in ws_b.
    let q_b = st
        .saved
        .create(
            ws_b,
            None,
            "secret-b",
            "SELECT 42",
            "",
            Some(egret_core::UserId(owner)),
        )
        .await
        .unwrap()
        .id;

    let app = sqllab_router().with_state(st);

    // ws_a on the path + ws_b saved-query id → 404 NotFound (IDOR blocked).
    let status = get_query(&app, &slug_a, q_b, &viewer_tok).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "loading a ws_b saved query via the ws_a path must be 404, not the ws_b row"
    );
    // Sanity: ws_a and ws_b are genuinely distinct.
    assert_ne!(ws_a, ws_b);
}

/// 2. Object-guest scoping: an OUTSIDE user (no ws role) granted `ws_viewer` on
///    q1 as an OBJECT grant is admitted as an object-guest and reads q1 (200) but
///    is denied its sibling q2 (403 — no grant on q2).
#[tokio::test]
async fn object_guest_can_load_granted_saved_query_only() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let owner = seed_user(&db).await;
    let (ws, slug) = seed_ws(&db, owner, "sqguest").await;

    // Outside user: NOT a member of the workspace.
    let guest = seed_user(&db).await;

    let pdp = build_pdp(&db).await;
    let st = build_state(db.clone(), pdp);
    let sessions = st.sessions.clone();
    let guest_tok = session_for(&sessions, guest).await;

    let q1 = st
        .saved
        .create(
            ws,
            None,
            "q1",
            "SELECT 1",
            "",
            Some(egret_core::UserId(owner)),
        )
        .await
        .unwrap()
        .id;
    let q2 = st
        .saved
        .create(
            ws,
            None,
            "q2",
            "SELECT 2",
            "",
            Some(egret_core::UserId(owner)),
        )
        .await
        .unwrap()
        .id;

    // Object grant: ws_viewer on q1 ONLY.
    grant_object_sq(&db, guest, q1, "ws_viewer", owner).await;

    let app = sqllab_router().with_state(st);

    // Admitted as object-guest via the q1 grant → reads q1 (200).
    let read_q1 = get_query(&app, &slug, q1, &guest_tok).await;
    assert_eq!(
        read_q1,
        StatusCode::OK,
        "object-guest with a ws_viewer grant on q1 must read q1 (200)"
    );
    // Same workspace, but no grant on q2 → require_object denies (403).
    let read_q2 = get_query(&app, &slug, q2, &guest_tok).await;
    assert_eq!(
        read_q2,
        StatusCode::FORBIDDEN,
        "object-guest must NOT read the ungranted sibling q2 (403)"
    );
}

/// 3. Role-based edit (owner gate removed): q is created_by user A; user B is a
///    ws_editor member (holds saved_query:manage). B updates q → 2xx and the new
///    name persists (re-fetch and assert).
#[tokio::test]
async fn ws_editor_can_edit_others_saved_query() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let user_a = seed_user(&db).await;
    let (ws, slug) = seed_ws(&db, user_a, "sqedit").await;

    // B is a ws_editor member; does NOT own the saved query.
    let user_b = seed_user(&db).await;
    add_member(&db, ws, user_b, "ws_editor").await;

    let pdp = build_pdp(&db).await;
    let st = build_state(db.clone(), pdp);
    let sessions = st.sessions.clone();
    let saved = st.saved.clone();
    let b_tok = session_for(&sessions, user_b).await;

    // q owned by A.
    let q = st
        .saved
        .create(
            ws,
            None,
            "orig-name",
            "SELECT 1",
            "",
            Some(egret_core::UserId(user_a)),
        )
        .await
        .unwrap()
        .id;

    let app = sqllab_router().with_state(st);

    // B (non-owner editor) renames q → 2xx.
    let status = update_query(&app, &slug, q, &b_tok, "renamed-by-b", "SELECT 2").await;
    assert!(
        status.is_success(),
        "ws_editor must update a saved query they do NOT own (role-based, 2xx); got {status}"
    );

    // The change actually persisted.
    let got = saved
        .get(q)
        .await
        .unwrap()
        .expect("saved query still exists");
    assert_eq!(
        got.name, "renamed-by-b",
        "the non-owner editor's rename must persist"
    );
    assert_eq!(
        got.sql, "SELECT 2",
        "the non-owner editor's sql edit must persist"
    );
}

/// 4. `require_folder` guard: q lives in ws_a; a folder is created in ws_b with
///    entity_type 'saved_query'. Moving q (ws_a path) into the ws_b folder must
///    be 404 — the folder does not belong to ws_a.
#[tokio::test]
async fn set_folder_rejects_foreign_folder() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let owner = seed_user(&db).await;
    let (ws_a, slug_a) = seed_ws(&db, owner, "sqfold-a").await;
    let (ws_b, _slug_b) = seed_ws(&db, owner, "sqfold-b").await;

    // Editor member of ws_a (holds saved_query:manage there).
    let editor = seed_user(&db).await;
    add_member(&db, ws_a, editor, "ws_editor").await;

    let pdp = build_pdp(&db).await;
    let st = build_state(db.clone(), pdp);
    let sessions = st.sessions.clone();
    let editor_tok = session_for(&sessions, editor).await;

    // q in ws_a.
    let q = st
        .saved
        .create(
            ws_a,
            None,
            "q",
            "SELECT 1",
            "",
            Some(egret_core::UserId(owner)),
        )
        .await
        .unwrap()
        .id;

    // A saved_query folder living in ws_b.
    let foreign_folder = st
        .folders
        .create(ws_b, "saved_query", "b-folder", Some(owner))
        .await
        .unwrap()
        .id;

    let app = sqllab_router().with_state(st);

    // Move q (ws_a) into a ws_b folder → 404 (require_folder rejects foreign ws).
    let status = set_folder(&app, &slug_a, q, &editor_tok, foreign_folder).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "moving a saved query into a folder from another workspace must be 404"
    );
}
