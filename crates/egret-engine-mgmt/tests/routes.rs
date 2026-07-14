//! Engine-mgmt seam HTTP route tests (engine CRUD, catalogs, monitor, exec/log
//! ownership). Moved from `egret-controlplane/tests/routes_api.rs` (E1c).
//! DB-gated; self-skips without `DATABASE_URL`.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret
//!   cargo test -p egret-engine-mgmt

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::response::Response;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_engine_mgmt::{engine_mgmt_router, EngineMgmtState};
use egret_engine_mgmt_api::EngineProvisioningPort;
use egret_metastore::Db;
use egret_orchestrator::KubeClient;
use http_body_util::BodyExt;
use std::sync::Arc;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

struct Fixture {
    app: axum::Router,
    users: UserRepo,
    sessions: SessionService,
    db: Db,
    engine_provisioner: Option<Arc<dyn EngineProvisioningPort>>,
    kube: Option<Arc<dyn KubeClient>>,
}

/// Incidental PDP for `EngineMgmtState`: every existing route test authenticates
/// a `platform_admin` (whose bypass predates the PDP) and asserts non-authz
/// behavior, so an allow-all stub keeps those fixtures light. Any test that
/// asserts a 403 (the one below) must instead build a real `CedarPdp` via
/// `attach_real_pdp` — an allow-all stub would falsely 200.
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

/// Build the engine-mgmt `Router` for a given db + provisioner + kube + PDP.
/// Shared by `setup_full` (allow-all PDP) and `attach_real_pdp` (real Cedar
/// PDP) so the two only differ in the injected decision point.
fn build_app(
    db: &Db,
    sessions: &SessionService,
    engine_provisioner: Option<Arc<dyn EngineProvisioningPort>>,
    kube: Option<Arc<dyn KubeClient>>,
    pdp: std::sync::Arc<dyn egret_core::authz::Pdp>,
) -> axum::Router {
    let crypto =
        egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secrets = egret_secrets::SecretStore::new(db.clone(), Some(crypto));
    let workspace_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets.clone());
    let databases = egret_query::DatabaseRepo::new(db.clone(), secrets, workspace_secrets.clone());

    engine_mgmt_router()
        .merge(egret_engine_mgmt::workspace_engines_router())
        .with_state(EngineMgmtState {
            engine_mgmt: egret_engine_mgmt::EngineMgmtStore::new(db.clone()),
            assignment: egret_engine_mgmt::EngineAssignmentService::new(
                egret_engine_mgmt::EngineMgmtStore::new(db.clone()),
                databases,
            ),
            catalog: std::sync::Arc::new(egret_catalog_engine::CatalogStore::new(db.clone())),
            storage_registry: egret_storage::StorageRegistry::new(db.clone()),
            authz: std::sync::Arc::new(egret_policy::PolicyStore::new(db.clone())),
            secrets: workspace_secrets,
            sessions: sessions.clone(),
            pdp,
            engine_provisioner,
            kube,
            engine_egret_url: "http://egret:8080".into(),
            engine_jwks_uri: "https://idp/jwks".into(),
        })
}

/// Rebuild `f.app` with a REAL `CedarPdp`. The PDP snapshots the
/// `roles`/`role_permissions` catalog at build time and (in these tests) never
/// reloads — raw-SQL seeding emits no `RolesChanged` event — so callers MUST
/// seed any ad-hoc roles the PDP needs to know about BEFORE calling this.
async fn attach_real_pdp(f: &mut Fixture) {
    let pdp = std::sync::Arc::new(
        egret_auth::pdp::CedarPdp::build(f.db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    f.app = build_app(
        &f.db,
        &f.sessions,
        f.engine_provisioner.clone(),
        f.kube.clone(),
        pdp,
    );
}

async fn into_json(resp: Response) -> (StatusCode, serde_json::Value) {
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(
        |_| serde_json::json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }),
    );
    (status, json)
}

async fn setup_full(
    engine_provisioner: Option<Arc<dyn EngineProvisioningPort>>,
    kube: Option<Arc<dyn KubeClient>>,
) -> Option<Fixture> {
    let dsn = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let users = UserRepo::new(db.clone());
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );

    // Existing route tests authenticate a platform_admin and assert non-authz
    // behavior, so an allow-all PDP suffices. The authz test rebuilds with a real
    // Cedar PDP via `attach_real_pdp`.
    let app = build_app(
        &db,
        &sessions,
        engine_provisioner.clone(),
        kube.clone(),
        std::sync::Arc::new(AllowAllPdp),
    );

    Some(Fixture {
        app,
        users,
        sessions,
        db,
        engine_provisioner,
        kube,
    })
}

/// Create a user (optionally a platform_admin) and return its session token.
async fn session_for(f: &Fixture, tag: &str, platform_admin: bool) -> String {
    let email = format!("{tag}_{}@egret.local", Uuid::new_v4().simple());
    let uid = f
        .users
        .insert_local(&email, "Test", "x")
        .await
        .expect("insert user");
    if platform_admin {
        sqlx::query(
            "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
             SELECT 'user', $1, id, 'platform', NULL FROM roles WHERE scope='platform' AND name='platform_admin'",
        )
        .bind(uid.0)
        .execute(&f.db.pool)
        .await
        .expect("grant platform_admin");
    }
    f.sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
}

/// Create a NON-admin user holding exactly one fine-grained permission key
/// (via a fresh ad-hoc platform-scoped role), and return its session token.
/// Exercises the `require_permission` scoped-principal path (as opposed to
/// `session_for`'s `platform_admin` bypass path): `platform_roles` stays
/// empty, `permissions` carries only `permission`.
async fn session_for_with_permission(f: &Fixture, tag: &str, permission: &str) -> String {
    let email = format!("{tag}_{}@egret.local", Uuid::new_v4().simple());
    let uid = f
        .users
        .insert_local(&email, "Test", "x")
        .await
        .expect("insert user");

    let role_name = format!("test_role_{}", Uuid::new_v4().simple());
    let (role_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO roles (scope, name, description) VALUES ('platform', $1, 'test-only role') \
         RETURNING id",
    )
    .bind(&role_name)
    .fetch_one(&f.db.pool)
    .await
    .expect("insert role");

    sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2)")
        .bind(role_id)
        .bind(permission)
        .execute(&f.db.pool)
        .await
        .expect("grant permission");

    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'platform', NULL)",
    )
    .bind(uid.0)
    .bind(role_id)
    .execute(&f.db.pool)
    .await
    .expect("assign role");

    f.sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
}

