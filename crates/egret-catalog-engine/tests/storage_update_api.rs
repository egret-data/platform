//! Session-authenticated catalog STORAGE-UPDATE API test (Workspace Admin
//! track): `PUT /api/ws/{workspace}/catalog/storage` RE-points a workspace's
//! lakekeeper warehouse storage. DB + MinIO gated; self-skips without
//! `DATABASE_URL`.
//!
//! This proves the storage-update route on the browse router
//! (`egret_catalog_engine::browse_router`):
//!   - a workspace-admin (`ws_admin`, holds `workspace:manage_members`) can
//!     re-point the warehouse storage with a valid MinIO config → 200, the
//!     response is the secret-free `WorkspaceStorageInfo`, the `workspace_storage`
//!     row is updated, and the NEW secret is fetchable from the vault by its ref,
//!   - omitting `secret_key` on a follow-up PUT keeps the prior secret,
//!   - a BOGUS credential is rejected by lakekeeper FIRST (validate_access on
//!     update) → 400 and the `workspace_storage` row is left UNCHANGED,
//!   - a non-admin (`ws_viewer`) → 403,
//!   - a workspace with NO warehouse → 409.
//!
//! lakekeeper requires the re-pointed profile to keep the SAME location (bucket +
//! key prefix), so the test re-points within the same `warehouse` bucket and the
//! handler reuses the `workspaces/<slug>/warehouse` prefix derivation.
//!
//! Needs MinIO reachable at `localhost:9000` with a writable `warehouse` bucket
//! (the egret dev stack ships it). Self-skips when `DATABASE_URL` is unset.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-catalog-engine --test storage_update_api

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::response::Response;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_catalog::service::CatalogStore;
use egret_catalog_engine::{
    browse_router, build_state, ensure_bootstrapped, provision_workspace_warehouse,
    run_lakekeeper_migrations, BrowseState,
};
use egret_core::ids::{UserId, WorkspaceId};
use egret_metastore::Db;
use egret_secrets::{Crypto, SecretStore};
use egret_storage::StorageConfig;
use egret_workspace::members::MemberRepo;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use http_body_util::BodyExt;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

const SYSTEM_USER: &str = "egret-system";

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

struct Fixture {
    app: axum::Router,
    store: CatalogStore,
    secret_store: SecretStore,
    sessions: SessionService,
    users: UserRepo,
    members: MemberRepo,
    db: Db,
    slug: String,
    workspace_id: WorkspaceId,
    owner: UserId,
}

async fn into_json(resp: Response) -> (StatusCode, serde_json::Value) {
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(
        |_| serde_json::json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }),
    );
    (status, json)
}

/// Build the fixture with a workspace that ALREADY has a provisioned warehouse
/// (live MinIO validate_access) plus a `workspace_storage` row carrying a sealed
/// secret_ref — the realistic starting state for a re-point.
async fn setup() -> Option<Fixture> {
    let dsn = std::env::var("DATABASE_URL").ok()?;

    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    run_lakekeeper_migrations(&dsn)
        .await
        .expect("lakekeeper migrations");
    let (engine_state, _registry) = build_state(&dsn, SYSTEM_USER)
        .await
        .expect("build engine state");
    ensure_bootstrapped(engine_state.clone(), SYSTEM_USER)
        .await
        .expect("bootstrap");

    let store = CatalogStore::new(db.clone());
    let crypto = Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secret_store = SecretStore::new(db.clone(), Some(crypto));
    let users = UserRepo::new(db.clone());
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );

    let suffix = Uuid::now_v7().simple().to_string();
    let owner = users
        .insert_local(
            &format!("stowner-{suffix}@egret.test"),
            "Storage Owner",
            "x",
        )
        .await
        .expect("seed owner");
    let slug = format!("stws{suffix}");
    let ws = WorkspaceRepo::new(db.clone())
        .create(&slug, "Storage Update Test", owner)
        .await
        .expect("seed workspace");
    let workspace_id: WorkspaceId = ws.id;

    // Provision a warehouse for the workspace (live MinIO validate_access),
    // mirroring how prod provisions on workspace create.
    provision_workspace_warehouse(
        engine_state.clone(),
        &store,
        workspace_id.0,
        &slug,
        &dev_storage_config(),
        SYSTEM_USER,
    )
    .await
    .expect("provision warehouse (needs MinIO reachable + writable `warehouse` bucket)");

    let app = browse_router().with_state(BrowseState {
        engine: engine_state,
        store: store.clone(),
        secret_store: secret_store.clone(),
        default_storage: dev_storage_config(),
        system_user: SYSTEM_USER.into(),
        sessions: sessions.clone(),
        workspaces: WorkspaceRepo::new(db.clone()),
        authz: Authorizer::new(db.clone()),
    });

    Some(Fixture {
        app,
        store,
        secret_store,
        sessions,
        users,
        members: MemberRepo::new(db.clone()),
        db,
        slug,
        workspace_id,
        owner,
    })
}

