//! Session-authenticated catalog browse API tests (CE-2 T6), reading the
//! embedded **lakekeeper** catalog. DB + MinIO gated; self-skips without
//! `DATABASE_URL`.
//!
//! This is the lakekeeper-reading successor to
//! `egret-catalog/tests/browse_api.rs`. It proves the new browse router
//! (`egret_catalog_engine::browse_router`) lists the namespace/table that exist
//! IN LAKEKEEPER (created via the engine) and preserves the egret tenant
//! boundary:
//!   - a `ws_viewer` member of the workspace can list the namespace/table and
//!     read table detail (200),
//!   - an authenticated non-member is Forbidden (403),
//!   - a member of workspace X requesting workspace Y is Forbidden (403, IDOR).
//!
//! Seeding the lakekeeper data reuses the roundtrip harness's flow: provision a
//! per-workspace warehouse, then drive the bare engine router (with an injected
//! in-process principal) to create a namespace + table under that warehouse.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-catalog-engine --test browse_api

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
    db: Db,
    slug: String,
    workspace_id: WorkspaceId,
    owner: UserId,
    warehouse_id: Uuid,
    ns_name: String,
    table_name: String,
}

/// Mirror lakekeeper's authn layer for the bare engine router (used only to seed
/// data): inject an in-process `RequestMetadata` so the catalog handlers'
/// `Extension<_>` extractor succeeds.
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

    // egret migrations (workspaces, workspace_members, workspace_warehouse, roles).
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    // lakekeeper schema + default project.
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

    // Seed a workspace (real WorkspaceRepo row so `WsContext` resolves the slug),
    // then provision its lakekeeper warehouse (live MinIO validate_access).
    let suffix = Uuid::now_v7().simple().to_string();
    let owner = users
        .insert_local(&format!("t6owner-{suffix}@egret.test"), "T6 Owner", "x")
        .await
        .expect("seed owner");
    let slug = format!("t6ws{suffix}");
    let ws = WorkspaceRepo::new(db.clone())
        .create(&slug, "T6 Browse Test", owner)
        .await
        .expect("seed workspace");
    let workspace_id: WorkspaceId = ws.id;

    let warehouse_id = provision_workspace_warehouse(
        engine_state.clone(),
        &store,
        workspace_id.0,
        &slug,
        &dev_storage_config(),
        SYSTEM_USER,
    )
    .await
    .expect("provision warehouse (needs MinIO reachable + writable `warehouse` bucket)");

    // Seed a namespace + table IN LAKEKEEPER via the bare engine router, using the
    // warehouse UUID as the `{prefix}` (lakekeeper's default prefix_template).
    let seed_app = egret_catalog_engine::catalog_engine_router(engine_state.clone())
        .layer(axum::middleware::from_fn(inject_request_metadata));
    let prefix = warehouse_id.to_string();
    let ns_name = format!("demo_{}", Uuid::now_v7().simple());
    let table_name = "events".to_string();

    // POST /{prefix}/namespaces
    let body = serde_json::json!({ "namespace": [ns_name], "properties": {} });
    let resp = seed_app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri(format!("/{prefix}/namespaces"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, ns) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "seed namespace; body={ns}");

    // POST /{prefix}/namespaces/{ns}/tables
    let create_table = serde_json::json!({
        "name": table_name,
        "schema": {
            "type": "struct",
            "schema-id": 0,
            "fields": [
                { "id": 1, "name": "id", "required": true, "type": "long" }
            ]
        }
    });
    let resp = seed_app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri(format!("/{prefix}/namespaces/{ns_name}/tables"))
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&create_table).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let (status, created) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "seed table; body={created}");

    // The browse router under test.
    let crypto =
        egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secret_store = egret_secrets::SecretStore::new(db.clone(), Some(crypto));
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
        warehouse_id,
        ns_name,
        table_name,
    })
}

