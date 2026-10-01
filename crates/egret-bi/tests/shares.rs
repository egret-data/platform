//! Object-grant share API (Effort B / B2-1a Task 4). DB-gated; self-skips without
//! `DATABASE_URL` ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS. Builds a REAL
//! `CedarPdp` AFTER `migrate()` so it snapshots the `ws_viewer` BI read perms added by
//! migration 0070.
//!
//! Proves: a `<kind>:manage` holder can share an object with an OUTSIDE user at
//! `ws_viewer`; that user can then read the object (require_object read passes
//! via the object grant + the ws_viewer read perm), but not a sibling; a
//! `ws_viewer` caller cannot grant; delete revokes; list returns the grant.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_bi::{
    dashboard_router, shares_router, BiState, ChartRepo, DashboardRepo, DatasetRepo,
    SavedQueryRepo, SchemaService,
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

async fn session_for(
    _db: &Db,
    sessions: &egret_auth::session::SessionService,
    uid: Uuid,
) -> String {
    sessions
        .create(egret_core::ids::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
}

/// Add `uid` to `ws` under workspace `role` (trigger mirrors into access_grants).
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

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).to_string()
}

#[tokio::test]
async fn object_share_grant_read_revoke() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    // Sanity: migration 0070 gave ws_viewer dashboard:read (Step 0). If this is 0
    // the whole outside-user-read premise fails, so assert it up front.
    let viewer_read: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM role_permissions rp \
         JOIN roles r ON r.id = rp.role_id \
         WHERE r.scope = 'workspace' AND r.name = 'ws_viewer' AND rp.permission = 'dashboard:read'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        viewer_read, 1,
        "migration 0070 must give ws_viewer dashboard:read"
    );

    // Workspace (active) + owner.
    let owner = seed_user(&db).await;
    let slug = format!("share-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'Share Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");

    // Members + outside user (seed BEFORE PDP build not required — grants read
    // live — but role templates ARE snapshotted, so build PDP after migrate).
    let editor = seed_user(&db).await;
    add_member(&db, ws, editor, "ws_editor").await; // holds dashboard:manage
    let viewer_member = seed_user(&db).await;
    add_member(&db, ws, viewer_member, "ws_viewer").await; // read-only
    let outsider = seed_user(&db).await; // NOT a member

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = build_state(db.clone(), pdp);
    let sessions = st.sessions.clone();

    let editor_tok = session_for(&db, &sessions, editor).await;
    let viewer_tok = session_for(&db, &sessions, viewer_member).await;
    let outsider_tok = session_for(&db, &sessions, outsider).await;

    // Two dashboards in the workspace: D (shared) and D2 (sibling, not shared).
    let d = st
        .dashboards
        .create(ws, "D", "", Some(owner))
        .await
        .unwrap()
        .id;
    let d2 = st
        .dashboards
        .create(ws, "D2", "", Some(owner))
        .await
        .unwrap()
        .id;

    let app = shares_router().merge(dashboard_router()).with_state(st);

    // 1. Editor grants ws_viewer to the OUTSIDE user on D → 2xx.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/ws/{slug}/objects/dashboard/{d}/grants"))
                .header("Cookie", format!("egret_session={editor_tok}"))
                .header("Content-Type", "application/json")
                .body(Body::from(format!(
                    r#"{{"principal_type":"user","principal_id":"{outsider}","role":"ws_viewer"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    let st1 = resp.status();
    assert!(
        st1.is_success(),
        "editor (dashboard:manage) must grant ws_viewer on D; got {st1}: {}",
        body_string(resp).await
    );

    // 2. The outside user is now an OBJECT-GUEST holding only a ws_viewer grant on
    //    D (dashboard:read, NOT dashboard:manage). Listing the ACL is manage-only
    //    (security review F2 — the ACL exposes every other grantee), so the
    //    read-only guest is DENIED: GET D's grants → 403. The object-guest
    //    admission + "reads its granted object" proof is covered by step 3c below
    //    (db_get on D → 200).
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/ws/{slug}/objects/dashboard/{d}/grants"))
                .header("Cookie", format!("egret_session={outsider_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let st2 = resp.status();
    assert_eq!(
        st2,
        StatusCode::FORBIDDEN,
        "read-only object-guest must NOT list D's ACL (grant_list is manage-only, F2); got {st2}: {}",
        body_string(resp).await
    );

    // 3. Same guest has NO grant on sibling D2 → require_object denies → 403.
    //    (Extractor admits — guest has a grant on D in this ws — then the handler
    //    denies the ungranted D2.)
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/ws/{slug}/objects/dashboard/{d2}/grants"))
                .header("Cookie", format!("egret_session={outsider_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "object-guest must NOT read a sibling dashboard it was not granted"
    );

    // 3b. SECURITY — no over-admission. Admitting the object-guest must NOT leak
    //     workspace-level access: a workspace-scoped route (list ALL dashboards,
    //     gated by `require(&ctx, dashboard:read)` at Resource::Workspace) must
    //     DENY the guest — they hold no workspace grant. If this ever 200s, the
    //     object-guest was over-admitted.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/ws/{slug}/dashboards"))
                .header("Cookie", format!("egret_session={outsider_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "SECURITY: object-guest must NOT reach workspace-level list-all (over-admission)"
    );

    // 3c. The per-ITEM dashboard read handler (`db_get`) is now object-scoped
    //     (`require_object`, Task 7), so the object-guest CAN read its OWN granted
    //     dashboard D (200) even though the workspace-level list-all above denies
    //     them. Object grant admits exactly the granted item, nothing wider.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/ws/{slug}/dashboards/{d}"))
                .header("Cookie", format!("egret_session={outsider_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "db_get is object-scoped (require_object, Task 7); admits the object-guest on its granted D"
    );

    // 4. A ws_viewer caller cannot grant (needs dashboard:manage) → 403.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/ws/{slug}/objects/dashboard/{d}/grants"))
                .header("Cookie", format!("egret_session={viewer_tok}"))
                .header("Content-Type", "application/json")
                .body(Body::from(format!(
                    r#"{{"principal_type":"user","principal_id":"{owner}","role":"ws_viewer"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a ws_viewer caller must NOT be able to grant (needs dashboard:manage)"
    );

    // 5. Editor lists D's grants → contains the outsider grant.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/ws/{slug}/objects/dashboard/{d}/grants"))
                .header("Cookie", format!("egret_session={editor_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let listed = body_string(resp).await;
    assert!(
        listed.contains(&outsider.to_string()) && listed.contains("ws_viewer"),
        "GET grants must list the outsider's ws_viewer grant; got {listed}"
    );
    let listed_json = serde_json::from_str::<serde_json::Value>(&listed).unwrap();
    // The list resolves each grantee's display name (a user's email here).
    let name = listed_json[0]["principal_name"]
        .as_str()
        .unwrap_or_default();
    assert!(
        name.ends_with("@egret.local"),
        "GET grants must resolve principal_name to the outsider's email; got {listed}"
    );
    let grant_id = listed_json[0]["id"].as_str().unwrap().to_string();

    // 6. Editor deletes the grant → outsider loses read (403).
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/ws/{slug}/objects/dashboard/{d}/grants/{grant_id}"
                ))
                .header("Cookie", format!("egret_session={editor_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.status().is_success(), "editor must revoke the grant");

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/ws/{slug}/objects/dashboard/{d}/grants"))
                .header("Cookie", format!("egret_session={outsider_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "after revoke, the outside user must lose read on D"
    );
}

/// B2-2 Task 1: notebook sharing is gated by `notebook:write` (Editor), not a
/// non-existent `notebook:manage`. A `ws_editor` (holds notebook:write) can grant
/// on a notebook; a `ws_viewer` (holds notebook:read but NOT notebook:write) cannot.
#[tokio::test]
async fn notebook_share_requires_editor() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let owner = seed_user(&db).await;
    let slug = format!("nbshare-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'NB Share', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");

    let editor = seed_user(&db).await;
    add_member(&db, ws, editor, "ws_editor").await; // holds notebook:write
    let viewer = seed_user(&db).await;
    add_member(&db, ws, viewer, "ws_viewer").await; // notebook:read only
    let outsider = seed_user(&db).await;

    // A notebook in the workspace.
    let nb: Uuid = sqlx::query_scalar(
        "INSERT INTO analytics.notebooks (workspace_id, owner_user_id, name) VALUES ($1, $2, 'NB') RETURNING id",
    )
    .bind(ws)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert notebook");

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = build_state(db.clone(), pdp);
    let sessions = st.sessions.clone();
    let editor_tok = session_for(&db, &sessions, editor).await;
    let viewer_tok = session_for(&db, &sessions, viewer).await;

    let app = shares_router().with_state(st);

    // Editor (notebook:write) grants ws_viewer on the notebook → 2xx.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/ws/{slug}/objects/notebook/{nb}/grants"))
                .header("Cookie", format!("egret_session={editor_tok}"))
                .header("Content-Type", "application/json")
                .body(Body::from(format!(
                    r#"{{"principal_type":"user","principal_id":"{outsider}","role":"ws_viewer"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    let s = resp.status();
    assert!(
        s.is_success(),
        "editor (notebook:write) must grant on a notebook; got {s}: {}",
        body_string(resp).await
    );

    // Viewer (notebook:read, NOT notebook:write) cannot grant → 403.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/ws/{slug}/objects/notebook/{nb}/grants"))
                .header("Cookie", format!("egret_session={viewer_tok}"))
                .header("Content-Type", "application/json")
                .body(Body::from(format!(
                    r#"{{"principal_type":"user","principal_id":"{outsider}","role":"ws_viewer"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a ws_viewer (no notebook:write) must NOT be able to share a notebook"
    );
}

/// Data Apps P2: an `app` is a share kind gated by `app:manage`, which only
/// `ws_admin` holds (migration 0164). The admin grants an outside user
/// `ws_viewer` on an app, lists it, and revokes it; a `ws_editor` (holds
/// `app:write` but NOT `app:manage`) is refused.
#[tokio::test]
async fn app_share_requires_app_manage() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let owner = seed_user(&db).await;
    let slug = format!("appshare-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'App Share', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");

    let admin = seed_user(&db).await;
    add_member(&db, ws, admin, "ws_admin").await; // holds app:manage
    let editor = seed_user(&db).await;
    add_member(&db, ws, editor, "ws_editor").await; // app:write, NOT app:manage
    let outsider = seed_user(&db).await;

    let app_id: Uuid = sqlx::query_scalar(
        "INSERT INTO analytics.apps (workspace_id, name, owner_user_id) \
         VALUES ($1, 'Sales app', $2) RETURNING id",
    )
    .bind(ws)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert app");

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = build_state(db.clone(), pdp);
    let sessions = st.sessions.clone();
    let admin_tok = session_for(&db, &sessions, admin).await;
    let editor_tok = session_for(&db, &sessions, editor).await;

    let router = shares_router().with_state(st);
    let grants = format!("/ws/{slug}/objects/app/{app_id}/grants");
    let put = |tok: &str| {
        Request::builder()
            .method("PUT")
            .uri(&grants)
            .header("Cookie", format!("egret_session={tok}"))
            .header("Content-Type", "application/json")
            .body(Body::from(format!(
                r#"{{"principal_type":"user","principal_id":"{outsider}","role":"ws_viewer"}}"#
            )))
            .unwrap()
    };

    // A ws_editor (no app:manage) cannot share the app.
    let resp = router.clone().oneshot(put(&editor_tok)).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a ws_editor holds app:write but not app:manage and must NOT share an app"
    );

    // The ws_admin can.
    let resp = router.clone().oneshot(put(&admin_tok)).await.unwrap();
    let s = resp.status();
    assert!(
        s.is_success(),
        "ws_admin (app:manage) must grant on an app; got {s}: {}",
        body_string(resp).await
    );

    // ... and list it.
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(&grants)
                .header("Cookie", format!("egret_session={admin_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let listed: serde_json::Value = serde_json::from_str(&body_string(resp).await).unwrap();
    let listed = listed.as_array().expect("grant list");
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0]["principal_id"], outsider.to_string());
    assert_eq!(listed[0]["role"], "ws_viewer");
    let grant_id = listed[0]["id"].as_str().unwrap().to_string();

    // ... and revoke it.
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("{grants}/{grant_id}"))
                .header("Cookie", format!("egret_session={admin_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.status().is_success(), "ws_admin must revoke the grant");
    let left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM access_grants WHERE scope_type = 'object' AND scope_id = $1",
    )
    .bind(app_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(left, 0, "the revoke removed the app grant");
}
