//! Engine-mgmt seam HTTP route tests (engine CRUD, catalogs, monitor, exec/log
//! ownership). Moved from `egret-controlplane/tests/routes_api.rs` (E1c).
//! DB-gated; self-skips without `DATABASE_URL` ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.
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
use egret_provisioner::KubeClient;
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
    ranger_enabled: bool,
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
    ranger_enabled: bool,
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
            users: egret_auth::users::UserRepo::new(db.clone()),
            roles: egret_auth::roles::RoleRepo::new(db.clone(), egret_events::EventBus::new()),
            pdp,
            engine_provisioner,
            kube,
            engine_egret_url: "http://egret:8080".into(),
            engine_jwks_uri: "https://idp/jwks".into(),
            engine_oidc_issuer: "https://idp/realms/egret".into(),
            ranger_enabled,
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
        f.ranger_enabled,
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
    setup_with_ranger(engine_provisioner, kube, false).await
}

/// `setup_full` with `config.ranger.enabled` under the test's control: the Trino
/// create path auto-creates (or reuses) a Ranger profile only when it is on.
async fn setup_with_ranger(
    engine_provisioner: Option<Arc<dyn EngineProvisioningPort>>,
    kube: Option<Arc<dyn KubeClient>>,
    ranger_enabled: bool,
) -> Option<Fixture> {
    let dsn = egret_test_db::url().await?;
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
        ranger_enabled,
    );

    Some(Fixture {
        app,
        users,
        sessions,
        db,
        engine_provisioner,
        kube,
        ranger_enabled,
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

/// Like [`seed_provisioned_warehouse`], but the storage connection carries a
/// (global) named secret so its `secret_ref` is set. The StarRocks create path
/// treats a missing secret as a hard error, so its tests need this variant.
async fn seed_provisioned_warehouse_with_secret(f: &Fixture) -> Uuid {
    let (ws_id,): (Uuid,) =
        sqlx::query_as("INSERT INTO workspaces (slug, name) VALUES ($1, $2) RETURNING id")
            .bind(format!("engine-ws-sec-{}", Uuid::new_v4().simple()))
            .bind("Engine Test Workspace (secret)")
            .fetch_one(&f.db.pool)
            .await
            .expect("insert workspace");

    // Seal a global named secret with the SAME crypto key `build_app` uses, so
    // the app's `resolve_plaintext(None, secret_ref)` can decrypt it.
    let crypto =
        egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secrets = egret_secrets::SecretStore::new(f.db.clone(), Some(crypto));
    let ws_secrets = egret_secrets::WorkspaceSecretStore::new(f.db.clone(), secrets);
    let secret_id = ws_secrets
        .create(
            None,
            &format!("engine-sec-{}", Uuid::new_v4().simple()),
            None,
            "STORAGESECRETKEY",
        )
        .await
        .expect("create global storage secret");

    let (storage_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key, secret_ref) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(format!("engine-sc-sec-{}", Uuid::new_v4().simple()))
    .bind("us-east-1")
    .bind("egret-engine-test")
    .bind("AKIATEST")
    .bind(secret_id)
    .fetch_one(&f.db.pool)
    .await
    .expect("insert storage connection");

    let catalog_store = egret_catalog_engine::CatalogStore::new(f.db.clone());
    let wh = catalog_store
        .create_warehouse(
            &format!("engine-wh-sec-{}", Uuid::new_v4().simple()),
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

/// Kube-free `EngineProvisioningPort` fake. `status` reports a single running,
/// ready pod named "coordinator-0" (so the pod-ownership check in
/// `pod_logs`/`pod_exec` passes for that name). Every other call is RECORDED in
/// shared `Arc` state, so a test that keeps a clone of the fake can inspect
/// exactly what the app asked the cluster to do. `fail_provision`/`fail_update`
/// make the matching call record its spec and then fail, standing in for a
/// Kubernetes API error; `delete_error`, when `Some`, makes `delete` fail with
/// that message instead (and record nothing) — for the 502/retry tests. The
/// kube-shaped ops (exec/pod-logs/coordinator-request) are NOT on this trait —
/// `egret_provisioner::kube_client::FakeKube` covers those, passed separately as the
/// fixture's `kube` field.
#[derive(Clone, Default)]
struct FakeEnginePort {
    restarted: Arc<std::sync::Mutex<Vec<(String, String)>>>,
    provisioned: Arc<std::sync::Mutex<Vec<egret_engine_mgmt_api::EngineSpec>>>,
    updated: Arc<std::sync::Mutex<Vec<egret_engine_mgmt_api::EngineSpec>>>,
    /// `(kind, namespace, name)` per successful `delete`.
    deleted: Arc<std::sync::Mutex<Vec<(String, String, String)>>>,
    /// When `Some`, `delete` fails with this message and records nothing.
    delete_error: Arc<std::sync::Mutex<Option<String>>>,
    fail_provision: Arc<std::sync::atomic::AtomicBool>,
    fail_update: Arc<std::sync::atomic::AtomicBool>,
}

impl FakeEnginePort {
    fn last_provisioned(&self) -> egret_engine_mgmt_api::EngineSpec {
        self.provisioned
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("provision was called")
    }
    fn last_updated(&self) -> egret_engine_mgmt_api::EngineSpec {
        self.updated
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("update was called")
    }
    fn set_fail_update(&self, fail: bool) {
        self.fail_update
            .store(fail, std::sync::atomic::Ordering::SeqCst);
    }
    #[allow(dead_code)]
    fn set_fail_provision(&self, fail: bool) {
        self.fail_provision
            .store(fail, std::sync::atomic::Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl EngineProvisioningPort for FakeEnginePort {
    async fn provision(&self, req: &egret_engine_mgmt_api::EngineSpec) -> Result<(), String> {
        self.provisioned.lock().unwrap().push(req.clone());
        if self
            .fail_provision
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err("injected provision failure".into());
        }
        Ok(())
    }
    async fn status(
        &self,
        _kind: &str,
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
    async fn update(&self, req: &egret_engine_mgmt_api::EngineSpec) -> Result<(), String> {
        self.updated.lock().unwrap().push(req.clone());
        if self.fail_update.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("injected update failure".into());
        }
        Ok(())
    }
    async fn delete(&self, kind: &str, ns: &str, name: &str) -> Result<(), String> {
        if let Some(msg) = self.delete_error.lock().unwrap().clone() {
            return Err(msg);
        }
        self.deleted
            .lock()
            .unwrap()
            .push((kind.to_string(), ns.to_string(), name.to_string()));
        Ok(())
    }
    async fn restart(&self, _kind: &str, ns: &str, name: &str) -> Result<(), String> {
        self.restarted
            .lock()
            .unwrap()
            .push((ns.to_string(), name.to_string()));
        Ok(())
    }
}

#[tokio::test]
async fn monitor_queries_maps_list_and_maps_bad_gateway() {
    let fake = Arc::new(egret_provisioner::kube_client::FakeKube::default());
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
    let fake = Arc::new(egret_provisioner::kube_client::FakeKube::default());
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
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
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
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
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

/// `POST /engines` with `kind: "starrocks"` persists a `"starrocks"`-type
/// engine row (not the back-compat-default `"trino"`), auto-wires a PRIMARY
/// `egret-warehouse` catalog row the same way the Trino path does (T6 — the
/// runtime `CREATE EXTERNAL CATALOG` DDL itself is smoke-only, exercised by a
/// spawned best-effort background task this test does not wait on), and still
/// calls through to the fake provisioning port (the create path selects the
/// StarRocks spec-building branch).
#[tokio::test]
async fn create_engine_starrocks_persists_starrocks_type() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_admin_sr", true).await;
    let wh_id = seed_provisioned_warehouse_with_secret(&f).await;
    let name = format!("sr-{}", Uuid::new_v4().simple());

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
                "kind": "starrocks",
                "coordinatorReplicas": 1,
                "workerReplicas": 1,
                "cpuMin": "1",
                "cpuMax": "2",
                "memory": "4Gi",
                "starrocks": {
                    "feReplicas": 1,
                    "cnReplicas": 2,
                },
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create starrocks engine; body={j}"
    );
    assert_eq!(j["name"], serde_json::json!(name), "engine name; body={j}");
    assert_eq!(j["type"], "starrocks", "engine type; body={j}");
    assert_eq!(j["status"], "provisioning", "engine status; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    // The persisted row's type is "starrocks" (not the default "trino"),
    // confirmed straight off the store (not just the DTO projection).
    let (db_type,): (String,) =
        sqlx::query_as("SELECT type FROM engine_mgmt.engines WHERE id = $1")
            .bind(Uuid::parse_str(&engine_id).unwrap())
            .fetch_one(&f.db.pool)
            .await
            .expect("fetch engine row");
    assert_eq!(db_type, "starrocks", "stored engine type");

    // A Catalog Client IS minted + a PRIMARY `egret-warehouse` catalog row
    // recorded for StarRocks now (T6), mirroring the Trino auto-wire exactly.
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
    let cats = j.as_array().expect("catalogs array");
    assert_eq!(
        cats.len(),
        1,
        "one auto-wired catalog for starrocks; body={j}"
    );
    assert_eq!(cats[0]["kind"], "egret-warehouse", "catalog kind; body={j}");
    assert_eq!(cats[0]["isPrimary"], true, "catalog is primary; body={j}");
    assert_eq!(cats[0]["name"], "egret", "catalog name; body={j}");

    // The client's credential lives on the primary catalog row and is revoked
    // when the engine is deleted, same as Trino's; confirm it was minted.
    let (client_count,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM catalog.catalog_clients WHERE warehouse_id = $1 AND name = $2",
    )
    .bind(wh_id)
    .bind(format!("engine-{name}"))
    .fetch_one(&f.db.pool)
    .await
    .expect("count catalog_clients");
    assert_eq!(client_count, 1, "catalog client minted for starrocks");

    // PUT (update) on a StarRocks engine is now functional: it merges the body
    // over the stored spec (`build_starrocks_spec`) and re-provisions via the
    // fake port. A config-only PUT that leaves the delegation block untouched
    // succeeds without re-bootstrapping/re-materializing.
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
                "coordinatorReplicas": 2,
                "workerReplicas": 2,
                "cpuMin": "1",
                "cpuMax": "2",
                "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "PUT on a starrocks engine re-provisions and succeeds"
    );

    // POST /restart on a StarRocks engine now rolls the FE/CN pods via the CR
    // annotation merge-patch (the fake port records the call), so it succeeds.
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
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "restart on a starrocks engine now rolls FE/CN pods"
    );
}

/// An unrecognized `kind` on create is rejected with 400 (rather than silently
/// falling through to the Trino path).
#[tokio::test]
async fn create_engine_unknown_kind_400() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_admin_badkind", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": format!("x-{}", Uuid::new_v4().simple()),
                "warehouseId": wh_id,
                "kind": "presto",
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
        "unknown engine kind must 400"
    );
}

#[tokio::test]
async fn pod_logs_known_pod_ok_unknown_pod_404() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
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
/// `egret-provisioner` for that fan-out). An unknown engine id is a 404.
#[tokio::test]
async fn restart_engine_calls_provisioner_unknown_404() {
    let fake = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(fake.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
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
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
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
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
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

/// The app's named-secret store, built with the SAME test crypto key as
/// `build_app`, so a test can read back what the handlers sealed.
fn secret_store(f: &Fixture) -> egret_secrets::WorkspaceSecretStore {
    egret_secrets::WorkspaceSecretStore::new(
        f.db.clone(),
        egret_secrets::SecretStore::new(
            f.db.clone(),
            Some(egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("crypto")),
        ),
    )
}

/// `(internal_secret_id, monitor_secret_id)` straight off the engine row.
async fn engine_secret_ids(f: &Fixture, engine_id: Uuid) -> (Option<Uuid>, Option<Uuid>) {
    sqlx::query_as(
        "SELECT internal_secret_id, monitor_secret_id FROM engine_mgmt.engines WHERE id = $1",
    )
    .bind(engine_id)
    .fetch_one(&f.db.pool)
    .await
    .expect("engine secret ids")
}

/// `POST /engines` for a Trino engine with the wizard defaults and the given
/// `trinoConfig`; returns the status and body.
async fn create_engine_via_api(
    f: &Fixture,
    token: &str,
    name: &str,
    wh_id: Uuid,
    trino_config: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
                "trinoConfig": trino_config,
            })),
        ))
        .await
        .unwrap();
    into_json(resp).await
}

