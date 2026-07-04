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

struct FakeEngineProvisioner;

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
}

#[tokio::test]
async fn create_and_get_engine() {
    let Some(f) = setup_full(None, Some(Arc::new(FakeEngineProvisioner))).await else {
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
async fn update_engine_per_role_trino_config_roundtrips() {
    let Some(f) = setup_full(None, Some(Arc::new(FakeEngineProvisioner))).await else {
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
    let Some(f) = setup_full(None, Some(Arc::new(FakeEngineProvisioner))).await else {
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
    let Some(f) = setup_full(None, Some(Arc::new(FakeEngineProvisioner))).await else {
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
async fn engine_catalog_routes() {
    let Some(f) = setup_full(None, Some(Arc::new(FakeEngineProvisioner))).await else {
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

#[tokio::test]
async fn create_engine_unprovisioned_warehouse_400() {
    let Some(f) = setup_full(None, Some(Arc::new(FakeEngineProvisioner))).await else {
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
