//! Explicit-deny object share (B2-3 Task 6). DB-gated; self-skips without
//! DATABASE_URL. Builds a REAL `CedarPdp` AFTER `migrate()`.
//!
//! Proves: an Editor (`dashboard:manage`) can PUT a `deny` grant on an object;
//! `grant_list` returns it with `effect="deny"`; and a non-member holding ONLY a
//! deny grant on an object in the workspace is NOT admitted as an object-guest by
//! `has_object_grant_in_workspace` (admission filters `effect = 'allow'`).

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

async fn session_for(sessions: &egret_auth::session::SessionService, uid: Uuid) -> String {
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

/// An Editor can PUT a `deny` grant; `grant_list` reports it with `effect="deny"`.
#[tokio::test]
async fn editor_can_put_deny_grant_and_list_shows_it() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let owner = seed_user(&db).await;
    let slug = format!("deny-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'Deny Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");

    let editor = seed_user(&db).await;
    add_member(&db, ws, editor, "ws_editor").await; // holds dashboard:manage
    let blocked = seed_user(&db).await; // the principal to deny

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = build_state(db.clone(), pdp);
    let sessions = st.sessions.clone();
    let editor_tok = session_for(&sessions, editor).await;

    let d = st
        .dashboards
        .create(ws, "D", "", Some(owner))
        .await
        .unwrap()
        .id;

    let app = shares_router().merge(dashboard_router()).with_state(st);

    // Editor PUTs a deny grant (effect="deny") on D → 200.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/ws/{slug}/objects/dashboard/{d}/grants"))
                .header("Cookie", format!("egret_session={editor_tok}"))
                .header("Content-Type", "application/json")
                .body(Body::from(format!(
                    r#"{{"principal_type":"user","principal_id":"{blocked}","role":"ws_editor","effect":"deny"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    let s = resp.status();
    assert!(
        s.is_success(),
        "editor must PUT a deny grant on D; got {s}: {}",
        body_string(resp).await
    );
    let created = body_string(resp).await;
    let created_json: serde_json::Value = serde_json::from_str(&created).unwrap();
    assert_eq!(
        created_json["effect"].as_str(),
        Some("deny"),
        "PUT response DTO must echo effect=deny; got {created}"
    );

    // grant_list returns the deny grant with effect="deny".
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
    let arr: serde_json::Value = serde_json::from_str(&listed).unwrap();
    let row = arr
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["principal_id"].as_str() == Some(&blocked.to_string()))
        .expect("deny grant must be listed");
    assert_eq!(
        row["effect"].as_str(),
        Some("deny"),
        "grant_list must report effect=deny; got {listed}"
    );
}

/// The ShareDialog picker sends a user's EMAIL (or a group's name), not a UUID.
/// `grant_put` must resolve it server-side: a known email → 200 and a grant row
/// keyed by the user's real UUID; an unknown email → 4xx (NotFound), not 422.
#[tokio::test]
async fn grant_put_resolves_principal_by_email() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let owner = seed_user(&db).await;
    let slug = format!("resolve-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'Resolve Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");

    let editor = seed_user(&db).await;
    add_member(&db, ws, editor, "ws_editor").await; // holds dashboard:manage

    // The grantee, seeded with a KNOWN email — the value the picker sends.
    let target_email = format!("grantee_{}@egret.local", Uuid::new_v4().simple());
    let target: Uuid =
        sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
            .bind(&target_email)
            .fetch_one(&db.pool)
            .await
            .expect("insert grantee");

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = build_state(db.clone(), pdp);
    let sessions = st.sessions.clone();
    let editor_tok = session_for(&sessions, editor).await;

    let d = st
        .dashboards
        .create(ws, "D", "", Some(owner))
        .await
        .unwrap()
        .id;

    let app = shares_router().merge(dashboard_router()).with_state(st);

    // PUT a grant addressing the principal by EMAIL (not UUID) → 200.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/ws/{slug}/objects/dashboard/{d}/grants"))
                .header("Cookie", format!("egret_session={editor_tok}"))
                .header("Content-Type", "application/json")
                .body(Body::from(format!(
                    r#"{{"principal_type":"user","principal_id":"{target_email}","role":"ws_viewer","effect":"allow"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    let s = resp.status();
    assert!(
        s.is_success(),
        "PUT with an email principal_id must resolve and return 200 (not 422); got {s}: {}",
        body_string(resp).await
    );
    let created = body_string(resp).await;
    let created_json: serde_json::Value = serde_json::from_str(&created).unwrap();
    // The DTO must carry the resolved real UUID, not the email.
    assert_eq!(
        created_json["principal_id"].as_str(),
        Some(target.to_string().as_str()),
        "DTO principal_id must be the resolved UUID; got {created}"
    );

    // A grant row keyed by the target's REAL uuid must now exist.
    let cnt: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM access_grants \
         WHERE scope_type = 'object' AND scope_id = $1 AND principal_id = $2 AND effect = 'allow'",
    )
    .bind(d)
    .bind(target)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(cnt, 1, "exactly one allow grant for the resolved user uuid");

    // An unknown email must be rejected with a client error (NotFound), not 422.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/ws/{slug}/objects/dashboard/{d}/grants"))
                .header("Cookie", format!("egret_session={editor_tok}"))
                .header("Content-Type", "application/json")
                .body(Body::from(
                    r#"{"principal_type":"user","principal_id":"nobody@nowhere.invalid","role":"ws_viewer","effect":"allow"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let s = resp.status();
    assert!(
        s.is_client_error(),
        "unknown email must be a 4xx, got {s}: {}",
        body_string(resp).await
    );
}

/// A non-member holding ONLY a deny grant on an object in the workspace must NOT
/// be admitted as an object-guest (admission filters `effect = 'allow'`).
#[tokio::test]
async fn lone_deny_grant_does_not_admit() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let authz = egret_workspace::rbac::Authorizer::new(db.clone());
    let user = egret_core::ids::UserId::new();

    let owner = seed_user(&db).await;
    let slug = format!("lonedeny-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'Lone Deny', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");

    // Also seed the user row so the id is real (not required by the query, but
    // keeps the fixture coherent).
    sqlx::query("INSERT INTO users (id, provider, email) VALUES ($1, 'local', $2)")
        .bind(user.0)
        .bind(format!("u_{}@egret.local", user.0.simple()))
        .execute(&db.pool)
        .await
        .expect("insert user");

    let dash: Uuid = sqlx::query_scalar(
        "INSERT INTO analytics.dashboards (workspace_id, name, description, created_by) \
         VALUES ($1, 'D', '', $2) RETURNING id",
    )
    .bind(ws)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert dashboard");

    let editor: Uuid =
        sqlx::query_scalar("SELECT id FROM roles WHERE scope='workspace' AND name='ws_editor'")
            .fetch_one(&db.pool)
            .await
            .unwrap();

    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, object_type, scope_id, effect) \
         VALUES ('user', $1, $2, 'object', 'dashboard', $3, 'deny')",
    )
    .bind(user.0)
    .bind(editor)
    .bind(dash)
    .execute(&db.pool)
    .await
    .unwrap();

    assert!(
        !authz
            .has_object_grant_in_workspace(user, egret_core::WorkspaceId(ws))
            .await
            .unwrap(),
        "a lone deny grant must not admit an object-guest"
    );
}

/// `grant_put` accepts an `expires_at` in the request body (P2a Task 2, part A)
/// and persists it on the `access_grants` row. Omitting it must leave the row's
/// `expires_at` NULL (never expires — unchanged default behavior).
#[tokio::test]
async fn grant_put_persists_expires_at_when_given_and_null_when_omitted() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let owner = seed_user(&db).await;
    let slug = format!("expiry-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'Expiry Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");

    let editor = seed_user(&db).await;
    add_member(&db, ws, editor, "ws_editor").await; // holds dashboard:manage
    let with_expiry = seed_user(&db).await;
    let without_expiry = seed_user(&db).await;

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = build_state(db.clone(), pdp);
    let sessions = st.sessions.clone();
    let editor_tok = session_for(&sessions, editor).await;

    let d = st
        .dashboards
        .create(ws, "D", "", Some(owner))
        .await
        .unwrap()
        .id;

    let app = shares_router().merge(dashboard_router()).with_state(st);

    // PUT a grant WITH expires_at set (1 day out, RFC3339) → 200.
    let expiry = chrono::Utc::now() + chrono::Duration::days(1);
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/ws/{slug}/objects/dashboard/{d}/grants"))
                .header("Cookie", format!("egret_session={editor_tok}"))
                .header("Content-Type", "application/json")
                .body(Body::from(format!(
                    r#"{{"principal_type":"user","principal_id":"{with_expiry}","role":"ws_viewer","effect":"allow","expires_at":"{}"}}"#,
                    expiry.to_rfc3339()
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    let s = resp.status();
    assert!(
        s.is_success(),
        "grant_put with expires_at must succeed; got {s}: {}",
        body_string(resp).await
    );

    // PUT a grant WITHOUT expires_at → 200, row stays NULL.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/ws/{slug}/objects/dashboard/{d}/grants"))
                .header("Cookie", format!("egret_session={editor_tok}"))
                .header("Content-Type", "application/json")
                .body(Body::from(format!(
                    r#"{{"principal_type":"user","principal_id":"{without_expiry}","role":"ws_viewer","effect":"allow"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    let s = resp.status();
    assert!(
        s.is_success(),
        "grant_put without expires_at must still succeed; got {s}: {}",
        body_string(resp).await
    );

    // Read both rows back directly from access_grants.
    let persisted: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT expires_at FROM access_grants \
         WHERE scope_type = 'object' AND scope_id = $1 AND principal_id = $2",
    )
    .bind(d)
    .bind(with_expiry)
    .fetch_one(&db.pool)
    .await
    .expect("fetch grant with expiry");
    let persisted = persisted.expect("expires_at must be populated when supplied in the body");
    assert!(
        (persisted - expiry).num_seconds().abs() <= 1,
        "persisted expires_at ({persisted}) must match the requested value ({expiry})"
    );

    let unset: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT expires_at FROM access_grants \
         WHERE scope_type = 'object' AND scope_id = $1 AND principal_id = $2",
    )
    .bind(d)
    .bind(without_expiry)
    .fetch_one(&db.pool)
    .await
    .expect("fetch grant without expiry");
    assert!(
        unset.is_none(),
        "expires_at must be NULL when omitted from the body; got {unset:?}"
    );
}
