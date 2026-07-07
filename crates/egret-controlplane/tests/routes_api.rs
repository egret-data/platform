//! Control-plane Storage + Secrets HTTP route tests (platform-admin gate).
//! DB-gated; self-skips without `DATABASE_URL`.
//!
//! Proves `egret_controlplane::control_plane_router`:
//!   - a non-platform-admin session is Forbidden (403) on the global routes,
//!   - a `platform_admin` session can list storage (200), create a global named
//!     secret (201) and create a global storage connection (201).
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret
//!   cargo test -p egret-controlplane

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::response::Response;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_controlplane::{control_plane_router, CpState};
use egret_metastore::Db;
use http_body_util::BodyExt;
use std::sync::Arc;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

struct Fixture {
    app: axum::Router,
    users: UserRepo,
    sessions: SessionService,
    db: Db,
}

async fn into_json(resp: Response) -> (StatusCode, serde_json::Value) {
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(
        |_| serde_json::json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }),
    );
    (status, json)
}

async fn setup_with_browser(
    browser: Option<Arc<dyn egret_controlplane::WarehouseBrowser>>,
) -> Option<Fixture> {
    setup_full(browser, None).await
}

async fn setup_full(
    browser: Option<Arc<dyn egret_controlplane::WarehouseBrowser>>,
    engine_provisioner: Option<Arc<dyn egret_controlplane::EngineProvisioner>>,
) -> Option<Fixture> {
    let dsn = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let crypto =
        egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secrets = egret_secrets::SecretStore::new(db.clone(), Some(crypto));
    let workspace_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets);

    let users = UserRepo::new(db.clone());
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );

    let app = control_plane_router().with_state(CpState {
        storage: egret_catalog::CatalogStore::new(db.clone()),
        authz: egret_authz::AuthzStore::new(db.clone()),
        users: users.clone(),
        groups: egret_auth::groups::GroupRepo::new(db.clone()),
        roles: egret_auth::roles::RoleRepo::new(db.clone()),
        secrets: workspace_secrets,
        sessions: sessions.clone(),
        provisioner: None,
        browser,
        engine_provisioner,
        engine_egret_url: "http://egret:8080".into(),
        engine_jwks_uri: "https://idp/jwks".into(),
    });

    Some(Fixture {
        app,
        users,
        sessions,
        db,
    })
}

async fn setup() -> Option<Fixture> {
    setup_with_browser(None).await
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
            "INSERT INTO platform_role_assignments (principal_type, principal_id, role_id) \
             SELECT 'user', $1, id FROM roles WHERE scope='platform' AND name='platform_admin'",
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
        "INSERT INTO platform_role_assignments (principal_type, principal_id, role_id) \
         VALUES ('user', $1, $2)",
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

#[tokio::test]
async fn non_admin_is_forbidden() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "plain", false).await;

    // GET /storage → 403.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/storage", &token, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non platform-admin must not list storage"
    );

    // POST /secrets → 403 (gate runs before persistence).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/secrets",
            &token,
            Some(serde_json::json!({ "name": "nope", "value": "x" })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non platform-admin must not create secrets"
    );
}

#[tokio::test]
async fn platform_admin_can_manage_storage_and_secrets() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "admin", true).await;
    let suffix = Uuid::new_v4().simple().to_string();

    // GET /storage → 200.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/storage", &token, None))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list storage; body={json}");
    assert!(json.is_array(), "storage list is an array; body={json}");

    // POST /secrets → 201, global named secret.
    let secret_name = format!("cp-secret-{suffix}");
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/secrets",
            &token,
            Some(serde_json::json!({ "name": secret_name, "value": "s3cr3t" })),
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create secret; body={json}");
    let secret_id = json["id"].as_str().expect("secret id in body").to_string();
    assert_eq!(json["name"], serde_json::json!(secret_name));

    // POST /storage → 201, global storage connection referencing the secret.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/storage",
            &token,
            Some(serde_json::json!({
                "name": format!("cp-store-{suffix}"),
                "provider": "s3",
                "region": "us-east-1",
                "bucket": "egret-cp-test",
                "access_key": "AKIATEST",
                "secret_ref": secret_id,
                "path_style": true,
            })),
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create storage; body={json}");
    assert_eq!(json["bucket"], serde_json::json!("egret-cp-test"));
    assert_eq!(json["secret_ref"], serde_json::json!(secret_id));
}

// ── Warehouse browse ──────────────────────────────────────────────────────────

struct FakeBrowser;

#[async_trait::async_trait]
impl egret_controlplane::WarehouseBrowser for FakeBrowser {
    async fn namespaces(
        &self,
        _wh: &str,
    ) -> Result<Vec<egret_controlplane::BrowseNamespace>, String> {
        Ok(vec![egret_controlplane::BrowseNamespace {
            parts: vec!["analytics".into()],
            id: None,
            protected: false,
        }])
    }
    async fn tabulars(
        &self,
        _wh: &str,
        _ns: &[String],
    ) -> Result<Vec<egret_controlplane::BrowseTabular>, String> {
        Ok(vec![egret_controlplane::BrowseTabular {
            name: "events".into(),
            kind: "table".into(),
            id: None,
            protected: false,
        }])
    }
    async fn schema(
        &self,
        _wh: &str,
        _ns: &[String],
        _t: &str,
    ) -> Result<egret_controlplane::BrowseTableDetail, String> {
        Ok(egret_controlplane::BrowseTableDetail {
            location: "s3://bucket/t".into(),
            format_version: 2,
            last_updated_ms: 0,
            current_snapshot_id: None,
            snapshot_count: 0,
            columns: vec![egret_controlplane::BrowseColumn {
                name: "id".into(),
                type_: "long".into(),
                required: true,
            }],
            partition_fields: vec![],
            sort_fields: vec![],
            properties: std::collections::BTreeMap::new(),
            snapshots: vec![],
        })
    }
    async fn view_columns(
        &self,
        _wh: &str,
        _ns: &[String],
        _view: &str,
    ) -> Result<Vec<egret_controlplane::BrowseColumn>, String> {
        Ok(vec![egret_controlplane::BrowseColumn {
            name: "id".into(),
            type_: "long".into(),
            required: true,
        }])
    }
    async fn deleted_tabulars(
        &self,
        _wh: &str,
    ) -> Result<Vec<egret_controlplane::BrowseDeletedTabular>, String> {
        Ok(vec![])
    }
    async fn undrop(&self, _wh: &str, _kind: &str, _id: uuid::Uuid) -> Result<(), String> {
        Ok(())
    }
    async fn set_protection(
        &self,
        _wh: &str,
        _target: &str,
        _id: uuid::Uuid,
        protected: bool,
    ) -> Result<bool, String> {
        Ok(protected)
    }
    async fn config(&self, _wh: &str) -> Result<egret_controlplane::BrowseWarehouseConfig, String> {
        Ok(egret_controlplane::BrowseWarehouseConfig {
            active: true,
            delete_profile_seconds: Some(604800),
            num_tables: 3,
            num_views: 1,
            stats_updated_ms: 0,
        })
    }
    async fn set_active(&self, _wh: &str, _active: bool) -> Result<(), String> {
        Ok(())
    }
    async fn set_delete_profile(&self, _wh: &str, _seconds: Option<i64>) -> Result<(), String> {
        Ok(())
    }
    async fn set_format_versions(&self, _wh: &str, _versions: Vec<u8>) -> Result<(), String> {
        Ok(())
    }
    async fn rotate_credential(
        &self,
        _wh: &str,
        _access_key: &str,
        _secret_key: &str,
    ) -> Result<(), String> {
        Ok(())
    }
    async fn search(
        &self,
        _wh: &str,
        _query: &str,
    ) -> Result<Vec<egret_controlplane::BrowseSearchResult>, String> {
        Ok(vec![])
    }
    async fn tasks(&self, _wh: &str) -> Result<Vec<egret_controlplane::BrowseTask>, String> {
        Ok(vec![])
    }
}

