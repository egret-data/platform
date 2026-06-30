//! Kernel lifecycle HTTP route tests.
//!
//! Covers:
//! - `POST /ws/{ws}/kernel` → 200 `{phase:"ready", url}`; FakeLauncher received
//!   env with `EGRET_NOTEBOOK_RUNTIME` key.
//! - `GET /ws/{ws}/kernel` → 200 `{phase, url}` after start.
//! - `DELETE /ws/{ws}/kernel` → 204; subsequent GET returns `{phase:"none"}`.
//! - NON-member → 403/404 on POST (auth gate fires BEFORE provisioning).
//! - User A's POST creates a row keyed to A; user B's GET in the same ws returns
//!   their own (none), not A's row — per-(user×workspace) keying.
//!
//! All tests receive a fresh auto-migrated scratch DB via `#[sqlx::test]` and
//! never touch the shared dev `egret` database.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::response::Response;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_catalog::service::CatalogStore;
use egret_core::ids::{UserId, WorkspaceId};
use egret_kernel::kernel_router;
use egret_kernel::launcher::FakeLauncher;
use egret_kernel::manager::KernelManager;
use egret_kernel::registry::KernelRegistry;
use egret_kernel::routes::KernelState;
use egret_metastore::Db;
use egret_notebook::error::NotebookApiError;
use egret_notebook::{NamespaceProvisioner, NotebookState, NotebookStore};
use egret_workspace::members::MemberRepo;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use http_body_util::BodyExt;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

// ── Fake NamespaceProvisioner ────────────────────────────────────────────────

/// Always succeeds; no lakekeeper in unit tests.
struct FakeProvisioner;

#[async_trait::async_trait]
impl NamespaceProvisioner for FakeProvisioner {
    async fn ensure_namespace(
        &self,
        _warehouse_id: Uuid,
        _namespace: &str,
    ) -> Result<(), NotebookApiError> {
        Ok(())
    }
}

// ── Stub EngineTokenMinter ────────────────────────────────────────────────────

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

// ── Fixture ──────────────────────────────────────────────────────────────────

struct Fixture {
    app: axum::Router,
    users: UserRepo,
    members: MemberRepo,
    sessions: SessionService,
    slug: String,
    ws: WorkspaceId,
    owner: UserId,
    db: Db,
    launcher: Arc<FakeLauncher>,
}

async fn into_json(resp: Response) -> (StatusCode, serde_json::Value) {
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(
        |_| serde_json::json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }),
    );
    (status, json)
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
        .insert_local(
            &format!("kernelowner-{suffix}@egret.test"),
            "Kernel Owner",
            "x",
        )
        .await
        .expect("seed owner");

    let workspaces = WorkspaceRepo::new(db.clone());
    let slug = format!("kws{suffix}");
    let ws_created = workspaces
        .create(&slug, "Kernel WS", owner)
        .await
        .expect("seed workspace");
    let ws = ws_created.id;

    let launcher = Arc::new(FakeLauncher::default());
    let registry = KernelRegistry::new(db.clone());
    let manager = Arc::new(KernelManager::new(
        registry,
        launcher.clone(),
        std::time::Duration::from_secs(1800),
    ));

    let notebook_state = NotebookState {
        store: NotebookStore::new(db.clone()),
        sessions: sessions.clone(),
        workspaces: workspaces.clone(),
        authz: Authorizer::new(db.clone()),
        catalog: CatalogStore::new(db.clone()),
        provisioner: Arc::new(FakeProvisioner),
        users: UserRepo::new(db.clone()),
        catalog_base_url: "http://localhost:8181".into(),
        engine_minter: Arc::new(StubMinter),
    };

    let app = kernel_router().with_state(KernelState {
        manager,
        notebook: notebook_state,
        sessions: sessions.clone(),
        workspaces: workspaces.clone(),
        authz: Authorizer::new(db.clone()),
        api_url: "http://localhost:3000".into(),
        notebook_kernels: egret_kernel::NotebookKernelStore::new(db.clone()),
        proxy: Arc::new(egret_kernel::BackendWsProxy),
        query_runner: Arc::new(egret_kernel::query::FakeQueryRunner::default()),
        engine_minter: Arc::new(StubMinter),
    });

    Fixture {
        app,
        users,
        members: MemberRepo::new(db.clone()),
        sessions,
        slug,
        ws,
        owner,
        db,
        launcher,
    }
}

