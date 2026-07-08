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

async fn setup_full(
    browser: Option<Arc<dyn egret_controlplane::WarehouseBrowser>>,
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
        authz: egret_policy::PolicyStore::new(db.clone()),
        users: users.clone(),
        groups: egret_auth::groups::GroupRepo::new(db.clone()),
        roles: egret_auth::roles::RoleRepo::new(db.clone()),
        secrets: workspace_secrets,
        sessions: sessions.clone(),
        provisioner: None,
        browser,
        // Cross-seam, read-only (owned by egret-engine-mgmt): used by the
        // Policies module's resource-lookup/profile listing + trino-config.
        engine_mgmt: egret_engine_mgmt::EngineMgmtStore::new(db.clone()),
        engine_egret_url: "http://egret:8080".into(),
    });

    Some(Fixture {
        app,
        users,
        sessions,
        db,
    })
}

async fn setup() -> Option<Fixture> {
    setup_full(None).await
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
    let Some(f) = setup_full(Some(Arc::new(FakeBrowser))).await else {
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
    let Some(f) = setup_full(None).await else {
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

#[tokio::test]
async fn browse_unprovisioned_warehouse_empty() {
    let Some(f) = setup_full(Some(Arc::new(FakeBrowser))).await else {
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

    let authz = egret_policy::PolicyStore::new(f.db.clone());
    egret_policy::seed_trino(&authz)
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

    let engine_mgmt = egret_engine_mgmt::EngineMgmtStore::new(f.db.clone());
    let authz = egret_policy::PolicyStore::new(f.db.clone());

    // Mirrors the `create_engine` auto-bind "create" branch: seed the Trino
    // service-def (required FK for `create_service`), then create a fresh
    // profile named after the engine.
    egret_policy::seed_trino(&authz)
        .await
        .expect("seed trino def");

    let engine_name = format!("trino-{}", Uuid::new_v4().simple());
    let engine = engine_mgmt
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

    engine_mgmt
        .set_engine_authz_service(engine.id, Some(svc.id))
        .await
        .expect("bind engine to profile");

    let engines = engine_mgmt.list_engines().await.expect("list engines");
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

    engine_mgmt
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

    let authz = egret_policy::PolicyStore::new(f.db.clone());
    egret_policy::seed_trino(&authz)
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

    let authz = egret_policy::PolicyStore::new(f.db.clone());
    egret_policy::seed_trino(&authz)
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

    let authz = egret_policy::PolicyStore::new(f.db.clone());
    egret_policy::seed_trino(&authz)
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

    let authz = egret_policy::PolicyStore::new(f.db.clone());
    egret_policy::seed_trino(&authz)
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

    let authz = egret_policy::PolicyStore::new(f.db.clone());
    egret_policy::seed_trino(&authz)
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

    let authz = egret_policy::PolicyStore::new(f.db.clone());
    egret_policy::seed_trino(&authz)
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
        authz: egret_policy::PolicyStore::new(db.clone()),
        users: users.clone(),
        groups: egret_auth::groups::GroupRepo::new(db.clone()),
        roles: egret_auth::roles::RoleRepo::new(db.clone()),
        secrets: workspace_secrets,
        sessions: sessions.clone(),
        provisioner: None,
        browser: None,
        engine_mgmt: egret_engine_mgmt::EngineMgmtStore::new(db.clone()),
        engine_egret_url: "http://egret:8080".into(),
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

    // GET /groups/{id}/members lists the member back (prefill for the
    // manage-members dialog).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/groups/{group_id}/members"),
            &admin,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list group members; body={j}");
    let members = j.as_array().expect("members array");
    assert_eq!(members.len(), 1, "one member; body={j}");
    assert_eq!(
        members[0]["id"],
        member_id.to_string(),
        "member id; body={j}"
    );
    assert_eq!(members[0]["email"], member_email, "member email; body={j}");

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
///
/// Writes to the authorization graph are a different story: they're
/// `platform_admin`-only now (not delegatable), so even a principal holding
/// `role:read` — enough to *see* roles — must still be 403 on
/// `POST /access/roles`, `PUT .../permissions`, and `POST /users/{id}/roles`.
/// The guard is the handler's first line, so these 403 before any DB lookup —
/// the path/body UUIDs below don't need to resolve to anything real.
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

    // A role:read holder can see roles but must not be able to create one —
    // role definitions are part of the authorization graph, platform_admin
    // only.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/access/roles",
            &reader,
            Some(serde_json::json!({
                "scope": "platform",
                "name": format!("escalation_attempt_{}", Uuid::new_v4().simple()),
                "description": "should never be created",
            })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a role:read principal must not create roles (platform_admin only)"
    );

    // Nor set a role's permissions — this is exactly how the closed
    // escalation worked (a delegated `role:assign_permission` let a
    // non-admin grant themselves anything).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/access/roles/{}/permissions", Uuid::new_v4()),
            &reader,
            Some(serde_json::json!({ "permissions": ["engine:read"] })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a role:read principal must not set role permissions (platform_admin only)"
    );

    // Nor assign a role to a user — the other half of the closed escalation
    // (a delegated `user:assign_role` let a non-admin hand themselves any
    // role, including platform_admin).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/users/{}/roles", Uuid::new_v4()),
            &reader,
            Some(serde_json::json!({ "roleId": Uuid::new_v4() })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a role:read principal must not assign roles to users (platform_admin only)"
    );
}

/// `GET /access/roles/{id}/assignees` lists a role's assignees by display
/// name (users by email, groups by name), for the admin console's
/// drill-into-a-role view. A scoped principal without `role:read` is 403.
#[tokio::test]
async fn role_assignees_lists_users_and_groups() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let admin = session_for(&f, "role_assignees_admin", true).await;
    let roles = egret_auth::roles::RoleRepo::new(f.db.clone());
    let groups = egret_auth::groups::GroupRepo::new(f.db.clone());

    // A fresh custom role, with no assignees yet.
    let role_name = format!("assignee_role_{}", Uuid::new_v4().simple());
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
                "description": "role assignees test",
            })),
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create custom role; body={j}");
    let role_id: Uuid = j["id"].as_str().expect("role id in body").parse().unwrap();

    // Assign the role to a user and a group directly (bypassing the
    // assign-role HTTP routes, which are exercised elsewhere).
    let member_email = format!("assignee_user_{}@egret.local", Uuid::new_v4().simple());
    let member_id = f
        .users
        .insert_local(&member_email, "Assignee User", "x")
        .await
        .expect("insert user")
        .0;
    roles
        .assign("user", member_id, role_id)
        .await
        .expect("assign role to user");

    let group_name = format!("assignee-group-{}", Uuid::new_v4().simple());
    let group_id = groups
        .create_local(&group_name)
        .await
        .expect("create group");
    roles
        .assign("group", group_id, role_id)
        .await
        .expect("assign role to group");

    // A scoped principal without `role:read` is 403.
    let wrong = session_for_with_permission(&f, "role_assignees_wrong", "user:read").await;
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/access/roles/{role_id}/assignees"),
            &wrong,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a principal without role:read must not list role assignees"
    );

    // The platform_admin sees both assignees, named.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/access/roles/{role_id}/assignees"),
            &admin,
            None,
        ))
        .await
        .unwrap();
    let (status, j) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list role assignees; body={j}");
    let assignees = j.as_array().expect("assignees array");
    assert_eq!(assignees.len(), 2, "two assignees; body={j}");
    assert!(
        assignees
            .iter()
            .any(|a| a["kind"] == "user" && a["name"] == member_email),
        "user assignee named by email; body={j}"
    );
    assert!(
        assignees
            .iter()
            .any(|a| a["kind"] == "group" && a["name"] == group_name),
        "group assignee named by group name; body={j}"
    );
}