/// Seed a workspace + storage_connection + warehouse with lakekeeper_warehouse_id set.
async fn seed_provisioned_warehouse(f: &Fixture) -> Uuid {
    let (ws_id,): (Uuid,) =
        sqlx::query_as("INSERT INTO workspaces (slug, name) VALUES ($1, $2) RETURNING id")
            .bind(format!("browse-ws-{}", Uuid::new_v4().simple()))
            .bind("Browse Test Workspace")
            .fetch_one(&f.db.pool)
            .await
            .expect("insert workspace");

    let (storage_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO storage_connections (name, region, bucket, access_key) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(format!("browse-sc-{}", Uuid::new_v4().simple()))
    .bind("us-east-1")
    .bind("egret-browse-test")
    .bind("AKIATEST")
    .fetch_one(&f.db.pool)
    .await
    .expect("insert storage connection");

    let catalog_store = egret_catalog::CatalogStore::new(f.db.clone());
    let wh = catalog_store
        .create_warehouse(
            &format!("browse-wh-{}", Uuid::new_v4().simple()),
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
            .bind(format!("browse-ws-unprov-{}", Uuid::new_v4().simple()))
            .bind("Browse Test Workspace Unprov")
            .fetch_one(&f.db.pool)
            .await
            .expect("insert workspace");

    let (storage_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO storage_connections (name, region, bucket, access_key) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(format!("browse-sc-unprov-{}", Uuid::new_v4().simple()))
    .bind("us-east-1")
    .bind("egret-browse-test")
    .bind("AKIATEST")
    .fetch_one(&f.db.pool)
    .await
    .expect("insert storage connection");

    let catalog_store = egret_catalog::CatalogStore::new(f.db.clone());
    catalog_store
        .create_warehouse(
            &format!("browse-wh-unprov-{}", Uuid::new_v4().simple()),
            ws_id,
            storage_id,
        )
        .await
        .expect("create warehouse")
        .id
}

#[tokio::test]
async fn browse_namespaces_tables_schema() {
    let Some(f) = setup_with_browser(Some(Arc::new(FakeBrowser))).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "browse_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;

    // GET /warehouses/{id}/namespaces → 200, [{parts: ["analytics"]}]
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/warehouses/{wh_id}/namespaces"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "namespaces; body={j}");
    assert_eq!(
        j[0]["parts"][0], "analytics",
        "namespace parts[0]; body={j}"
    );

    // GET /warehouses/{id}/namespaces/analytics/tables → 200
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/warehouses/{wh_id}/namespaces/analytics/tables"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "tables; body={j}");
    assert_eq!(j[0]["name"], "events", "table name; body={j}");
    assert_eq!(j[0]["kind"], "table", "table kind; body={j}");

    // GET /warehouses/{id}/namespaces/analytics/tables/events → 200
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/warehouses/{wh_id}/namespaces/analytics/tables/events"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "schema; body={j}");
    assert_eq!(j["columns"][0]["name"], "id", "column name; body={j}");
    assert_eq!(j["columns"][0]["type"], "long", "column type; body={j}");
}

#[tokio::test]
async fn browse_non_admin_forbidden() {
    let Some(f) = setup_with_browser(None).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "browse_nonadmin", false).await;
    // 403 fires before any DB lookup — use a random UUID.
    let wh_id = Uuid::new_v4();

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/warehouses/{wh_id}/namespaces"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "non-admin must be forbidden"
    );
}

// ── Engines ────────────────────────────────────────────────────────────────────

/// Engine provisioner fake. `restarted` records `(namespace, name)` pairs from
/// `restart` calls (shared via `Arc` so a test can hold onto the fake and
/// inspect it after the request completes, mirroring `MonitorFakeProvisioner`).
#[derive(Clone, Default)]
struct FakeEngineProvisioner {
    restarted: Arc<std::sync::Mutex<Vec<(String, String)>>>,
}

#[async_trait::async_trait]
impl egret_controlplane::EngineProvisioner for FakeEngineProvisioner {
    async fn provision(&self, _req: &egret_controlplane::EngineProvisionReq) -> Result<(), String> {
        Ok(())
    }
    async fn status(
        &self,
        _ns: &str,
        _name: &str,
    ) -> Result<egret_controlplane::EngineLiveStatus, String> {
        Ok(egret_controlplane::EngineLiveStatus {
            phase: "running".into(),
            message: None,
            pods: vec![egret_controlplane::EnginePod {
                name: "coordinator-0".into(),
                phase: "Running".into(),
                ready: true,
            }],
        })
    }
    async fn update(&self, _req: &egret_controlplane::EngineProvisionReq) -> Result<(), String> {
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
    async fn coordinator_request(
        &self,
        _namespace: &str,
        _service: &str,
        _method: &str,
        _path: &str,
        _headers: &[(String, String)],
        _body: Option<Vec<u8>>,
    ) -> anyhow::Result<(u16, Vec<u8>)> {
        Ok((200, b"[]".to_vec()))
    }
    async fn pod_logs(
        &self,
        _namespace: &str,
        pod: &str,
        _container: Option<&str>,
        _tail_lines: i64,
        _previous: bool,
    ) -> anyhow::Result<String> {
        Ok(format!("logs for {pod} (fake)\n"))
    }
    async fn pod_logs_stream(
        &self,
        _namespace: &str,
        _pod: &str,
        _tail_lines: i64,
    ) -> anyhow::Result<
        std::pin::Pin<Box<dyn futures_util::Stream<Item = anyhow::Result<bytes::Bytes>> + Send>>,
    > {
        Ok(Box::pin(futures_util::stream::once(async {
            Ok(bytes::Bytes::from_static(b"fake log line\n"))
        })))
    }
    async fn exec(
        &self,
        _namespace: &str,
        _pod: &str,
        _command: &[String],
    ) -> anyhow::Result<kube::api::AttachedProcess> {
        anyhow::bail!("exec not supported in fake provisioner")
    }
}

/// Engine provisioner fake whose `coordinator_request` returns a canned
/// `(status, body)` that the test can flip between calls (shared via `Arc`).
#[derive(Clone, Default)]
struct MonitorFakeProvisioner {
    response: Arc<std::sync::Mutex<(u16, Vec<u8>)>>,
}

impl MonitorFakeProvisioner {
    fn set(&self, code: u16, body: &[u8]) {
        *self.response.lock().unwrap() = (code, body.to_vec());
    }
}

#[async_trait::async_trait]
impl egret_controlplane::EngineProvisioner for MonitorFakeProvisioner {
    async fn provision(&self, _req: &egret_controlplane::EngineProvisionReq) -> Result<(), String> {
        Ok(())
    }
    async fn status(
        &self,
        _ns: &str,
        _name: &str,
    ) -> Result<egret_controlplane::EngineLiveStatus, String> {
        Ok(egret_controlplane::EngineLiveStatus {
            phase: "running".into(),
            message: None,
            pods: vec![],
        })
    }
    async fn update(&self, _req: &egret_controlplane::EngineProvisionReq) -> Result<(), String> {
        Ok(())
    }
    async fn delete(&self, _ns: &str, _name: &str) -> Result<(), String> {
        Ok(())
    }
    async fn restart(&self, _ns: &str, _name: &str) -> Result<(), String> {
        Ok(())
    }
    async fn coordinator_request(
        &self,
        _namespace: &str,
        _service: &str,
        _method: &str,
        _path: &str,
        _headers: &[(String, String)],
        _body: Option<Vec<u8>>,
    ) -> anyhow::Result<(u16, Vec<u8>)> {
        Ok(self.response.lock().unwrap().clone())
    }
    async fn pod_logs(
        &self,
        _namespace: &str,
        pod: &str,
        _container: Option<&str>,
        _tail_lines: i64,
        _previous: bool,
    ) -> anyhow::Result<String> {
        Ok(format!("logs for {pod} (fake)\n"))
    }
    async fn pod_logs_stream(
        &self,
        _namespace: &str,
        _pod: &str,
        _tail_lines: i64,
    ) -> anyhow::Result<
        std::pin::Pin<Box<dyn futures_util::Stream<Item = anyhow::Result<bytes::Bytes>> + Send>>,
    > {
        Ok(Box::pin(futures_util::stream::once(async {
            Ok(bytes::Bytes::from_static(b"fake log line\n"))
        })))
    }
    async fn exec(
        &self,
        _namespace: &str,
        _pod: &str,
        _command: &[String],
    ) -> anyhow::Result<kube::api::AttachedProcess> {
        anyhow::bail!("exec not supported in fake provisioner")
    }
}

#[tokio::test]
async fn monitor_queries_maps_list_and_maps_bad_gateway() {
    let fake = MonitorFakeProvisioner::default();
    let Some(f) = setup_full(None, Some(Arc::new(fake.clone()))).await else {
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
    fake.set(
        200,
        br#"[{"queryId":"20260704_x","state":"RUNNING","query":"SELECT 1",
            "session":{"user":"bob"},
            "queryStats":{"elapsedTime":"1.23s","totalCpuTime":"0.50s","userMemoryReservation":"12MB","progressPercentage":42.0}}]"#,
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
    fake.set(503, b"<html>service unavailable</html>");
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
/// LIMITATION: `MonitorFakeProvisioner` holds a single canned `(status, body)`
/// shared across every `coordinator_request` call, so it cannot return a
/// first response with `nextUri` and then a different second response — this
/// test therefore only exercises the immediate-terminal (no `nextUri`) path
/// through `test_catalog`'s poll loop, not a multi-hop `nextUri` chase.
#[tokio::test]
async fn test_catalog_reports_ok_error_and_non_2xx() {
    let fake = MonitorFakeProvisioner::default();
    let Some(f) = setup_full(None, Some(Arc::new(fake.clone()))).await else {
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
    fake.set(200, br#"{"id":"q1","stats":{"state":"FINISHED"}}"#);
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
    fake.set(
        200,
        br#"{"id":"q2","stats":{"state":"FAILED"},"error":{"message":"catalog not found"}}"#,
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
    fake.set(503, b"<html>service unavailable</html>");
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

#[tokio::test]
async fn create_and_get_engine() {
    let Some(f) = setup_full(None, Some(Arc::new(FakeEngineProvisioner::default()))).await else {
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
    let Some(f) = setup_full(None, Some(Arc::new(FakeEngineProvisioner::default()))).await else {
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
/// config: the route calls `EngineProvisioner::restart` with the engine's
/// namespace + name (from which the real egret-bin adapter derives BOTH the
/// `{name}-coordinator` and `{name}-worker` Deployment names — see
/// `fake_kube_restart_deployment_records_coordinator_and_worker` in
/// `egret-orchestrator` for that fan-out). An unknown engine id is a 404.
#[tokio::test]
async fn restart_engine_calls_provisioner_unknown_404() {
    let fake = FakeEngineProvisioner::default();
    let Some(f) = setup_full(None, Some(Arc::new(fake.clone()))).await else {
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
    let Some(f) = setup_full(None, Some(Arc::new(FakeEngineProvisioner::default()))).await else {
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
        "INSERT INTO storage_connections (name, region, bucket, access_key, endpoint, secret_ref, path_style) \
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
    let Some(f) = setup_full(None, Some(Arc::new(FakeEngineProvisioner::default()))).await else {
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
    let Some(f) = setup_full(None, Some(Arc::new(FakeEngineProvisioner::default()))).await else {
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
    let Some(f) = setup_full(None, Some(Arc::new(FakeEngineProvisioner::default()))).await else {
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
    let Some(f) = setup_full(None, Some(Arc::new(FakeEngineProvisioner::default()))).await else {
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
    let Some(f) = setup_full(None, Some(Arc::new(FakeEngineProvisioner::default()))).await else {
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
    let Some(f) = setup_full(None, Some(Arc::new(FakeEngineProvisioner::default()))).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_cat_clientid_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());

    // Seed a pre-existing, user-owned client on a custom (non-default) role.
    let catalog_store = egret_catalog::CatalogStore::new(f.db.clone());
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
        .create_client(&egret_catalog::service::embedded::NewClient {
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
    let stored = catalog_store
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
    let Some(f) = setup_full(None, Some(Arc::new(FakeEngineProvisioner::default()))).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "engine_cat_bad_clientid_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;
    let other_wh_id = seed_provisioned_warehouse(&f).await;
    let name = format!("trino-{}", Uuid::new_v4().simple());

    // A client that belongs to a DIFFERENT warehouse.
    let catalog_store = egret_catalog::CatalogStore::new(f.db.clone());
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
        .create_client(&egret_catalog::service::embedded::NewClient {
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
    let Some(f) = setup_full(None, Some(Arc::new(FakeEngineProvisioner::default()))).await else {
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
async fn browse_unprovisioned_warehouse_empty() {
    let Some(f) = setup_with_browser(Some(Arc::new(FakeBrowser))).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "browse_admin_unprov", true).await;
    let wh_id = seed_unprovisioned_warehouse(&f).await;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/warehouses/{wh_id}/namespaces"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "unprovisioned warehouse namespaces; body={j}"
    );
    assert_eq!(j, serde_json::json!([]), "should be empty array; body={j}");
}

#[tokio::test]
async fn platform_admin_can_list_policies_by_type() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "policies_admin", true).await;

    let authz = egret_authz::AuthzStore::new(f.db.clone());
    egret_authz::seed_trino(&authz)
        .await
        .expect("seed trino def");
    let svc_name = format!("trino-{}", Uuid::new_v4().simple());
    let svc = authz
        .create_service(&svc_name, "trino", &serde_json::json!({}))
        .await
        .expect("create service");
    authz
        .create_policy(
            svc.id,
            "allow-analysts-select",
            &serde_json::json!({
                "name": "allow-analysts-select",
                "policyType": 0,
                "resources": {
                    "catalog": {"values": ["iceberg"]},
                    "schema":  {"values": ["sales"]},
                    "table":   {"values": ["orders"]},
                    "column":  {"values": ["*"]}
                },
                "policyItems": [
                    {"users": ["alice"], "groups": ["analysts"], "accesses": [{"type": "select"}]}
                ]
            }),
        )
        .await
        .expect("create policy");

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/policies?service={svc_name}&type=access"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list policies; body={body}");
    let arr = body.as_array().expect("array body");
    assert_eq!(arr.len(), 1, "expected exactly one summary; body={body}");
    assert_eq!(arr[0]["policy_type"], serde_json::json!("access"));

    authz.delete_service(svc.id).await.expect("cleanup service");
}

/// Proves the Task 6 auto-bind primitives at the STORE level (no live
/// Kubernetes `EngineProvisioner` needed): a fresh authz profile bound via
/// `CatalogStore::set_engine_authz_service` round-trips through
/// `list_engines`, and the bound engine surfaces under its profile on
/// `GET /policies/services`.
#[tokio::test]
async fn platform_admin_autobind_engine_to_new_profile() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "autobind_admin", true).await;
    let wh_id = seed_provisioned_warehouse(&f).await;

    let catalog = egret_catalog::CatalogStore::new(f.db.clone());
    let authz = egret_authz::AuthzStore::new(f.db.clone());

    // Mirrors the `create_engine` auto-bind "create" branch: seed the Trino
    // service-def (required FK for `create_service`), then create a fresh
    // profile named after the engine.
    egret_authz::seed_trino(&authz)
        .await
        .expect("seed trino def");

    let engine_name = format!("trino-{}", Uuid::new_v4().simple());
    let engine = catalog
        .create_engine(
            &engine_name,
            "trino",
            "egret-engines",
            wh_id,
            &serde_json::json!({}),
        )
        .await
        .expect("create engine");
    assert_eq!(
        engine.authz_service_id, None,
        "engine starts unbound before auto-bind"
    );

    let svc = authz
        .create_service(&engine_name, "trino", &serde_json::json!({}))
        .await
        .expect("create service");

    catalog
        .set_engine_authz_service(engine.id, Some(svc.id))
        .await
        .expect("bind engine to profile");

    let engines = catalog.list_engines().await.expect("list engines");
    let bound = engines
        .iter()
        .find(|e| e.id == engine.id)
        .expect("engine present");
    assert_eq!(
        bound.authz_service_id,
        Some(svc.id),
        "authz_service_id round-trips through list_engines"
    );

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/policies/services", &token, None))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list profiles; body={body}");
    let arr = body.as_array().expect("array body");
    let profile = arr
        .iter()
        .find(|p| p["id"] == serde_json::json!(svc.id))
        .unwrap_or_else(|| panic!("profile {} not found; body={body}", svc.id));
    let profile_engines = profile["engines"].as_array().expect("engines array");
    assert!(
        profile_engines
            .iter()
            .any(|e| e["id"] == serde_json::json!(engine.id)),
        "bound engine should appear under its profile; body={body}"
    );

    catalog
        .delete_engine(engine.id)
        .await
        .expect("cleanup engine");
    authz.delete_service(svc.id).await.expect("cleanup service");
}

/// The Policies routes are platform-admin-gated (spec §5): a non-admin session
/// must be Forbidden (403), and the gate runs before any store read.
#[tokio::test]
async fn non_admin_cannot_list_policies() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "policies_plain", false).await;

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/policies?service=whatever", &token, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non platform-admin must not list policies"
    );
}

/// P2 Task 2: a platform admin can POST a new Access policy from the editor
/// form, see it in the list, then DELETE it.
#[tokio::test]
async fn admin_can_create_and_delete_access_policy() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "crud", true).await;

    let authz = egret_authz::AuthzStore::new(f.db.clone());
    egret_authz::seed_trino(&authz)
        .await
        .expect("seed trino def");
    let svc_name = format!("trino-{}", Uuid::new_v4().simple());
    let svc = authz
        .create_service(&svc_name, "trino", &serde_json::json!({}))
        .await
        .expect("create service");

    let create = serde_json::json!({
        "name": "p-create", "service": svc_name, "isEnabled": true,
        "resources": {"catalog": {"values": ["iceberg"]}},
        "items": [{"users": ["alice"], "accesses": ["select"], "effect": "allow"}]
    });
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", "/policies", &token, Some(create)))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "create policy; body={body}");
    let id = body["id"].as_i64().expect("id in response");

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/policies?service={svc_name}&type=access"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (_, list) = into_json(resp).await;
    assert_eq!(
        list.as_array().expect("array body").len(),
        1,
        "expected exactly one summary; body={list}"
    );

    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &format!("/policies/{id}"), &token, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "delete policy should return 204"
    );

    authz.delete_service(svc.id).await.expect("cleanup service");
}

/// P4 Task 3: `PUT /policies/{id}/enabled` toggles `enabled` without touching
/// the stored body, and `GET /policies/{id}` reflects the new `isEnabled`.
#[tokio::test]
async fn admin_can_toggle_policy_enabled() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "toggle", true).await;

    let authz = egret_authz::AuthzStore::new(f.db.clone());
    egret_authz::seed_trino(&authz)
        .await
        .expect("seed trino def");
    let svc_name = format!("trino-{}", Uuid::new_v4().simple());
    let svc = authz
        .create_service(&svc_name, "trino", &serde_json::json!({}))
        .await
        .expect("create service");

    let create = serde_json::json!({
        "name": "p-toggle", "service": svc_name, "isEnabled": true,
        "resources": {"catalog": {"values": ["iceberg"]}},
        "items": [{"users": ["alice"], "accesses": ["select"], "effect": "allow"}]
    });
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", "/policies", &token, Some(create)))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "create policy; body={body}");
    let id = body["id"].as_i64().expect("id in response");

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/policies/{id}/enabled"),
            &token,
            Some(serde_json::json!({ "enabled": false })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "toggle policy enabled should return 204"
    );

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("/policies/{id}"), &token, None))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "get policy; body={body}");
    assert_eq!(
        body["isEnabled"], false,
        "policy should be disabled; body={body}"
    );

    authz.delete_service(svc.id).await.expect("cleanup service");
}

/// P3 Task 2: the same `POST /policies` / `PUT /policies/{id}` endpoints
/// dispatch on `kind` to create Masking and Row-filter policies too.
#[tokio::test]
async fn admin_can_create_masking_and_rowfilter_policies() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "mrf", true).await;

    let authz = egret_authz::AuthzStore::new(f.db.clone());
    egret_authz::seed_trino(&authz)
        .await
        .expect("seed trino def");
    let svc_name = format!("trino-{}", Uuid::new_v4().simple());
    let svc = authz
        .create_service(&svc_name, "trino", &serde_json::json!({}))
        .await
        .expect("create service");

    let mask = serde_json::json!({
        "kind": "masking", "name": "m1", "service": svc_name, "isEnabled": true,
        "resources": {"catalog": {"values": ["c"]}, "schema": {"values": ["s"]},
                       "table": {"values": ["t"]}, "column": {"values": ["email"]}},
        "items": [{"groups": ["pii"], "maskType": "MASK_HASH"}]
    });
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", "/policies", &token, Some(mask)))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "create masking policy; body={body}");

    let rf = serde_json::json!({
        "kind": "rowfilter", "name": "r1", "service": svc_name, "isEnabled": true,
        "resources": {"catalog": {"values": ["c"]}, "schema": {"values": ["s"]},
                       "table": {"values": ["t"]}},
        "items": [{"users": ["bob"], "filterExpr": "country = 'US'"}]
    });
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", "/policies", &token, Some(rf)))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "create rowfilter policy; body={body}"
    );

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/policies?service={svc_name}&type=masking"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (_, masking_list) = into_json(resp).await;
    assert_eq!(
        masking_list.as_array().expect("array body").len(),
        1,
        "expected exactly one masking summary; body={masking_list}"
    );

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/policies?service={svc_name}&type=rowfilter"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (_, rf_list) = into_json(resp).await;
    assert_eq!(
        rf_list.as_array().expect("array body").len(),
        1,
        "expected exactly one rowfilter summary; body={rf_list}"
    );

    authz.delete_service(svc.id).await.expect("cleanup service");
}