fn req(method: &str, uri: &str, token: &str, body: Option<serde_json::Value>) -> HttpRequest<Body> {
    let mut b = HttpRequest::builder()
        .method(method)
        .uri(uri)
        .header("Cookie", format!("egret_session={token}"));
    let body = match body {
        Some(v) => {
            b = b.header("Content-Type", "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
        }
        None => Body::empty(),
    };
    b.body(body).unwrap()
}

/// Seed a workspace + storage_connection + warehouse with lakekeeper_warehouse_id set.
async fn seed_provisioned_warehouse(f: &Fixture) -> Uuid {
    let (ws_id,): (Uuid,) =
        sqlx::query_as("INSERT INTO workspaces (slug, name) VALUES ($1, $2) RETURNING id")
            .bind(format!("engine-ws-{}", Uuid::new_v4().simple()))
            .bind("Engine Test Workspace")
            .fetch_one(&f.db.pool)
            .await
            .expect("insert workspace");

    let (storage_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(format!("engine-sc-{}", Uuid::new_v4().simple()))
    .bind("us-east-1")
    .bind("egret-engine-test")
    .bind("AKIATEST")
    .fetch_one(&f.db.pool)
    .await
    .expect("insert storage connection");

    let catalog_store = egret_catalog_engine::CatalogStore::new(f.db.clone());
    let wh = catalog_store
        .create_warehouse(
            &format!("engine-wh-{}", Uuid::new_v4().simple()),
            ws_id,
            storage_id,
        )
        .await
        .expect("create warehouse");
    catalog_store
        .set_lakekeeper_warehouse_id(wh.id, "wh-123")
        .await
        .expect("set lakekeeper id");
    wh.id
}

/// Seed a workspace + storage_connection + warehouse WITHOUT lakekeeper_warehouse_id.
async fn seed_unprovisioned_warehouse(f: &Fixture) -> Uuid {
    let (ws_id,): (Uuid,) =
        sqlx::query_as("INSERT INTO workspaces (slug, name) VALUES ($1, $2) RETURNING id")
            .bind(format!("engine-ws-unprov-{}", Uuid::new_v4().simple()))
            .bind("Engine Test Workspace Unprov")
            .fetch_one(&f.db.pool)
            .await
            .expect("insert workspace");

    let (storage_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(format!("engine-sc-unprov-{}", Uuid::new_v4().simple()))
    .bind("us-east-1")
    .bind("egret-engine-test")
    .bind("AKIATEST")
    .fetch_one(&f.db.pool)
    .await
    .expect("insert storage connection");

    let catalog_store = egret_catalog_engine::CatalogStore::new(f.db.clone());
    catalog_store
        .create_warehouse(
            &format!("engine-wh-unprov-{}", Uuid::new_v4().simple()),
            ws_id,
            storage_id,
        )
        .await
        .expect("create warehouse")
        .id
}

// ── Engines ────────────────────────────────────────────────────────────────────

/// Kube-free `EngineProvisioningPort` fake: provision/update/delete are no-ops,
/// `status` reports a single running pod named "coordinator-0" (so the
/// pod-ownership check in `pod_logs`/`pod_exec` passes for that name), and
/// `restart` records `(namespace, name)` pairs (shared via `Arc` so a test can
/// hold onto the fake and inspect it after the request completes). The
/// kube-shaped ops (exec/pod-logs/coordinator-request) are NOT on this trait —
/// `egret_orchestrator::kube_client::FakeKube` covers those, passed separately as the
/// fixture's `kube` field.
#[derive(Clone, Default)]
struct FakeEnginePort {
    restarted: Arc<std::sync::Mutex<Vec<(String, String)>>>,
}

#[async_trait::async_trait]
impl EngineProvisioningPort for FakeEnginePort {
    async fn provision(&self, _req: &egret_engine_mgmt_api::EngineSpec) -> Result<(), String> {
        Ok(())
    }
    async fn status(
        &self,
        _ns: &str,
        _name: &str,
    ) -> Result<egret_engine_mgmt_api::EngineStatus, String> {
        Ok(egret_engine_mgmt_api::EngineStatus {
            phase: "running".into(),
            message: None,
            pods: vec![egret_engine_mgmt_api::PodStatus {
                name: "coordinator-0".into(),
                phase: "Running".into(),
                ready: true,
            }],
        })
    }
    async fn update(&self, _req: &egret_engine_mgmt_api::EngineSpec) -> Result<(), String> {
        Ok(())
    }
    async fn delete(&self, _ns: &str, _name: &str) -> Result<(), String> {
        Ok(())
    }
    async fn restart(&self, ns: &str, name: &str) -> Result<(), String> {
        self.restarted
            .lock()
            .unwrap()
            .push((ns.to_string(), name.to_string()));
        Ok(())
    }
}

#[tokio::test]
async fn monitor_queries_maps_list_and_maps_bad_gateway() {
    let fake = Arc::new(egret_orchestrator::kube_client::FakeKube::default());
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(fake.clone()),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_monitor_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());

    // Create the engine.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    // Coordinator returns a `/v1/query` array → mapped to Vec<QueryDto>.
    fake.set_coordinator_response(
        200,
        br#"[{"queryId":"20260704_x","state":"RUNNING","query":"SELECT 1",
            "session":{"user":"bob"},
            "queryStats":{"elapsedTime":"1.23s","totalCpuTime":"0.50s","userMemoryReservation":"12MB","progressPercentage":42.0}}]"#.to_vec(),
    );
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/engines/{engine_id}/queries"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list queries; body={j}");
    assert_eq!(j[0]["id"], "20260704_x", "mapped query id; body={j}");
    assert_eq!(j[0]["user"], "bob", "mapped query user; body={j}");
    assert_eq!(j[0]["memory"], "12MB", "mapped query memory; body={j}");

    // A non-2xx coordinator response → the route returns 502.
    fake.set_coordinator_response(503, b"<html>service unavailable</html>".to_vec());
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/engines/{engine_id}/queries"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_GATEWAY,
        "non-2xx coordinator response must 502"
    );
}

