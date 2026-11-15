//! Service accounts as principals (SA1): the `/service-accounts` routes, their
//! membership guards, and the unchanged human `PATCH /users/{id}` behaviour.
//!
//! DB-gated; fails without `DATABASE_URL` unless `EGRET_ALLOW_DB_SKIP`. The router
//! is `control_plane_router()` UNNESTED, so routes are relative.

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::response::Response;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_controlplane::{control_plane_router, CpState};
use egret_metastore::Db;
use http_body_util::BodyExt;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

struct Fixture {
    app: axum::Router,
    users: UserRepo,
    sessions: SessionService,
    db: Db,
}

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
    let workspace_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets);
    let (audit, _audit_rx) = egret_audit::AuditSink::new(16);

    let app = control_plane_router().with_state(CpState {
        storage: egret_catalog_engine::CatalogStore::new(db.clone()),
        storage_registry: egret_storage::StorageRegistry::new(db.clone()),
        users: users.clone(),
        groups: egret_auth::groups::GroupRepo::new(db.clone()),
        roles: egret_auth::roles::RoleRepo::new(db.clone(), egret_events::EventBus::new()),
        access_resolver: egret_auth::effective::AccessResolver::new(db.clone()),
        secrets: workspace_secrets,
        sessions: sessions.clone(),
        browser: None,
        pdp: std::sync::Arc::new(AllowAllPdp),
        audit,
        audit_store: egret_audit::AuditStore::new(db.clone()),
        tags: egret_bi::TagRepo::new(db.clone()),
        classification_policies: egret_bi::ClassificationPolicyRepo::new(
            db.clone(),
            egret_events::EventBus::new(),
        ),
        db: db.clone(),
        mv_lister: std::sync::Arc::new(NoListing),
        mv_list_timeout: std::time::Duration::from_secs(15),
        lineage_sweeper: std::sync::Arc::new(egret_catalog_engine::NoCatalogSweeper),
        view_catalog: std::sync::Arc::new(egret_catalog_engine::NoViewCatalog),
        view_audit_timeout: std::time::Duration::from_secs(15),
        catalog_lister: std::sync::Arc::new(egret_controlplane::UnwiredCatalogLister),
        browse_timeout: std::time::Duration::from_secs(15),
        audit_retention_days: Some(90),
        storage_scope: egret_controlplane::StorageScope::s3(
            egret_controlplane::SystemPrefixes::default(),
        ),
    });

    Some(Fixture {
        app,
        users,
        sessions,
        db,
    })
}

/// Create a user (optionally a platform_admin) and return its session token.
/// Mirrors `routes_api.rs::session_for`.
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

/// These tests never reach the materialized-view report; a lister that is asked
/// anyway fails loudly instead of reporting an empty engine.
struct NoListing;

#[async_trait::async_trait]
impl egret_controlplane::MaterializedViewLister for NoListing {
    async fn list(
        &self,
        _engine: &egret_controlplane::ExposureEngine,
    ) -> Result<Vec<egret_controlplane::MvRow>, String> {
        Err("no StarRocks in this test".to_string())
    }
}

async fn call(
    f: &Fixture,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    into_json(
        f.app
            .clone()
            .oneshot(req(method, uri, token, body))
            .await
            .unwrap(),
    )
    .await
}

async fn workspace(f: &Fixture) -> Uuid {
    sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1, 'SA') RETURNING id")
        .bind(format!("sa-{}", Uuid::new_v4().simple()))
        .fetch_one(&f.db.pool)
        .await
        .expect("insert workspace")
}