/// Create a user and mint a session token (the cookie value). Not a member.
async fn mint_session(f: &Fixture, email_tag: &str) -> String {
    let email = format!("{email_tag}_{}@egret.local", Uuid::new_v4().simple());
    let uid = f
        .users
        .insert_local(&email, "Test", "x")
        .await
        .expect("insert user");
    f.sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
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

/// Seed a second workspace Y (the fixture user is NOT a member). Returns its slug.
async fn seed_other_workspace(f: &Fixture) -> String {
    let suffix = Uuid::now_v7().simple().to_string();
    let owner = f
        .users
        .insert_local(&format!("t6y-{suffix}@egret.test"), "Y Owner", "x")
        .await
        .expect("seed Y owner");
    let slug = format!("t6y{suffix}");
    WorkspaceRepo::new(f.db.clone())
        .create(&slug, "T6 Other", owner)
        .await
        .expect("seed other workspace");
    slug
}

fn get(uri: &str, token: Option<&str>) -> HttpRequest<Body> {
    let mut b = HttpRequest::builder().uri(uri);
    if let Some(t) = token {
        b = b.header("Cookie", format!("egret_session={t}"));
    }
    b.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn viewer_browses_lakekeeper_namespace_table_and_detail() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "viewer", "ws_viewer").await;

    // summary → warehouse id/name.
    let resp = f
        .app
        .clone()
        .oneshot(get(&format!("/ws/{}/catalog", f.slug), Some(&token)))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "summary; body={json}");
    assert_eq!(
        json["id"],
        f.warehouse_id.to_string(),
        "id is warehouse uuid"
    );
    assert_eq!(json["name"], f.slug, "name is the warehouse (= slug)");
    assert!(
        json["warehouse_location"].as_str().is_some(),
        "warehouse_location present; body={json}"
    );

    // list namespaces → the seeded namespace appears.
    let resp = f
        .app
        .clone()
        .oneshot(get(
            &format!("/ws/{}/catalog/namespaces", f.slug),
            Some(&token),
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "namespaces; body={json}");
    let found = json
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n["levels"] == serde_json::json!([f.ns_name]));
    assert!(found, "seeded namespace listed from lakekeeper: {json}");

    // list tables in the namespace.
    let resp = f
        .app
        .clone()
        .oneshot(get(
            &format!("/ws/{}/catalog/namespaces/{}/tables", f.slug, f.ns_name),
            Some(&token),
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "tables; body={json}");
    assert_eq!(
        json["tables"],
        serde_json::json!([f.table_name]),
        "table listed from lakekeeper; body={json}"
    );
    assert_eq!(json["views"], serde_json::json!([]), "no views");

    // table detail DTO — shape parity with the old browse.
    let resp = f
        .app
        .clone()
        .oneshot(get(
            &format!(
                "/ws/{}/catalog/namespaces/{}/tables/{}",
                f.slug, f.ns_name, f.table_name
            ),
            Some(&token),
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "table detail; body={json}");
    assert_eq!(json["name"], f.table_name);
    assert_eq!(json["columns"][0]["name"], "id");
    assert_eq!(json["columns"][0]["type"], "long");
    assert_eq!(json["columns"][0]["required"], true);
    assert!(
        json["location"].as_str().is_some_and(|s| !s.is_empty()),
        "location present; body={json}"
    );
}

#[tokio::test]
async fn non_member_is_403() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = mint_session(&f, "stranger").await;
    let resp = f
        .app
        .clone()
        .oneshot(get(
            &format!("/ws/{}/catalog/namespaces", f.slug),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

/// Cross-workspace IDOR: a member of X requesting Y's catalog must be 403.
#[tokio::test]
async fn member_of_x_cannot_read_y_catalog_idor() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "x_member", "ws_viewer").await;
    let slug_y = seed_other_workspace(&f).await;

    // Sanity: the token reaches its own workspace X.
    let ok = f
        .app
        .clone()
        .oneshot(get(&format!("/ws/{}/catalog", f.slug), Some(&token)))
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK, "member reaches own workspace");

    // IDOR: X-member token against Y → 403.
    let resp = f
        .app
        .clone()
        .oneshot(get(&format!("/ws/{slug_y}/catalog"), Some(&token)))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a member of X must not read workspace Y's catalog"
    );
}
