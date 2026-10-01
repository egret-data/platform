//! Router smoke test — unauthenticated requests rejected — plus a PDP-backed
//! workspace-authz gate (this seam migrated onto `egret_core::authz::require`).
use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_bi::{
    chart_router, dashboard_router, dataset_router, sqllab_router, BiState, ChartRepo,
    DashboardRepo, DatasetRepo, SavedQueryRepo, SchemaService,
};
use egret_metastore::Db;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

/// Incidental PDP for the `requires_auth` smoke tests: those requests are
/// rejected at the session layer (401) before any authz check runs, so an
/// allow-all stub keeps them light. The `pdp_backed_*` test below builds a real
/// `CedarPdp` instead — an allow-all stub would falsely 200 the denied case.
struct AllowAllPdp;
#[async_trait::async_trait]
impl egret_core::authz::Pdp for AllowAllPdp {
    async fn authorize(
        &self,
        _s: &egret_core::authz::Subject,
        _a: &str,
        _r: &egret_core::authz::Resource,
    ) -> egret_core::Result<egret_core::authz::Decision> {
        Ok(egret_core::authz::Decision {
            allow: true,
            reasons: vec![],
        })
    }
}

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

async fn state() -> Option<BiState> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(build_state(db, Arc::new(AllowAllPdp)))
}

#[tokio::test]
async fn saved_list_requires_auth() {
    let Some(st) = state().await else {
        eprintln!("skip");
        return;
    };
    let app = sqllab_router().with_state(st);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ws/eng/sqllab/queries")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn schema_catalogs_requires_auth() {
    let Some(st) = state().await else {
        eprintln!("skip");
        return;
    };
    let app = sqllab_router().with_state(st);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ws/eng/sqllab/databases/00000000-0000-0000-0000-000000000000/catalogs")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn dataset_list_requires_auth() {
    let Some(st) = state().await else {
        eprintln!("skip");
        return;
    };
    let app = dataset_router().with_state(st);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ws/eng/datasets")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn chart_list_requires_auth() {
    let Some(st) = state().await else {
        eprintln!("skip");
        return;
    };
    let app = chart_router().with_state(st);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ws/eng/charts")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn dashboard_list_requires_auth() {
    let Some(st) = state().await else {
        eprintln!("skip");
        return;
    };
    let app = dashboard_router().with_state(st);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/ws/eng/dashboards")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// Seed a workspace-scoped role holding `permission` and return its id + name.
/// The PDP snapshots `roles`/`role_permissions` at build, so this must run
/// BEFORE the `CedarPdp::build` below.
async fn seed_workspace_role(db: &Db, permission: &str) -> (Uuid, String) {
    let name = format!("bi_authz_{}", Uuid::new_v4().simple());
    let role_id: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (scope, name, description) \
         VALUES ('workspace', $1, 'bi authz test role') RETURNING id",
    )
    .bind(&name)
    .fetch_one(&db.pool)
    .await
    .expect("insert role");
    sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2)")
        .bind(role_id)
        .bind(permission)
        .execute(&db.pool)
        .await
        .expect("grant permission");
    (role_id, name)
}

