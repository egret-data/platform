//! Per-object authorization for the per-notebook kernel handlers (Effort B /
//! B2-2 Task 3). Builds a REAL `CedarPdp` (after migrate) so it snapshots the
//! `ws_viewer`/`ws_editor` notebook perms and consults `access_grants` live.
//!
//! Proves a shared notebook's kernel is reachable object-scoped:
//!   - an OUTSIDE user (not a workspace member) holding a `ws_viewer` object
//!     grant on notebook N reaches `connect` for N (authz passes — the object
//!     gate + object-guest admission both fire), while `connect` for an ungranted
//!     sibling N2 is denied (403);
//!   - a workspace `ws_viewer` MEMBER (role-based) reaches `connect` for any
//!     workspace notebook.
//!
//! The outside grantee reaching the handler at all also proves Task 1's
//! object-guest admission (a notebook-only-shared non-member is admitted).
//!
//! All tests receive a fresh auto-migrated scratch DB via `#[sqlx::test]`.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_core::ids::{UserId, WorkspaceId};
use egret_kernel::kernel_router;
use egret_kernel::launcher::FakeLauncher;
use egret_kernel::manager::KernelManager;
use egret_kernel::registry::KernelRegistry;
use egret_kernel::routes::KernelState;
use egret_metastore::Db;
use egret_notebook::{NotebookState, NotebookStore};
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

// ── Stubs ─────────────────────────────────────────────────────────────────────

/// Returns a canned engine token regardless of user.
struct StubMinter;

#[async_trait::async_trait]
impl egret_notebook::engine_token::EngineTokenMinter for StubMinter {
    async fn mint(
        &self,
        _u: uuid::Uuid,
    ) -> Result<egret_notebook::engine_token::EngineToken, egret_notebook::engine_token::BrokerError>
    {
        Ok(egret_notebook::engine_token::EngineToken {
            access_token: "stub.jwt".into(),
            expires_at: chrono::DateTime::<chrono::Utc>::from_timestamp(4102444800, 0).unwrap(),
        })
    }
}

/// Incidental PDP for the `NotebookState` used only as a `NotebookDocsPort`
/// (never consults `pdp`). Enforcement uses `KernelState.pdp` (the real Cedar).
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

// ── Fixture ──────────────────────────────────────────────────────────────────

struct Fixture {
    app: axum::Router,
    users: UserRepo,
    sessions: SessionService,
    slug: String,
    ws: WorkspaceId,
    owner: UserId,
    db: Db,
    launcher: Arc<FakeLauncher>,
}

/// Real `CedarPdp` snapshotted against the migrated `role_permissions`.
async fn build_pdp(db: &Db) -> Arc<dyn egret_core::authz::Pdp> {
    Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    )
}

async fn setup(pool: sqlx::PgPool) -> Fixture {
    let db = Db { pool };

    let users = UserRepo::new(db.clone());
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );

    let suffix = Uuid::new_v4().simple().to_string();
    let owner = users
        .insert_local(&format!("kobjowner-{suffix}@egret.test"), "Owner", "x")
        .await
        .expect("seed owner");

    let workspaces = WorkspaceRepo::new(db.clone());
    let slug = format!("kobj{suffix}");
    let ws = workspaces
        .create(&slug, "Kernel Obj WS", owner)
        .await
        .expect("seed workspace")
        .id;

    let launcher = Arc::new(FakeLauncher::default());
    let registry = KernelRegistry::new(db.clone());
    let manager = Arc::new(KernelManager::new(
        registry,
        launcher.clone(),
        std::time::Duration::from_secs(1800),
    ));

    let notebook_state = NotebookState {
        store: NotebookStore::new(db.clone()),
        catalog: egret_catalog_engine::CatalogStore::new(db.clone()),
        sessions: sessions.clone(),
        workspaces: workspaces.clone(),
        authz: Authorizer::new(db.clone()),
        pdp: Arc::new(AllowAllPdp),
        users: UserRepo::new(db.clone()),
        catalog_base_url: "http://localhost:8181".into(),
        engine_minter: Arc::new(StubMinter),
    };

    let pdp = build_pdp(&db).await;

    let state = KernelState {
        manager,
        notebook_docs: Arc::new(notebook_state),
        sessions: sessions.clone(),
        workspaces: workspaces.clone(),
        authz: Authorizer::new(db.clone()),
        pdp,
        api_url: "http://localhost:3000".into(),
        notebook_kernels: egret_kernel::NotebookKernelStore::new(db.clone()),
        proxy: Arc::new(egret_kernel::BackendWsProxy),
        engine_minter: Arc::new(StubMinter),
        engine_conn: Arc::new(egret_kernel::engine_conn::FakeEngineConnResolver {
            conn: Some(egret_kernel::engine_conn::EngineConn {
                kind: "trino".into(),
                host: "trino".into(),
                port: 8080,
            }),
        }),
        active: Default::default(),
    };
    let app = kernel_router().with_state(state);

    Fixture {
        app,
        users,
        sessions,
        slug,
        ws,
        owner,
        db,
        launcher,
    }
}

/// Seed a notebook owned by `owner` in `ws`. Returns its id.
async fn seed_notebook(db: &Db, ws: WorkspaceId, owner: UserId, name: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO analytics.notebooks (workspace_id, owner_user_id, name) VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(ws.0)
    .bind(owner.0)
    .bind(name)
    .fetch_one(&db.pool)
    .await
    .expect("seed notebook")
}