async fn create_platform_sa(f: &Fixture, token: &str) -> Uuid {
    let (s, j) = call(
        f,
        "POST",
        "/service-accounts",
        token,
        Some(serde_json::json!({
            "scope": "platform",
            "name": format!("bot {}", Uuid::new_v4().simple()),
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{j}");
    j["user_id"].as_str().unwrap().parse().unwrap()
}

async fn role_id(f: &Fixture, name: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM roles WHERE scope='platform' AND name=$1")
        .bind(name)
        .fetch_one(&f.db.pool)
        .await
        .expect("role")
}

#[tokio::test]
async fn create_list_get_and_duplicate() {
    let Some(f) = setup().await else {
        return;
    };
    let t = session_for(&f, "sa_admin", true).await;
    let ws = workspace(&f).await;
    let body = serde_json::json!({
        "scope": "workspace", "workspace_id": ws, "name": "etl bot", "description": "d"
    });
    let (s, j) = call(&f, "POST", "/service-accounts", &t, Some(body.clone())).await;
    assert_eq!(s, StatusCode::OK, "{j}");
    assert_eq!(j["scope"], "workspace");
    assert_eq!(j["name"], "etl bot");
    assert_eq!(j["workspace_id"], ws.to_string());
    let id = j["user_id"].as_str().unwrap().to_string();
    let (s, _) = call(&f, "POST", "/service-accounts", &t, Some(body)).await;
    assert_eq!(s, StatusCode::CONFLICT, "duplicate create");

    let pid = create_platform_sa(&f, &t).await;
    let (_, j) = call(&f, "GET", "/service-accounts?scope=platform", &t, None).await;
    let arr = j.as_array().unwrap();
    assert!(arr.iter().all(|a| a["scope"] == "platform"));
    assert!(arr.iter().any(|a| a["user_id"] == pid.to_string()));
    assert!(arr.iter().all(|a| a["user_id"] != id));
    assert!(arr.iter().all(|a| a["workspace_id"].is_null()));

    let (_, j) = call(
        &f,
        "GET",
        &format!("/service-accounts?workspace_id={ws}"),
        &t,
        None,
    )
    .await;
    let arr = j.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["user_id"], id);

    let (s, j) = call(&f, "GET", &format!("/service-accounts/{id}"), &t, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(j["groups"], serde_json::json!([]));
    assert_eq!(j["roles"], serde_json::json!([]));
}

#[tokio::test]
async fn group_membership_and_oidc_refusal() {
    let Some(f) = setup().await else {
        return;
    };
    let t = session_for(&f, "sa_admin", true).await;
    let id = create_platform_sa(&f, &t).await;
    let local: Uuid =
        sqlx::query_scalar("INSERT INTO groups (name, source) VALUES ($1, 'local') RETURNING id")
            .bind(format!("loc-{}", Uuid::new_v4().simple()))
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    let oidc: Uuid =
        sqlx::query_scalar("INSERT INTO groups (name, source) VALUES ($1, 'oidc') RETURNING id")
            .bind(format!("idp-{}", Uuid::new_v4().simple()))
            .fetch_one(&f.db.pool)
            .await
            .unwrap();

    let (s, j) = call(
        &f,
        "POST",
        &format!("/service-accounts/{id}/groups"),
        &t,
        Some(serde_json::json!({ "group_id": local })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{j}");
    assert_eq!(j["groups"].as_array().unwrap().len(), 1);

    let (s, j) = call(
        &f,
        "POST",
        &format!("/service-accounts/{id}/groups"),
        &t,
        Some(serde_json::json!({ "group_id": oidc })),
    )
    .await;
    assert!(s.is_client_error(), "oidc group refused: {s} {j}");
    assert!(
        j["error"]
            .as_str()
            .unwrap_or_default()
            .contains("identity provider"),
        "{j}"
    );

    let (s, j) = call(
        &f,
        "DELETE",
        &format!("/service-accounts/{id}/groups/{local}"),
        &t,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{j}");
    assert_eq!(j["groups"], serde_json::json!([]));
}

#[tokio::test]
async fn role_membership_and_platform_admin_refusal() {
    let Some(f) = setup().await else {
        return;
    };
    let t = session_for(&f, "sa_admin", true).await;
    let id = create_platform_sa(&f, &t).await;
    let admin = role_id(&f, "platform_admin").await;
    let (s, j) = call(
        &f,
        "POST",
        &format!("/service-accounts/{id}/roles"),
        &t,
        Some(serde_json::json!({ "role_id": admin, "scope_type": "platform" })),
    )
    .await;
    assert!(s.is_client_error(), "platform_admin refused: {s} {j}");

    let other: Uuid = sqlx::query_scalar(
        "SELECT id FROM roles WHERE scope='platform' AND name <> 'platform_admin' ORDER BY name LIMIT 1",
    )
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    let (s, j) = call(
        &f,
        "POST",
        &format!("/service-accounts/{id}/roles"),
        &t,
        Some(serde_json::json!({ "role_id": other, "scope_type": "platform" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{j}");
    assert_eq!(j["roles"].as_array().unwrap().len(), 1);

    let (s, j) = call(
        &f,
        "DELETE",
        &format!("/service-accounts/{id}/roles/{other}"),
        &t,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{j}");
    assert_eq!(j["roles"], serde_json::json!([]));
}

#[tokio::test]
async fn disable_enable_delete_and_human_route_unchanged() {
    let Some(f) = setup().await else {
        return;
    };
    let t = session_for(&f, "sa_admin", true).await;
    let id = create_platform_sa(&f, &t).await;
    let status = || async {
        sqlx::query_scalar::<_, String>("SELECT status FROM users WHERE id = $1")
            .bind(id)
            .fetch_one(&f.db.pool)
            .await
            .unwrap()
    };

    // `ClientStore::verify` is not reachable from this crate's tests, so the
    // kill switch is asserted on `users.status` (the column verify reads).
    let (s, j) = call(
        &f,
        "POST",
        &format!("/service-accounts/{id}/disable"),
        &t,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{j}");
    assert_eq!(j["status"], "disabled");
    assert_ne!(status().await, "active");
    let (s, _) = call(
        &f,
        "POST",
        &format!("/service-accounts/{id}/enable"),
        &t,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(status().await, "active");

    // Human route unchanged: profile edit refused, status still accepted.
    let (s, _) = call(
        &f,
        "PATCH",
        &format!("/users/{id}"),
        &t,
        Some(serde_json::json!({ "name": "Person" })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = call(
        &f,
        "PATCH",
        &format!("/users/{id}"),
        &t,
        Some(serde_json::json!({ "status": "disabled" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let (s, _) = call(&f, "DELETE", &format!("/service-accounts/{id}"), &t, None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = call(&f, "GET", &format!("/service-accounts/{id}"), &t, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = call(&f, "DELETE", &format!("/service-accounts/{id}"), &t, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "delete of a missing SA");
    // A human user id is not a service account.
    let (s, _) = call(
        &f,
        "DELETE",
        &format!("/service-accounts/{}", Uuid::new_v4()),
        &t,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn adding_to_a_group_holding_platform_admin_is_refused() {
    let Some(f) = setup().await else {
        return;
    };
    let t = session_for(&f, "sa_admin", true).await;
    let id = create_platform_sa(&f, &t).await;
    let admin = role_id(&f, "platform_admin").await;
    let g: Uuid =
        sqlx::query_scalar("INSERT INTO groups (name, source) VALUES ($1, 'local') RETURNING id")
            .bind(format!("adm-{}", Uuid::new_v4().simple()))
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('group', $1, $2, 'platform', NULL)",
    )
    .bind(g)
    .bind(admin)
    .execute(&f.db.pool)
    .await
    .unwrap();
    let (s, j) = call(
        &f,
        "POST",
        &format!("/service-accounts/{id}/groups"),
        &t,
        Some(serde_json::json!({ "group_id": g })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{j}");
    assert!(j["error"]
        .as_str()
        .unwrap_or_default()
        .contains("platform_admin"));
}
