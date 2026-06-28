//! Workspace-admin per-connection FOLDER-OPS HTTP route tests (Workspace Storage
//! track). DB-gated AND MinIO-gated; self-skips without `DATABASE_URL`.
//!
//! These prove the folder-management routes added to the browse router
//! (`egret_catalog_engine::browse_router`) for a registered storage connection:
//!   - `POST .../objects/folder?path=docs` creates a `docs/` marker (201),
//!   - `DELETE .../objects?key=docs/&recursive=true` removes a folder + contents
//!     (204),
//!   - `POST .../objects/move {from,to}` moves/renames a single object (200),
//!   - folder move (`from`/`to` both end `/`) moves every object under the prefix,
//!   - PATH SAFETY: `..` paths (folder/move from/to) are rejected with 400,
//!   - folder-into-itself and `from == to` moves are rejected with 400,
//!   - a non-admin member (`ws_viewer`) is Forbidden (403),
//!   - a connection id from ANOTHER workspace is a 404 (cross-tenant IDOR closed).
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-catalog-engine --test storage_folder_ops_api
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
}

async fn into_json(resp: Response) -> (StatusCode, serde_json::Value) {
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(
        |_| serde_json::json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }),
    );
    (status, json)
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
        .insert_local(
            &format!("folderowner-{suffix}@egret.test"),
            "Folder Owner",
            "x",
        )
        .await
        .expect("seed owner");
    let slug = format!("folderws{suffix}");
    let ws = WorkspaceRepo::new(db.clone())
        .create(&slug, "Storage Folder Ops Test", owner)
        .await
        .expect("seed workspace");
    let workspace_id: WorkspaceId = ws.id;

    let app = browse_router().with_state(BrowseState {
        engine: engine_state,
        store: store.clone(),
        workspace_secrets: egret_secrets::WorkspaceSecretStore::new(
            db.clone(),
            secret_store.clone(),
        ),
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
async fn seed_conn(f: &Fixture, secret_store: &egret_secrets::SecretStore) -> Uuid {
    let root = format!("conns/{}", Uuid::new_v4().simple());
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

/// Seed a connection in a DIFFERENT workspace; returns its id to attack across
/// the tenant boundary via `f`'s workspace path.
async fn seed_other_workspace_conn(f: &Fixture) -> Uuid {
    let suffix = Uuid::now_v7().simple().to_string();
    let owner = f
        .users
        .insert_local(&format!("foldery-{suffix}@egret.test"), "Y Owner", "x")
        .await
        .expect("seed Y owner");
    let slug = format!("foldery{suffix}");
    let ws = WorkspaceRepo::new(f.db.clone())
        .create(&slug, "Folder Other", owner)
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

fn json_req(method: &str, uri: &str, token: &str, body: serde_json::Value) -> HttpRequest<Body> {
    HttpRequest::builder()
        .method(method)
        .uri(uri)
        .header("Cookie", format!("egret_session={token}"))
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// Fetch the root listing as `(folders, object_keys)` (root-relative).
async fn list_root(f: &Fixture, base: &str, token: &str) -> (Vec<String>, Vec<String>) {
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("{base}?prefix="), token, None))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list → 200; body={json}");
    let folders = json["folders"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    let objects = json["objects"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|o| o["key"].as_str().map(str::to_string))
        .collect();
    (folders, objects)
}

/// List a sub-prefix's object keys (root-relative).
async fn list_prefix_objects(f: &Fixture, base: &str, token: &str, prefix: &str) -> Vec<String> {
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("{base}?prefix={prefix}"), token, None))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list {prefix} → 200; body={json}");
    json["objects"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|o| o["key"].as_str().map(str::to_string))
        .collect()
}

async fn upload(f: &Fixture, base: &str, token: &str, key: &str, bytes: &[u8]) {
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("{base}?key={key}"),
            token,
            Some(Body::from(bytes.to_vec())),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "upload {key} → 201");
}

#[tokio::test]
async fn admin_can_create_delete_and_move_folders() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let secret_store = {
        let crypto = egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef")
            .expect("build test crypto");
        egret_secrets::SecretStore::new(f.db.clone(), Some(crypto))
    };
    let id = seed_conn(&f, &secret_store).await;
    let token = add_member(&f, "admin", "ws_admin").await;
    let base = format!("/ws/{}/storages/{}/objects", f.slug, id);
    let folder_url = format!("{base}/folder");
    let move_url = format!("{base}/move");

    // --- NEW FOLDER ------------------------------------------------------
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("{folder_url}?path=docs"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "new folder → 201");

    let (folders, _) = list_root(&f, &base, &token).await;
    assert!(
        folders.iter().any(|x| x == "docs/"),
        "docs/ folder listed; got {folders:?}"
    );

    // Listing INSIDE the freshly-created empty folder must return no objects:
    // the folder's own marker (`docs/`) backs the folder in the parent listing
    // but must not leak as a phantom 0-byte file when browsing into it.
    let inside_empty = list_prefix_objects(&f, &base, &token, "docs/").await;
    assert!(
        inside_empty.is_empty(),
        "empty folder shows no phantom marker object; got {inside_empty:?}"
    );

    // --- RECURSIVE DELETE ------------------------------------------------
    upload(&f, &base, &token, "docs/x.txt", b"inside docs").await;
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("{base}?key=docs/&recursive=true"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "recursive delete → 204"
    );
    let (folders, _) = list_root(&f, &base, &token).await;
    assert!(
        !folders.iter().any(|x| x == "docs/"),
        "docs/ gone after recursive delete; got {folders:?}"
    );
    let inside = list_prefix_objects(&f, &base, &token, "docs/").await;
    assert!(
        inside.is_empty(),
        "docs/x.txt gone after recursive delete; got {inside:?}"
    );

    // --- FILE MOVE -------------------------------------------------------
    upload(&f, &base, &token, "a.txt", b"file body").await;
    let resp = f
        .app
        .clone()
        .oneshot(json_req(
            "POST",
            &move_url,
            &token,
            serde_json::json!({ "from": "a.txt", "to": "sub/a.txt" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "file move → 200");
    let (_, root_objs) = list_root(&f, &base, &token).await;
    assert!(
        !root_objs.iter().any(|k| k == "a.txt"),
        "a.txt gone from root after move; got {root_objs:?}"
    );
    let sub = list_prefix_objects(&f, &base, &token, "sub/").await;
    assert!(
        sub.iter().any(|k| k == "sub/a.txt"),
        "sub/a.txt present after move; got {sub:?}"
    );
    // Bytes preserved.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("{base}/content?key=sub/a.txt"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "download moved file → 200");
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"file body", "moved file bytes preserved");

    // --- RENAME ----------------------------------------------------------
    let resp = f
        .app
        .clone()
        .oneshot(json_req(
            "POST",
            &move_url,
            &token,
            serde_json::json!({ "from": "sub/a.txt", "to": "sub/renamed.txt" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "rename → 200");
    let sub = list_prefix_objects(&f, &base, &token, "sub/").await;
    assert!(
        sub.iter().any(|k| k == "sub/renamed.txt") && !sub.iter().any(|k| k == "sub/a.txt"),
        "renamed in sub/; got {sub:?}"
    );

    // --- FOLDER MOVE -----------------------------------------------------
    upload(&f, &base, &token, "f/1.txt", b"one").await;
    upload(&f, &base, &token, "f/2.txt", b"two").await;
    let resp = f
        .app
        .clone()
        .oneshot(json_req(
            "POST",
            &move_url,
            &token,
            serde_json::json!({ "from": "f/", "to": "g/" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "folder move → 200");
    let g = list_prefix_objects(&f, &base, &token, "g/").await;
    assert!(
        g.iter().any(|k| k == "g/1.txt") && g.iter().any(|k| k == "g/2.txt"),
        "g/1.txt + g/2.txt present after folder move; got {g:?}"
    );
    let leftover = list_prefix_objects(&f, &base, &token, "f/").await;
    assert!(
        leftover.is_empty(),
        "f/* gone after folder move; got {leftover:?}"
    );

    // --- FOLDER INTO ITSELF → 400 ---------------------------------------
    let resp = f
        .app
        .clone()
        .oneshot(json_req(
            "POST",
            &move_url,
            &token,
            serde_json::json!({ "from": "g/", "to": "g/h/" }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "folder into itself → 400"
    );

    // --- from == to → 400 ------------------------------------------------
    let resp = f
        .app
        .clone()
        .oneshot(json_req(
            "POST",
            &move_url,
            &token,
            serde_json::json!({ "from": "g/1.txt", "to": "g/1.txt" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "from == to → 400");
}

#[tokio::test]
async fn path_safety_rejects_traversal() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let secret_store = {
        let crypto = egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef")
            .expect("build test crypto");
        egret_secrets::SecretStore::new(f.db.clone(), Some(crypto))
    };
    let id = seed_conn(&f, &secret_store).await;
    let token = add_member(&f, "admin", "ws_admin").await;
    let base = format!("/ws/{}/storages/{}/objects", f.slug, id);

    // NEW FOLDER with `..` → 400.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("{base}/folder?path=../x"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "new folder with `..` → 400"
    );

    // MOVE from `..` → 400.
    let resp = f
        .app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("{base}/move"),
            &token,
            serde_json::json!({ "from": "../x", "to": "ok.txt" }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "move from `..` → 400"
    );

    // MOVE to `..` → 400.
    let resp = f
        .app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("{base}/move"),
            &token,
            serde_json::json!({ "from": "ok.txt", "to": "../y" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "move to `..` → 400");
}

#[tokio::test]
async fn non_admin_member_is_403() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let secret_store = {
        let crypto = egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef")
            .expect("build test crypto");
        egret_secrets::SecretStore::new(f.db.clone(), Some(crypto))
    };
    let id = seed_conn(&f, &secret_store).await;
    let token = add_member(&f, "viewer", "ws_viewer").await;
    let base = format!("/ws/{}/storages/{}/objects", f.slug, id);

    // NEW FOLDER → 403.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("{base}/folder?path=docs"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "viewer new folder → 403"
    );

    // MOVE → 403.
    let resp = f
        .app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("{base}/move"),
            &token,
            serde_json::json!({ "from": "a.txt", "to": "b.txt" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "viewer move → 403");

    // RECURSIVE DELETE → 403.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("{base}?key=docs/&recursive=true"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "viewer recursive delete → 403"
    );
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

    // NEW FOLDER against another workspace's conn id → 404 (IDOR closed).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("{base}/folder?path=docs"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "cross-workspace new folder → 404"
    );

    // MOVE against another workspace's conn id → 404.
    let resp = f
        .app
        .clone()
        .oneshot(json_req(
            "POST",
            &format!("{base}/move"),
            &token,
            serde_json::json!({ "from": "a.txt", "to": "b.txt" }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "cross-workspace move → 404"
    );
}