/// Insert a user (no membership). Returns `(user_id, session_token)`.
async fn seed_user_with_session(f: &Fixture, tag: &str) -> (UserId, String) {
    let email = format!("{tag}_{}@egret.local", Uuid::new_v4().simple());
    let uid = f
        .users
        .insert_local(&email, "Test", "x")
        .await
        .expect("insert user");
    let token = f
        .sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("create session")
        .token;
    (uid, token)
}

/// Add `uid` as a member of the workspace with `role`.
async fn add_member(f: &Fixture, uid: UserId, role: &str) {
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id, granted_by) \
         SELECT 'user', $2, r.id, 'workspace', $1, $4 FROM roles r \
         WHERE r.scope = 'workspace' AND r.name = $3 ON CONFLICT DO NOTHING",
    )
    .bind(f.ws.0)
    .bind(uid.0)
    .bind(role)
    .bind(f.owner.0)
    .execute(&f.db.pool)
    .await
    .expect("add member");
}

/// Seed a direct object grant of workspace `role` to `uid` on notebook `nb`.
async fn grant_object(f: &Fixture, uid: UserId, nb: Uuid, role: &str) {
    let role_id: Uuid =
        sqlx::query_scalar("SELECT id FROM roles WHERE scope = 'workspace' AND name = $1")
            .bind(role)
            .fetch_one(&f.db.pool)
            .await
            .expect("role id");
    sqlx::query(
        "INSERT INTO access_grants \
           (principal_type, principal_id, role_id, scope_type, object_type, scope_id, granted_by) \
         VALUES ('user', $1, $2, 'object', 'notebook', $3, $4) ON CONFLICT DO NOTHING",
    )
    .bind(uid.0)
    .bind(role_id)
    .bind(nb)
    .bind(f.owner.0)
    .execute(&f.db.pool)
    .await
    .expect("seed object grant");
}

/// A fake Jupyter Kernel Gateway (POST creates kernel "k1"; GET k1 → 200).
async fn spawn_fake_gateway() -> String {
    use axum::routing::{get, post};
    async fn create() -> axum::Json<serde_json::Value> {
        axum::Json(serde_json::json!({ "id": "k1" }))
    }
    async fn get_one(axum::extract::Path(id): axum::extract::Path<String>) -> StatusCode {
        if id == "k1" {
            StatusCode::OK
        } else {
            StatusCode::NOT_FOUND
        }
    }
    let app = axum::Router::new()
        .route("/api/kernels", post(create))
        .route("/api/kernels/{id}", get(get_one));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn connect_req(slug: &str, nb: Uuid, token: &str) -> HttpRequest<Body> {
    HttpRequest::builder()
        .method("POST")
        .uri(format!("/ws/{slug}/notebooks/{nb}/kernel/connect"))
        .header("Cookie", format!("egret_session={token}"))
        .body(Body::empty())
        .unwrap()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// An OUTSIDE user with a `ws_viewer` object grant on notebook N reaches
/// `connect` for N (authz passes — object gate + object-guest admission), and
/// with a ready fake gateway gets a 200 kernel. The ungranted sibling N2 → 403.
#[sqlx::test(migrations = "../../migrations")]
async fn outside_object_grant_reaches_shared_notebook_kernel(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    // Two notebooks owned by the workspace owner; only N is shared to the outsider.
    let n = seed_notebook(&f.db, f.ws, f.owner, "N").await;
    let n2 = seed_notebook(&f.db, f.ws, f.owner, "N2").await;

    let (outsider, tok) = seed_user_with_session(&f, "outside_viewer").await;
    grant_object(&f, outsider, n, "ws_viewer").await;

    // Ready gateway so a passing gate reaches a clean 200.
    let gw = spawn_fake_gateway().await;
    f.launcher.inner.lock().unwrap().url = Some(gw);

    // N: object gate passes → handler reached → 200 (definitely NOT 401/403).
    let resp = f
        .app
        .clone()
        .oneshot(connect_req(&f.slug, n, &tok))
        .await
        .unwrap();
    let status = resp.status();
    assert!(
        status != StatusCode::UNAUTHORIZED && status != StatusCode::FORBIDDEN,
        "outside object-grant on N must REACH the kernel handler (not 401/403); got {status}"
    );
    assert_eq!(
        status,
        StatusCode::OK,
        "outside object-grant on N reaches a ready gateway → 200"
    );

    // N2: no grant, no ws role → denied by require_object → 403.
    let resp = f
        .app
        .clone()
        .oneshot(connect_req(&f.slug, n2, &tok))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "outside user must NOT reach the kernel for an ungranted sibling N2 (403)"
    );
}

/// A workspace `ws_viewer` MEMBER (role-based) reaches `connect` for any
/// workspace notebook — no per-object grant needed.
#[sqlx::test(migrations = "../../migrations")]
async fn ws_viewer_member_reaches_any_notebook_kernel(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    let n = seed_notebook(&f.db, f.ws, f.owner, "N").await;

    let (member, tok) = seed_user_with_session(&f, "viewer_member").await;
    add_member(&f, member, "ws_viewer").await;

    let gw = spawn_fake_gateway().await;
    f.launcher.inner.lock().unwrap().url = Some(gw);

    let resp = f
        .app
        .clone()
        .oneshot(connect_req(&f.slug, n, &tok))
        .await
        .unwrap();
    let status = resp.status();
    assert!(
        status != StatusCode::UNAUTHORIZED && status != StatusCode::FORBIDDEN,
        "ws_viewer member must REACH the kernel handler for a workspace notebook; got {status}"
    );
    assert_eq!(
        status,
        StatusCode::OK,
        "ws_viewer member reaches a ready gateway → 200"
    );
}