/// Add a new user as a workspace member with `role`, returning a session token.
async fn add_member(f: &Fixture, email_tag: &str, role: &str) -> String {
    let email = format!("{email_tag}_{}@egret.local", Uuid::new_v4().simple());
    let uid = f
        .users
        .insert_local(&email, "Test", "x")
        .await
        .expect("insert user");
    f.members
        .add(f.workspace_id, "user", uid.0, role, f.owner)
        .await
        .expect("seed member");
    f.sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
}

fn put(uri: &str, token: &str, body: serde_json::Value) -> HttpRequest<Body> {
    HttpRequest::builder()
        .method("PUT")
        .uri(uri)
        .header("Cookie", format!("egret_session={token}"))
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

#[tokio::test]
async fn admin_repoints_storage_then_keeps_secret_then_rejects_bad_creds() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "admin", "ws_admin").await;
    let uri = format!("/ws/{}/catalog/storage", f.slug);

    // --- 1. Valid re-point with a distinct secret. ---
    const NEW_SECRET: &str = "egretsecret";
    let body = serde_json::json!({
        "provider": "s3",
        "endpoint": "http://localhost:9000",
        "region": "us-east-1",
        "bucket": "warehouse",
        "key_prefix": "ignored-by-server",
        "access_key": "egret",
        "secret_key": NEW_SECRET,
        "path_style": true,
    });
    let resp = f
        .app
        .clone()
        .oneshot(put(&uri, &token, body))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "valid re-point → 200; body={json}");
    // Response is the secret-free WorkspaceStorageInfo.
    assert_eq!(json["bucket"], serde_json::json!("warehouse"));
    assert_eq!(json["region"], serde_json::json!("us-east-1"));
    assert_eq!(json["access_key"], serde_json::json!("egret"));
    assert_eq!(json["has_secret"], serde_json::json!(true));
    // The request's `key_prefix` ("ignored-by-server") is NOT applied; the row +
    // response carry the CANONICAL warehouse prefix matching the real layout.
    assert_eq!(
        json["key_prefix"],
        serde_json::json!(format!("workspaces/{}/warehouse", f.slug)),
        "response carries the canonical warehouse key_prefix, not the request value; body={json}"
    );
    assert!(
        json.get("secret_key").is_none() && json.get("secret").is_none(),
        "no secret field in the response; body={json}"
    );
    assert!(
        !json.to_string().contains(NEW_SECRET),
        "the plaintext secret must never appear in the body; body={json}"
    );

    // The row is updated and the NEW secret is fetchable from the vault by ref.
    let row = f
        .store
        .workspace_storage_for(f.workspace_id.0)
        .await
        .expect("read row")
        .expect("row exists after re-point");
    assert_eq!(row.region, "us-east-1");
    let secret_ref = row.secret_ref.expect("secret_ref set");
    assert_eq!(
        f.secret_store.get(secret_ref).await.expect("fetch secret"),
        NEW_SECRET,
        "the new secret round-trips through the vault"
    );

    // --- 2. PUT WITHOUT secret_key → keeps the prior secret. ---
    let body = serde_json::json!({
        "provider": "s3",
        "endpoint": "http://localhost:9000",
        "region": "us-east-1",
        "bucket": "warehouse",
        "access_key": "egret",
        "secret_key": null,
        "path_style": true,
    });
    let resp = f
        .app
        .clone()
        .oneshot(put(&uri, &token, body))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "re-point without secret_key keeps the prior secret → 200; body={json}"
    );
    let row2 = f
        .store
        .workspace_storage_for(f.workspace_id.0)
        .await
        .expect("read row")
        .expect("row still exists");
    let secret_ref2 = row2.secret_ref.expect("secret_ref still set");
    assert_eq!(
        f.secret_store.get(secret_ref2).await.expect("fetch secret"),
        NEW_SECRET,
        "omitting secret_key keeps the prior secret value"
    );

    // --- 3. BOGUS credential → lakekeeper validates first → 400, row UNCHANGED. ---
    let body = serde_json::json!({
        "provider": "s3",
        "endpoint": "http://localhost:9000",
        "region": "us-east-1",
        "bucket": "warehouse",
        "access_key": "egret",
        "secret_key": "wrong",
        "path_style": true,
    });
    let resp = f
        .app
        .clone()
        .oneshot(put(&uri, &token, body))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "bogus credential rejected by lakekeeper validate_access → 400"
    );
    let row3 = f
        .store
        .workspace_storage_for(f.workspace_id.0)
        .await
        .expect("read row")
        .expect("row still exists");
    assert_eq!(
        row3.secret_ref, row2.secret_ref,
        "a rejected re-point must NOT change the egret row (lakekeeper-validated-first)"
    );
}

