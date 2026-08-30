//! Catalog seam admin router HTTP tests — moved with the warehouse admin router out of
//! egret-controlplane in E3d-3. DB-gated; self-skips without `DATABASE_URL` ONLY under
//! `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS. Auth is a session cookie (the CSRF layer is
//! applied above this router in egret-server, not here).
//!
//! Proves `egret_catalog_engine::catalog_admin_router`:
//!   - warehouse browse (namespaces/tables/schema) over a `FakeBrowser`,
//!   - a non-admin session is Forbidden (403),
//!   - an unprovisioned warehouse browses empty,
//!   - the fine-grained `warehouse:read` / `warehouse:create` gates.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-catalog-engine

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::response::Response;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_catalog_engine::{catalog_admin_router, CatalogAdminState};
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
    browser: Option<Arc<dyn egret_catalog_engine::WarehouseBrowser>>,
}

/// Incidental PDP for `CatalogAdminState`: the browse tests authenticate a
/// `platform_admin` (whose bypass predates the PDP) and assert non-authz
/// behavior, so an allow-all stub keeps those fixtures light. Any test that
/// asserts a 403 (`browse_non_admin_forbidden`,
/// `scoped_permission_warehouses_read_and_create`) must instead build a real
/// `CedarPdp` via `attach_real_pdp` — an allow-all stub would falsely 200.
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

async fn into_json(resp: Response) -> (StatusCode, serde_json::Value) {
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(
        |_| serde_json::json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }),
    );
    (status, json)
}

