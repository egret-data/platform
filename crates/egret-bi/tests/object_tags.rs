//! Per-object classification-tag endpoints (Access P3.1). DB-gated; self-skips without
//! `DATABASE_URL` ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS. Builds a REAL
//! `CedarPdp` after `migrate()`.
//!
//! Proves: a ws object-manager (ws_editor) PUTs a tag on a dataset → 200; GET
//! `/tags` lists it; DELETE removes it (GET then empty). A ws_viewer (no
//! dataset:manage) PUT → 403. A dataset in ANOTHER workspace → 404 (cross-tenant
//! guard via `resolve`).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_bi::{
    shares_router, BiState, ChartRepo, DashboardRepo, DatasetRepo, NewColumn, SavedQueryRepo,
    SchemaService, SourceSpec, TagRepo,
};
use egret_metastore::Db;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

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

async fn seed_workspace(db: &Db, owner: Uuid) -> (Uuid, String) {
    let slug = format!("tagobj-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'Tag Obj Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");
    (ws, slug)
}

async fn seed_dataset(st: &BiState, ws: Uuid, owner: Uuid, name: &str) -> Uuid {
    let db_id: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id) \
         VALUES ($1,'trino','localhost',8080,$2) RETURNING id",
    )
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(ws)
    .fetch_one(&st.db.pool)
    .await
    .unwrap();
    st.datasets
        .create(
            ws,
            db_id,
            name,
            "",
            &SourceSpec::Virtual {
                sql: "SELECT 1".into(),
            },
            &[NewColumn {
                column_name: "one".into(),
                data_type: "integer".into(),
                type_generic: "numeric".into(),
                is_temporal: false,
            }],
            Some(egret_core::UserId(owner)),
        )
        .await
        .unwrap()
        .id
}

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).to_string()
}

#[tokio::test]
async fn object_tag_assign_list_unassign_and_gating() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let owner = seed_user(&db).await;
    let (ws, slug) = seed_workspace(&db, owner).await;

    let editor = seed_user(&db).await;
    add_member(&db, ws, editor, "ws_editor").await; // holds dataset:manage
    let viewer = seed_user(&db).await;
    add_member(&db, ws, viewer, "ws_viewer").await; // dataset:read only, no manage

    // A dataset in this workspace, and a dataset in a DIFFERENT workspace.
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = build_state(db.clone(), pdp);
    let sessions = st.sessions.clone();

    let ds = seed_dataset(&st, ws, owner, "DS").await;

    // Another workspace + dataset (cross-tenant target).
    let owner2 = seed_user(&db).await;
    let (ws2, _slug2) = seed_workspace(&db, owner2).await;
    let ds_other = seed_dataset(&st, ws2, owner2, "DS-other").await;

    // A classification tag to assign.
    let tag = TagRepo::new(db.clone())
        .create(
            &format!("pii-{}", Uuid::new_v4().simple()),
            "PII",
            "Personal data",
            "#ff0000",
            Some(owner),
        )
        .await
        .expect("create tag");
    let tag_id = tag.id;

    let editor_tok = session_for(&sessions, editor).await;
    let viewer_tok = session_for(&sessions, viewer).await;

    let app = shares_router().with_state(st);

    // 1. ws_editor (dataset:manage) assigns the tag → 200 { ok: true }.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/ws/{slug}/objects/dataset/{ds}/tags/{tag_id}"))
                .header("Cookie", format!("egret_session={editor_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let s = resp.status();
    assert!(
        s.is_success(),
        "ws_editor (dataset:manage) must assign a tag; got {s}: {}",
        body_string(resp).await
    );

    // 2. GET lists the assigned tag.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/ws/{slug}/objects/dataset/{ds}/tags"))
                .header("Cookie", format!("egret_session={editor_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let listed = body_string(resp).await;
    assert!(
        listed.contains(&tag_id.to_string()) && listed.contains("assignment_count"),
        "GET tags must list the assigned tag (snake_case dto); got {listed}"
    );

    // 3. A ws_viewer (no dataset:manage) PUT → 403.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/ws/{slug}/objects/dataset/{ds}/tags/{tag_id}"))
                .header("Cookie", format!("egret_session={viewer_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a ws_viewer (no dataset:manage) must NOT assign a tag"
    );

    // 4. A dataset in ANOTHER workspace → 404 (cross-tenant guard via resolve).
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/ws/{slug}/objects/dataset/{ds_other}/tags/{tag_id}"
                ))
                .header("Cookie", format!("egret_session={editor_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "assigning a tag to an object in another workspace must 404"
    );

    // 5. DELETE removes the tag → 200, then GET is empty.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/ws/{slug}/objects/dataset/{ds}/tags/{tag_id}"))
                .header("Cookie", format!("egret_session={editor_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "ws_editor must unassign the tag"
    );

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/ws/{slug}/objects/dataset/{ds}/tags"))
                .header("Cookie", format!("egret_session={editor_tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let listed = body_string(resp).await;
    assert!(
        !listed.contains(&tag_id.to_string()),
        "after unassign, GET tags must be empty; got {listed}"
    );
}