/// `TRINO_INTERNAL_SECRET` exactly as the env Secret renders it.
fn rendered_internal_secret(spec: &egret_engine_mgmt_api::EngineSpec) -> String {
    egret_provisioner::trino::trino_env_secret(spec)["stringData"]["TRINO_INTERNAL_SECRET"]
        .as_str()
        .expect("TRINO_INTERNAL_SECRET rendered")
        .to_string()
}

/// The `egret-monitor` bcrypt hash exactly as `password.db` renders it,
/// relabelled `$2y$`→`$2b$` for `bcrypt::verify`.
fn monitor_hash(spec: &egret_engine_mgmt_api::EngineSpec) -> String {
    let cm = egret_provisioner::trino::trino_config_map(spec);
    let db = cm["data"]["password.db"]
        .as_str()
        .expect("password.db rendered");
    let line = db
        .lines()
        .find(|l| l.starts_with("egret-monitor:"))
        .expect("egret-monitor entry");
    line["egret-monitor:".len()..].replacen("$2y$", "$2b$", 1)
}

/// Names of every internal/monitor secret minted for this engine.
async fn internal_secret_names(f: &Fixture, engine_id: Uuid) -> Vec<String> {
    let names: Vec<String> =
        sqlx::query_scalar("SELECT name FROM workspace_secrets WHERE workspace_id IS NULL")
            .fetch_all(&f.db.pool)
            .await
            .expect("list global secret names");
    let (a, b) = (
        format!("engine-internal-{engine_id}-"),
        format!("engine-monitor-{engine_id}-"),
    );
    names
        .into_iter()
        .filter(|n| n.starts_with(&a) || n.starts_with(&b))
        .collect()
}