/// Build the catalog-admin `Router` for a given db + browser + PDP. Shared by
/// `setup_full` (allow-all PDP) and `attach_real_pdp` (real Cedar PDP) so the two
/// only differ in the injected decision point.
fn build_app(
    db: &Db,
    sessions: &SessionService,
    browser: Option<Arc<dyn egret_catalog_engine::WarehouseBrowser>>,
    pdp: std::sync::Arc<dyn egret_core::authz::Pdp>,
) -> axum::Router {
    let crypto =
        egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secrets = egret_secrets::SecretStore::new(db.clone(), Some(crypto));
    let workspace_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets);

    catalog_admin_router().with_state(CatalogAdminState {
        store: egret_catalog_engine::CatalogStore::new(db.clone()),
        storage_registry: egret_storage::StorageRegistry::new(db.clone()),
        secrets: workspace_secrets,
        provisioner: None,
        browser,
        sessions: sessions.clone(),
        pdp,
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
    f.app = build_app(&f.db, &f.sessions, f.browser.clone(), pdp);
}

async fn setup_full(
    browser: Option<Arc<dyn egret_catalog_engine::WarehouseBrowser>>,
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

    // Browse tests authenticate a platform_admin and assert non-authz behavior,
    // so an allow-all PDP suffices. Authz tests rebuild with a real Cedar PDP via
    // `attach_real_pdp`.
    let app = build_app(
        &db,
        &sessions,
        browser.clone(),
        std::sync::Arc::new(AllowAllPdp),
    );

    Some(Fixture {
        app,
        users,
        sessions,
        db,
        browser,
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
/// `session_for`'s `platform_admin` bypass path).
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

// ── Warehouse browse ──────────────────────────────────────────────────────────

struct FakeBrowser;

#[async_trait::async_trait]
impl egret_catalog_engine::WarehouseBrowser for FakeBrowser {
    async fn namespaces(
        &self,
        _wh: &str,
    ) -> Result<Vec<egret_catalog_engine::BrowseNamespace>, egret_catalog_engine::CatalogEngineError>
    {
        Ok(vec![egret_catalog_engine::BrowseNamespace {
            parts: vec!["analytics".into()],
            id: None,
            protected: false,
        }])
    }
    async fn tabulars(
        &self,
        _wh: &str,
        _ns: &[String],
    ) -> Result<Vec<egret_catalog_engine::BrowseTabular>, egret_catalog_engine::CatalogEngineError>
    {
        Ok(vec![egret_catalog_engine::BrowseTabular {
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
    ) -> Result<egret_catalog_engine::BrowseTableDetail, egret_catalog_engine::CatalogEngineError>
    {
        Ok(egret_catalog_engine::BrowseTableDetail {
            location: "s3://bucket/t".into(),
            format_version: 2,
            last_updated_ms: 0,
            current_snapshot_id: None,
            snapshot_count: 0,
            columns: vec![egret_catalog_engine::BrowseColumn {
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
    ) -> Result<Vec<egret_catalog_engine::BrowseColumn>, egret_catalog_engine::CatalogEngineError>
    {
        Ok(vec![egret_catalog_engine::BrowseColumn {
            name: "id".into(),
            type_: "long".into(),
            required: true,
        }])
    }
    async fn deleted_tabulars(
        &self,
        _wh: &str,
    ) -> Result<
        Vec<egret_catalog_engine::BrowseDeletedTabular>,
        egret_catalog_engine::CatalogEngineError,
    > {
        Ok(vec![])
    }
    async fn undrop(
        &self,
        _wh: &str,
        _kind: &str,
        _id: uuid::Uuid,
    ) -> Result<(), egret_catalog_engine::CatalogEngineError> {
        Ok(())
    }
    async fn set_protection(
        &self,
        _wh: &str,
        _target: &str,
        _id: uuid::Uuid,
        protected: bool,
    ) -> Result<bool, egret_catalog_engine::CatalogEngineError> {
        Ok(protected)
    }
    async fn config(
        &self,
        _wh: &str,
    ) -> Result<egret_catalog_engine::BrowseWarehouseConfig, egret_catalog_engine::CatalogEngineError>
    {
        Ok(egret_catalog_engine::BrowseWarehouseConfig {
            active: true,
            delete_profile_seconds: Some(604800),
            num_tables: 3,
            num_views: 1,
            stats_updated_ms: 0,
        })
    }
    async fn set_active(
        &self,
        _wh: &str,
        _active: bool,
    ) -> Result<(), egret_catalog_engine::CatalogEngineError> {
        Ok(())
    }
    async fn set_delete_profile(
        &self,
        _wh: &str,
        _seconds: Option<i64>,
    ) -> Result<(), egret_catalog_engine::CatalogEngineError> {
        Ok(())
    }
    async fn set_format_versions(
        &self,
        _wh: &str,
        _versions: Vec<u8>,
    ) -> Result<(), egret_catalog_engine::CatalogEngineError> {
        Ok(())
    }
    async fn rotate_credential(
        &self,
        _wh: &str,
        _access_key: &str,
        _secret_key: &str,
    ) -> Result<(), egret_catalog_engine::CatalogEngineError> {
        Ok(())
    }
    async fn search(
        &self,
        _wh: &str,
        _query: &str,
    ) -> Result<
        Vec<egret_catalog_engine::BrowseSearchResult>,
        egret_catalog_engine::CatalogEngineError,
    > {
        Ok(vec![])
    }
    async fn tasks(
        &self,
        _wh: &str,
    ) -> Result<Vec<egret_catalog_engine::BrowseTask>, egret_catalog_engine::CatalogEngineError>
    {
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
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(format!("browse-sc-{}", Uuid::new_v4().simple()))
    .bind("us-east-1")
    .bind("egret-browse-test")
    .bind("AKIATEST")
    .fetch_one(&f.db.pool)
    .await
    .expect("insert storage connection");

    let catalog_store = egret_catalog_engine::CatalogStore::new(f.db.clone());
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
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(format!("browse-sc-unprov-{}", Uuid::new_v4().simple()))
    .bind("us-east-1")
    .bind("egret-browse-test")
    .bind("AKIATEST")
    .fetch_one(&f.db.pool)
    .await
    .expect("insert storage connection");

    let catalog_store = egret_catalog_engine::CatalogStore::new(f.db.clone());
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
    let Some(mut f) = setup_full(None).await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "browse_nonadmin", false).await;
    // A real Cedar PDP (not the allow-all stub) must back this 403 — the principal
    // holds no platform role/permission, so the PDP denies `warehouse:read`.
    attach_real_pdp(&mut f).await;
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

/// Warehouses: `warehouse:read` gates `GET /warehouses`, `warehouse:create`
/// gates `POST /warehouses`. Proves the NEW `resource:action` gate: a scoped,
/// non-admin principal holding the wrong (or no) permission is Forbidden, and one
/// holding exactly the right permission succeeds — without the superuser bypass.
#[tokio::test]
async fn scoped_permission_warehouses_read_and_create() {
    let Some(mut f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };

    // Seed all three ad-hoc platform roles BEFORE the real Cedar PDP snapshots the
    // roles catalog (raw-SQL seeding emits no RolesChanged event, so the PDP never
    // reloads). This is the required real-PDP authz assertion: a non-platform_admin
    // `warehouse:read` principal lists warehouses (200) but is denied create (403),
    // resolved by a real `CedarPdp` — not the allow-all stub.
    let wrong = session_for_with_permission(&f, "wh_scoped_wrong", "engine:read").await;
    let reader = session_for_with_permission(&f, "wh_scoped_reader", "warehouse:read").await;
    let creator = session_for_with_permission(&f, "wh_scoped_creator", "warehouse:create").await;
    attach_real_pdp(&mut f).await;

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
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
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