/// Seed a workspace_warehouses row so `build_notebook_runtime` can find a warehouse.
async fn seed_warehouse(db: &Db, ws_id: WorkspaceId) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO workspace_warehouses (workspace_id, warehouse_id, name) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(ws_id.0)
    .bind(Uuid::new_v4())
    .bind(format!("ktest-wh-{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("seed warehouse")
}

/// Seed a notebook owned by `owner` in `ws`. Returns its id.
async fn seed_notebook(db: &Db, ws: WorkspaceId, owner: UserId) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO notebooks (workspace_id, owner_user_id, name) VALUES ($1,$2,'nb') RETURNING id",
    )
    .bind(ws.0)
    .bind(owner.0)
    .fetch_one(&db.pool)
    .await
    .expect("seed notebook")
}

/// Seed a database (engine) row + enable it for notebooks; return its database id.
async fn seed_enabled_engine(db: &Db, ws: WorkspaceId) -> Uuid {
    let dbid: Uuid = sqlx::query_scalar(
        "INSERT INTO databases (workspace_id, name, engine, host, port) \
         VALUES ($1,'trino-dev','trino','localhost',8080) RETURNING id",
    )
    .bind(ws.0)
    .fetch_one(&db.pool)
    .await
    .expect("seed database");
    sqlx::query("INSERT INTO notebook_engine_access (workspace_id, database_id) VALUES ($1,$2)")
        .bind(ws.0)
        .bind(dbid)
        .execute(&db.pool)
        .await
        .expect("enable engine");
    dbid
}

/// Mint a kernel token for (ws,user) directly via the registry; return the plaintext bearer.
async fn mint_kernel_token(db: &Db, ws: WorkspaceId, user: UserId) -> String {
    let reg = KernelRegistry::new(db.clone());
    let key = egret_kernel::registry::KernelKey {
        user_id: user.0,
        workspace_id: ws.0,
    };
    reg.upsert(key, "http://127.0.0.1:1", Some(1), "ready")
        .await
        .unwrap();
    let (plain, hash) = egret_kernel::kernel_token::mint();
    reg.set_token(key, &hash, chrono::Utc::now() + chrono::Duration::hours(1))
        .await
        .unwrap();
    plain
}

