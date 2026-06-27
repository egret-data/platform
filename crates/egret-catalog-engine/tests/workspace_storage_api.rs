//! DB + MinIO gated end-to-end test for the per-workspace storage API (CE-5
//! final task).
//!
//! This is the only place both halves of the seam are constructible: the
//! `egret-workspace` HTTP router (POST create + GET storage) AND the
//! `EngineWarehouseProvisioner` (lakekeeper engine + egret store + secret vault)
//! that backs it. It mounts the REAL workspace router with a `Some(provisioner)`
//! `WorkspaceState` — the same wiring `egret-bin` assembles in prod — and drives
//! it over `tower::oneshot` (no socket).
//!
//! Coverage:
//!   1. Create a workspace WITH a custom storage spec (live MinIO `validate_access`
//!      via `provision`). Assert: the workspace+warehouse provisioned; the
//!      `workspace_storage` row carries `secret_ref = Some(..)` and NOT the
//!      plaintext; `secret_store.get(secret_ref)` returns the plaintext; then
//!      `GET /{slug}/storage` → 200 with `has_secret: true`, bucket/endpoint/region
//!      present, and NO secret field anywhere in the JSON body.
//!   2. Create a workspace WITHOUT storage → provisions against the global default,
//!      `workspace_storage_for` is `None`, and `GET /{slug}/storage` → 404.
//!
//! Needs MinIO reachable at `localhost:9000` with a writable `warehouse` bucket
//! (the egret dev stack ships it). Self-skips when `DATABASE_URL` is unset.
//!
//! Live MinIO I/O can transiently flake ("Unknown S3 error during write"); if you
//! hit it, retry once — do NOT weaken assertions.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-catalog-engine --test workspace_storage_api

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::password::hash_password;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_catalog::service::CatalogStore;
use egret_catalog_engine::{
    build_state, ensure_bootstrapped, run_lakekeeper_migrations, EngineWarehouseProvisioner,
};
use egret_core::UserId;
use egret_metastore::Db;
use egret_secrets::{Crypto, SecretStore};
use egret_storage::StorageConfig;
use egret_workspace::provision::WarehouseProvisioner;
use egret_workspace::rbac::Authorizer;
use egret_workspace::routes::{admin_workspace_router, ws_member_router, WorkspaceState};
use egret_workspace::workspaces::WorkspaceRepo;
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;

const SYSTEM_USER: &str = "egret-system";

/// Dev MinIO config matching the egret dev stack (bucket `warehouse`,
/// egret/egretsecret, S3-compat at localhost:9000). Used as the GLOBAL default
/// the no-storage workspace falls back to.
fn dev_storage_config() -> StorageConfig {
    StorageConfig {
        endpoint: Some("http://localhost:9000".to_string()),
        region: "local-01".to_string(),
        bucket: "warehouse".to_string(),
        access_key: "egret".to_string(),
        secret_key: "egretsecret".to_string(),
        path_style: true,
    }
}

async fn seed_user(db: &Db) -> UserId {
    UserRepo::new(db.clone())
        .insert_local(
            &format!("{}@egret.test", Uuid::new_v4()),
            "CE5 API",
            &hash_password("pw").unwrap(),
        )
        .await
        .unwrap()
}

async fn login_cookie(sessions: &SessionService, user: UserId) -> String {
    let issued = sessions.create(user, "a", "").await.unwrap();
    format!("egret_session={}", issued.token)
}

/// Assemble a prod-shaped `WorkspaceState` with a live `EngineWarehouseProvisioner`.
async fn ws_state_with_provisioner(db: &Db, dsn: &str) -> (WorkspaceState, SecretStore) {
    run_lakekeeper_migrations(dsn)
        .await
        .expect("lakekeeper migrations apply");
    let (engine_state, _registry) = build_state(dsn, SYSTEM_USER)
        .await
        .expect("build engine state");
    ensure_bootstrapped(engine_state.clone(), SYSTEM_USER)
        .await
        .expect("bootstrap default project");

    let store = CatalogStore::new(db.clone());
    // Real crypto so secrets actually seal/open (a None crypto fails `put`).
    let crypto = Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secret_store = SecretStore::new(db.clone(), Some(crypto));

    let provisioner: Arc<dyn WarehouseProvisioner> = Arc::new(EngineWarehouseProvisioner::new(
        engine_state,
        store,
        dev_storage_config(),
        SYSTEM_USER.to_string(),
        secret_store.clone(),
    ));

    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let st = WorkspaceState {
        workspaces: WorkspaceRepo::new(db.clone()),
        members: egret_workspace::members::MemberRepo::new(db.clone()),
        authz: Authorizer::new(db.clone()),
        users: UserRepo::new(db.clone()),
        sessions,
        warehouse_provisioner: Some(provisioner),
    };
    (st, secret_store)
}