/// P3 Task 3: `GET /policies/services/{service}/trino-config` renders the
/// access-control.properties + ranger-trino-security.xml bundle a Trino
/// coordinator needs for the native ranger plugin.
#[tokio::test]
async fn admin_gets_trino_config_for_profile() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "cfg", true).await;

    let authz = egret_authz::AuthzStore::new(f.db.clone());
    egret_authz::seed_trino(&authz)
        .await
        .expect("seed trino def");
    let svc_name = format!("trino-{}", Uuid::new_v4().simple());
    let svc = authz
        .create_service(&svc_name, "trino", &serde_json::json!({}))
        .await
        .expect("create service");

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/policies/services/{svc_name}/trino-config"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "trino-config; body={body}");
    let files = body.as_array().expect("array body");
    assert_eq!(files.len(), 2);
    assert!(files
        .iter()
        .any(|f| f["name"] == "access-control.properties"
            && f["content"]
                .as_str()
                .unwrap()
                .contains(&format!("ranger.service.name={svc_name}"))));
    assert!(files
        .iter()
        .any(|f| f["name"] == "ranger-trino-security.xml"
            && f["content"]
                .as_str()
                .unwrap()
                .contains("ranger.plugin.trino.policy.rest.url")));

    authz.delete_service(svc.id).await.expect("cleanup service");
}

