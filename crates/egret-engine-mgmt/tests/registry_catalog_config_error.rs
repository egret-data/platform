//! `configError` on `GET /api/control-plane/catalogs` (SP3c-2 task 3). A
//! stored row the tightened validator now refuses must stay visible and
//! carry the reason, and — critically — must not fail the WHOLE list with a
//! 400 for every other catalog on the platform (the defect SP3c-1's review N1
//! fixed on the legacy `/engines/{id}/catalogs` route; this is the same
//! defect class on the registry list). DB-gated; self-skips without
//! `DATABASE_URL` ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-engine-mgmt registry_catalog_config_error

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::response::Response;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_catalog_registry::CatalogRow;
use egret_engine_mgmt::{engine_mgmt_router, EngineMgmtState, FakeStarRocksDdl};
use egret_metastore::Db;
use http_body_util::BodyExt;
use uuid::Uuid;

struct Fixture {
    app: axum::Router,
    users: UserRepo,
    sessions: SessionService,
    db: Db,
}

/// Every existing route test authenticates a `platform_admin` and asserts
/// non-authz behavior, so an allow-all PDP stub keeps this fixture light —
/// this test asserts nothing about permissions.
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

async fn setup() -> Option<Fixture> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let users = UserRepo::new(db.clone());
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );

    let crypto =
        egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secrets = egret_secrets::SecretStore::new(db.clone(), Some(crypto));
    let workspace_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets.clone());
    let databases = egret_query::DatabaseRepo::new(db.clone(), secrets, workspace_secrets.clone());

    let app = engine_mgmt_router()
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
            pdp: std::sync::Arc::new(AllowAllPdp),
            engine_provisioner: None,
            kube: None,
            coordinator_transport: None,
            engine_egret_url: "http://egret:8080".into(),
            engine_jwks_uri: "https://idp/jwks".into(),
            engine_oidc_issuer: "https://idp/realms/egret".into(),
            ranger_enabled: false,
            ranger_surface_mode: egret_engine_mgmt::state::RangerSurfaceMode::Required,
            sr_jdbc_drivers: egret_catalog_registry::JdbcDrivers::default(),
            starrocks_ddl: std::sync::Arc::new(FakeStarRocksDdl::default()),
            perm_compiler: None,
            catalog_assignment_compiler: None,
        });

    Some(Fixture {
        app,
        users,
        sessions,
        db,
    })
}

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

async fn into_json(resp: Response) -> (StatusCode, serde_json::Value) {
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(
        |_| serde_json::json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }),
    );
    (status, json)
}

async fn call(
    f: &Fixture,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    use tower::ServiceExt; // oneshot
    into_json(
        f.app
            .clone()
            .oneshot(req(method, uri, token, body))
            .await
            .unwrap(),
    )
    .await
}

fn unique_catalog_name(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..10])
}

/// Write a `jdbc` catalog row straight into `registry.catalogs`, bypassing
/// `CatalogDef::parse` — exactly what a row that predates a tightened
/// validator looks like; the API cannot create one, since
/// `create_registry_catalog` parses and refuses it. The stored `config` is an
/// empty object: `JdbcConfig` requires `dialect`/`host`/`port`/`user`/
/// `password_secret_id`, all absent.
async fn seed_unparseable_jdbc_catalog(f: &Fixture, name: &str) -> Uuid {
    let (id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO registry.catalogs (name, kind, config) VALUES ($1, 'jdbc', '{}'::jsonb) \
         RETURNING id",
    )
    .bind(name)
    .fetch_one(&f.db.pool)
    .await
    .expect("insert unparseable jdbc catalog");
    id
}

/// A catalog row whose stored config no longer parses must not take the list
/// with it: `GET /catalogs` returns 200, lists BOTH catalogs, and carries
/// `configError` on the broken one only.
///
/// The broken row is written directly with sqlx, bypassing the validator —
/// the API cannot create one, which is exactly why this case is reachable
/// only from a stored row that predates a tightened validator.
#[tokio::test]
async fn one_unparseable_registry_catalog_does_not_take_the_whole_list_with_it() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "reg_cat_cfgerr", true).await;

    let broken_name = unique_catalog_name("jdbc_broken");
    let broken_id = seed_unparseable_jdbc_catalog(&f, &broken_name).await;

    // Arrange-phase check: confirm the fixture actually fails to parse, or
    // this whole test would pass for the wrong reason (a config that quietly
    // remains valid would still return 200 with no `configError` and the
    // "still lists both" assertion would hold trivially).
    let broken_row: CatalogRow = sqlx::query_as(
        "SELECT id, name, kind, config, advanced, description, warehouse_id, \
         storage_connection_id, created_at, updated_at FROM registry.catalogs WHERE id = $1",
    )
    .bind(broken_id)
    .fetch_one(&f.db.pool)
    .await
    .expect("fetch the row back");
    assert!(
        broken_row.def().is_err(),
        "fixture must not parse, or this test proves nothing"
    );

    let valid_name = unique_catalog_name("tpch_ok");
    let (status, j) = call(
        &f,
        "POST",
        "/catalogs",
        &token,
        Some(serde_json::json!({ "name": valid_name, "kind": "tpch", "config": {} })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "seed the valid catalog; {j}");
    let valid_id = j["id"].as_str().unwrap().to_string();

    let (status, j) = call(&f, "GET", "/catalogs", &token, None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "one bad row must not 400 the whole registry list; {j}"
    );
    let rows = j.as_array().expect("a list");
    assert_eq!(
        rows.len(),
        2,
        "both the valid and the broken catalog are listed; {j}"
    );

    let broken = rows
        .iter()
        .find(|r| r["id"] == serde_json::json!(broken_id.to_string()))
        .expect("the broken catalog is listed, not dropped");
    assert_eq!(
        broken["name"],
        serde_json::json!(broken_name),
        "the degraded entry still identifies the row to repair; {j}"
    );
    assert!(
        broken["configError"]
            .as_str()
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        "the broken entry carries a non-empty configError; {j}"
    );

    let valid = rows
        .iter()
        .find(|r| r["id"] == serde_json::json!(valid_id))
        .expect("the valid catalog is listed");
    assert!(
        !valid.as_object().unwrap().contains_key("configError"),
        "a healthy row carries no configError key at all (skip_serializing_if); {j}"
    );

    // The same degrade-not-fail behavior on the single-catalog route,
    // `GET /catalogs/{id}`: it must return the row with its `configError`
    // rather than 400, exactly like the list. Same fixture, same process — a
    // second seeded catalog here would contaminate the list assertion above
    // (this test binary's DB is one-per-process, shared by every #[tokio::test]
    // in it), so this reuses `broken_id` rather than seeding a third row.
    let (status, j) = call(&f, "GET", &format!("/catalogs/{broken_id}"), &token, None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a single unparseable catalog degrades rather than 400s; {j}"
    );
    assert!(
        j["configError"]
            .as_str()
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        "the response carries a non-empty configError; {j}"
    );
}
