//! Workspace-admin named-secret HTTP route tests (Epic B). DB-gated AND, for the
//! storage-conn used-by/InUse path, MinIO-gated; self-skips without `DATABASE_URL`.
//!
//! These prove the `/ws/{ws}/secrets` routes added to the browse router
//! (`egret_catalog_engine::browse_router`) for the per-workspace named-secret
//! registry (`workspace_secrets`):
//!   - a `ws_admin` member (holds `workspace:manage_members`) can create, list,
//!     rotate (PUT value), and delete named secrets,
//!   - create returns the meta WITHOUT the value; list shows `used_by`,
//!   - a non-admin member (`ws_viewer`) is Forbidden (403),
//!   - fetching another workspace's secret by id is a 404 (cross-tenant IDOR),
//!   - a secret referenced by a registered storage connection has `used_by = 1`
//!     and DELETE is rejected 409 (InUse); an unreferenced secret deletes (204).
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-catalog-engine --test secrets_api -- --test-threads=1

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
        .insert_local(&format!("secowner-{suffix}@egret.test"), "Sec Owner", "x")
        .await
        .expect("seed owner");
    let slug = format!("secws{suffix}");
    let ws = WorkspaceRepo::new(db.clone())
        .create(&slug, "Secrets Admin Test", owner)
        .await
        .expect("seed workspace");
    let workspace_id: WorkspaceId = ws.id;

    let app = browse_router().with_state(BrowseState {
        engine: engine_state,
        store: store.clone(),
        secret_store: secret_store.clone(),
        workspace_secrets: egret_secrets::WorkspaceSecretStore::new(db.clone(), secret_store),
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

/// Seed a second workspace Y and return its (slug, workspace_id, owner-token).
async fn seed_other_workspace(f: &Fixture) -> (String, WorkspaceId, String) {
    let suffix = Uuid::now_v7().simple().to_string();
    let owner = f
        .users
        .insert_local(&format!("secy-{suffix}@egret.test"), "Y Owner", "x")
        .await
        .expect("seed Y owner");
    let slug = format!("secy{suffix}");
    let ws = WorkspaceRepo::new(f.db.clone())
        .create(&slug, "Secrets Other", owner)
        .await
        .expect("seed other workspace");
    // A ws_admin in Y to legitimately read Y's own secrets path.
    f.members
        .add(ws.id, "user", owner.0, "ws_admin", owner)
        .await
        .expect("seed Y admin");
    let token = f
        .sessions
        .create(owner, "test", "127.0.0.1")
        .await
        .expect("session")
        .token;
    (slug, ws.id, token)
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
async fn admin_can_create_list_rotate_and_delete_secrets() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "admin", "ws_admin").await;
    let base = format!("/ws/{}/secrets", f.slug);

    // CREATE → 201 with the meta, NO value echoed.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &base,
            &token,
            Some(serde_json::json!({
                "name": "minio-creds",
                "description": "object store key",
                "value": "supersecret",
            })),
        ))
        .await
        .unwrap();
    let (status, bytes, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create; body={json}");
    let id = json["id"].as_str().expect("id in body").to_string();
    assert_eq!(json["name"], serde_json::json!("minio-creds"));
    assert_eq!(json["used_by"], serde_json::json!(0));
    let raw = String::from_utf8_lossy(&bytes);
    assert!(
        !raw.contains("supersecret"),
        "the value must never be echoed; body={raw}"
    );
    assert!(!raw.contains("\"value\""), "no value field; body={raw}");

    // LIST shows it with used_by = 0.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &base, &token, None))
        .await
        .unwrap();
    let (status, _bytes, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list; body={json}");
    let entry = json
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"].as_str() == Some(id.as_str()))
        .expect("created secret listed");
    assert_eq!(entry["used_by"], serde_json::json!(0));

    // PUT rotate the value → 200, still no value echoed.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("{base}/{id}"),
            &token,
            Some(serde_json::json!({ "value": "rotated" })),
        ))
        .await
        .unwrap();
    let (status, bytes, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "rotate; body={json}");
    assert!(
        !String::from_utf8_lossy(&bytes).contains("rotated"),
        "rotated value must not be echoed"
    );

    // DELETE the unused secret → 204.
    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &format!("{base}/{id}"), &token, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "delete unused → 204");
}

#[tokio::test]
async fn cross_workspace_get_is_404() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "admin", "ws_admin").await;
    let (slug_b, _ws_b, token_b) = seed_other_workspace(&f).await;
    let base = format!("/ws/{}/secrets", f.slug);

    // Create a secret in workspace A.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &base,
            &token,
            Some(serde_json::json!({ "name": "a-secret", "value": "v" })),
        ))
        .await
        .unwrap();
    let (status, _b, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create in A; body={json}");
    let id = json["id"].as_str().unwrap().to_string();

    // Reading A's secret id through B's path (B's own admin) → 404 (IDOR closed).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "PUT",
            &format!("/ws/{slug_b}/secrets/{id}"),
            &token_b,
            Some(serde_json::json!({ "value": "x" })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "another workspace's secret must 404 through B's path"
    );

    // And it must not appear in B's list.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &format!("/ws/{slug_b}/secrets"), &token_b, None))
        .await
        .unwrap();
    let (status, _b, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK);
    let leaked = json
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["id"].as_str() == Some(id.as_str()));
    assert!(!leaked, "A's secret must not be listed for B");
}

