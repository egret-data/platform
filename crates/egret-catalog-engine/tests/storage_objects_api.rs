//! Workspace-admin per-connection OBJECT browser HTTP route tests (Workspace
//! Storage track). DB-gated AND MinIO-gated; self-skips without `DATABASE_URL`.
//!
//! These prove the file-manager object routes added to the browse router
//! (`egret_catalog_engine::browse_router`) for a registered storage connection:
//!   - `POST .../objects?key=hello.txt` uploads bytes under the connection root,
//!   - `GET  .../objects?prefix=` lists root-relative keys (root stripped),
//!   - `GET  .../objects/content?key=hello.txt` downloads the bytes with an
//!     `attachment` Content-Disposition,
//!   - `DELETE .../objects?key=hello.txt` removes the object,
//!   - PATH SAFETY: `..` / absolute / NUL paths are rejected with 400, confining
//!     every op under the connection's `key_prefix` root,
//!   - a non-admin member (`ws_viewer`) is Forbidden (403) on every route,
//!   - a connection id from ANOTHER workspace is a 404 (cross-tenant IDOR closed).
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-catalog-engine --test storage_objects_api
//! (requires the dev MinIO at localhost:9000, creds egret/egretsecret, bucket
//! `warehouse`, path_style).

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::response::Response;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_catalog::service::storage_conn::StorageConnInput;
use egret_catalog::service::CatalogStore;
use egret_catalog_engine::{
    browse_router, build_state, ensure_bootstrapped, run_lakekeeper_migrations, BrowseState,
};
use egret_core::ids::{UserId, WorkspaceId};
use egret_metastore::Db;
use egret_storage::ObjectStore;
use egret_workspace::members::MemberRepo;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use http_body_util::BodyExt;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

const SYSTEM_USER: &str = "egret-system";

struct Fixture {
    app: axum::Router,
    store: CatalogStore,
    sessions: SessionService,
    users: UserRepo,
    members: MemberRepo,
    db: Db,
    slug: String,
    workspace_id: WorkspaceId,
    owner: UserId,
    /// The connection root for the seeded conn (`conns/<uuid>`), so the test can
    /// assert objects land under it in MinIO.
    root: String,
}

async fn into_json(resp: Response) -> (StatusCode, Vec<u8>, serde_json::Value) {
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(
        |_| serde_json::json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }),
    );
    (status, bytes.to_vec(), json)
}

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
    let users = UserRepo::new(db.clone());
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );

    let crypto =
        egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secret_store = egret_secrets::SecretStore::new(db.clone(), Some(crypto));

    let suffix = Uuid::now_v7().simple().to_string();
    let owner = users
        .insert_local(&format!("objowner-{suffix}@egret.test"), "Obj Owner", "x")
        .await
        .expect("seed owner");
    let slug = format!("objws{suffix}");
    let ws = WorkspaceRepo::new(db.clone())
        .create(&slug, "Storage Objects Test", owner)
        .await
        .expect("seed workspace");
    let workspace_id: WorkspaceId = ws.id;

    let app = browse_router().with_state(BrowseState {
        engine: engine_state,
        store: store.clone(),
        secret_store: secret_store.clone(),
        workspace_secrets: egret_secrets::WorkspaceSecretStore::new(
            db.clone(),
            secret_store.clone(),
        ),
        default_storage: egret_storage::StorageConfig {
            endpoint: Some("http://localhost:9000".to_string()),
            region: "local-01".to_string(),
            bucket: "warehouse".to_string(),
            access_key: "egret".to_string(),
            secret_key: "egretsecret".to_string(),
            path_style: true,
        },
        system_user: SYSTEM_USER.into(),
        sessions: sessions.clone(),
        workspaces: WorkspaceRepo::new(db.clone()),
        authz: Authorizer::new(db.clone()),
    });

    Some(Fixture {
        app,
        store,
        sessions,
        users,
        members: MemberRepo::new(db.clone()),
        db,
        slug,
        workspace_id,
        owner,
        root: String::new(),
    })
}

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