/// P1: a new Trino engine gets random stored secrets; the rendered env Secret
/// and password.db carry them, not the id-derived values.
#[tokio::test]
async fn create_renders_stored_random_internal_secrets() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_p1_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(
        &f,
        &token,
        &name,
        wh_id,
        serde_json::json!({ "delegationMode": "impersonate", "auth": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();

    let (internal_id, monitor_id) = engine_secret_ids(&f, id).await;
    let secrets = secret_store(&f);
    let internal = secrets
        .resolve_plaintext(None, internal_id.expect("internal secret recorded"))
        .await
        .unwrap();
    let monitor = secrets
        .resolve_plaintext(None, monitor_id.expect("monitor secret recorded"))
        .await
        .unwrap();

    let spec = port.last_provisioned();
    let rendered = rendered_internal_secret(&spec);
    assert_eq!(rendered, internal);
    assert_ne!(rendered, format!("egret-internal-{id}"));
    let hash = monitor_hash(&spec);
    assert!(bcrypt::verify(&monitor, &hash).unwrap());
    assert!(!bcrypt::verify(format!("egret-monitor-{id}"), &hash).unwrap());
}

/// P1 transition + F1 fix: an engine rendered before this change (no secret
/// ids) adopts stored secrets on its FIRST apply attempt EVEN WHEN that
/// attempt fails. `TrinoProvisioner::apply_all` writes the env Secret and the
/// `password.db` ConfigMap before the Deployments and does not roll those
/// back on a later failure, so a failed apply may already have handed the
/// cluster the new pair — discarding it would risk deleting a value the
/// running pods carry. The row therefore COMMITS the pending pair on both the
/// success and the failure path, and the NEXT apply (successful or not)
/// REUSES that same committed pair rather than minting a third generation.
#[tokio::test]
async fn legacy_engine_adopts_internal_secrets_even_from_a_failed_apply() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_p1_legacy_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let trino_config = serde_json::json!({ "delegationMode": "impersonate", "auth": {} });
    let engine = egret_engine_mgmt::EngineMgmtStore::new(f.db.clone())
        .create_engine(
            &name,
            "trino",
            "egret-engines",
            wh_id,
            &serde_json::json!({
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
                "imageVersion": "470", "trinoConfig": trino_config.clone(),
            }),
            None,
        )
        .await
        .expect("seed legacy engine row");
    let put_body = serde_json::json!({
        "name": name, "warehouseId": wh_id,
        "coordinatorReplicas": 1, "workerReplicas": 1,
        "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
        "trinoConfig": trino_config,
    });

    port.set_fail_update(true);
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{}", engine.id),
            &token,
            Some(put_body.clone()),
        ))
        .await
        .unwrap();
    assert!(!resp.status().is_success(), "injected failure must surface");
    let (internal_id, monitor_id) = engine_secret_ids(&f, engine.id).await;
    assert!(
        internal_id.is_some() && monitor_id.is_some(),
        "a failed apply still COMMITS the pending pair — the cluster may already carry it"
    );
    assert_eq!(
        internal_secret_names(&f, engine.id).await.len(),
        2,
        "exactly one generation exists after the failed attempt"
    );

    port.set_fail_update(false);
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{}", engine.id),
            &token,
            Some(put_body),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let (internal_id2, monitor_id2) = engine_secret_ids(&f, engine.id).await;
    assert_eq!(
        internal_id2, internal_id,
        "the next apply REUSES the already-committed pair"
    );
    assert_eq!(monitor_id2, monitor_id);
    let secrets = secret_store(&f);
    let internal = secrets
        .resolve_plaintext(None, internal_id2.unwrap())
        .await
        .unwrap();
    let monitor = secrets
        .resolve_plaintext(None, monitor_id2.unwrap())
        .await
        .unwrap();
    let spec = port.last_updated();
    assert_eq!(rendered_internal_secret(&spec), internal);
    assert!(bcrypt::verify(&monitor, &monitor_hash(&spec)).unwrap());
    assert_eq!(
        internal_secret_names(&f, engine.id).await.len(),
        2,
        "no third generation minted — the pair was reused, not rotated"
    );
}