/// P2 Task 3: `GET /principals` sources egret users (and groups) for the
/// policy picker. `session_for` creates a `platform_admin` user, so at least
/// one user principal always exists.
#[tokio::test]
async fn admin_can_list_principals() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "principals", true).await;

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/principals?q=", &token, None))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list principals; body={body}");
    let arr = body.as_array().expect("array body");
    assert!(
        arr.iter().any(|p| p["kind"] == "user"),
        "expected at least one user principal; body={body}"
    );
}

/// P4 Task 4: `GET /policies/resource-lookup` returns distinct values used in
/// the service's existing policies, filtered by parent-level match.
#[tokio::test]
async fn platform_admin_can_lookup_resource_values() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "resource_lookup_admin", true).await;

    let authz = egret_authz::AuthzStore::new(f.db.clone());
    egret_authz::seed_trino(&authz)
        .await
        .expect("seed trino def");
    let svc_name = format!("trino-{}", Uuid::new_v4().simple());
    let svc = authz
        .create_service(&svc_name, "trino", &serde_json::json!({}))
        .await
        .expect("create service");
    authz
        .create_policy(
            svc.id,
            "allow-analysts-select",
            &serde_json::json!({
                "name": "allow-analysts-select",
                "policyType": 0,
                "resources": {
                    "catalog": {"values": ["iceberg"]},
                    "schema":  {"values": ["sales"]}
                },
                "policyItems": [
                    {"users": ["alice"], "accesses": [{"type": "select"}]}
                ]
            }),
        )
        .await
        .expect("create policy");

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/policies/resource-lookup?service={svc_name}&level=schema&catalog=iceberg"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "resource-lookup; body={body}");
    let arr = body.as_array().expect("array body");
    assert!(
        arr.iter().any(|v| v == "sales"),
        "expected 'sales' in schema suggestions; body={body}"
    );

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/policies/resource-lookup?service={svc_name}&level=schema&catalog=other"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "resource-lookup; body={body}");
    let arr = body.as_array().expect("array body");
    assert!(
        !arr.iter().any(|v| v == "sales"),
        "parent-filter mismatch should exclude 'sales'; body={body}"
    );

    authz.delete_service(svc.id).await.expect("cleanup service");
}

