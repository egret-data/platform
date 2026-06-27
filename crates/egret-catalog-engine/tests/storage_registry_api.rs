//! Workspace-admin storage-registry HTTP route tests (Workspace Storage track).
//! DB-gated AND MinIO-gated; self-skips without `DATABASE_URL`.
//!
//! These prove the register/list/edit/delete routes added to the browse router
//! (`egret_catalog_engine::browse_router`) for the per-workspace storage
//! connection registry (`workspace_storages`):
//!   - a `ws_admin` member (holds `workspace:manage_members`) can register, list,
//!     edit, and delete storage connections,
//!   - registration runs a LIVE reachability probe (`S3Store::list`), so bogus
//!     credentials are rejected with 400 before anything is persisted,
//!   - the raw S3 secret is never echoed back in ANY response (only stored,
//!     sealed, in the secret vault, surfaced as a `secret_ref`/`has_secret`),
//!   - a non-admin member (`ws_viewer`) is Forbidden (403) on every route,
//!   - deleting a connection that belongs to ANOTHER workspace via this
//!     workspace's path is a 404 (cross-tenant IDOR closed),
//!   - editing without a `secret_key` keeps the existing sealed secret.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-catalog-engine --test storage_registry_api
//! (requires the dev MinIO at localhost:9000, creds egret/egretsecret, bucket
//! `warehouse`, path_style).

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::response::Response;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
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
    secret_store: egret_secrets::SecretStore,
    sessions: SessionService,
    users: UserRepo,
    members: MemberRepo,
    db: Db,
    slug: String,
    workspace_id: WorkspaceId,
    owner: UserId,
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
        .insert_local(&format!("storowner-{suffix}@egret.test"), "Stor Owner", "x")
        .await
        .expect("seed owner");
    let slug = format!("storws{suffix}");
    let ws = WorkspaceRepo::new(db.clone())
        .create(&slug, "Storage Admin Test", owner)
        .await
        .expect("seed workspace");
    let workspace_id: WorkspaceId = ws.id;

    let app = browse_router().with_state(BrowseState {
        engine: engine_state,
        store: store.clone(),
        secret_store: secret_store.clone(),
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

/// Seed a second workspace Y and register a storage conn in it directly via the
/// store. Returns `(workspace_id, conn_id)` to attack across the tenant boundary.
async fn seed_other_workspace_conn(f: &Fixture) -> Uuid {
    let suffix = Uuid::now_v7().simple().to_string();
    let owner = f
        .users
        .insert_local(&format!("story-{suffix}@egret.test"), "Y Owner", "x")
        .await
        .expect("seed Y owner");
    let slug = format!("story{suffix}");
    let ws = WorkspaceRepo::new(f.db.clone())
        .create(&slug, "Storage Other", owner)
        .await
        .expect("seed other workspace");
    let input = egret_catalog::service::storage_conn::StorageConnInput {
        name: "y-conn".to_string(),
        purpose: None,
        provider: "s3".to_string(),
        endpoint: Some("http://localhost:9000".to_string()),
        region: "us-east-1".to_string(),
        bucket: "warehouse".to_string(),
        key_prefix: Some("conns/y".to_string()),
        access_key: "egret".to_string(),
        secret_ref: None,
        path_style: true,
    };
    let conn = f
        .store
        .create_storage_conn(ws.id.0, &input)
        .await
        .expect("seed Y conn");
    conn.id
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
async fn admin_can_register_list_edit_and_delete_storage_conns() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "admin", "ws_admin").await;
    let base = format!("/ws/{}/storages", f.slug);

    // REGISTER (live reachability probe against MinIO).
    let body = serde_json::json!({
        "name": "minio-main",
        "provider": "s3",
        "endpoint": "http://localhost:9000",
        "region": "us-east-1",
        "bucket": "warehouse",
        "key_prefix": "conns/test",
        "access_key": "egret",
        "secret_key": "egretsecret",
        "path_style": true,
    });
    let resp = f
        .app
        .clone()
        .oneshot(req("POST", &base, &token, Some(body)))
        .await
        .unwrap();
    let (status, bytes, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "register; body={json}");
    let id = json["id"].as_str().expect("id in body").to_string();
    // No secret value in the create response.
    let raw_str = String::from_utf8_lossy(&bytes);
    assert!(
        !raw_str.contains("egretsecret"),
        "secret value must never be echoed in register response; body={raw_str}"
    );
    assert!(
        !raw_str.contains("secret_key"),
        "no `secret_key` field in register response; body={raw_str}"
    );
    // The sealed secret resolves to the supplied plaintext via the store row.
    let conn = f
        .store
        .get_storage_conn(f.workspace_id.0, id.parse().unwrap())
        .await
        .expect("get conn")
        .expect("conn exists");
    let secret_ref = conn.secret_ref.expect("secret_ref persisted");
    let plaintext = f.secret_store.get(secret_ref).await.expect("unseal secret");
    assert_eq!(plaintext, "egretsecret", "sealed secret round-trips");

    // LIST — present, NO secret leaked.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &base, &token, None))
        .await
        .unwrap();
    let (status, bytes, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list; body={json}");
    let listed = json
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["id"].as_str() == Some(id.as_str()));
    assert!(listed, "registered conn listed; body={json}");
    let raw_str = String::from_utf8_lossy(&bytes);
    assert!(
        !raw_str.contains("egretsecret"),
        "raw secret must never be echoed in list"
    );
    assert!(
        !raw_str.contains("secret_key"),
        "no `secret_key` field in list JSON"
    );

    // EDIT — change region, omit secret_key (keep-current).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("{base}/{id}"),
            &token,
            Some(serde_json::json!({
                "name": "minio-main",
                "provider": "s3",
                "endpoint": "http://localhost:9000",
                "region": "eu-west-1",
                "bucket": "warehouse",
                "key_prefix": "conns/test",
                "access_key": "egret",
                "path_style": true,
            })),
        ))
        .await
        .unwrap();
    let (status, _bytes, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "edit; body={json}");
    let conn = f
        .store
        .get_storage_conn(f.workspace_id.0, id.parse().unwrap())
        .await
        .expect("get conn")
        .expect("conn exists");
    assert_eq!(conn.region, "eu-west-1", "region updated");
    assert_eq!(
        conn.secret_ref,
        Some(secret_ref),
        "secret unchanged when secret_key omitted"
    );

    // DELETE → 204, then gone from list.
    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &format!("{base}/{id}"), &token, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "delete → 204");

    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &base, &token, None))
        .await
        .unwrap();
    let (status, _bytes, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK);
    let still = json
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["id"].as_str() == Some(id.as_str()));
    assert!(!still, "deleted conn no longer listed; body={json}");
}