fn bearer_post(uri: &str, token: &str, body: serde_json::Value) -> HttpRequest<Body> {
    HttpRequest::builder()
        .method("POST")
        .uri(uri)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// A fake Jupyter Kernel Gateway HTTP server (POST creates kernel "k1"; GET k1 → 200; else 404).
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

/// Insert a user, add as member of `workspace` with `role`, return `(user_id, token)`.
async fn add_member(
    f: &Fixture,
    workspace: WorkspaceId,
    tag: &str,
    role: &str,
) -> (UserId, String) {
    let email = format!("{tag}_{}@egret.local", Uuid::new_v4().simple());
    let uid = f
        .users
        .insert_local(&email, "Test", "x")
        .await
        .expect("insert user");
    f.members
        .add(workspace, "user", uid.0, role, f.owner)
        .await
        .expect("seed member");
    let token = f
        .sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("create session")
        .token;
    (uid, token)
}

fn req(method: &str, uri: &str, token: &str) -> HttpRequest<Body> {
    HttpRequest::builder()
        .method(method)
        .uri(uri)
        .header("Cookie", format!("egret_session={token}"))
        .body(Body::empty())
        .unwrap()
}

/// Perform a real WebSocket upgrade handshake against `app` and return the HTTP
/// status of the response (101 on a successful upgrade, otherwise the error
/// status the handler produced).
///
/// axum's `WebSocketUpgrade` extractor requires hyper's `OnUpgrade` request
/// extension, which only exists when the request is driven through a real HTTP
/// server connection — `tower::oneshot` never populates it, so the extractor
/// would always reject with 426. We therefore serve `app` on an ephemeral TCP
/// port and use `tokio-tungstenite` as a real client: it returns the `101`
/// response on success, or `Http(resp)` carrying the handler's error status.
async fn ws_handshake(app: axum::Router, path: &str, token: &str) -> StatusCode {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::Error as TError;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let url = format!("ws://{addr}{path}");
    let mut request = url.into_client_request().unwrap();
    request
        .headers_mut()
        .insert("Cookie", format!("egret_session={token}").parse().unwrap());

    match tokio_tungstenite::connect_async(request).await {
        Ok((stream, resp)) => {
            // Successful upgrade. Close immediately so the proxy task unwinds.
            drop(stream);
            StatusCode::from_u16(resp.status().as_u16()).unwrap()
        }
        Err(TError::Http(resp)) => StatusCode::from_u16(resp.status().as_u16()).unwrap(),
        Err(e) => panic!("ws handshake failed unexpectedly: {e}"),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// `POST /ws/{ws}/kernel` → 200 `{phase:"ready", url}`.
/// The FakeLauncher must have received an env containing `EGRET_NOTEBOOK_RUNTIME`.
#[sqlx::test(migrations = "../../migrations")]
async fn post_kernel_starts_and_launcher_got_runtime_env(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    seed_warehouse(&f.db, f.ws).await;
    let (_uid, token) = add_member(&f, f.ws, "starter", "ws_editor").await;

    let resp = f
        .app
        .clone()
        .oneshot(req("POST", &format!("/ws/{}/kernel", f.slug), &token))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "POST kernel → 200; body={json}");
    assert_eq!(
        json["phase"].as_str(),
        Some("ready"),
        "phase must be ready; body={json}"
    );
    assert!(
        json["url"].as_str().map(|u| !u.is_empty()).unwrap_or(false),
        "url must be non-empty; body={json}"
    );

    // The FakeLauncher must have recorded an ensure() call with EGRET_NOTEBOOK_RUNTIME.
    let inner = f.launcher.inner.lock().unwrap();
    assert_eq!(
        inner.ensured.len(),
        1,
        "launcher.ensure called exactly once"
    );
    let env = &inner.ensured[0].1;
    assert!(
        env.contains_key("EGRET_NOTEBOOK_RUNTIME"),
        "env must contain EGRET_NOTEBOOK_RUNTIME; keys={:?}",
        env.keys().collect::<Vec<_>>()
    );
    // Sanity: the EGRET_NOTEBOOK_RUNTIME value must be valid JSON.
    let raw = env.get("EGRET_NOTEBOOK_RUNTIME").unwrap();
    let v: serde_json::Value =
        serde_json::from_str(raw).expect("EGRET_NOTEBOOK_RUNTIME must be valid JSON");
    assert!(
        v.get("username").is_some(),
        "EGRET_NOTEBOOK_RUNTIME JSON must have 'username'"
    );
}

/// `GET /ws/{ws}/kernel` → 200 with phase + url after a start.
#[sqlx::test(migrations = "../../migrations")]
async fn get_kernel_returns_status_after_start(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    seed_warehouse(&f.db, f.ws).await;
    let (_uid, token) = add_member(&f, f.ws, "getter", "ws_editor").await;
    let uri = format!("/ws/{}/kernel", f.slug);

    // Start the kernel first.
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", &uri, &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "POST kernel → 200");

    // GET → 200 with phase and url.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &uri, &token))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "GET kernel → 200; body={json}");
    assert!(
        json.get("phase").is_some(),
        "GET must return a phase; body={json}"
    );
}

/// `GET /ws/{ws}/kernel` before any start → 200 `{phase:"none"}`.
#[sqlx::test(migrations = "../../migrations")]
async fn get_kernel_before_start_returns_none(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    seed_warehouse(&f.db, f.ws).await;
    let (_uid, token) = add_member(&f, f.ws, "prestart", "ws_viewer").await;

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("/ws/{}/kernel", f.slug), &token))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "GET before start → 200; body={json}"
    );
    assert_eq!(
        json["phase"].as_str(),
        Some("none"),
        "phase must be none before start; body={json}"
    );
}