fn app(st: WorkspaceState) -> axum::Router {
    axum::Router::new()
        .nest("/api/admin", admin_workspace_router())
        .nest("/api/ws", ws_member_router())
        .with_state(st)
}

#[tokio::test]
async fn create_with_custom_storage_then_get_storage_omits_secret() {
    let Ok(dsn) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL not set — skipping workspace storage API test");
        return;
    };
    let db = Db::connect(&dsn, 5).await.expect("connect egret db");
    db.migrate().await.expect("egret migrations apply");

    let (st, secret_store) = ws_state_with_provisioner(&db, &dsn).await;
    let store = CatalogStore::new(db.clone());

    // A platform admin (workspace:create) + their session cookie.
    let admin = seed_user(&db).await;
    st.authz
        .replace_platform_roles(admin, &["platform_admin".into()])
        .await
        .unwrap();
    let cookie = login_cookie(&st.sessions, admin).await;
    let app = app(st);

    // --- 1. Create WITH custom per-workspace storage (live MinIO provision). ---
    let slug = format!("ce5api{}", Uuid::new_v4().simple());
    const SECRET_KEY: &str = "egretsecret";
    let body = serde_json::json!({
        "slug": slug,
        "name": "CE5 Storage API",
        "storage": {
            "provider": "s3",
            "endpoint": "http://localhost:9000",
            "region": "us-east-1",
            "bucket": "warehouse",
            "access_key": "egret",
            "secret_key": SECRET_KEY,
            "path_style": true
        }
    });
    let resp = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/api/admin/workspaces")
                .header("cookie", &cookie)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "create-with-storage provisions the warehouse (needs MinIO + writable `warehouse` bucket)"
    );
    let created: serde_json::Value = {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    };
    let ws_id: Uuid = created["id"].as_str().unwrap().parse().unwrap();

    // Warehouse mapping exists (the workspace is usable).
    assert!(
        store.warehouse_for_slug(&slug).await.unwrap().is_some(),
        "workspace→warehouse mapping was provisioned"
    );

    // The persisted storage row carries a secret_ref, NOT the plaintext.
    let row = store
        .workspace_storage_for(ws_id)
        .await
        .expect("read workspace_storage")
        .expect("row exists for a workspace created WITH storage");
    let secret_ref = row.secret_ref.expect("secret_ref is set");
    assert_eq!(row.bucket, "warehouse");
    assert_eq!(row.endpoint.as_deref(), Some("http://localhost:9000"));
    assert_eq!(row.region, "us-east-1");
    assert_eq!(row.access_key, "egret");
    // The plaintext is fetchable ONLY through the vault, by ref.
    assert_eq!(
        secret_store.get(secret_ref).await.expect("fetch secret"),
        SECRET_KEY,
        "secret round-trips through the vault, fetched by secret_ref"
    );

    // --- GET /{slug}/storage → 200, has_secret, NO secret in the body. ---
    let resp = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri(format!("/api/ws/{slug}/storage"))
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "GET storage returns 200");
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let info: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(info["has_secret"], serde_json::json!(true));
    assert_eq!(info["bucket"], serde_json::json!("warehouse"));
    assert_eq!(info["endpoint"], serde_json::json!("http://localhost:9000"));
    assert_eq!(info["region"], serde_json::json!("us-east-1"));
    assert_eq!(info["access_key"], serde_json::json!("egret"));
    assert_eq!(info["provider"], serde_json::json!("s3"));

    // The secret value NEVER appears: no `secret_key`/`secret` field, and the
    // plaintext string is nowhere in the serialized body.
    assert!(
        info.get("secret_key").is_none(),
        "response must NOT carry a secret_key field"
    );
    assert!(
        info.get("secret").is_none(),
        "response must NOT carry a secret field"
    );
    let raw = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        !raw.contains(SECRET_KEY),
        "the plaintext secret must not appear anywhere in the GET storage body"
    );
}