/// `POST /engines/{id}/catalogs/{cid}/test`: a canned `FINISHED` (no `nextUri`,
/// no `error`) reports `ok:true`; a canned `error` body reports `ok:false` with
/// the message surfaced; a non-2xx coordinator response reports `ok:false`
/// with a status/body summary (NOT a 502 — this route always answers 200 with
/// its verdict in the body); a catalog id from a different engine 404s.
///
/// LIMITATION: `FakeKube` holds a single canned `(status, body)`
/// shared across every `coordinator_request` call, so it cannot return a
/// first response with `nextUri` and then a different second response — this
/// test therefore only exercises the immediate-terminal (no `nextUri`) path
/// through `test_catalog`'s poll loop, not a multi-hop `nextUri` chase.
#[tokio::test]
async fn test_catalog_reports_ok_error_and_non_2xx() {
    let fake = Arc::new(egret_orchestrator::kube_client::FakeKube::default());
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(fake.clone()),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_catalog_test_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());

    // Create the engine (auto-wires the primary egret-warehouse catalog).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    // Fetch the auto-wired primary catalog id.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/engines/{engine_id}/catalogs"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list catalogs; body={j}");
    let cat_id = j[0]["id"].as_str().expect("catalog id").to_string();

    let test_uri = format!("/engines/{engine_id}/catalogs/{cat_id}/test");

    // A canned FINISHED response with no nextUri/error → ok:true.
    fake.set_coordinator_response(200, br#"{"id":"q1","stats":{"state":"FINISHED"}}"#.to_vec());
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", &test_uri, &token, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "test catalog; body={j}");
    assert_eq!(j["ok"], true, "finished with no error → ok; body={j}");

    // A canned error body → ok:false + the error message surfaced verbatim.
    fake.set_coordinator_response(
        200,
        br#"{"id":"q2","stats":{"state":"FAILED"},"error":{"message":"catalog not found"}}"#
            .to_vec(),
    );
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", &test_uri, &token, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "test catalog error; body={j}");
    assert_eq!(j["ok"], false, "error body → not ok; body={j}");
    assert_eq!(
        j["message"], "catalog not found",
        "error message surfaced; body={j}"
    );

    // A non-2xx coordinator response → ok:false with a status/body summary,
    // never a 502 (this route's contract is always 200 + a verdict body).
    fake.set_coordinator_response(503, b"<html>service unavailable</html>".to_vec());
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", &test_uri, &token, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "test catalog non-2xx; body={j}");
    assert_eq!(j["ok"], false, "non-2xx → not ok; body={j}");
    assert!(
        j["message"].as_str().unwrap().contains("503"),
        "message surfaces coordinator status; body={j}"
    );

    // A catalog id that belongs to a DIFFERENT engine must 404 (ownership guard).
    let other_name = format!("trino-{}", Uuid::new_v4().simple());
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": other_name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create other engine; body={j}");
    let other_engine_id = j["id"].as_str().expect("other engine id").to_string();
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/engines/{other_engine_id}/catalogs/{cat_id}/test"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "catalog from a different engine must 404"
    );
}

/// PDP-backed engine authz (this seam migrated onto
/// `egret_core::authz::require`): a non-`platform_admin` principal granted
/// `engine:read` through a platform-scoped role — resolved by a real
/// `CedarPdp`, not the allow-all stub — can GET /engines (200) but is denied
/// POST /engines (403, it lacks `engine:create`). The 403 fires on the
/// handler's first line, before any warehouse lookup, so the body UUIDs need
/// not resolve.
#[tokio::test]
async fn pdp_backed_engine_reader_reads_but_cannot_create() {
    let Some(mut f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(
            egret_orchestrator::kube_client::FakeKube::default(),
        )),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };

    // Seed the reader's `engine:read` platform role BEFORE the PDP snapshots the
    // roles catalog, then swap in the real Cedar PDP.
    let reader = session_for_with_permission(&f, "pdp_engine_reader", "engine:read").await;
    attach_real_pdp(&mut f).await;

    // GET /engines → 200 (the PDP allows engine:read at platform scope).
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/engines", &reader, None))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "PDP-backed engine:read must list engines; body={body}"
    );

    // POST /engines → 403 (the same principal lacks engine:create).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &reader,
            Some(serde_json::json!({
                "name": format!("trino-{}", Uuid::new_v4().simple()),
                "warehouseId": Uuid::new_v4(),
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "PDP-backed engine:read alone must not create an engine (needs engine:create)"
    );
}

#[tokio::test]
async fn create_and_get_engine() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(
            egret_orchestrator::kube_client::FakeKube::default(),
        )),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());

    // POST /engines → 201, auto-wired to the provisioned warehouse.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": name,
                "warehouseId": wh_id,
                "coordinatorReplicas": 1,
                "workerReplicas": 2,
                "cpuMin": "1",
                "cpuMax": "2",
                "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    assert_eq!(j["name"], serde_json::json!(name), "engine name; body={j}");
    assert_eq!(j["type"], "trino", "engine type; body={j}");
    assert_eq!(j["status"], "provisioning", "engine status; body={j}");
    assert_eq!(
        j["spec"]["workerReplicas"], 2,
        "engine worker replicas; body={j}"
    );
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    // GET /engines/{id} → 200 with live phase "running" from the fake provisioner.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("/engines/{engine_id}"), &token, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "get engine; body={j}");
    assert_eq!(j["status"]["phase"], "running", "live phase; body={j}");
    assert_eq!(j["status"]["pods"][0]["ready"], true, "pod ready; body={j}");
    // The wizard inputs seeded a per-role trinoConfig on the spec.
    assert_eq!(
        j["spec"]["trinoConfig"]["coordinator"]["replicas"], 1,
        "seeded coordinator replicas; body={j}"
    );
    assert_eq!(
        j["spec"]["trinoConfig"]["worker"]["replicas"], 2,
        "seeded worker replicas; body={j}"
    );
    assert_eq!(
        j["spec"]["trinoConfig"]["coordinator"]["resources"]["cpuLimit"], "2",
        "seeded coordinator cpu limit; body={j}"
    );
}