/// Create a local user, add them to `ws` under workspace `role` (the
/// `sync_workspace_grant` trigger mirrors this into `access_grants`, which the
/// PDP reads), and return an authenticated session cookie token.
async fn member_session(
    db: &Db,
    sessions: &egret_auth::session::SessionService,
    ws: Uuid,
    role: &str,
) -> String {
    let uid: Uuid =
        sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
            .bind(format!("m_{}@egret.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .expect("insert user");
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
    .expect("add workspace member");
    sessions
        .create(egret_core::ids::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
}

/// `data_api_endpoints` is a valid `{kind}` on the generic folder router
/// (`kind_perms`), the same way `notebooks` and `alerts` share it despite
/// living in other crates. Without the arm, `kind_perms` returns `None` and the
/// route 404s — which would mean Data API endpoints can never appear in the
/// Access Control console's folder tree alongside dashboards and datasets. A
/// workspace member holding `data_api_endpoint:read`, resolved by a real
/// `CedarPdp`, must be able to list its (empty) folders.
#[tokio::test]
async fn data_api_endpoint_folders_are_a_known_kind() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let owner: Uuid =
        sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
            .bind(format!("owner_{}@egret.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .expect("insert owner");
    let slug = format!("bi-dataapi-folders-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'DataAPI Folders Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");

    let (_role, role_name) = seed_workspace_role(&db, "data_api_endpoint:read").await;

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = build_state(db.clone(), pdp);
    let sessions = st.sessions.clone();
    let reader = member_session(&db, &sessions, ws, &role_name).await;

    let app = egret_bi::folder_router().with_state(st);
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/ws/{slug}/folders/data_api_endpoints"))
                .header("Cookie", format!("egret_session={reader}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "endpoints must appear in the Access Control console like dashboards and datasets"
    );
}

/// `apps` is a valid `{kind}` on the generic folder router (`kind_perms`), the
/// way `data_api_endpoints` is: data apps live in egret-apps and file into
/// `analytics.folders` with `entity_type = 'app'`. A `ws_editor` (holds
/// `app:write` from migration 0162), resolved by a real `CedarPdp`, can create
/// one; without the arm the route 404s.
#[tokio::test]
async fn app_folders_are_a_known_kind() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let owner: Uuid =
        sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
            .bind(format!("owner_{}@egret.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .expect("insert owner");
    let slug = format!("bi-app-folders-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'App Folders Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = build_state(db.clone(), pdp);
    let sessions = st.sessions.clone();
    let editor = member_session(&db, &sessions, ws, "ws_editor").await;

    let app = egret_bi::folder_router().with_state(st);
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/ws/{slug}/folders/apps"))
                .header("Cookie", format!("egret_session={editor}"))
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"name":"Sales apps"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "a ws_editor creates an apps folder"
    );

    let entity_type: String = sqlx::query_scalar(
        "SELECT entity_type FROM analytics.folders WHERE workspace_id = $1 AND name = 'Sales apps'",
    )
    .bind(ws)
    .fetch_one(&db.pool)
    .await
    .expect("folder row");
    assert_eq!(entity_type, "app");
}

/// PDP-backed workspace authz (this seam migrated onto
/// `egret_core::authz::require`): a workspace member whose workspace role grants
/// `dashboard:read` — resolved by a real `CedarPdp`, not the allow-all stub —
/// can GET the dashboards list (200); a member whose role lacks it is denied
/// (403). The 403 fires on the handler's first line, exercising the migrated
/// decision point (an allow-all stub would falsely 200 it).
#[tokio::test]
async fn pdp_backed_dashboard_read_gate() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    // Workspace + owner.
    let owner: Uuid =
        sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
            .bind(format!("owner_{}@egret.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .expect("insert owner");
    let slug = format!("bi-authz-{}", Uuid::new_v4().simple());
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'BI Authz Test', $2, 'active') RETURNING id",
    )
    .bind(&slug)
    .bind(owner)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");

    // Two workspace roles: one grants dashboard:read, one only chart:read. Seed
    // BEFORE building the PDP so it snapshots them.
    let (_reader_role, reader_name) = seed_workspace_role(&db, "dashboard:read").await;
    let (_other_role, other_name) = seed_workspace_role(&db, "chart:read").await;

    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let st = build_state(db.clone(), pdp);
    let sessions = st.sessions.clone();

    // Members (workspace-scope access_grants inserted directly).
    let reader = member_session(&db, &sessions, ws, &reader_name).await;
    let non_reader = member_session(&db, &sessions, ws, &other_name).await;

    let app = dashboard_router().with_state(st);

    // Member with dashboard:read → 200.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/ws/{slug}/dashboards"))
                .header("Cookie", format!("egret_session={reader}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "workspace member holding dashboard:read must list dashboards"
    );

    // Member without dashboard:read → 403 (denied by the PDP `require`).
    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/ws/{slug}/dashboards"))
                .header("Cookie", format!("egret_session={non_reader}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "workspace member lacking dashboard:read must be denied"
    );
}