#[tokio::test]
async fn create_without_storage_uses_default_and_get_storage_is_404() {
    let Ok(dsn) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL not set — skipping workspace storage API (no-storage) test");
        return;
    };
    let db = Db::connect(&dsn, 5).await.expect("connect egret db");
    db.migrate().await.expect("egret migrations apply");

    let (st, _secret_store) = ws_state_with_provisioner(&db, &dsn).await;
    let store = CatalogStore::new(db.clone());

    let admin = seed_user(&db).await;
    st.authz
        .replace_platform_roles(admin, &["platform_admin".into()])
        .await
        .unwrap();
    let cookie = login_cookie(&st.sessions, admin).await;
    let app = app(st);

    // Create WITHOUT a storage spec → provisions against the global default.
    let slug = format!("ce5def{}", Uuid::new_v4().simple());
    let body = serde_json::json!({ "slug": slug, "name": "CE5 Default Storage" });
    let resp = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/api/admin/workspaces")
                .header("cookie", &cookie)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "create-without-storage provisions against the global default (needs MinIO)"
    );
    let created: serde_json::Value = {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    };
    let ws_id: Uuid = created["id"].as_str().unwrap().parse().unwrap();

    // No per-workspace storage row was written.
    assert!(
        store
            .workspace_storage_for(ws_id)
            .await
            .expect("read workspace_storage")
            .is_none(),
        "a workspace created WITHOUT storage has no workspace_storage row (uses the default)"
    );

    // GET /{slug}/storage → 404 (no per-workspace storage to expose).
    let resp = app
        .oneshot(
            HttpRequest::builder()
                .uri(format!("/api/ws/{slug}/storage"))
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "GET storage is 404 when the workspace has no per-workspace storage"
    );
}

#[tokio::test]
async fn get_storage_is_admin_only_viewer_403_unauthenticated_401() {
    let Ok(dsn) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL not set — skipping workspace storage API (authz) test");
        return;
    };
    let db = Db::connect(&dsn, 5).await.expect("connect egret db");
    db.migrate().await.expect("egret migrations apply");

    let (st, _secret_store) = ws_state_with_provisioner(&db, &dsn).await;

    // A platform admin provisions a workspace WITH per-workspace storage (so a
    // row exists — proving the 403 is the AUTHZ gate, not a missing-row 404).
    let admin = seed_user(&db).await;
    st.authz
        .replace_platform_roles(admin, &["platform_admin".into()])
        .await
        .unwrap();
    let admin_cookie = login_cookie(&st.sessions, admin).await;

    // A workspace member with ONLY ws_viewer — lacks `workspace:manage_members`.
    let viewer = seed_user(&db).await;
    let viewer_cookie = login_cookie(&st.sessions, viewer).await;

    let members = st.members.clone();
    let app = app(st);

    let slug = format!("ce5authz{}", Uuid::new_v4().simple());
    let body = serde_json::json!({
        "slug": slug,
        "name": "CE5 Authz",
        "storage": {
            "endpoint": "http://localhost:9000",
            "region": "us-east-1",
            "bucket": "warehouse",
            "access_key": "egret",
            "secret_key": "egretsecret"
        }
    });
    let resp = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/api/admin/workspaces")
                .header("cookie", &admin_cookie)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "admin create-with-storage provisions (needs MinIO)"
    );
    let created: serde_json::Value = {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    };
    let ws_id: Uuid = created["id"].as_str().unwrap().parse().unwrap();

    // Make `viewer` a ws_viewer member of the workspace.
    members
        .add(
            egret_core::WorkspaceId(ws_id),
            "user",
            viewer.0,
            "ws_viewer",
            admin,
        )
        .await
        .expect("add ws_viewer member");

    // ws_viewer (no `workspace:manage_members`) → 403, even though a row exists.
    let resp = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri(format!("/api/ws/{slug}/storage"))
                .header("cookie", &viewer_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "ws_viewer is forbidden — GET storage is admin-only (workspace:manage_members)"
    );

    // Admin (bypasses via platform_admin) still gets 200 on the same row.
    let resp = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri(format!("/api/ws/{slug}/storage"))
                .header("cookie", &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "platform admin can read the storage config"
    );

    // Unauthenticated (no cookie) → 401.
    let resp = app
        .oneshot(
            HttpRequest::builder()
                .uri(format!("/api/ws/{slug}/storage"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "unauthenticated request is rejected before authz"
    );
}