#[tokio::test]
async fn pod_logs_known_pod_ok_unknown_pod_404() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(
            egret_orchestrator::kube_client::FakeKube::default(),
        )),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_pod_logs_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    // The fake provisioner's `status` reports a single pod named "coordinator-0".
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/engines/{engine_id}/pods/coordinator-0/logs"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "known pod logs; body={j}");
    assert_eq!(
        j["logs"], "logs for coordinator-0 (fake)\n",
        "canned logs; body={j}"
    );

    // A pod name that isn't in the engine's live pod list is a 404, not a
    // pass-through to the kube client.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/engines/{engine_id}/pods/not-my-pod/logs"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "unknown pod must 404");
}

// NOTE: no `pod_exec` route test here. `axum::extract::ws::WebSocketUpgrade`'s
// `FromRequestParts` unconditionally rejects with 426 Upgrade Required unless
// `hyper::upgrade::OnUpgrade` is present in the request extensions — an
// extension only a real hyper connection ever inserts. `tower::ServiceExt::
// oneshot`-driven requests (as used throughout this file) never have it, so a
// WS-upgrade request 426s before the handler body (and therefore the
// ownership guard) ever runs, regardless of pod name — there is no way to
// distinguish "unknown pod" from "known pod" through this harness. The guard
// itself is the exact same construct as `pod_logs`'s (already covered by
// `pod_logs_known_pod_ok_unknown_pod_404` above), so its correctness is not
// unverified — only the WS-specific wiring around it is untested here.

/// `POST /engines/{id}/restart` rolls the engine's pods without changing
/// config: the route calls `EngineProvisioningPort::restart` with the engine's
/// namespace + name (from which the real egret-bin adapter derives BOTH the
/// `{name}-coordinator` and `{name}-worker` Deployment names — see
/// `fake_kube_restart_deployment_records_coordinator_and_worker` in
/// `egret-orchestrator` for that fan-out). An unknown engine id is a 404.
#[tokio::test]
async fn restart_engine_calls_provisioner_unknown_404() {
    let fake = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(fake.clone())),
        Some(Arc::new(
            egret_orchestrator::kube_client::FakeKube::default(),
        )),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_restart_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();
    let namespace = j["namespace"]
        .as_str()
        .expect("engine namespace")
        .to_string();

    // Restart the known engine → 200, and the fake provisioner recorded the
    // call with the engine's namespace + name.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/engines/{engine_id}/restart"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "restart known engine");
    assert_eq!(
        *fake.restarted.lock().unwrap(),
        vec![(namespace, name)],
        "restart recorded with engine namespace + name"
    );

    // An unknown engine id is a 404, not a pass-through to the provisioner.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/engines/{}/restart", Uuid::new_v4()),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "unknown engine must 404"
    );
    assert_eq!(
        fake.restarted.lock().unwrap().len(),
        1,
        "unknown-engine 404 must not call the provisioner"
    );
}

#[tokio::test]
async fn update_engine_per_role_trino_config_roundtrips() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(
            egret_orchestrator::kube_client::FakeKube::default(),
        )),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_update_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());

    // Create with the simple wizard.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": name,
                "warehouseId": wh_id,
                "coordinatorReplicas": 1,
                "workerReplicas": 1,
                "cpuMin": "1",
                "cpuMax": "2",
                "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    // PUT a full per-role trinoConfig (coordinator heap + worker graceful shutdown).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}"),
            &token,
            Some(serde_json::json!({
                "name": name,
                "warehouseId": wh_id,
                "coordinatorReplicas": 1,
                "workerReplicas": 1,
                "cpuMin": "1",
                "cpuMax": "2",
                "memory": "4Gi",
                "trinoConfig": {
                    "coordinator": { "replicas": 1, "heap": { "maxSize": "3G" } },
                    "worker": {
                        "replicas": 1,
                        "gracefulShutdown": { "enabled": true, "gracePeriodSeconds": 30 }
                    }
                }
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "update engine; body={j}");

    // GET reflects the persisted per-role config.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("/engines/{engine_id}"), &token, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "get engine; body={j}");
    assert_eq!(
        j["spec"]["trinoConfig"]["coordinator"]["heap"]["maxSize"], "3G",
        "coordinator heap round-trips; body={j}"
    );
    assert_eq!(
        j["spec"]["trinoConfig"]["worker"]["gracefulShutdown"]["enabled"], true,
        "worker graceful shutdown round-trips; body={j}"
    );
    assert_eq!(
        j["spec"]["trinoConfig"]["worker"]["gracefulShutdown"]["gracePeriodSeconds"], 30,
        "worker grace period round-trips; body={j}"
    );
}

/// Seed a global storage_connection with a resolvable secret_ref; returns its id.
async fn seed_exchange_storage_conn(f: &Fixture) -> Uuid {
    let secrets = egret_secrets::WorkspaceSecretStore::new(
        f.db.clone(),
        egret_secrets::SecretStore::new(
            f.db.clone(),
            Some(egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("crypto")),
        ),
    );
    let secret_id = secrets
        .create(
            None,
            &format!("exchange-secret-{}", Uuid::new_v4().simple()),
            None,
            "s3cr3t",
        )
        .await
        .expect("create exchange secret");
    let (id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key, endpoint, secret_ref, path_style) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(format!("exchange-sc-{}", Uuid::new_v4().simple()))
    .bind("us-east-1")
    .bind("egret-exchange-test")
    .bind("AKIATEST")
    .bind("http://minio:9000")
    .bind(secret_id)
    .bind(true)
    .fetch_one(&f.db.pool)
    .await
    .expect("insert exchange storage connection");
    id
}

#[tokio::test]
async fn update_engine_resource_groups_and_fault_tolerant() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(
            egret_orchestrator::kube_client::FakeKube::default(),
        )),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_rg_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let exchange_conn = seed_exchange_storage_conn(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());

    // Create with the simple wizard.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    // PUT resourceGroups (root `global` + selector {group:"global"}) + faultTolerant.
    let put_body = serde_json::json!({
        "name": name, "warehouseId": wh_id,
        "coordinatorReplicas": 1, "workerReplicas": 1,
        "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
        "trinoConfig": {
            "coordinator": { "replicas": 1 },
            "worker": { "replicas": 1 },
            "resourceGroups": {
                "rootGroups": [{
                    "name": "global", "softMemoryLimit": "80%",
                    "hardConcurrencyLimit": 50, "maxQueued": 100
                }],
                "selectors": [{ "group": "global" }]
            },
            "faultTolerant": {
                "retryPolicy": "TASK",
                "exchangeStorageConnId": exchange_conn,
                "exchangePrefix": "exchange"
            }
        }
    });
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}"),
            &token,
            Some(put_body),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "update engine with rg + ft; body={j}"
    );

    // GET reflects the persisted resourceGroups.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("/engines/{engine_id}"), &token, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "get engine; body={j}");
    assert_eq!(
        j["spec"]["trinoConfig"]["resourceGroups"]["rootGroups"][0]["name"], "global",
        "resource group round-trips; body={j}"
    );
    assert_eq!(
        j["spec"]["trinoConfig"]["faultTolerant"]["retryPolicy"], "TASK",
        "fault-tolerant round-trips; body={j}"
    );

    // PUT with a dangling selector group → 400.
    let bad_body = serde_json::json!({
        "name": name, "warehouseId": wh_id,
        "coordinatorReplicas": 1, "workerReplicas": 1,
        "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
        "trinoConfig": {
            "coordinator": { "replicas": 1 },
            "worker": { "replicas": 1 },
            "resourceGroups": {
                "rootGroups": [{
                    "name": "global", "softMemoryLimit": "80%",
                    "hardConcurrencyLimit": 50, "maxQueued": 100
                }],
                "selectors": [{ "group": "nope" }]
            }
        }
    });
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}"),
            &token,
            Some(bad_body),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "dangling resource-group selector must 400"
    );
}