/// Reviewer coverage: a SECOND successful apply on the same (new, this-side-of-
/// the-change) engine reuses the already-committed pair — no third generation
/// minted, ids unchanged.
#[tokio::test]
async fn second_successful_apply_reuses_the_same_internal_secret_pair() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_p1_reuse_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(
        &f,
        &token,
        &name,
        wh_id,
        serde_json::json!({ "delegationMode": "impersonate", "auth": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    let (internal_id, monitor_id) = engine_secret_ids(&f, id).await;
    assert!(internal_id.is_some() && monitor_id.is_some());
    assert_eq!(internal_secret_names(&f, id).await.len(), 2);

    let put_body = serde_json::json!({
        "name": name, "warehouseId": wh_id,
        "coordinatorReplicas": 1, "workerReplicas": 2,
        "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
        "trinoConfig": { "delegationMode": "impersonate", "auth": {} },
    });
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{id}"),
            &token,
            Some(put_body),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let (internal_id2, monitor_id2) = engine_secret_ids(&f, id).await;
    assert_eq!(
        internal_id2, internal_id,
        "ids unchanged across the second apply"
    );
    assert_eq!(monitor_id2, monitor_id);
    assert_eq!(
        internal_secret_names(&f, id).await.len(),
        2,
        "no third generation minted"
    );
    let secrets = secret_store(&f);
    let internal = secrets
        .resolve_plaintext(None, internal_id2.unwrap())
        .await
        .unwrap();
    let spec = port.last_updated();
    assert_eq!(rendered_internal_secret(&spec), internal);
}

/// A jwt-delegation engine has no PASSWORD authenticator (`is_jwt_delegation`
/// skips `inject_monitor_user`, so `password.db` carries no `egret-monitor`
/// entry), but it still needs a `TRINO_INTERNAL_SECRET` for coordinator↔worker
/// traffic — and `resolve_internal_secrets` always mints/commits the pair
/// together, so a monitor secret is still recorded even though nothing
/// references it in the rendered spec.
#[tokio::test]
async fn jwt_delegation_engine_mints_and_commits_internal_secret_without_a_monitor_user() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_p1_jwt_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(
        &f,
        &token,
        &name,
        wh_id,
        serde_json::json!({ "delegationMode": "jwt" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();

    let (internal_id, monitor_id) = engine_secret_ids(&f, id).await;
    let secrets = secret_store(&f);
    let internal = secrets
        .resolve_plaintext(None, internal_id.expect("internal secret recorded"))
        .await
        .unwrap();
    monitor_id.expect("monitor secret is still minted+committed alongside the internal one");

    let spec = port.last_provisioned();
    assert_eq!(rendered_internal_secret(&spec), internal);
    let cm = egret_provisioner::trino::trino_config_map(&spec);
    assert!(
        cm["data"].get("password.db").is_none(),
        "jwt-only engines render no password.db at all (no PASSWORD authenticator users)"
    );
}

#[tokio::test]
async fn update_engine_auth_and_ingress() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
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
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
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
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
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
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
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
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
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
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
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
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
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
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
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
    let fake = Arc::new(egret_provisioner::kube_client::FakeKube::default());
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
            None,
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
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
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
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
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

// ── engine delete (SP1 Part A) ─────────────────────────────────────────────────

/// The stored lifecycle status, or `None` when the row is gone.
async fn engine_status(f: &Fixture, id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT status FROM engine_mgmt.engines WHERE id = $1")
        .bind(id)
        .fetch_optional(&f.db.pool)
        .await
        .expect("read engine status")
}

/// The materialized connection for (workspace, engine).
async fn materialized_db_id(f: &Fixture, ws: Uuid, engine_id: Uuid) -> Uuid {
    sqlx::query_scalar("SELECT id FROM query.databases WHERE workspace_id = $1 AND engine_id = $2")
        .bind(ws)
        .bind(engine_id)
        .fetch_one(&f.db.pool)
        .await
        .expect("materialized connection")
}

/// `PUT /engines/{id}/workspaces` → 200.
async fn put_workspaces(f: &Fixture, token: &str, engine_id: Uuid, ws: &[Uuid]) {
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}/workspaces"),
            token,
            Some(serde_json::json!({ "workspaceIds": ws })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "put workspaces; body={j}");
}

/// Seed one blocking dependent of `kind` (a `dependents::EngineDependentKind`
/// wire name) called `name` on connection `database_id`.
async fn seed_dependent(f: &Fixture, ws: Uuid, database_id: Uuid, kind: &str, name: &str) {
    match kind {
        "dataapi_endpoint" | "dataapi_endpoint_trashed" => {
            let id: Uuid = sqlx::query_scalar(
                "INSERT INTO dataapi.endpoints (workspace_id, name, path, method, database_id, statements, state) \
                 VALUES ($1, $2, $3, 'GET', $4, \
                         jsonb_build_array(jsonb_build_object('name', 'main', 'sql', 'SELECT 1')), 'draft') \
                 RETURNING id",
            )
            .bind(ws)
            .bind(name)
            .bind(format!("/{name}"))
            .bind(database_id)
            .fetch_one(&f.db.pool)
            .await
            .expect("seed endpoint");
            if kind == "dataapi_endpoint_trashed" {
                sqlx::query("UPDATE dataapi.endpoints SET deleted_at = now() WHERE id = $1")
                    .bind(id)
                    .execute(&f.db.pool)
                    .await
                    .expect("trash endpoint");
            }
        }
        "schedule" => {
            sqlx::query(
                "INSERT INTO automation.schedules (workspace_id, type, name, crontab, database_id, sql, validator_type) \
                 VALUES ($1, 'alert', $2, '0 9 * * *', $3, 'SELECT 1', 'not_null')",
            )
            .bind(ws)
            .bind(name)
            .bind(database_id)
            .execute(&f.db.pool)
            .await
            .expect("seed alert");
        }
        "dataset" => {
            sqlx::query(
                "INSERT INTO analytics.datasets (workspace_id, database_id, name, table_name) \
                 VALUES ($1, $2, $3, 't')",
            )
            .bind(ws)
            .bind(database_id)
            .bind(name)
            .execute(&f.db.pool)
            .await
            .expect("seed dataset");
        }
        other => panic!("unknown dependent kind {other}"),
    }
}

/// D1/D2/D8: one blocking dependent → 409 naming it; the cluster and the row untouched.
async fn assert_engine_delete_blocked_by(kind: &str) {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_delete_blocked", true).await;
    let wh = seed_provisioned_warehouse(&f).await;
    let engine_id = seed_jwt_engine(&f, wh).await;
    let ws = seed_workspace(&f).await;
    put_workspaces(&f, &token, engine_id, &[ws]).await;
    let db = materialized_db_id(&f, ws, engine_id).await;
    seed_dependent(&f, ws, db, kind, "orders-by-day").await;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/engines/{engine_id}"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CONFLICT, "kind={kind}; body={j}");
    assert_eq!(j["code"], "engine_has_dependents", "body={j}");
    let deps = j["dependents"].as_array().expect("dependents array");
    assert_eq!(deps.len(), 1, "body={j}");
    assert_eq!(deps[0]["workspaceId"], ws.to_string(), "body={j}");
    assert_eq!(deps[0]["kind"], kind, "body={j}");
    assert_eq!(deps[0]["count"], 1, "body={j}");
    assert_eq!(
        deps[0]["sample"],
        serde_json::json!(["orders-by-day"]),
        "body={j}"
    );
    assert!(
        port.deleted.lock().unwrap().is_empty(),
        "the cluster must not be touched"
    );
    assert_eq!(
        engine_status(&f, engine_id).await.as_deref(),
        Some("provisioning"),
        "the row must survive with its status unchanged"
    );
    assert_eq!(
        materialized_count(&f, ws, engine_id).await,
        1,
        "the connection survives"
    );
}

#[tokio::test]
async fn delete_engine_is_blocked_by_a_data_api_endpoint() {
    assert_engine_delete_blocked_by("dataapi_endpoint").await;
}

#[tokio::test]
async fn delete_engine_is_blocked_by_a_trashed_data_api_endpoint() {
    assert_engine_delete_blocked_by("dataapi_endpoint_trashed").await;
}

#[tokio::test]
async fn delete_engine_is_blocked_by_a_sql_alert() {
    assert_engine_delete_blocked_by("schedule").await;
}

#[tokio::test]
async fn delete_engine_is_blocked_by_a_dataset() {
    assert_engine_delete_blocked_by("dataset").await;
}

/// D7: create refuses without Kubernetes, so delete must too — deleting only the
/// row would orphan the cluster.
#[tokio::test]
async fn delete_engine_without_kubernetes_is_400_and_keeps_the_row() {
    let Some(f) = setup_full(None, None).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_delete_nokube", true).await;
    let wh = seed_provisioned_warehouse(&f).await;
    let engine_id = seed_jwt_engine(&f, wh).await;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/engines/{engine_id}"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={j}");
    assert!(
        j["error"]
            .as_str()
            .unwrap_or("")
            .contains("kubernetes not configured"),
        "body={j}"
    );
    assert_eq!(
        engine_status(&f, engine_id).await.as_deref(),
        Some("provisioning")
    );
}

/// D4/D5/D6: a failed cluster delete is a 502 with the cause, leaves a visible
/// `deleting` row, and the same request succeeds once the cluster answers.
#[tokio::test]
async fn delete_engine_cluster_failure_is_502_deleting_and_retryable() {
    let port = FakeEnginePort::default();
    *port.delete_error.lock().unwrap() = Some("apiserver: 500 Internal Server Error".into());
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_delete_retry", true).await;
    let wh = seed_provisioned_warehouse(&f).await;
    let engine_id = seed_jwt_engine(&f, wh).await;
    let uri = format!("/engines/{engine_id}");

    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &uri, &token, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "body={j}");
    assert_eq!(j["code"], "engine_cluster_delete_failed", "body={j}");
    assert!(
        j["error"].as_str().unwrap_or("").contains("apiserver: 500"),
        "body={j}"
    );
    assert_eq!(
        engine_status(&f, engine_id).await.as_deref(),
        Some("deleting")
    );

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &uri, &token, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "body={j}");
    assert_eq!(
        j["status"]["phase"], "deleting",
        "stored status must not be masked; body={j}"
    );

    *port.delete_error.lock().unwrap() = None;
    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &uri, &token, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "retry succeeds");
    assert_eq!(
        engine_status(&f, engine_id).await,
        None,
        "row gone after the retry"
    );
}