/// Register a storage connection (in `f`'s workspace) pointing at MinIO with a
/// unique `conns/<uuid>` root; seals the real secret so object ops authenticate.
async fn seed_conn(f: &mut Fixture, secret_store: &egret_secrets::SecretStore) -> Uuid {
    let root = format!("conns/{}", Uuid::new_v4().simple());
    f.root = root.clone();
    // Epic B: the connection references a NAMED workspace secret, not a raw vault
    // id — `conn_s3` resolves `secret_ref` via `WorkspaceSecretStore`.
    let ws_secrets = egret_secrets::WorkspaceSecretStore::new(f.db.clone(), secret_store.clone());
    let secret_ref = ws_secrets
        .create(
            f.workspace_id.0,
            &format!("sec-{}", Uuid::new_v4().simple()),
            None,
            "egretsecret",
        )
        .await
        .expect("create named secret");
    let input = StorageConnInput {
        name: format!("conn-{}", Uuid::new_v4().simple()),
        purpose: None,
        provider: "s3".to_string(),
        endpoint: Some("http://localhost:9000".to_string()),
        region: "us-east-1".to_string(),
        bucket: "warehouse".to_string(),
        key_prefix: Some(root),
        access_key: "egret".to_string(),
        secret_ref: Some(secret_ref),
        path_style: true,
    };
    f.store
        .create_storage_conn(f.workspace_id.0, &input)
        .await
        .expect("seed conn")
        .id
}

/// Seed a connection in a DIFFERENT workspace; returns its id to attack across the
/// tenant boundary via `f`'s workspace path.
async fn seed_other_workspace_conn(f: &Fixture) -> Uuid {
    let suffix = Uuid::now_v7().simple().to_string();
    let owner = f
        .users
        .insert_local(&format!("objy-{suffix}@egret.test"), "Y Owner", "x")
        .await
        .expect("seed Y owner");
    let slug = format!("objy{suffix}");
    let ws = WorkspaceRepo::new(f.db.clone())
        .create(&slug, "Objects Other", owner)
        .await
        .expect("seed other workspace");
    let input = StorageConnInput {
        name: "y-conn".to_string(),
        purpose: None,
        provider: "s3".to_string(),
        endpoint: Some("http://localhost:9000".to_string()),
        region: "us-east-1".to_string(),
        bucket: "warehouse".to_string(),
        key_prefix: Some(format!("conns/{}", Uuid::new_v4().simple())),
        access_key: "egret".to_string(),
        secret_ref: None,
        path_style: true,
    };
    f.store
        .create_storage_conn(ws.id.0, &input)
        .await
        .expect("seed Y conn")
        .id
}

fn req(method: &str, uri: &str, token: &str, body: Option<Body>) -> HttpRequest<Body> {
    let b = HttpRequest::builder()
        .method(method)
        .uri(uri)
        .header("Cookie", format!("egret_session={token}"));
    b.body(body.unwrap_or_else(Body::empty)).unwrap()
}