/// Create a global named secret holding `value`; returns its id.
async fn seed_secret(f: &Fixture, value: &str) -> Uuid {
    let secrets = egret_secrets::WorkspaceSecretStore::new(
        f.db.clone(),
        egret_secrets::SecretStore::new(
            f.db.clone(),
            Some(egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("crypto")),
        ),
    );
    secrets
        .create(
            None,
            &format!("pw-secret-{}", Uuid::new_v4().simple()),
            None,
            value,
        )
        .await
        .expect("create password secret")
}

#[tokio::test]
async fn update_engine_auth_and_ingress() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(
            egret_orchestrator::kube_client::FakeKube::default(),
        )),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_auth_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let secret_id = seed_secret(&f, "s3cret").await;
    let name = format!("trino-{}", Uuid::new_v4().simple());

    // Create with the simple wizard.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    // PUT auth (password user `bob` → seeded secret) + ingress → 200. The control
    // plane resolves the secret + bcrypts it for provisioning; the spec persists
    // the RAW secretRef.
    let put_body = serde_json::json!({
        "name": name, "warehouseId": wh_id,
        "coordinatorReplicas": 1, "workerReplicas": 1,
        "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
        "trinoConfig": {
            "coordinator": { "replicas": 1 },
            "worker": { "replicas": 1 },
            "auth": {
                "passwordUsers": [{ "username": "bob", "secretRef": secret_id }],
                "jwt": { "requiredAudience": "trino" }
            },
            "ingress": { "enabled": true, "className": "nginx", "host": "h.local" }
        }
    });
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}"),
            &token,
            Some(put_body),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "update engine auth+ingress; body={j}"
    );

    // GET reflects the usernames + host — but NOT the plaintext password.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("/engines/{engine_id}"), &token, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "get engine; body={j}");
    assert_eq!(
        j["spec"]["trinoConfig"]["auth"]["passwordUsers"][0]["username"], "bob",
        "auth username round-trips; body={j}"
    );
    assert_eq!(
        j["spec"]["trinoConfig"]["ingress"]["host"], "h.local",
        "ingress host round-trips; body={j}"
    );
    let raw = j["spec"]["trinoConfig"].to_string();
    assert!(
        !raw.contains("s3cret"),
        "plaintext password must not be persisted; body={j}"
    );

    // PUT with a bogus secretRef → 400 (secret cannot be resolved for bcrypt).
    let bad_body = serde_json::json!({
        "name": name, "warehouseId": wh_id,
        "coordinatorReplicas": 1, "workerReplicas": 1,
        "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
        "trinoConfig": {
            "coordinator": { "replicas": 1 },
            "worker": { "replicas": 1 },
            "auth": {
                "passwordUsers": [{ "username": "bob", "secretRef": Uuid::new_v4() }]
            }
        }
    });
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}"),
            &token,
            Some(bad_body),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "bogus password secretRef must 400"
    );
}

#[tokio::test]
async fn update_engine_pod_scheduling_roundtrips() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(
            egret_orchestrator::kube_client::FakeKube::default(),
        )),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_pod_sched_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());

    // Create with the simple wizard.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    // PUT a trinoConfig with per-role nodeSelector + affinity.rawOverride, a
    // cluster serviceAccount, and an init container → 200.
    let put_body = serde_json::json!({
        "name": name, "warehouseId": wh_id,
        "coordinatorReplicas": 1, "workerReplicas": 1,
        "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
        "trinoConfig": {
            "coordinator": {
                "replicas": 1,
                "nodeSelector": { "disktype": "ssd" },
                "affinity": { "rawOverride": { "custom": true } }
            },
            "worker": { "replicas": 1 },
            "serviceAccount": "sa",
            "initContainers": [{ "name": "wait", "image": "busybox" }]
        }
    });
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}"),
            &token,
            Some(put_body),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "update engine pod scheduling; body={j}"
    );

    // GET reflects the persisted pod/scheduling fields.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("/engines/{engine_id}"), &token, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "get engine; body={j}");
    let tc = &j["spec"]["trinoConfig"];
    assert_eq!(
        tc["coordinator"]["nodeSelector"]["disktype"], "ssd",
        "per-role nodeSelector round-trips; body={j}"
    );
    assert_eq!(
        tc["serviceAccount"], "sa",
        "cluster serviceAccount round-trips; body={j}"
    );
    assert_eq!(
        tc["initContainers"][0]["name"], "wait",
        "init container round-trips; body={j}"
    );
    assert_eq!(
        tc["coordinator"]["affinity"]["rawOverride"]["custom"], true,
        "raw affinity override round-trips verbatim; body={j}"
    );
}

