//! Per-object authorization for the `notebooks/{id}` handlers (Effort B / B2-2
//! Task 2). DB-gated; self-skips without DATABASE_URL. Builds a REAL `CedarPdp`
//! AFTER `migrate()` so it snapshots the `ws_viewer`/`ws_editor` notebook perms.
//!
//! Proves notebooks are now object-scoped (`require_object`) AND ROLE-BASED (the
//! owner/public gate is gone — the private-by-default feature is intentionally
//! dropped):
//!   - an OUTSIDE user (not a workspace member) with a `ws_viewer` object grant
//!     on notebook N READs it (200) but not a sibling N2 (403), and cannot WRITE
//!     it (403 — viewer lacks notebook:write);
//!   - an OUTSIDE user with a `ws_editor` object grant on N UPDATEs N (2xx) but
//!     not the sibling N2 (403);
//!   - a workspace `ws_editor` MEMBER (NOT the notebook owner A) reads AND edits
//!     A's notebook N (2xx) — the key change: a non-owner member now edits
//!     others' notebooks;
//!   - a workspace `ws_viewer` MEMBER reads N (200) but cannot write (403).
//!
//! The outside grantees reaching the handler at all also proves Task 1's
//! object-guest admission (a notebook-only-shared non-member is admitted).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_metastore::Db;
use egret_notebook::{notebook_router, NotebookState, NotebookStore};
use tower::ServiceExt;
use uuid::Uuid;

// ── Fake EngineTokenMinter (no IdP in tests) ─────────────────────────────────
struct FakeEngineMinter;

#[async_trait::async_trait]
impl egret_notebook::engine_token::EngineTokenMinter for FakeEngineMinter {
    async fn mint(
        &self,
        _user: Uuid,
    ) -> Result<egret_notebook::engine_token::EngineToken, egret_notebook::engine_token::BrokerError>
    {
        Ok(egret_notebook::engine_token::EngineToken {
            access_token: "fake.engine.jwt".into(),
            expires_at: chrono::DateTime::<chrono::Utc>::from_timestamp(4102444800, 0).unwrap(),
        })
    }
}