/// The `resource-lookup` route is platform-admin-gated like every other
/// `/policies` route.
#[tokio::test]
async fn non_admin_cannot_lookup_resource_values() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "resource_lookup_plain", false).await;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            "/policies/resource-lookup?service=whatever&level=catalog",
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non platform-admin must not use resource-lookup"
    );
}

// ── P1-T5: fine-grained `require_permission` enforcement ─────────────────────
//
// `platform_admin`/blanket-forbidden coverage lives above (per-module, from
// the `require_platform_admin` era). These prove the NEW `resource:action`
// gate: a scoped, non-admin principal holding the wrong (or no) permission is
// Forbidden, and a scoped, non-admin principal holding exactly the right
// permission succeeds — same as `platform_admin`, without the superuser
// bypass.

/// Engines: `engine:read` gates `GET /engines`, `engine:create` gates
/// `POST /engines`.
#[tokio::test]
async fn scoped_permission_engines_read_and_create() {
    let Some(f) = setup_full(None, Some(Arc::new(FakeEngineProvisioner::default()))).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };

    let wrong = session_for_with_permission(&f, "engine_scoped_wrong", "warehouse:read").await;
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

    let reader = session_for_with_permission(&f, "engine_scoped_reader", "engine:read").await;
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

    let creator = session_for_with_permission(&f, "engine_scoped_creator", "engine:create").await;
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