#[tokio::test]
async fn update_engine_autoscaling_roundtrips() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(
            egret_orchestrator::kube_client::FakeKube::default(),
        )),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_autoscale_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());

    // Create with the simple wizard.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    // PUT a trinoConfig with HPA autoscaling → 200.
    let put_body = serde_json::json!({
        "name": name, "warehouseId": wh_id,
        "coordinatorReplicas": 1, "workerReplicas": 1,
        "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
        "trinoConfig": {
            "coordinator": { "replicas": 1 },
            "worker": { "replicas": 1 },
            "autoscaling": {
                "mode": "hpa",
                "hpa": { "maxReplicas": 5, "targetCpuPercent": 60 }
            }
        }
    });
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}"),
            &token,
            Some(put_body),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "update engine hpa; body={j}");

    // GET reflects the HPA autoscaling block.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("/engines/{engine_id}"), &token, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "get engine; body={j}");
    let asc = &j["spec"]["trinoConfig"]["autoscaling"];
    assert_eq!(asc["mode"], "hpa", "autoscaling mode round-trips; body={j}");
    assert_eq!(
        asc["hpa"]["maxReplicas"], 5,
        "hpa maxReplicas round-trips; body={j}"
    );

    // PUT a KEDA config with a cron trigger → 200; GET reflects the trigger kind.
    let put_body = serde_json::json!({
        "name": name, "warehouseId": wh_id,
        "coordinatorReplicas": 1, "workerReplicas": 1,
        "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
        "trinoConfig": {
            "coordinator": { "replicas": 1 },
            "worker": { "replicas": 1 },
            "autoscaling": {
                "mode": "keda",
                "keda": {
                    "minReplicaCount": 0, "maxReplicaCount": 4,
                    "triggers": [{
                        "kind": "cron",
                        "cron": { "start": "0 8 * * *", "end": "0 20 * * *", "desiredReplicas": 3, "timezone": "UTC" }
                    }]
                }
            }
        }
    });
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}"),
            &token,
            Some(put_body),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "update engine keda; body={j}");

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("/engines/{engine_id}"), &token, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "get engine; body={j}");
    let asc = &j["spec"]["trinoConfig"]["autoscaling"];
    assert_eq!(asc["mode"], "keda", "keda mode round-trips; body={j}");
    assert_eq!(
        asc["keda"]["triggers"][0]["kind"], "cron",
        "keda trigger kind round-trips; body={j}"
    );
}

#[tokio::test]
async fn engine_catalog_routes() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(
            egret_orchestrator::kube_client::FakeKube::default(),
        )),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_cat_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());

    // Create the engine (auto-wires the primary egret-warehouse catalog).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": name,
                "warehouseId": wh_id,
                "coordinatorReplicas": 1,
                "workerReplicas": 1,
                "cpuMin": "1",
                "cpuMax": "2",
                "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    // POST a tpch catalog → 201.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/engines/{engine_id}/catalogs"),
            &token,
            Some(serde_json::json!({ "name": "bench", "kind": "tpch", "config": {} })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "add tpch catalog; body={j}");
    assert_eq!(j["kind"], "tpch", "catalog kind; body={j}");
    assert_eq!(j["isPrimary"], false, "tpch is not primary; body={j}");
    let tpch_id = j["id"].as_str().expect("tpch catalog id").to_string();

    // GET → 2 catalogs, primary first.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/engines/{engine_id}/catalogs"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list catalogs; body={j}");
    assert_eq!(
        j.as_array().map(|a| a.len()),
        Some(2),
        "two catalogs; body={j}"
    );
    assert_eq!(j[0]["isPrimary"], true, "primary listed first; body={j}");
    assert_eq!(j[0]["kind"], "egret-warehouse", "primary kind; body={j}");
    let primary_id = j[0]["id"].as_str().expect("primary catalog id").to_string();

    // DELETE the tpch catalog → 200.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/engines/{engine_id}/catalogs/{tpch_id}"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "delete tpch catalog");

    // DELETE the primary catalog → 400.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/engines/{engine_id}/catalogs/{primary_id}"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "deleting the primary catalog must 400"
    );
}