/// `DELETE /ws/{ws}/kernel` → 204 and the registry row is gone.
#[sqlx::test(migrations = "../../migrations")]
async fn delete_kernel_removes_row(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    seed_warehouse(&f.db, f.ws).await;
    let (_uid, token) = add_member(&f, f.ws, "stopper", "ws_editor").await;
    let uri = format!("/ws/{}/kernel", f.slug);

    // Start.
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", &uri, &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "POST kernel → 200");

    // DELETE → 204.
    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &uri, &token))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "DELETE kernel → 204");

    // GET now returns none (registry row deleted).
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &uri, &token))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "GET after delete → 200; body={json}"
    );
    assert_eq!(
        json["phase"].as_str(),
        Some("none"),
        "phase must be none after stop; body={json}"
    );

    // The launcher must have recorded a stop call.
    let inner = f.launcher.inner.lock().unwrap();
    assert_eq!(
        inner.stopped.len(),
        1,
        "launcher.stop must have been called once"
    );
}

/// A NON-member (no workspace membership) → 403/404 on POST.
///
/// Critical: the auth gate must fire BEFORE any provisioning work.
/// We confirm by asserting the FakeLauncher was NOT called.
#[sqlx::test(migrations = "../../migrations")]
async fn non_member_post_is_rejected_before_provisioning(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    seed_warehouse(&f.db, f.ws).await;

    // A user with NO membership in this workspace.
    let outsider = f
        .users
        .insert_local(
            &format!("outsider_{}@egret.test", Uuid::new_v4().simple()),
            "Outsider",
            "x",
        )
        .await
        .expect("insert outsider");
    let outsider_tok = f
        .sessions
        .create(outsider, "test", "127.0.0.1")
        .await
        .expect("create session")
        .token;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/ws/{}/kernel", f.slug),
            &outsider_tok,
        ))
        .await
        .unwrap();

    let status = resp.status();
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND,
        "non-member POST must be 403 or 404; got {status}"
    );

    // The launcher must NOT have been called (gate fires before provisioning).
    let inner = f.launcher.inner.lock().unwrap();
    assert_eq!(
        inner.ensured.len(),
        0,
        "launcher.ensure must NOT be called for an unauthorized request; ensured={:?}",
        inner.ensured.iter().map(|(k, _)| k).collect::<Vec<_>>()
    );
}

/// User A's kernel is NOT visible to user B.
///
/// A does POST → kernel row keyed to A. B does GET → must see `{phase:"none"}`,
/// not A's kernel, proving per-(user×workspace) keying.
#[sqlx::test(migrations = "../../migrations")]
async fn per_user_keying_user_b_does_not_see_user_a_kernel(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    seed_warehouse(&f.db, f.ws).await;

    let (_uid_a, token_a) = add_member(&f, f.ws, "user-a", "ws_editor").await;
    let (_uid_b, token_b) = add_member(&f, f.ws, "user-b", "ws_editor").await;
    let uri = format!("/ws/{}/kernel", f.slug);

    // User A starts their kernel.
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", &uri, &token_a))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "user A POST → 200");

    // User B's GET must return none (their own key has no row).
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &uri, &token_b))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "user B GET → 200; body={json}");
    assert_eq!(
        json["phase"].as_str(),
        Some("none"),
        "user B must see their own (none) kernel, not user A's; body={json}"
    );
}

/// Ready gateway → connect creates a per-notebook kernel and upserts notebook_kernels.
#[sqlx::test(migrations = "../../migrations")]
async fn connect_ready_creates_kernel_and_upserts(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    seed_warehouse(&f.db, f.ws).await;
    let (uid, token) = add_member(&f, f.ws, "connector", "ws_editor").await;
    let nb = seed_notebook(&f.db, f.ws, uid).await;

    // Point the FakeLauncher (gateway url) at a real fake gateway HTTP server.
    let gw = spawn_fake_gateway().await;
    f.launcher.inner.lock().unwrap().url = Some(gw);

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/ws/{}/notebooks/{}/kernel/connect", f.slug, nb),
            &token,
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "connect → 200; body={json}");
    assert_eq!(
        json["phase"].as_str(),
        Some("ready"),
        "phase ready; body={json}"
    );
    assert_eq!(
        json["kernel_id"].as_str(),
        Some("k1"),
        "kernel_id k1; body={json}"
    );

    // notebook_kernels row must exist for (nb, uid).
    let kid: Option<String> = sqlx::query_scalar(
        "SELECT kernel_id FROM notebook_kernels WHERE notebook_id=$1 AND user_id=$2",
    )
    .bind(nb)
    .bind(uid.0)
    .fetch_optional(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(
        kid.as_deref(),
        Some("k1"),
        "notebook_kernels must be upserted"
    );
}