/// Warehouses: `warehouse:read` gates `GET /warehouses`, `warehouse:create`
/// gates `POST /warehouses`.
#[tokio::test]
async fn scoped_permission_warehouses_read_and_create() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };

    let wrong = session_for_with_permission(&f, "wh_scoped_wrong", "engine:read").await;
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/warehouses", &wrong, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a principal without warehouse:read must not list warehouses"
    );

    let reader = session_for_with_permission(&f, "wh_scoped_reader", "warehouse:read").await;
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/warehouses", &reader, None))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a scoped warehouse:read principal must list warehouses; body={body}"
    );

    let (ws_id,): (Uuid,) =
        sqlx::query_as("INSERT INTO workspaces (slug, name) VALUES ($1, $2) RETURNING id")
            .bind(format!("wh-scoped-ws-{}", Uuid::new_v4().simple()))
            .bind("Scoped Perm Test Workspace")
            .fetch_one(&f.db.pool)
            .await
            .expect("insert workspace");
    let (storage_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO storage_connections (name, region, bucket, access_key) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(format!("wh-scoped-sc-{}", Uuid::new_v4().simple()))
    .bind("us-east-1")
    .bind("egret-scoped-test")
    .bind("AKIATEST")
    .fetch_one(&f.db.pool)
    .await
    .expect("insert storage connection");
    let wh_body = serde_json::json!({
        "name": format!("wh-scoped-{}", Uuid::new_v4().simple()),
        "workspaceId": ws_id,
        "storageConnectionId": storage_id,
    });

    // warehouse:read alone must not create.
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", "/warehouses", &reader, Some(wh_body.clone())))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "warehouse:read alone must not create a warehouse"
    );

    let creator = session_for_with_permission(&f, "wh_scoped_creator", "warehouse:create").await;
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", "/warehouses", &creator, Some(wh_body)))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "a scoped warehouse:create principal must create; body={body}"
    );
}

/// Storage: `storage:read` gates `GET /storage`, `storage:create` gates
/// `POST /storage`. A principal with only one of the two must not get the other.
#[tokio::test]
async fn scoped_permission_storage_read_and_create() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };

    let wrong = session_for_with_permission(&f, "storage_scoped_wrong", "secret:read").await;
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/storage", &wrong, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a principal without storage:read must not list storage"
    );

    let reader = session_for_with_permission(&f, "storage_scoped_reader", "storage:read").await;
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/storage", &reader, None))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a scoped storage:read principal must list storage; body={body}"
    );

    // storage:read alone must not create.
    let suffix = Uuid::new_v4().simple().to_string();
    let create_body = serde_json::json!({
        "name": format!("scoped-store-{suffix}"),
        "provider": "s3",
        "region": "us-east-1",
        "bucket": "egret-scoped-test",
        "access_key": "AKIATEST",
        "path_style": true,
    });
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", "/storage", &reader, Some(create_body.clone())))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "storage:read alone must not create a storage connection"
    );

    let creator = session_for_with_permission(&f, "storage_scoped_creator", "storage:create").await;
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", "/storage", &creator, Some(create_body)))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "a scoped storage:create principal must create; body={body}"
    );
}

/// Secrets: `secret:read` gates `GET /secrets`, `secret:create` gates
/// `POST /secrets`.
#[tokio::test]
async fn scoped_permission_secrets_read_and_create() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };

    let wrong = session_for_with_permission(&f, "secret_scoped_wrong", "storage:read").await;
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/secrets", &wrong, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a principal without secret:read must not list secrets"
    );

    let reader = session_for_with_permission(&f, "secret_scoped_reader", "secret:read").await;
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/secrets", &reader, None))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a scoped secret:read principal must list secrets; body={body}"
    );

    // secret:read alone must not create.
    let suffix = Uuid::new_v4().simple().to_string();
    let create_body = serde_json::json!({
        "name": format!("scoped-secret-{suffix}"),
        "value": "s3cr3t",
    });
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", "/secrets", &reader, Some(create_body.clone())))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "secret:read alone must not create a secret"
    );

    let creator = session_for_with_permission(&f, "secret_scoped_creator", "secret:create").await;
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", "/secrets", &creator, Some(create_body)))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "a scoped secret:create principal must create; body={body}"
    );
}

/// Policies: `policy:read` gates `GET /policies`, `policy:create` gates
/// `POST /policies`.
#[tokio::test]
async fn scoped_permission_policies_read_and_create() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };

    let authz = egret_authz::AuthzStore::new(f.db.clone());
    egret_authz::seed_trino(&authz)
        .await
        .expect("seed trino def");
    let svc_name = format!("trino-{}", Uuid::new_v4().simple());
    let svc = authz
        .create_service(&svc_name, "trino", &serde_json::json!({}))
        .await
        .expect("create service");

    let wrong = session_for_with_permission(&f, "policy_scoped_wrong", "engine:read").await;
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/policies?service={svc_name}"),
            &wrong,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a principal without policy:read must not list policies"
    );

    let reader = session_for_with_permission(&f, "policy_scoped_reader", "policy:read").await;
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/policies?service={svc_name}"),
            &reader,
            None,
        ))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a scoped policy:read principal must list policies; body={body}"
    );

    let create_body = serde_json::json!({
        "name": "scoped-create",
        "service": svc_name,
        "isEnabled": true,
        "resources": {"catalog": {"values": ["iceberg"]}},
        "items": [{"users": ["alice"], "accesses": ["select"], "effect": "allow"}]
    });

    // policy:read alone must not create.
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", "/policies", &reader, Some(create_body.clone())))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "policy:read alone must not create a policy"
    );

    let creator = session_for_with_permission(&f, "policy_scoped_creator", "policy:create").await;
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", "/policies", &creator, Some(create_body)))
        .await
        .unwrap();
    let (status, body) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a scoped policy:create principal must create; body={body}"
    );

    authz.delete_service(svc.id).await.expect("cleanup service");
}