/// `config.clientId`: adding an egret-warehouse catalog under an existing
/// client's role must succeed, store `clientRowId` pointing at a NEWLY minted
/// client (never the referenced one — see `add_catalog`'s doc comment: a
/// `CatalogClient`'s secret is write-once and not retrievable, so "use an
/// existing client" mints a fresh client under that client's role instead of
/// reusing it verbatim) with `mintedClient: true`, and removing the catalog
/// must revoke that MINTED client, not the user's original one.
#[tokio::test]
async fn add_catalog_with_client_id_inherits_role_and_mints_new_client() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(
            egret_orchestrator::kube_client::FakeKube::default(),
        )),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_cat_clientid_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());

    // Seed a pre-existing, user-owned client on a custom (non-default) role.
    let catalog_store = egret_catalog_engine::CatalogStore::new(f.db.clone());
    let engine_mgmt = egret_engine_mgmt::EngineMgmtStore::new(f.db.clone());
    let wh = catalog_store
        .get_warehouse(wh_id)
        .await
        .expect("get warehouse")
        .expect("warehouse exists");
    let role = catalog_store
        .create_role(
            wh_id,
            "readonly_analyst",
            "external",
            Some("read-only role"),
        )
        .await
        .expect("create role");
    let existing_client = catalog_store
        .create_client(&egret_catalog_engine::NewClient {
            name: "pre-existing-client".into(),
            consumer_type: "external".into(),
            workspace_id: wh.workspace_id,
            warehouse_id: wh_id,
            catalog_role_id: role.id,
        })
        .await
        .expect("create existing client");

    // Create the engine (auto-wires the primary egret-warehouse catalog).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": name,
                "warehouseId": wh_id,
                "coordinatorReplicas": 1,
                "workerReplicas": 1,
                "cpuMin": "1",
                "cpuMax": "2",
                "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    // POST an egret-warehouse catalog with config.clientId → 201.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/engines/{engine_id}/catalogs"),
            &token,
            Some(serde_json::json!({
                "name": "governed",
                "kind": "egret-warehouse",
                "config": { "warehouseId": wh_id, "clientId": existing_client.id },
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "add catalog w/ clientId; body={j}"
    );
    let cat_id = j["id"].as_str().expect("catalog id").to_string();

    // GET the catalog detail: clientRowId must be a NEW client (not the
    // referenced one), and mintedClient defaults true (harmless to expose).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/engines/{engine_id}/catalogs/{cat_id}"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "get catalog detail; body={j}");
    assert_eq!(j["config"]["mintedClient"], true, "mintedClient; body={j}");
    // credential/clientRowId are redacted from the HTTP response; inspect the
    // stored row directly via the storage layer instead.
    let stored = engine_mgmt
        .list_engine_catalogs(Uuid::parse_str(&engine_id).unwrap())
        .await
        .expect("list engine catalogs")
        .into_iter()
        .find(|c| c.id.to_string() == cat_id)
        .expect("stored catalog row");
    let minted_client_row_id = stored.config["clientRowId"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .expect("clientRowId present");
    assert_ne!(
        minted_client_row_id, existing_client.id,
        "a fresh client must be minted, not the referenced one reused"
    );
    // The minted client inherited the referenced client's role.
    let minted_row = catalog_store
        .list_clients(wh_id)
        .await
        .expect("list clients")
        .into_iter()
        .find(|c| c.id == minted_client_row_id)
        .expect("minted client row exists");
    assert_eq!(
        minted_row.catalog_role_id, role.id,
        "minted client inherits the referenced client's role"
    );

    // DELETE the catalog → the MINTED client is revoked, the user's original
    // client is untouched.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/engines/{engine_id}/catalogs/{cat_id}"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "delete catalog");

    let remaining = catalog_store
        .list_clients(wh_id)
        .await
        .expect("list clients after delete");
    assert!(
        !remaining.iter().any(|c| c.id == minted_client_row_id),
        "minted client must be revoked on catalog removal"
    );
    assert!(
        remaining.iter().any(|c| c.id == existing_client.id),
        "the user's pre-existing client must NOT be revoked"
    );
}

/// An invalid or foreign `config.clientId` (not belonging to the target
/// warehouse) must 400, not silently fall back to the default role or 500.
#[tokio::test]
async fn add_catalog_with_foreign_client_id_400() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(
            egret_orchestrator::kube_client::FakeKube::default(),
        )),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_cat_bad_clientid_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let other_wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());

    // A client that belongs to a DIFFERENT warehouse.
    let catalog_store = egret_catalog_engine::CatalogStore::new(f.db.clone());
    let other_wh = catalog_store
        .get_warehouse(other_wh_id)
        .await
        .expect("get warehouse")
        .expect("warehouse exists");
    let role = catalog_store
        .create_role(other_wh_id, "other_role", "external", None)
        .await
        .expect("create role");
    let foreign_client = catalog_store
        .create_client(&egret_catalog_engine::NewClient {
            name: "foreign-client".into(),
            consumer_type: "external".into(),
            workspace_id: other_wh.workspace_id,
            warehouse_id: other_wh_id,
            catalog_role_id: role.id,
        })
        .await
        .expect("create foreign client");

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": name,
                "warehouseId": wh_id,
                "coordinatorReplicas": 1,
                "workerReplicas": 1,
                "cpuMin": "1",
                "cpuMax": "2",
                "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    // A foreign clientId (belongs to a different warehouse) → 400.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/engines/{engine_id}/catalogs"),
            &token,
            Some(serde_json::json!({
                "name": "governed",
                "kind": "egret-warehouse",
                "config": { "warehouseId": wh_id, "clientId": foreign_client.id },
            })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "foreign clientId must 400"
    );

    // A malformed (non-UUID) clientId → 400.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/engines/{engine_id}/catalogs"),
            &token,
            Some(serde_json::json!({
                "name": "governed2",
                "kind": "egret-warehouse",
                "config": { "warehouseId": wh_id, "clientId": "not-a-uuid" },
            })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "malformed clientId must 400"
    );
}

#[tokio::test]
async fn create_engine_unprovisioned_warehouse_400() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(
            egret_orchestrator::kube_client::FakeKube::default(),
        )),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_admin_unprov", true).await;
    let wh_id = seed_unprovisioned_warehouse(&f).await;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": format!("trino-{}", Uuid::new_v4().simple()),
                "warehouseId": wh_id,
                "coordinatorReplicas": 1,
                "workerReplicas": 1,
                "cpuMin": "1",
                "cpuMax": "2",
                "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "unprovisioned warehouse must 400"
    );
}

#[tokio::test]
async fn scoped_permission_engines_read_and_create() {
    let Some(mut f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(
            egret_orchestrator::kube_client::FakeKube::default(),
        )),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };

    // These 403/200 assertions gate on `state.pdp`; seed all ad-hoc scoped roles
    // BEFORE building the Cedar PDP (it snapshots the roles catalog at build),
    // then swap in a real PDP so the allow-all stub can't mask the scoped grants.
    let wrong = session_for_with_permission(&f, "engine_scoped_wrong", "warehouse:read").await;
    let reader = session_for_with_permission(&f, "engine_scoped_reader", "engine:read").await;
    let creator = session_for_with_permission(&f, "engine_scoped_creator", "engine:create").await;
    attach_real_pdp(&mut f).await;

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/engines", &wrong, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a principal without engine:read must not list engines"
    );

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/engines", &reader, None))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a scoped engine:read principal must list engines; body={body}"
    );

    let wh_id = seed_provisioned_warehouse(&f).await;
    let engine_body = serde_json::json!({
        "name": format!("trino-{}", Uuid::new_v4().simple()),
        "warehouseId": wh_id,
        "coordinatorReplicas": 1,
        "workerReplicas": 1,
        "cpuMin": "1",
        "cpuMax": "2",
        "memory": "4Gi",
    });

    // engine:read alone must not create.
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", "/engines", &reader, Some(engine_body.clone())))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "engine:read alone must not create an engine"
    );

    let resp = f
        .app
        .clone()
        .oneshot(req("POST", "/engines", &creator, Some(engine_body)))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "a scoped engine:create principal must create; body={body}"
    );
}