#[tokio::test]
async fn admin_can_upload_list_download_and_delete_objects() {
    let Some(mut f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let secret_store = {
        // Re-derive the same secret store the app uses (same crypto key) to seal.
        let crypto = egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef")
            .expect("build test crypto");
        egret_secrets::SecretStore::new(f.db.clone(), Some(crypto))
    };
    let id = seed_conn(&mut f, &secret_store).await;
    let token = add_member(&f, "admin", "ws_admin").await;
    let base = format!("/ws/{}/storages/{}/objects", f.slug, id);

    // UPLOAD `hello.txt`.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("{base}?key=hello.txt"),
            &token,
            Some(Body::from(&b"hi there"[..])),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "upload → 201");

    // The object landed at `<root>/hello.txt` in MinIO (verified via a fresh
    // direct S3 listing under the root).
    let direct = egret_storage::S3Store::new(&egret_storage::StorageConfig {
        endpoint: Some("http://localhost:9000".to_string()),
        region: "us-east-1".to_string(),
        bucket: "warehouse".to_string(),
        access_key: "egret".to_string(),
        secret_key: "egretsecret".to_string(),
        path_style: true,
    })
    .await
    .expect("direct store");
    let listing = direct
        .list(&format!("{}/", f.root), Some("/"))
        .await
        .expect("direct list");
    assert!(
        listing
            .objects
            .iter()
            .any(|o| o.key == format!("{}/hello.txt", f.root) && o.size == 8),
        "object lands at <root>/hello.txt with size 8; got {:?}",
        listing.objects
    );

    // LIST root — `hello.txt` present, ROOT STRIPPED, size 8.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("{base}?prefix="), &token, None))
        .await
        .unwrap();
    let (status, _bytes, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list → 200; body={json}");
    let objects = json["objects"].as_array().expect("objects array");
    let hello = objects
        .iter()
        .find(|o| o["key"].as_str() == Some("hello.txt"))
        .expect("hello.txt listed root-relative");
    assert_eq!(hello["size"].as_i64(), Some(8), "size 8; body={json}");

    // DOWNLOAD `hello.txt`.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!(
                "/ws/{}/storages/{}/objects/content?key=hello.txt",
                f.slug, id
            ),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "download → 200");
    let cd = resp
        .headers()
        .get(axum::http::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        cd.starts_with("attachment"),
        "Content-Disposition attachment; got {cd:?}"
    );
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"hi there", "download body matches upload");

    // DELETE `hello.txt` → 204, then absent from list.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("{base}?key=hello.txt"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "delete → 204");

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("{base}?prefix="), &token, None))
        .await
        .unwrap();
    let (status, _bytes, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK);
    let still = json["objects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|o| o["key"].as_str() == Some("hello.txt"));
    assert!(!still, "deleted object no longer listed; body={json}");
}

#[tokio::test]
async fn path_traversal_is_rejected() {
    let Some(mut f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let secret_store = {
        let crypto = egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef")
            .expect("build test crypto");
        egret_secrets::SecretStore::new(f.db.clone(), Some(crypto))
    };
    let id = seed_conn(&mut f, &secret_store).await;
    let token = add_member(&f, "admin", "ws_admin").await;
    let base = format!("/ws/{}/storages/{}/objects", f.slug, id);

    // LIST with `..` → 400.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("{base}?prefix=../escape"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "list with `..` must 400"
    );

    // DELETE with `..` → 400.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("{base}?key=../../etc"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "delete with `..` must 400"
    );

    // UPLOAD with an absolute key → 400.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("{base}?key=/abs"),
            &token,
            Some(Body::from(&b"x"[..])),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "upload with absolute key must 400"
    );
}

#[tokio::test]
async fn non_admin_member_is_403() {
    let Some(mut f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let secret_store = {
        let crypto = egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef")
            .expect("build test crypto");
        egret_secrets::SecretStore::new(f.db.clone(), Some(crypto))
    };
    let id = seed_conn(&mut f, &secret_store).await;
    let token = add_member(&f, "viewer", "ws_viewer").await;
    let base = format!("/ws/{}/storages/{}/objects", f.slug, id);

    // LIST → 403.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("{base}?prefix="), &token, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "viewer list → 403");

    // DOWNLOAD → 403.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!(
                "/ws/{}/storages/{}/objects/content?key=hello.txt",
                f.slug, id
            ),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "viewer download → 403"
    );

    // UPLOAD → 403.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("{base}?key=x.txt"),
            &token,
            Some(Body::from(&b"x"[..])),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "viewer upload → 403");

    // DELETE → 403.
    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &format!("{base}?key=x.txt"), &token, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "viewer delete → 403");
}

#[tokio::test]
async fn cross_workspace_conn_is_404() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "admin", "ws_admin").await;
    let foreign_id = seed_other_workspace_conn(&f).await;
    let base = format!("/ws/{}/storages/{}/objects", f.slug, foreign_id);

    // LIST against another workspace's conn id → 404 (IDOR closed).
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("{base}?prefix="), &token, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "cross-workspace conn id must 404"
    );
}