/// Spawning gateway → connect returns {phase:"spawning"} and creates NO kernel.
#[sqlx::test(migrations = "../../migrations")]
async fn connect_spawning_returns_without_kernel(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    seed_warehouse(&f.db, f.ws).await;
    let (uid, token) = add_member(&f, f.ws, "spawning", "ws_editor").await;
    let nb = seed_notebook(&f.db, f.ws, uid).await;
    f.launcher.inner.lock().unwrap().phase = Some(egret_kernel::launcher::KernelPhase::Spawning);

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/ws/{}/notebooks/{}/kernel/connect", f.slug, nb),
            &token,
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "connect spawning → 200; body={json}"
    );
    assert_eq!(
        json["phase"].as_str(),
        Some("spawning"),
        "phase spawning; body={json}"
    );
    assert!(
        json.get("kernel_id").is_none(),
        "no kernel_id while spawning; body={json}"
    );

    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notebook_kernels WHERE notebook_id=$1 AND user_id=$2",
    )
    .bind(nb)
    .bind(uid.0)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(count, 0, "no notebook_kernels row created while spawning");
}

/// Non-member → 403/404 on connect; gateway never provisioned.
#[sqlx::test(migrations = "../../migrations")]
async fn connect_non_member_rejected(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    seed_warehouse(&f.db, f.ws).await;
    // A notebook owned by the workspace owner.
    let nb = seed_notebook(&f.db, f.ws, f.owner).await;
    let outsider = f
        .users
        .insert_local(
            &format!("out_{}@egret.test", Uuid::new_v4().simple()),
            "Out",
            "x",
        )
        .await
        .unwrap();
    let tok = f
        .sessions
        .create(outsider, "test", "127.0.0.1")
        .await
        .unwrap()
        .token;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/ws/{}/notebooks/{}/kernel/connect", f.slug, nb),
            &tok,
        ))
        .await
        .unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND,
        "non-member connect must be 403/404; got {status}"
    );
    assert_eq!(
        f.launcher.inner.lock().unwrap().ensured.len(),
        0,
        "launcher must NOT be called for unauthorized connect"
    );
}

// ── channels (WS proxy) tests ─────────────────────────────────────────────────

/// Non-member → rejected (403/404) before any upgrade.
#[sqlx::test(migrations = "../../migrations")]
async fn channels_non_member_rejected(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    seed_warehouse(&f.db, f.ws).await;
    let nb = seed_notebook(&f.db, f.ws, f.owner).await;
    let outsider = f
        .users
        .insert_local(
            &format!("chout_{}@egret.test", Uuid::new_v4().simple()),
            "Out",
            "x",
        )
        .await
        .unwrap();
    let tok = f
        .sessions
        .create(outsider, "test", "127.0.0.1")
        .await
        .unwrap()
        .token;

    let status = ws_handshake(
        f.app.clone(),
        &format!("/ws/{}/notebooks/{}/kernel/channels", f.slug, nb),
        &tok,
    )
    .await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND,
        "non-member channels must be 403/404; got {status}"
    );
}

/// Member but no kernel started → 409 Conflict (FE must call connect first).
#[sqlx::test(migrations = "../../migrations")]
async fn channels_kernel_not_started_conflict(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    seed_warehouse(&f.db, f.ws).await;
    let (uid, token) = add_member(&f, f.ws, "chmember", "ws_editor").await;
    let nb = seed_notebook(&f.db, f.ws, uid).await;

    let status = ws_handshake(
        f.app.clone(),
        &format!("/ws/{}/notebooks/{}/kernel/channels", f.slug, nb),
        &token,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "channels with no started kernel must be 409"
    );
}