#[tokio::test]
async fn non_admin_member_is_403() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "viewer", "ws_viewer").await;
    let base = format!("/ws/{}/secrets", f.slug);

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
        "a non-admin must not list secrets"
    );

    // POST (create) → 403.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &base,
            &token,
            Some(serde_json::json!({ "name": "x", "value": "y" })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non-admin must not create secrets"
    );

    // DELETE → 403.
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
        "a non-admin must not delete secrets"
    );
}

#[tokio::test]
async fn referenced_secret_delete_is_409_then_unused_deletes() {
    // MinIO-gated: registering the storage conn runs a live reachability probe.
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "admin", "ws_admin").await;
    let secrets_base = format!("/ws/{}/secrets", f.slug);
    let storages_base = format!("/ws/{}/storages", f.slug);

    // Create a named secret holding the real MinIO key.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &secrets_base,
            &token,
            Some(serde_json::json!({ "name": "minio-key", "value": "egretsecret" })),
        ))
        .await
        .unwrap();
    let (status, _b, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create secret; body={json}");
    let secret_id = json["id"].as_str().unwrap().to_string();

    // Register a storage connection referencing that named secret by id.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &storages_base,
            &token,
            Some(serde_json::json!({
                "name": "minio-main",
                "provider": "s3",
                "endpoint": "http://localhost:9000",
                "region": "us-east-1",
                "bucket": "warehouse",
                "key_prefix": "conns/sec",
                "access_key": "egret",
                "secret_id": secret_id,
                "path_style": true,
            })),
        ))
        .await
        .unwrap();
    let (status, _b, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "register conn; body={json}");

    // The secret now reports used_by = 1.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &secrets_base, &token, None))
        .await
        .unwrap();
    let (_status, _b, json) = into_json(resp).await;
    let entry = json
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"].as_str() == Some(secret_id.as_str()))
        .expect("secret listed");
    assert_eq!(
        entry["used_by"],
        serde_json::json!(1),
        "referenced secret must report used_by=1; body={json}"
    );

    // DELETE the referenced secret → 409 InUse.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("{secrets_base}/{secret_id}"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, _b, json) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "deleting a referenced secret must be 409; body={json}"
    );
    assert!(
        json["error"]
            .as_str()
            .map(|m| m.contains("in use"))
            .unwrap_or(false),
        "409 body must say it is in use; body={json}"
    );

    // Create a SECOND, unreferenced secret and delete it → 204.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &secrets_base,
            &token,
            Some(serde_json::json!({ "name": "unused", "value": "v" })),
        ))
        .await
        .unwrap();
    let (_status, _b, json) = into_json(resp).await;
    let unused_id = json["id"].as_str().unwrap().to_string();
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("{secrets_base}/{unused_id}"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "deleting an unused secret must be 204"
    );
}

#[tokio::test]
async fn failed_storage_register_with_new_secret_creates_no_orphan() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "admin", "ws_admin").await;
    let secrets_base = format!("/ws/{}/secrets", f.slug);
    let storages_base = format!("/ws/{}/storages", f.slug);

    // Baseline: no named secrets in the workspace yet.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &secrets_base, &token, None))
        .await
        .unwrap();
    let (_status, _b, json) = into_json(resp).await;
    let before = json.as_array().unwrap().len();

    // Register a storage connection with an UNREACHABLE endpoint plus an inline
    // `new_secret`. The reachability probe must fail → 400, and because the named
    // secret is minted only AFTER a successful probe, NOTHING is created.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &storages_base,
            &token,
            Some(serde_json::json!({
                "name": "orphan-test",
                "provider": "s3",
                "endpoint": "http://127.0.0.1:1",
                "region": "us-east-1",
                "bucket": "warehouse",
                "key_prefix": "conns/orphan",
                "access_key": "egret",
                "new_secret": { "name": "would-orphan", "value": "egretsecret" },
                "path_style": true,
            })),
        ))
        .await
        .unwrap();
    let (status, _b, json) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an unreachable endpoint must fail the probe → 400; body={json}"
    );

    // No new named secret was created — the list count is unchanged and the would-be
    // orphan name is absent.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &secrets_base, &token, None))
        .await
        .unwrap();
    let (_status, _b, json) = into_json(resp).await;
    let after = json.as_array().unwrap();
    assert_eq!(
        after.len(),
        before,
        "a failed storage register must not mint a named secret; body={json}"
    );
    assert!(
        after
            .iter()
            .all(|s| s["name"].as_str() != Some("would-orphan")),
        "the inline new_secret must not be persisted on probe failure; body={json}"
    );
}