/// Happy path: 204, cluster deleted once, engine-generated secrets gone, the
/// minted catalog client revoked, a client we did NOT mint left alone.
#[tokio::test]
async fn delete_engine_removes_cluster_row_secrets_and_minted_clients_only() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_delete_ok", true).await;
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
    let engine_id: Uuid = j["id"].as_str().unwrap().parse().unwrap();

    let store = egret_engine_mgmt::EngineMgmtStore::new(f.db.clone());
    let crypto =
        egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secrets = egret_secrets::WorkspaceSecretStore::new(
        f.db.clone(),
        egret_secrets::SecretStore::new(f.db.clone(), Some(crypto)),
    );
    let delegator = secrets
        .create(
            None,
            &format!("test-delegator-{}", Uuid::new_v4().simple()),
            None,
            "pw",
        )
        .await
        .unwrap();
    let root = secrets
        .create(
            None,
            &format!("test-root-{}", Uuid::new_v4().simple()),
            None,
            "pw",
        )
        .await
        .unwrap();
    store
        .set_delegator_secret(engine_id, delegator)
        .await
        .unwrap();
    store.set_root_secret(engine_id, root).await.unwrap();
    let engine = store.get_engine(engine_id).await.unwrap().unwrap();
    let mut engine_secrets = vec![delegator, root];
    engine_secrets.extend(engine.tls_secret_id);

    let primary = store
        .list_engine_catalogs(engine_id)
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.is_primary)
        .expect("primary catalog");
    let minted: Uuid = primary.config["clientRowId"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let catalog_store = egret_catalog_engine::CatalogStore::new(f.db.clone());
    let wh = catalog_store.get_warehouse(wh_id).await.unwrap().unwrap();
    let role = catalog_store
        .create_role(
            wh_id,
            "readonly_analyst",
            "external",
            Some("read-only role"),
        )
        .await
        .unwrap();
    let foreign = catalog_store
        .create_client(&egret_catalog_engine::NewClient {
            name: "not-minted-by-the-engine".into(),
            consumer_type: "external".into(),
            workspace_id: wh.workspace_id,
            warehouse_id: wh_id,
            catalog_role_id: role.id,
        })
        .await
        .unwrap();
    store
        .create_engine_catalog(
            engine_id,
            "external",
            "egret-warehouse",
            &serde_json::json!({ "clientRowId": foreign.id, "mintedClient": false }),
            None,
            false,
        )
        .await
        .unwrap();

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/engines/{engine_id}"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "body={j}");

    assert_eq!(
        *port.deleted.lock().unwrap(),
        vec![(
            "trino".to_string(),
            "egret-engines".to_string(),
            name.clone()
        )]
    );
    assert_eq!(engine_status(&f, engine_id).await, None, "row gone");
    let secrets_left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workspace_secrets WHERE id = ANY($1)")
            .bind(engine_secrets.clone())
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    assert_eq!(
        secrets_left, 0,
        "engine-generated secrets {engine_secrets:?} must be deleted"
    );
    let client_count = |id: Uuid| {
        let pool = f.db.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM catalog.catalog_clients WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(
        client_count(minted).await,
        0,
        "the minted client is revoked"
    );
    assert_eq!(
        client_count(foreign.id).await,
        1,
        "a client the engine did not mint survives"
    );
}

/// Seed a trino profile named `name` with its `{name}_tag` tag service, as
/// auto-bind creates them, and bind `engines` to it.
async fn seed_ranger_profile(f: &Fixture, name: &str, engines: &[Uuid]) -> i64 {
    use egret_policy_api::PolicyPort;
    let policy = egret_policy::PolicyStore::new(f.db.clone());
    PolicyPort::ensure_trino_def(&policy)
        .await
        .expect("trino def");
    PolicyPort::ensure_tag_def(&policy).await.expect("tag def");
    let tag = format!("{name}_tag");
    PolicyPort::create_service(&policy, &tag, "tag", &serde_json::json!({}))
        .await
        .expect("tag service");
    let profile = PolicyPort::create_service(
        &policy,
        name,
        "trino",
        &serde_json::json!({ "tagService": tag }),
    )
    .await
    .expect("trino profile");
    let store = egret_engine_mgmt::EngineMgmtStore::new(f.db.clone());
    for e in engines {
        store
            .set_engine_authz_service(*e, Some(profile.id))
            .await
            .unwrap();
    }
    profile.id
}

async fn service_exists(f: &Fixture, name: &str) -> bool {
    use egret_policy_api::PolicyPort;
    let policy = egret_policy::PolicyStore::new(f.db.clone());
    PolicyPort::get_service_opt(&policy, name)
        .await
        .unwrap()
        .is_some()
}

/// D9, by user decision: a profile can hold admin-written policies and nothing
/// records that egret created it, so engine delete unbinds and never deletes.
#[tokio::test]
async fn delete_engine_keeps_its_ranger_profile_and_tag_service() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_delete_keeps_ranger", true).await;
    let wh = seed_provisioned_warehouse(&f).await;
    let engine_id = seed_jwt_engine(&f, wh).await;
    let name: String = sqlx::query_scalar("SELECT name FROM engine_mgmt.engines WHERE id = $1")
        .bind(engine_id)
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    let profile_id = seed_ranger_profile(&f, &name, &[engine_id]).await;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/engines/{engine_id}"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(
        service_exists(&f, &name).await,
        "the profile survives the engine"
    );
    assert!(
        service_exists(&f, &format!("{name}_tag")).await,
        "and so does its tag service"
    );
    let bound: i64 =
        sqlx::query_scalar("SELECT count(*) FROM engine_mgmt.engines WHERE authz_service_id = $1")
            .bind(profile_id)
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    assert_eq!(bound, 0, "no engine is bound to it any more");
}

// ── engine ↔ workspace preflight (SP1 Part A) ────────────────────────────────