/// Ready kernel → handler resolves the kernel id, upserts it, and upgrades (101).
#[sqlx::test(migrations = "../../migrations")]
async fn channels_ready_upgrades_and_resolves_kernel(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    seed_warehouse(&f.db, f.ws).await;
    let (uid, token) = add_member(&f, f.ws, "chready", "ws_editor").await;
    let nb = seed_notebook(&f.db, f.ws, uid).await;

    // Point the launcher (gateway url) at a fake gateway, then start the kernel (Ready).
    let gw = spawn_fake_gateway().await;
    f.launcher.inner.lock().unwrap().url = Some(gw);
    let started = f
        .app
        .clone()
        .oneshot(req("POST", &format!("/ws/{}/kernel", f.slug), &token))
        .await
        .unwrap();
    assert_eq!(started.status(), StatusCode::OK, "kernel start → 200");

    // Open channels — handler reaches on_upgrade and returns 101 Switching Protocols.
    let status = ws_handshake(
        f.app.clone(),
        &format!("/ws/{}/notebooks/{}/kernel/channels", f.slug, nb),
        &token,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::SWITCHING_PROTOCOLS,
        "ready kernel channels upgrade → 101; got {status}"
    );

    // The per-notebook kernel id was resolved (k1 from the fake gateway) and upserted.
    let kid: Option<String> = sqlx::query_scalar(
        "SELECT kernel_id FROM notebook_kernels WHERE notebook_id=$1 AND user_id=$2",
    )
    .bind(nb)
    .bind(uid.0)
    .fetch_optional(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(
        kid.as_deref(),
        Some("k1"),
        "channels must resolve+upsert the kernel id"
    );
}

// ── kernel query callback tests ───────────────────────────────────────────────

/// Valid token + enabled engine → 200 with columns/rows arrays.
#[sqlx::test(migrations = "../../migrations")]
async fn kernel_query_runs_for_valid_token_and_enabled_engine(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    let dbid = seed_enabled_engine(&f.db, f.ws).await;
    let token = mint_kernel_token(&f.db, f.ws, f.owner).await;

    let resp = f
        .app
        .clone()
        .oneshot(bearer_post(
            "/kernel/query",
            &token,
            serde_json::json!({ "engine": dbid.to_string(), "sql": "SELECT 1" }),
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "valid query → 200; body={json}");
    assert!(
        json["columns"].is_array() && json["rows"].is_array(),
        "body={json}"
    );
}

/// Bad/unknown token → 401.
#[sqlx::test(migrations = "../../migrations")]
async fn kernel_query_rejects_bad_token(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    seed_enabled_engine(&f.db, f.ws).await;
    let resp = f
        .app
        .clone()
        .oneshot(bearer_post(
            "/kernel/query",
            "egk_not_a_real_token",
            serde_json::json!({ "engine": "trino-dev", "sql": "SELECT 1" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "bad token → 401");
}

// ── notebook-runtime token refresh tests ──────────────────────────────────────

/// Valid kernel token → 200 with the minted engine token body.
#[sqlx::test(migrations = "../../migrations")]
async fn notebook_runtime_token_returns_minted_token(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    let token = mint_kernel_token(&f.db, f.ws, f.owner).await;

    let resp = f
        .app
        .clone()
        .oneshot(bearer_post(
            &format!("/ws/{}/notebook-runtime/token", f.slug),
            &token,
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "valid token → 200; body={json}");
    assert_eq!(
        json["access_token"].as_str(),
        Some("stub.jwt"),
        "must return the minted access_token; body={json}"
    );
}

/// No Authorization header → 401.
#[sqlx::test(migrations = "../../migrations")]
async fn notebook_runtime_token_requires_auth(pool: sqlx::PgPool) {
    let f = setup(pool).await;

    let req = HttpRequest::builder()
        .method("POST")
        .uri(format!("/ws/{}/notebook-runtime/token", f.slug))
        .header("Content-Type", "application/json")
        .body(Body::from("{}"))
        .unwrap();
    let resp = f.app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "no auth header → 401"
    );
}

/// Engine present but NOT enabled for notebooks → 403.
#[sqlx::test(migrations = "../../migrations")]
async fn kernel_query_rejects_disabled_engine(pool: sqlx::PgPool) {
    let f = setup(pool).await;
    // A database NOT enabled for notebooks (no notebook_engine_access row).
    let dbid: Uuid = sqlx::query_scalar(
        "INSERT INTO databases (workspace_id, name, engine, host, port) \
         VALUES ($1,'secret','trino','localhost',8080) RETURNING id",
    )
    .bind(f.ws.0)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    let token = mint_kernel_token(&f.db, f.ws, f.owner).await;

    let resp = f
        .app
        .clone()
        .oneshot(bearer_post(
            "/kernel/query",
            &token,
            serde_json::json!({ "engine": dbid.to_string(), "sql": "SELECT 1" }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "disabled engine → 403"
    );
}