#[tokio::test]
async fn non_admin_member_is_403() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "viewer", "ws_viewer").await;
    let uri = format!("/ws/{}/catalog/storage", f.slug);
    let body = serde_json::json!({
        "provider": "s3",
        "endpoint": "http://localhost:9000",
        "region": "us-east-1",
        "bucket": "warehouse",
        "access_key": "egret",
        "secret_key": "egretsecret",
        "path_style": true,
    });
    let resp = f
        .app
        .clone()
        .oneshot(put(&uri, &token, body))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non-admin member must not update workspace storage"
    );
}

#[tokio::test]
async fn non_s3_provider_is_400() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "admin", "ws_admin").await;
    let uri = format!("/ws/{}/catalog/storage", f.slug);
    let body = serde_json::json!({
        "provider": "gcs",
        "endpoint": "http://localhost:9000",
        "region": "us-east-1",
        "bucket": "warehouse",
        "access_key": "egret",
        "secret_key": "egretsecret",
        "path_style": true,
    });
    let resp = f
        .app
        .clone()
        .oneshot(put(&uri, &token, body))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "a non-s3 provider is rejected with 400"
    );
    // The row must NOT have been written with the wrong provider.
    let row = f
        .store
        .workspace_storage_for(f.workspace_id.0)
        .await
        .expect("read row");
    assert!(
        row.is_none() || row.is_some_and(|r| r.provider == "s3"),
        "a rejected non-s3 provider must never be persisted"
    );
}

#[tokio::test]
async fn put_on_workspace_without_warehouse_provisions_then_repoints() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };

    // A SECOND workspace with NO provisioned warehouse (predates catalog
    // enablement). PUT must now lazily provision + re-point → 200, not 409.
    let suffix = Uuid::now_v7().simple().to_string();
    let owner = f
        .users
        .insert_local(&format!("nowh-{suffix}@egret.test"), "No WH Owner", "x")
        .await
        .expect("seed owner");
    let slug = format!("nowh{suffix}");
    let ws = WorkspaceRepo::new(f.db.clone())
        .create(&slug, "No Warehouse", owner)
        .await
        .expect("seed workspace");
    assert!(
        f.store
            .warehouse_for_workspace(ws.id.0)
            .await
            .expect("read mapping")
            .is_none(),
        "precondition: the workspace starts with NO warehouse"
    );
    // Make the caller an admin of this warehouse-less workspace.
    let admin = f
        .users
        .insert_local(&format!("nowha-{suffix}@egret.test"), "No WH Admin", "x")
        .await
        .expect("seed admin");
    f.members
        .add(ws.id, "user", admin.0, "ws_admin", owner)
        .await
        .expect("seed member");
    let token = f
        .sessions
        .create(admin, "test", "127.0.0.1")
        .await
        .expect("session")
        .token;

    let body = serde_json::json!({
        "provider": "s3",
        "endpoint": "http://localhost:9000",
        "region": "us-east-1",
        "bucket": "warehouse",
        "access_key": "egret",
        "secret_key": "egretsecret",
        "path_style": true,
    });
    let resp = f
        .app
        .clone()
        .oneshot(put(&format!("/ws/{slug}/catalog/storage"), &token, body))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "PUT on a never-visited workspace lazily provisions + re-points → 200; body={json}"
    );
    // The warehouse is now provisioned.
    assert!(
        f.store
            .warehouse_for_workspace(ws.id.0)
            .await
            .expect("read mapping")
            .is_some(),
        "PUT lazily provisioned the warehouse"
    );
}