/// Every blocking kind refuses an unassign, and nothing changes.
#[tokio::test]
async fn unassign_is_refused_while_the_workspace_uses_the_engine() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "unassign_blocked", true).await;
    let wh = seed_provisioned_warehouse(&f).await;
    for kind in [
        "dataapi_endpoint",
        "dataapi_endpoint_trashed",
        "schedule",
        "dataset",
    ] {
        let engine_id = seed_jwt_engine(&f, wh).await;
        let ws = seed_workspace(&f).await;
        put_workspaces(&f, &token, engine_id, &[ws]).await;
        let db = materialized_db_id(&f, ws, engine_id).await;
        seed_dependent(&f, ws, db, kind, "nightly").await;

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
        let (status, j) = into_json(resp).await;
        assert_eq!(status, StatusCode::CONFLICT, "kind={kind}; body={j}");
        assert_eq!(j["code"], "engine_has_dependents", "kind={kind}; body={j}");
        assert_eq!(j["dependents"][0]["kind"], kind, "body={j}");
        assert_eq!(
            materialized_count(&f, ws, engine_id).await,
            1,
            "kind={kind}: connection kept"
        );
        let assigned: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM engine_mgmt.engine_workspaces WHERE engine_id = $1 AND workspace_id = $2",
        )
        .bind(engine_id)
        .bind(ws)
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
        assert_eq!(assigned, 1, "kind={kind}: assignment kept");
    }
}

/// `PUT {wsB}` over `{wsA}` is one assign + one blocked unassign: nothing applies.
#[tokio::test]
async fn put_workspaces_applies_nothing_when_one_unassign_is_blocked() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "put_ws_blocked", true).await;
    let wh = seed_provisioned_warehouse(&f).await;
    let engine_id = seed_jwt_engine(&f, wh).await;
    let ws_a = seed_workspace(&f).await;
    let ws_b = seed_workspace(&f).await;
    put_workspaces(&f, &token, engine_id, &[ws_a]).await;
    let db = materialized_db_id(&f, ws_a, engine_id).await;
    seed_dependent(&f, ws_a, db, "dataapi_endpoint", "orders").await;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}/workspaces"),
            &token,
            Some(serde_json::json!({ "workspaceIds": [ws_b] })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CONFLICT, "body={j}");
    assert_eq!(
        j["dependents"][0]["workspaceId"],
        ws_a.to_string(),
        "body={j}"
    );
    assert_eq!(
        materialized_count(&f, ws_a, engine_id).await,
        1,
        "A untouched"
    );
    assert_eq!(
        materialized_count(&f, ws_b, engine_id).await,
        0,
        "B was NOT assigned first"
    );
}

#[tokio::test]
async fn unknown_workspaces_are_404_and_apply_nothing() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "unknown_ws", true).await;
    let wh = seed_provisioned_warehouse(&f).await;
    let engine_id = seed_jwt_engine(&f, wh).await;
    let real = seed_workspace(&f).await;
    let ghost = Uuid::new_v4();

    // PUT with one real and one unknown workspace: 404, the real one not assigned.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}/workspaces"),
            &token,
            Some(serde_json::json!({ "workspaceIds": [real, ghost] })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "PUT unknown workspace (was 500)"
    );
    assert_eq!(materialized_count(&f, real, engine_id).await, 0);

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/workspaces/{ghost}/engines"),
            &token,
            Some(serde_json::json!({ "engineId": engine_id })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "assign to unknown workspace (was 500)"
    );

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/workspaces/{ghost}/engines/{engine_id}"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "unassign from unknown workspace (was 204)"
    );
}

/// C1: `add_catalog` is connector-first — any non-blank kind is a Trino
/// connector name. The 0057 CHECK (`egret-warehouse|postgresql|tpch|tpcds|generic`)
/// turned a `mysql` catalog into a 500.
#[tokio::test]
async fn add_catalog_accepts_a_connector_kind_outside_the_legacy_enum() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_c1_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/engines/{engine_id}/catalogs"),
            &token,
            Some(serde_json::json!({
                "name": "shop",
                "kind": "mysql",
                "config": { "properties": {
                    "connection-url": "jdbc:mysql://db:3306",
                    "connection-user": "reader"
                } }
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "add mysql catalog; body={j}");
    assert_eq!(j["kind"], "mysql", "stored kind; body={j}");
}

/// P8: an inline password on a non-warehouse catalog is masked on read, and
/// re-saving the form (which PUTs the masked config back) keeps the real value
/// in the row AND in what reaches the cluster.
#[tokio::test]
async fn catalog_detail_masks_inline_secrets_and_a_resave_keeps_them() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_p8_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create engine; body={j}");
    let engine_id = j["id"].as_str().expect("engine id").to_string();

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/engines/{engine_id}/catalogs"),
            &token,
            Some(serde_json::json!({
                "name": "pg", "kind": "postgresql",
                "config": { "properties": {
                    "connection-url": "jdbc:postgresql://db:5432/x",
                    "connection-user": "u",
                    "connection-password": "hunter2"
                } }
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "add pg catalog; body={j}");
    let cat_id = j["id"].as_str().expect("catalog id").to_string();

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
    let (status, detail) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "catalog detail; body={detail}");
    assert_eq!(
        detail["config"]["properties"]["connection-password"],
        "********"
    );
    assert_eq!(detail["config"]["properties"]["connection-user"], "u");
    assert!(
        !detail.to_string().contains("hunter2"),
        "no plaintext in GET; body={detail}"
    );

    // Re-save exactly what the form received.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{engine_id}/catalogs/{cat_id}"),
            &token,
            Some(serde_json::json!({ "config": detail["config"].clone() })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "re-save catalog; body={j}");

    let stored = egret_engine_mgmt::EngineMgmtStore::new(f.db.clone())
        .get_engine_catalog(Uuid::parse_str(&cat_id).unwrap())
        .await
        .expect("get catalog")
        .expect("catalog row");
    assert_eq!(
        stored.config["properties"]["connection-password"],
        "hunter2"
    );
    let rendered = port
        .last_updated()
        .catalogs
        .into_iter()
        .find(|c| c.name == "pg")
        .expect("pg catalog rendered");
    assert_eq!(rendered.properties["connection-password"], "hunter2");
}

async fn stored_status(f: &Fixture, engine_id: &str) -> String {
    sqlx::query_scalar("SELECT status FROM engine_mgmt.engines WHERE id = $1")
        .bind(Uuid::parse_str(engine_id).unwrap())
        .fetch_one(&f.db.pool)
        .await
        .expect("engine status")
}

/// P6: `GET /engines/{id}` records `running` once live status reports every pod
/// ready, and never overwrites `deleting`.
#[tokio::test]
async fn get_engine_records_running_and_leaves_deleting_alone() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_p6_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;

    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let id = j["id"].as_str().unwrap().to_string();
    assert_eq!(stored_status(&f, &id).await, "provisioning");

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("/engines/{id}"), &token, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        stored_status(&f, &id).await,
        "running",
        "all pods ready ⇒ running recorded"
    );

    let name2 = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name2, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create 2; body={j}");
    let id2 = j["id"].as_str().unwrap().to_string();
    sqlx::query("UPDATE engine_mgmt.engines SET status = 'deleting' WHERE id = $1")
        .bind(Uuid::parse_str(&id2).unwrap())
        .execute(&f.db.pool)
        .await
        .unwrap();
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("/engines/{id2}"), &token, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(stored_status(&f, &id2).await, "deleting");
}