/// Monitor: every route in `monitor.rs` is gated on `engine:read` (monitoring
/// is read-only cluster/query observation, even `kill_query`'s DELETE verb).
/// Proves enforcement on both `/queries` (list) and `/cluster` (aggregate).
#[tokio::test]
async fn scoped_permission_monitor_queries_and_cluster() {
    let fake = MonitorFakeProvisioner::default();
    let Some(f) = setup_full(None, Some(Arc::new(fake.clone()))).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let admin_token = session_for(&f, "monitor_scoped_setup", true).await;
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

    fake.set(200, b"[]");

    let wrong = session_for_with_permission(&f, "monitor_scoped_wrong", "warehouse:read").await;
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

    let reader = session_for_with_permission(&f, "monitor_scoped_reader", "engine:read").await;
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

// ── Access (Users / Groups / Roles / Permissions) ─────────────────────────────

/// Build a `Fixture` backed by a dedicated, freshly-migrated Postgres SCHEMA
/// (not the shared `public` one every other test in this file uses) via a
/// pool-level `search_path` pin — the same per-connection isolation
/// `egret-catalog-engine::pool::build_isolated_pool` uses to keep lakekeeper's
/// tables out of `public`.
///
/// `last_admin_guard` needs this: `guard_last_admin` blocks on
/// `RoleRepo::count_platform_admins()` being GLOBALLY exactly 1, but dozens of
/// other `#[tokio::test]`s in this file call `session_for(.., true)`, each
/// inserting its own `platform_admin` assignment into the shared `public`
/// schema, and tests run concurrently — so the ambient count there is never
/// reliably 1. A private schema (starting at 0 assignments) sidesteps the
/// race instead of asserting on an unstable global.
async fn setup_isolated_schema() -> Option<Fixture> {
    // NB: this workspace pins `sqlx = "0.8"` (see egret-controlplane's
    // Cargo.toml), which predates the 0.9 `AssertSqlSafe`/`sql_str` API that
    // `egret-catalog-engine::pool::build_isolated_pool` uses (that crate
    // pulls in its own separate sqlx 0.9 dependency just for this trick — see
    // the `sqlx = { version = "0.9", ... }` comment in its Cargo.toml). On
    // 0.8, `sqlx::query(&str)` runs unparameterized SQL directly, no wrapper
    // needed.
    use sqlx::{ConnectOptions, Connection};
    use std::str::FromStr;

    let dsn = std::env::var("DATABASE_URL").ok()?;
    let schema = format!("rbac_isolated_{}", Uuid::new_v4().simple());
    let opts = sqlx::postgres::PgConnectOptions::from_str(&dsn).expect("parse DATABASE_URL");

    // Create the schema via a one-shot connection on the default search_path.
    let mut conn = opts
        .clone()
        .connect()
        .await
        .expect("connect for schema setup");
    sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS \"{schema}\""))
        .execute(&mut conn)
        .await
        .expect("create isolated schema");
    conn.close().await.ok();

    // Every pooled connection pins search_path to the fresh schema, so
    // unqualified DDL/DML (including `_sqlx_migrations`) lands there instead
    // of `public`.
    let set_search_path = format!("SET search_path = \"{schema}\", public");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .after_connect(move |conn, _meta| {
            let stmt = set_search_path.clone();
            Box::pin(async move {
                sqlx::query(&stmt).execute(&mut *conn).await?;
                Ok(())
            })
        })
        .connect_with(opts)
        .await
        .expect("connect isolated pool");

    let db = Db { pool };
    db.migrate().await.expect("migrate isolated schema");

    let crypto =
        egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secrets = egret_secrets::SecretStore::new(db.clone(), Some(crypto));
    let workspace_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets);

    let users = UserRepo::new(db.clone());
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );

    let app = control_plane_router().with_state(CpState {
        storage: egret_catalog::CatalogStore::new(db.clone()),
        authz: egret_authz::AuthzStore::new(db.clone()),
        users: users.clone(),
        groups: egret_auth::groups::GroupRepo::new(db.clone()),
        roles: egret_auth::roles::RoleRepo::new(db.clone()),
        secrets: workspace_secrets,
        sessions: sessions.clone(),
        provisioner: None,
        browser: None,
        engine_provisioner: None,
        engine_egret_url: "http://egret:8080".into(),
        engine_jwks_uri: "https://idp/jwks".into(),
    });

    Some(Fixture {
        app,
        users,
        sessions,
        db,
    })
}