#[tokio::test]
async fn register_with_bogus_creds_is_400() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "admin", "ws_admin").await;
    let base = format!("/ws/{}/storages", f.slug);

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &base,
            &token,
            Some(serde_json::json!({
                "name": "bad",
                "provider": "s3",
                "endpoint": "http://localhost:9000",
                "region": "us-east-1",
                "bucket": "warehouse",
                "key_prefix": "conns/bad",
                "access_key": "egret",
                "secret_key": "wrong",
                "path_style": true,
            })),
        ))
        .await
        .unwrap();
    let (status, _bytes, json) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "bogus creds fail reachability → 400; body={json}"
    );

    // Nothing was persisted.
    let conns = f
        .store
        .list_storage_conns(f.workspace_id.0)
        .await
        .expect("list");
    assert!(
        conns.iter().all(|c| c.name != "bad"),
        "failed register must not persist a row"
    );
}

#[tokio::test]
async fn delete_of_foreign_workspace_conn_is_404() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "admin", "ws_admin").await;
    let foreign_id = seed_other_workspace_conn(&f).await;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/ws/{}/storages/{}", f.slug, foreign_id),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "deleting another workspace's storage conn must 404 (IDOR closed)"
    );

    // And it must NOT appear in this workspace's list.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/ws/{}/storages", f.slug),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, _bytes, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK);
    let leaked = json
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["id"].as_str() == Some(foreign_id.to_string().as_str()));
    assert!(!leaked, "another workspace's conn must not be listed");
}

#[tokio::test]
async fn non_admin_member_is_403() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "viewer", "ws_viewer").await;
    let base = format!("/ws/{}/storages", f.slug);

    // GET (list) → 403.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &base, &token, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non-admin member must not list storage conns"
    );

    // POST (register) → 403 (admin gate runs before reachability/body).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &base,
            &token,
            Some(serde_json::json!({
                "name": "x",
                "provider": "s3",
                "endpoint": "http://localhost:9000",
                "region": "us-east-1",
                "bucket": "warehouse",
                "access_key": "egret",
                "secret_key": "egretsecret",
                "path_style": true,
            })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non-admin member must not register storage conns"
    );

    // PUT (edit) → 403 (admin gate runs before body parsing, so a minimal body
    // against a random id must still be Forbidden, not 400/404).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("{base}/{}", Uuid::new_v4()),
            &token,
            Some(serde_json::json!({
                "name": "x",
                "provider": "s3",
                "endpoint": "http://localhost:9000",
                "region": "us-east-1",
                "bucket": "warehouse",
                "access_key": "egret",
                "path_style": true,
            })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non-admin member must not edit storage conns"
    );

    // DELETE → 403, before any store lookup.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("{base}/{}", Uuid::new_v4()),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non-admin member must not delete storage conns"
    );
}