fn build_state(db: Db, pdp: Arc<dyn egret_core::authz::Pdp>) -> NotebookState {
    NotebookState {
        store: NotebookStore::new(db.clone()),
        catalog: egret_catalog_engine::CatalogStore::new(db.clone()),
        sessions: egret_auth::session::SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        workspaces: egret_workspace::workspaces::WorkspaceRepo::new(db.clone()),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        pdp,
        users: egret_auth::users::UserRepo::new(db.clone()),
        catalog_base_url: "http://localhost:8181".into(),
        engine_minter: Arc::new(FakeEngineMinter),
        keys: egret_machine_idp::PgSigningKeyStore::new(db.clone()),
        idp_issuer: "http://localhost:8080".into(),
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

/// Seed a direct object grant of workspace `role` to `uid` on notebook `nb`.
async fn grant_object(db: &Db, uid: Uuid, nb: Uuid, role: &str, granted_by: Uuid) {
    let role_id: Uuid =
        sqlx::query_scalar("SELECT id FROM roles WHERE scope = 'workspace' AND name = $1")
            .bind(role)
            .fetch_one(&db.pool)
            .await
            .expect("role id");
    sqlx::query(
        "INSERT INTO access_grants \
           (principal_type, principal_id, role_id, scope_type, object_type, scope_id, granted_by) \
         VALUES ('user', $1, $2, 'object', 'notebook', $3, $4) ON CONFLICT DO NOTHING",
    )
    .bind(uid)
    .bind(role_id)
    .bind(nb)
    .bind(granted_by)
    .execute(&db.pool)
    .await
    .expect("seed object grant");
}

async fn get_notebook(app: &axum::Router, slug: &str, nb: Uuid, tok: &str) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(format!("/ws/{slug}/notebooks/{nb}"))
                .header("Cookie", format!("egret_session={tok}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// PATCH a notebook rename (WRITE).
async fn update_notebook(
    app: &axum::Router,
    slug: &str,
    nb: Uuid,
    tok: &str,
    name: &str,
) -> StatusCode {
    let body = format!(r#"{{"name":"{name}"}}"#);
    app.clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/ws/{slug}/notebooks/{nb}"))
                .header("Cookie", format!("egret_session={tok}"))
                .header("Content-Type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn notebook_per_object_handlers_are_object_scoped() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    // Sanity: ws_viewer holds notebook:read but NOT notebook:write; ws_editor
    // holds both. Underpins read-yes / write-no for viewers.
    let viewer_read: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM role_permissions rp JOIN roles r ON r.id = rp.role_id \
         WHERE r.scope = 'workspace' AND r.name = 'ws_viewer' AND rp.permission = 'notebook:read'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(viewer_read, 1, "ws_viewer must have notebook:read");
    let viewer_write: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM role_permissions rp JOIN roles r ON r.id = rp.role_id \
         WHERE r.scope = 'workspace' AND r.name = 'ws_viewer' AND rp.permission = 'notebook:write'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(viewer_write, 0, "ws_viewer must NOT have notebook:write");

    // Notebook owner A (creator only; not a member — reads/edits below come from
    // role/share, not ownership).
    let owner_a = seed_user(&db).await;
    let slug = format!("nbobj-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'NB Obj Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner_a)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");

    // Principals: two workspace MEMBERS (editor + viewer, neither is owner A) and
    // two OUTSIDE users (not members) — one gets a Viewer object grant, one an
    // Editor object grant.
    let editor_member = seed_user(&db).await;
    add_member(&db, ws, editor_member, "ws_editor").await;
    let viewer_member = seed_user(&db).await;
    add_member(&db, ws, viewer_member, "ws_viewer").await;
    let outside_viewer = seed_user(&db).await;
    let outside_editor = seed_user(&db).await;

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = build_state(db.clone(), pdp);
    let sessions = st.sessions.clone();
    let store = st.store.clone();

    let editor_tok = session_for(&sessions, editor_member).await;
    let viewer_tok = session_for(&sessions, viewer_member).await;
    let outside_viewer_tok = session_for(&sessions, outside_viewer).await;
    let outside_editor_tok = session_for(&sessions, outside_editor).await;

    // Two notebooks in the workspace, BOTH owned by owner A: N (shared) and N2
    // (sibling, not shared). N2 proves a non-owner member edits a notebook they
    // don't own.
    let n = store.create(ws, owner_a, "N").await.unwrap();
    let n2 = store.create(ws, owner_a, "N2").await.unwrap();

    // Direct object grants seeded straight into access_grants.
    grant_object(&db, outside_viewer, n, "ws_viewer", owner_a).await;
    grant_object(&db, outside_editor, n, "ws_editor", owner_a).await;

    let app = notebook_router().with_state(st);

    // 1. Outside VIEWER: reads N via object grant (200, also proves Task 1's
    //    object-guest admission), denied sibling N2 (403), denied WRITE (viewer
    //    lacks notebook:write → 403).
    assert_eq!(
        get_notebook(&app, &slug, n, &outside_viewer_tok).await,
        StatusCode::OK,
        "outside ws_viewer object-grant must READ its granted notebook N (200)"
    );
    assert_eq!(
        get_notebook(&app, &slug, n2, &outside_viewer_tok).await,
        StatusCode::FORBIDDEN,
        "outside ws_viewer must NOT read ungranted sibling N2 (403)"
    );
    assert_eq!(
        update_notebook(&app, &slug, n, &outside_viewer_tok, "v_rename").await,
        StatusCode::FORBIDDEN,
        "outside ws_viewer must NOT update N (viewer lacks notebook:write → 403)"
    );

    // 2. Outside EDITOR: updates the granted N (2xx) but not the sibling N2 (403).
    let editor_update_n = update_notebook(&app, &slug, n, &outside_editor_tok, "e_rename").await;
    assert!(
        editor_update_n.is_success(),
        "outside ws_editor object-grant must UPDATE its granted notebook N (2xx); got {editor_update_n}"
    );
    assert_eq!(
        update_notebook(&app, &slug, n2, &outside_editor_tok, "e_rename2").await,
        StatusCode::FORBIDDEN,
        "outside ws_editor must NOT update ungranted sibling N2 (403)"
    );

    // 3. Workspace ws_editor MEMBER reads + updates ANY notebook. N2 is owned by
    //    owner A, NOT the member — a successful update here locks in the ROLE-BASED
    //    edit model (owner gate removed).
    assert_eq!(
        get_notebook(&app, &slug, n, &editor_tok).await,
        StatusCode::OK,
        "ws_editor member must read N (200)"
    );
    let member_update = update_notebook(&app, &slug, n2, &editor_tok, "m_rename").await;
    assert!(
        member_update.is_success(),
        "ws_editor member must update a notebook they do NOT own (role-based, 2xx); got {member_update}"
    );

    // 4. Workspace ws_viewer MEMBER reads any notebook (200) but cannot write (403).
    assert_eq!(
        get_notebook(&app, &slug, n, &viewer_tok).await,
        StatusCode::OK,
        "ws_viewer member reads any workspace notebook (200)"
    );
    assert_eq!(
        update_notebook(&app, &slug, n, &viewer_tok, "vm_rename").await,
        StatusCode::FORBIDDEN,
        "ws_viewer member must NOT write (lacks notebook:write → 403)"
    );
}
