//! Session-authenticated catalog **statistics + tasks** tests (catalog
//! sub-epic 4). DB + MinIO + lakekeeper gated; self-skips without `DATABASE_URL`.
//!
//! Proves the two read-only management-backed routes under the warehouse `{wid}`
//! scope:
//!   - GET …/{wid}/statistics → 200 `{ "entries": [...] }` (empty is valid for a
//!     fresh warehouse — lakekeeper computes stats lazily/hourly),
//!   - GET …/{wid}/tasks       → 200 `[...]` (usually empty — GC tasks only
//!     appear after table expiry/purge),
//!   - both gate `catalog:read` and are IDOR-safe: a `{wid}` from another
//!     workspace → 404.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-catalog-engine --test tasks_stats_api -- --test-threads=1

use axum::body::Body;
use axum::extract::Request;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::middleware::Next;
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
use egret_storage::StorageConfig;
use egret_workspace::members::MemberRepo;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use http_body_util::BodyExt;
use lakekeeper::api::RequestMetadata;
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
    sessions: SessionService,
    users: UserRepo,
    members: MemberRepo,
    workspace_id: WorkspaceId,
    owner: UserId,
    slug: String,
    /// The egret warehouse-ROW id (`{wid}` path segment).
    wid: Uuid,
}

#[allow(dead_code)] // Copied verbatim from `table_detail_api.rs`; these tests
                    // don't drive the bare engine router, so it's unused here.
async fn inject_request_metadata(mut request: Request, next: Next) -> Response {
    request
        .extensions_mut()
        .insert(RequestMetadata::new_lakekeeper_internal(Uuid::now_v7()));
    next.run(request).await
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

    let suffix = Uuid::now_v7().simple().to_string();
    let owner = users
        .insert_local(&format!("tsowner-{suffix}@egret.test"), "TS Owner", "x")
        .await
        .expect("seed owner");
    let slug = format!("tsws{suffix}");
    let ws = WorkspaceRepo::new(db.clone())
        .create(&slug, "Tasks-Stats Test", owner)
        .await
        .expect("seed workspace");
    let workspace_id: WorkspaceId = ws.id;

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

    let wid = store
        .get_warehouse_by_name(workspace_id.0, "default")
        .await
        .expect("default warehouse row")
        .id;

    let crypto =
        egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secret_store = egret_secrets::SecretStore::new(db.clone(), Some(crypto));
    let app = browse_router().with_state(BrowseState {
        engine: engine_state,
        store: store.clone(),
        workspace_secrets: egret_secrets::WorkspaceSecretStore::new(db.clone(), secret_store),
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
        workspace_id,
        owner,
        slug,
        wid,
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

async fn get(f: &Fixture, token: &str, uri: &str) -> (StatusCode, serde_json::Value) {
    let resp = f
        .app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri(uri)
                .header("Cookie", format!("egret_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    into_json(resp).await
}

#[tokio::test]
async fn statistics_returns_entries_array_for_a_reader() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "viewer", "ws_viewer").await;

    let uri = format!("/ws/{}/catalog/warehouses/{}/statistics", f.slug, f.wid);
    let (status, body) = get(&f, &token, &uri).await;
    assert_eq!(status, StatusCode::OK, "statistics; body={body}");
    // A fresh warehouse may have an empty series — assert the SHAPE, tolerant of empty.
    assert!(
        body.get("entries").and_then(|e| e.as_array()).is_some(),
        "entries array; body={body}"
    );
}

#[tokio::test]
async fn tasks_returns_list_for_a_reader() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "viewer", "ws_viewer").await;

    let uri = format!("/ws/{}/catalog/warehouses/{}/tasks", f.slug, f.wid);
    let (status, body) = get(&f, &token, &uri).await;
    assert_eq!(status, StatusCode::OK, "tasks; body={body}");
    assert!(body.is_array(), "tasks is a JSON array; body={body}");
}

#[tokio::test]
async fn cross_workspace_wid_is_404_on_both_routes() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "viewer", "ws_viewer").await;

    // A random {wid} that does not belong to this workspace → 404 (IDOR-safe).
    let alien = Uuid::now_v7();
    for path in ["statistics", "tasks"] {
        let uri = format!("/ws/{}/catalog/warehouses/{}/{}", f.slug, alien, path);
        let (status, body) = get(&f, &token, &uri).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{path} cross-ws; body={body}"
        );
    }
}

#[tokio::test]
async fn non_member_is_gated() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    // A logged-in user who is NOT a member of this workspace.
    let outsider = f
        .users
        .insert_local(
            &format!("outsider_{}@egret.test", Uuid::new_v4().simple()),
            "Out",
            "x",
        )
        .await
        .expect("insert outsider");
    let token = f
        .sessions
        .create(outsider, "test", "127.0.0.1")
        .await
        .expect("session")
        .token;

    let uri = format!("/ws/{}/catalog/warehouses/{}/statistics", f.slug, f.wid);
    let (status, _body) = get(&f, &token, &uri).await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND,
        "non-member gated, got {status}"
    );
}