/// Rotate: `apply_built` commits the pending pair on BOTH the success and the
/// failure path (Task 7's reviewed decision), so a failed apply replaces the
/// row's ids — but F2 (standing rule: never delete a value the cluster may
/// still hold) means it must NOT delete the pair it superseded, because a
/// failure can happen either before or after the cluster actually received
/// it. A LATER successful apply deletes ONLY the one pair its own
/// compare-and-set displaced (never a wider sweep — that could catch a
/// DIFFERENT, concurrently in-flight apply's freshly-minted pair, see
/// `commit_internal_secrets`'s doc comment), so a generation left behind by a
/// failed rotation survives as a permanent orphan until engine delete (Task
/// 15) — `internal_secret_names` count asserted at each step to pin exactly
/// which pairs are deleted and which survive. Non-Trino → 400, unknown → 404.
#[tokio::test]
async fn rotate_secrets_replaces_both_values_only_on_a_successful_apply() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_rotate_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(
        &f,
        &token,
        &name,
        wh_id,
        serde_json::json!({ "delegationMode": "impersonate", "auth": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    let secrets = secret_store(&f);
    let (old_internal_id, old_monitor_id) = engine_secret_ids(&f, id).await;
    let (old_internal_id, old_monitor_id) = (old_internal_id.unwrap(), old_monitor_id.unwrap());
    let old_internal = secrets
        .resolve_plaintext(None, old_internal_id)
        .await
        .unwrap();
    let old_monitor = secrets
        .resolve_plaintext(None, old_monitor_id)
        .await
        .unwrap();
    assert_eq!(
        internal_secret_names(&f, id).await.len(),
        2,
        "baseline: one generation exists"
    );
    let rotate = || {
        req(
            "POST",
            &format!("/engines/{id}/rotate-secrets"),
            &token,
            None,
        )
    };

    port.set_fail_update(true);
    let (status, j) = into_json(f.app.clone().oneshot(rotate()).await.unwrap()).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "body={j}");
    assert!(
        j["error"]
            .as_str()
            .unwrap()
            .contains("injected update failure"),
        "body={j}"
    );
    // F2 (standing rule: never delete a value the cluster may still hold): the
    // failed apply still commits the pair it attempted to hand the cluster
    // (the row moves on), but it must NOT delete the pair it superseded — the
    // failure could have happened before OR after the cluster received the
    // new pair, so both must stay resolvable until a later success proves
    // which one the cluster actually converged on.
    let (failed_internal_id, failed_monitor_id) = engine_secret_ids(&f, id).await;
    let (failed_internal_id, failed_monitor_id) =
        (failed_internal_id.unwrap(), failed_monitor_id.unwrap());
    assert_ne!(failed_internal_id, old_internal_id);
    assert_ne!(failed_monitor_id, old_monitor_id);
    assert_eq!(
        internal_secret_names(&f, id).await.len(),
        4,
        "both generations on record: the failed apply deleted nothing"
    );
    assert_eq!(
        secrets
            .resolve_plaintext(None, old_internal_id)
            .await
            .expect("the superseded pair must still resolve after a failed apply"),
        old_internal
    );
    assert_eq!(
        secrets
            .resolve_plaintext(None, old_monitor_id)
            .await
            .expect("the superseded pair must still resolve after a failed apply"),
        old_monitor
    );

    port.set_fail_update(false);
    let resp = f.app.clone().oneshot(rotate()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let (new_internal_id, new_monitor_id) = engine_secret_ids(&f, id).await;
    let (new_internal_id, new_monitor_id) = (new_internal_id.unwrap(), new_monitor_id.unwrap());
    assert_ne!(new_internal_id, old_internal_id);
    assert_ne!(new_monitor_id, old_monitor_id);
    assert_ne!(new_internal_id, failed_internal_id);
    assert_ne!(new_monitor_id, failed_monitor_id);
    let new_internal = secrets
        .resolve_plaintext(None, new_internal_id)
        .await
        .unwrap();
    let new_monitor = secrets
        .resolve_plaintext(None, new_monitor_id)
        .await
        .unwrap();
    assert_ne!(new_internal, old_internal);

    let spec = port.last_updated();
    assert_eq!(rendered_internal_secret(&spec), new_internal);
    let hash = monitor_hash(&spec);
    // bcrypt salts every hash, so "the hash changed" proves nothing; verify.
    assert!(bcrypt::verify(&new_monitor, &hash).unwrap());
    assert!(!bcrypt::verify(&old_monitor, &hash).unwrap());
    // F2 round 2 (re-review): the successful apply's own compare-and-set only
    // displaces the ONE pair it read as `expected` — the failed rotation's
    // pair, since that is what the row named when THIS build started. It must
    // NOT reach further back and delete the original pair too: a wider,
    // prefix-based sweep was tried in round 1 and reverted because it could
    // just as easily catch a DIFFERENT, concurrently in-flight apply's
    // freshly-minted (not-yet-committed) pair — see `commit_internal_secrets`'s
    // doc comment. So the original pair survives as a permanent orphan here;
    // only engine delete (Task 15) is a safe place to sweep it.
    assert_eq!(
        secrets
            .resolve_plaintext(None, old_internal_id)
            .await
            .expect("the original pair is NOT this commit's `expected` — it must survive"),
        old_internal
    );
    assert_eq!(
        secrets
            .resolve_plaintext(None, old_monitor_id)
            .await
            .expect("the original pair is NOT this commit's `expected` — it must survive"),
        old_monitor
    );
    assert!(
        matches!(
            secrets.resolve_plaintext(None, failed_internal_id).await,
            Err(egret_secrets::SecretsError::NotFound)
        ),
        "the failed rotation's pair WAS this commit's `expected` — it is deleted"
    );
    assert!(matches!(
        secrets.resolve_plaintext(None, failed_monitor_id).await,
        Err(egret_secrets::SecretsError::NotFound)
    ));
    assert_eq!(
        internal_secret_names(&f, id).await.len(),
        4,
        "the original pair is an orphan (2 rows) alongside the current pair (2 rows) — \
         narrowing the delete to only what this apply superseded means it is NOT swept here"
    );

    let sr = egret_engine_mgmt::EngineMgmtStore::new(f.db.clone())
        .create_engine(
            &format!("sr-{}", Uuid::new_v4().simple()),
            "starrocks",
            "egret-engines",
            wh_id,
            &serde_json::json!({ "starrocksConfig": {} }),
            None,
        )
        .await
        .unwrap();
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/engines/{}/rotate-secrets", sr.id),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/engines/{}/rotate-secrets", Uuid::new_v4()),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

async fn stored_spec(f: &Fixture, id: &str) -> serde_json::Value {
    sqlx::query_scalar("SELECT spec FROM engine_mgmt.engines WHERE id = $1")
        .bind(Uuid::parse_str(id).unwrap())
        .fetch_one(&f.db.pool)
        .await
        .expect("engine spec")
}

/// P3: a rejected apply restores the stored spec and is a 502 carrying the cause;
/// a body whose trinoConfig omits delegationMode keeps the stored mode.
#[tokio::test]
async fn trino_update_failure_restores_the_spec_and_keeps_delegation_mode() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_p3_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let id = j["id"].as_str().unwrap().to_string();
    let before = stored_spec(&f, &id).await;
    assert_eq!(before["trinoConfig"]["delegationMode"], "jwt");

    port.set_fail_update(true);
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{id}"),
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 5,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "body={j}");
    assert!(
        j["error"]
            .as_str()
            .unwrap()
            .contains("injected update failure"),
        "body={j}"
    );
    assert_eq!(stored_spec(&f, &id).await, before, "spec restored");

    port.set_fail_update(false);
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{id}"),
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
                "trinoConfig": {
                    "coordinator": { "replicas": 1 },
                    "worker": { "replicas": 1 },
                    "auth": { "jwt": {} }
                }
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "update; body={j}");
    assert_eq!(
        stored_spec(&f, &id).await["trinoConfig"]["delegationMode"],
        "jwt",
        "omitted delegationMode must keep the stored mode"
    );
}