/// `POST /users` creates a local user, `GET /users` lists it (with its
/// assigned role), and `POST`/`DELETE .../roles(/{roleId})` assign/unassign a
/// custom (non-admin) role — so no `guard_last_admin` guard is in play here.
#[tokio::test]
async fn users_crud_and_role_assign() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let admin = session_for(&f, "users_admin", true).await;
    let email = format!("cruduser_{}@egret.local", Uuid::new_v4().simple());

    // POST /users → 201, a local user.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/users",
            &admin,
            Some(serde_json::json!({
                "email": email,
                "name": "Crud User",
                "password": "S3curePassw0rd!",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create user; body={j}");
    assert_eq!(j["email"], serde_json::json!(email), "user email; body={j}");
    assert_eq!(j["provider"], "local", "user provider; body={j}");
    let user_id = j["id"].as_str().expect("user id in body").to_string();

    // GET /users → the new user shows up.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/users", &admin, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list users; body={j}");
    assert!(
        j.as_array()
            .expect("users array")
            .iter()
            .any(|u| u["id"] == user_id),
        "new user must be listed; body={j}"
    );

    // Create a custom (non-admin) role to assign/unassign.
    let role_name = format!("crud_role_{}", Uuid::new_v4().simple());
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/access/roles",
            &admin,
            Some(serde_json::json!({
                "scope": "platform",
                "name": role_name,
                "description": "crud test role",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create role; body={j}");
    let role_id = j["id"].as_str().expect("role id in body").to_string();

    // Assign the role to the new user.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/users/{user_id}/roles"),
            &admin,
            Some(serde_json::json!({ "roleId": role_id })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "assign role to user");

    // GET /users → the user's roles now include it.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/users", &admin, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list users after assign; body={j}");
    let entry = j
        .as_array()
        .expect("users array")
        .iter()
        .find(|u| u["id"] == user_id)
        .expect("assigned user still listed");
    assert!(
        entry["roles"]
            .as_array()
            .expect("roles array")
            .contains(&serde_json::json!(role_name)),
        "assigned role must show up on the user; entry={entry}"
    );

    // Unassign — a non-admin role, so `guard_last_admin` never fires.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/users/{user_id}/roles/{role_id}"),
            &admin,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "unassign a non-admin role"
    );
}

/// `guard_last_admin` (shared by the user- and group-role unassign routes)
/// blocks removing the platform-wide LAST `platform_admin` assignment — 409,
/// not 200/204. Run against `setup_isolated_schema` (see its docstring for
/// why the shared-schema fixture can't give this a deterministic count).
#[tokio::test]
async fn last_admin_guard() {
    let Some(f) = setup_isolated_schema().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };

    // The lone platform_admin in this isolated schema.
    let token = session_for(&f, "solo_admin", true).await;
    let (admin_id, role_id): (Uuid, Uuid) = sqlx::query_as(
        "SELECT pra.principal_id, pra.role_id FROM platform_role_assignments pra \
         JOIN roles r ON r.id = pra.role_id WHERE r.name = 'platform_admin'",
    )
    .fetch_one(&f.db.pool)
    .await
    .expect("the sole platform_admin assignment");

    // Removing the only platform_admin assignment must 409, not succeed.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/users/{admin_id}/roles/{role_id}"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "removing the last platform_admin must 409; body={j}"
    );
    assert!(
        j["error"]
            .as_str()
            .unwrap_or_default()
            .contains("last platform admin"),
        "409 body should explain the last-admin guard; body={j}"
    );

    // A second admin breaks the tie: the same removal now succeeds.
    let _second_admin_token = session_for(&f, "second_admin", true).await;
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/users/{admin_id}/roles/{role_id}"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "removal succeeds once a second platform_admin exists"
    );
}

/// Local groups support create/add-member/delete; an OIDC-synced group
/// (`upsert_oidc_group`) cannot be deleted (409) — mirrors the OIDC rename
/// guard on the same `groups` table (`rows == 0` ⇒ `CpError::Conflict`).
#[tokio::test]
async fn groups_local_and_oidc() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let admin = session_for(&f, "groups_admin", true).await;
    let group_name = format!("crud-group-{}", Uuid::new_v4().simple());

    // POST /groups → 201, a local group.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/groups",
            &admin,
            Some(serde_json::json!({ "name": group_name })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create local group; body={j}");
    assert_eq!(j["source"], "local", "group source; body={j}");
    let group_id = j["id"].as_str().expect("group id in body").to_string();

    // Add a member.
    let member_email = format!("groupmember_{}@egret.local", Uuid::new_v4().simple());
    let member_id = f
        .users
        .insert_local(&member_email, "Group Member", "x")
        .await
        .expect("insert member")
        .0;
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/groups/{group_id}/members"),
            &admin,
            Some(serde_json::json!({ "userId": member_id })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "add group member");

    // DELETE the local group → 204.
    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &format!("/groups/{group_id}"), &admin, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "delete local group");

    // An OIDC-synced group cannot be deleted through the admin route.
    let oidc_name = format!("oidc-group-{}", Uuid::new_v4().simple());
    let oidc_repo = egret_auth::groups::GroupRepo::new(f.db.clone());
    let oidc_group_id = oidc_repo
        .upsert_oidc_group(&oidc_name)
        .await
        .expect("upsert oidc group");
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/groups/{oidc_group_id}"),
            &admin,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "deleting an oidc-synced group must 409; body={j}"
    );
}

/// `POST /access/roles` creates a custom role; `PUT .../permissions` accepts
/// a known permission (204) and rejects an unknown one (400, validated
/// against `egret_core::rbac::is_known` before any DB write); a seeded role
/// (`data_steward`) cannot be deleted (409) but the custom role can (204).
#[tokio::test]
async fn roles_seeded_and_permissions() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let admin = session_for(&f, "roles_admin", true).await;
    let role_name = format!("custom_role_{}", Uuid::new_v4().simple());

    // POST /access/roles → 201, a custom role.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/access/roles",
            &admin,
            Some(serde_json::json!({
                "scope": "platform",
                "name": role_name,
                "description": "custom test role",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create custom role; body={j}");
    let role_id = j["id"].as_str().expect("role id in body").to_string();

    // PUT permissions with a known permission → 204.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/access/roles/{role_id}/permissions"),
            &admin,
            Some(serde_json::json!({ "permissions": ["engine:read"] })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "set a known permission"
    );

    // PUT permissions with an unknown permission → 400, no partial write.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/access/roles/{role_id}/permissions"),
            &admin,
            Some(serde_json::json!({ "permissions": ["engine:frobnicate"] })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "unknown permission must 400; body={j}"
    );

    // Seeded roles (migration 0003) cannot be deleted.
    let (seeded_id,): (Uuid,) =
        sqlx::query_as("SELECT id FROM roles WHERE scope = 'platform' AND name = 'data_steward'")
            .fetch_one(&f.db.pool)
            .await
            .expect("data_steward seeded by migration 0003");
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/access/roles/{seeded_id}"),
            &admin,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "deleting a seeded role must 409; body={j}"
    );

    // The custom role, not seeded, can be deleted.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/access/roles/{role_id}"),
            &admin,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "delete the custom role"
    );
}

/// A scoped principal without `role:read` is 403 on `GET /access/roles`; with
/// it (or platform_admin), 200. `GET /permissions` returns the non-empty
/// code-defined catalog (`egret_core::rbac::PERMISSIONS`, independent of DB
/// content) for an authorized caller — it too is gated on `role:read`.
#[tokio::test]
async fn permission_enforcement() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };

    let wrong = session_for_with_permission(&f, "access_scoped_wrong", "user:read").await;
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/access/roles", &wrong, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a principal without role:read must not list roles"
    );

    let reader = session_for_with_permission(&f, "access_scoped_reader", "role:read").await;
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/access/roles", &reader, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a scoped role:read principal must list roles; body={j}"
    );

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/permissions", &reader, None))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list permissions; body={j}");
    let groups = j.as_array().expect("permission groups array");
    assert!(
        !groups.is_empty(),
        "permission catalog must not be empty; body={j}"
    );
    assert!(
        groups
            .iter()
            .any(|g| g["permissions"].as_array().is_some_and(|p| !p.is_empty())),
        "at least one permission group must list permissions; body={j}"
    );
}