#[tokio::test]
async fn scoped_permission_monitor_queries_and_cluster() {
    let fake = Arc::new(egret_orchestrator::kube_client::FakeKube::default());
    let Some(mut f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(fake.clone()),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };

    // The monitor gates consult `state.pdp`; seed the admin + scoped roles BEFORE
    // building the Cedar PDP (it snapshots the roles catalog at build), then swap
    // in a real PDP so the under-privileged 403s are genuine (not allow-all).
    let admin_token = session_for(&f, "monitor_scoped_setup", true).await;
    let wrong = session_for_with_permission(&f, "monitor_scoped_wrong", "warehouse:read").await;
    let reader = session_for_with_permission(&f, "monitor_scoped_reader", "engine:read").await;
    attach_real_pdp(&mut f).await;

    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &admin_token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    fake.set_coordinator_response(200, b"[]".to_vec());

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/engines/{engine_id}/queries"),
            &wrong,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a principal without engine:read must not list queries"
    );
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/engines/{engine_id}/cluster"),
            &wrong,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a principal without engine:read must not fetch cluster state"
    );

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/engines/{engine_id}/queries"),
            &reader,
            None,
        ))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a scoped engine:read principal must list queries; body={body}"
    );
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/engines/{engine_id}/cluster"),
            &reader,
            None,
        ))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a scoped engine:read principal must fetch cluster state; body={body}"
    );
}

// ── engine ↔ workspace assignment (P1) ─────────────────────────────────────────

/// Seed a JWT-capable Trino engine (`spec.trinoConfig.auth.jwt` object) tied to
/// the given warehouse; returns its id.
async fn seed_jwt_engine(f: &Fixture, warehouse_id: Uuid) -> Uuid {
    let store = egret_engine_mgmt::EngineMgmtStore::new(f.db.clone());
    let spec = serde_json::json!({
        "trinoConfig": { "auth": { "jwt": { "jwks_uri": "http://kc/jwks" } } }
    });
    store
        .create_engine(
            &format!("trino-{}", Uuid::new_v4().simple()),
            "trino",
            "egret-engines",
            warehouse_id,
            &spec,
        )
        .await
        .expect("seed jwt engine")
        .id
}

/// Seed a bare active workspace (assignment target); returns its id.
async fn seed_workspace(f: &Fixture) -> Uuid {
    let (ws_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO workspaces (slug, name, status) VALUES ($1, $2, 'active') RETURNING id",
    )
    .bind(format!("assign-ws-{}", Uuid::new_v4().simple()))
    .bind("Assignment Target Workspace")
    .fetch_one(&f.db.pool)
    .await
    .expect("insert workspace");
    ws_id
}

/// Count materialized `query.databases` rows for an (workspace, engine) pair.
async fn materialized_count(f: &Fixture, ws: Uuid, engine_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM query.databases WHERE workspace_id = $1 AND engine_id = $2",
    )
    .bind(ws)
    .bind(engine_id)
    .fetch_one(&f.db.pool)
    .await
    .expect("count materialized")
}

/// CP `PUT /engines/{id}/workspaces {workspaceIds:[ws]}` records the assignment
/// AND materializes a `query.databases` row; `GET` returns exactly `[ws]`; an
/// empty `PUT` unassigns and removes the materialized row.
#[tokio::test]
async fn cp_set_engine_workspaces_assigns_and_materializes() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(
            egret_orchestrator::kube_client::FakeKube::default(),
        )),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_assign_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let engine_id = seed_jwt_engine(&f, wh_id).await;
    let ws = seed_workspace(&f).await;

    // PUT the full set {ws} → 200.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}/workspaces"),
            &token,
            Some(serde_json::json!({ "workspaceIds": [ws] })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "set workspaces; body={j}");
    assert_eq!(j.as_array().unwrap().len(), 1, "one assigned ws; body={j}");
    assert_eq!(
        j[0]["workspaceId"],
        ws.to_string(),
        "assigned ws id; body={j}"
    );

    // GET reflects the assignment.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/engines/{engine_id}/workspaces"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list workspaces; body={j}");
    assert_eq!(
        j[0]["workspaceId"],
        ws.to_string(),
        "listed ws id; body={j}"
    );

    // The delegated connection was materialized.
    assert_eq!(
        materialized_count(&f, ws, engine_id).await,
        1,
        "a query.databases row must be materialized for the assignment"
    );

    // PUT the empty set → unassign; GET is empty; the materialized row is gone.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}/workspaces"),
            &token,
            Some(serde_json::json!({ "workspaceIds": [] })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "clear workspaces; body={j}");
    assert!(
        j.as_array().unwrap().is_empty(),
        "no assignments left; body={j}"
    );
    assert_eq!(
        materialized_count(&f, ws, engine_id).await,
        0,
        "the materialized row must be removed on unassign"
    );

    // Unknown engine → 404.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/engines/{}/workspaces", Uuid::new_v4()),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "unknown engine must 404"
    );
}

/// Workspace-admin `POST /workspaces/{id}/engines {engineId}` assigns; `GET`
/// shows the engine; `DELETE /workspaces/{id}/engines/{engine_id}` removes it.
#[tokio::test]
async fn workspace_admin_assign_list_unassign_engine() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(
            egret_orchestrator::kube_client::FakeKube::default(),
        )),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "ws_engine_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let engine_id = seed_jwt_engine(&f, wh_id).await;
    let ws = seed_workspace(&f).await;

    // Initially empty.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/workspaces/{ws}/engines"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list ws engines; body={j}");
    assert!(j.as_array().unwrap().is_empty(), "no engines yet; body={j}");

    // POST assigns → 204.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/workspaces/{ws}/engines"),
            &token,
            Some(serde_json::json!({ "engineId": engine_id })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "assign engine");

    // GET now shows the engine with its DTO fields.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/workspaces/{ws}/engines"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list ws engines; body={j}");
    assert_eq!(j.as_array().unwrap().len(), 1, "one engine; body={j}");
    assert_eq!(j[0]["id"], engine_id.to_string(), "engine id; body={j}");
    assert_eq!(j[0]["type"], "trino", "engine type; body={j}");
    assert_eq!(
        materialized_count(&f, ws, engine_id).await,
        1,
        "assignment materialized a connection"
    );

    // DELETE unassigns → 204; GET is empty again.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/workspaces/{ws}/engines/{engine_id}"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "unassign engine");

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/workspaces/{ws}/engines"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list ws engines; body={j}");
    assert!(j.as_array().unwrap().is_empty(), "engine removed; body={j}");
    assert_eq!(
        materialized_count(&f, ws, engine_id).await,
        0,
        "unassign removed the materialized connection"
    );
}