/// P4: a Trino update re-materializes the connections of assigned workspaces.
#[tokio::test]
async fn trino_update_rematerializes_assigned_workspace_connections() {
    let Some(f) = setup_full(
        Some(Arc::new(FakeEnginePort::default())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_p4_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    let ws = seed_workspace(&f).await;
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{id}/workspaces"),
            &token,
            Some(serde_json::json!({ "workspaceIds": [ws] })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Make the materialized connection stale.
    sqlx::query(
        "UPDATE query.databases SET options = '{}'::jsonb WHERE workspace_id = $1 AND engine_id = $2",
    )
    .bind(ws)
    .bind(id)
    .execute(&f.db.pool)
    .await
    .unwrap();

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/engines/{id}"),
            &token,
            Some(serde_json::json!({
                "name": name, "warehouseId": wh_id,
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
            })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let scheme: Option<String> = sqlx::query_scalar(
        "SELECT options->>'http_scheme' FROM query.databases WHERE workspace_id = $1 AND engine_id = $2",
    )
    .bind(ws)
    .bind(id)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assert_eq!(
        scheme.as_deref(),
        Some("https"),
        "connection re-materialized"
    );
}

/// P5: removing a catalog applies the cluster WITHOUT it first; a rejected apply
/// leaves the row and its minted client in place so a retry can act.
#[tokio::test]
async fn remove_catalog_applies_before_deleting_and_revoking() {
    let port = FakeEnginePort::default();
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_p5_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(&f, &token, &name, wh_id, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "create; body={j}");
    let engine_id = j["id"].as_str().unwrap().to_string();

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/engines/{engine_id}/catalogs"),
            &token,
            Some(serde_json::json!({
                "name": "extra", "kind": "egret-warehouse",
                "config": { "warehouseId": wh_id }
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "add warehouse catalog; body={j}"
    );
    let cat_id = Uuid::parse_str(j["id"].as_str().unwrap()).unwrap();
    let store = egret_engine_mgmt::EngineMgmtStore::new(f.db.clone());
    let client_id = store
        .get_engine_catalog(cat_id)
        .await
        .unwrap()
        .unwrap()
        .config["clientRowId"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .expect("minted client id");
    let catalog_store = egret_catalog_engine::CatalogStore::new(f.db.clone());
    let remove = || {
        req(
            "DELETE",
            &format!("/engines/{engine_id}/catalogs/{cat_id}"),
            &token,
            None,
        )
    };

    port.set_fail_update(true);
    let resp = f.app.clone().oneshot(remove()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(
        port.last_updated()
            .catalogs
            .iter()
            .all(|c| c.name != "extra"),
        "the apply rendered the engine without the catalog"
    );
    assert!(
        store.get_engine_catalog(cat_id).await.unwrap().is_some(),
        "row kept"
    );
    assert!(
        catalog_store
            .list_clients(wh_id)
            .await
            .unwrap()
            .iter()
            .any(|c| c.id == client_id),
        "client not revoked"
    );

    port.set_fail_update(false);
    let resp = f.app.clone().oneshot(remove()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        store.get_engine_catalog(cat_id).await.unwrap().is_none(),
        "row deleted"
    );
    assert!(
        !catalog_store
            .list_clients(wh_id)
            .await
            .unwrap()
            .iter()
            .any(|c| c.id == client_id),
        "minted client revoked"
    );
}

/// P2: a create whose provision fails leaves no engine row and no catalog client,
/// and asks the cluster to delete what it may have applied — for both kinds.
#[tokio::test]
async fn failed_provision_rolls_back_row_client_and_cluster() {
    let port = FakeEnginePort::default();
    port.set_fail_provision(true);
    let Some(f) = setup_full(
        Some(Arc::new(port.clone())),
        Some(Arc::new(egret_provisioner::kube_client::FakeKube::default())),
    )
    .await
    else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_p2_admin", true).await;
    let catalog_store = egret_catalog_engine::CatalogStore::new(f.db.clone());
    let row_exists = |name: String| {
        let pool = f.db.pool.clone();
        async move {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM engine_mgmt.engines WHERE name = $1)",
            )
            .bind(name)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };

    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());
    let (status, j) = create_engine_via_api(
        &f,
        &token,
        &name,
        wh_id,
        serde_json::json!({ "delegationMode": "impersonate", "auth": {} }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body={j}");
    assert!(!row_exists(name.clone()).await, "Trino row rolled back");
    assert!(
        catalog_store.list_clients(wh_id).await.unwrap().is_empty(),
        "Trino client revoked"
    );
    assert!(port.deleted.lock().unwrap().contains(&(
        "trino".to_string(),
        "egret-engines".to_string(),
        name.clone()
    )));

    let sr_wh = seed_provisioned_warehouse_with_secret(&f).await;
    let sr_name = format!("sr-{}", Uuid::new_v4().simple());
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/engines",
            &token,
            Some(serde_json::json!({
                "name": sr_name, "warehouseId": sr_wh, "kind": "starrocks",
                "coordinatorReplicas": 1, "workerReplicas": 1,
                "cpuMin": "1", "cpuMax": "2", "memory": "4Gi",
                "starrocks": { "feReplicas": 1, "cnReplicas": 1 },
            })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(
        !row_exists(sr_name.clone()).await,
        "StarRocks row rolled back"
    );
    assert!(
        catalog_store.list_clients(sr_wh).await.unwrap().is_empty(),
        "StarRocks client revoked"
    );
    assert!(port.deleted.lock().unwrap().contains(&(
        "starrocks".to_string(),
        "egret-engines".to_string(),
        sr_name
    )));
}
