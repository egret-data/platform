//! Workspace-admin per-warehouse HTTP route tests (Catalog sub-epic 1, Task 2).
//! DB-gated AND MinIO-gated; self-skips without `DATABASE_URL`.
//!
//! These prove the per-warehouse management routes added to the browse router
//! (`egret_catalog_engine::browse_router`). A workspace now owns N user-managed
//! warehouses, each created on a registered storage connection with its OWN
//! lakekeeper warehouse (a real S3 `validate_access` at create time):
//!   - a `ws_admin` member (holds `workspace:manage_members`) can create a
//!     warehouse on a registered, reachable storage connection → 201, and the
//!     create drives a LIVE lakekeeper `create_warehouse`,
//!   - the new warehouse's `{wid}/namespaces` route lists (empty) namespaces 200,
//!   - a cross-workspace warehouse-row id `{wid}` is 404 (IDOR closed),
//!   - a non-admin member (`ws_viewer`) is Forbidden (403),
//!   - creating against an UNREACHABLE storage connection is 400 (and persists
//!     no warehouse row),
//!   - deleting drives the lakekeeper `delete_warehouse` then drops the row → 204.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-catalog-engine --test warehouse_api
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
    secret_store: egret_secrets::SecretStore,
    sessions: SessionService,
    users: UserRepo,
    members: MemberRepo,
    db: Db,
    slug: String,
    workspace_id: WorkspaceId,
    owner: UserId,
    /// A registered, reachable storage connection in the fixture workspace.
    conn_id: Uuid,
}

async fn into_json(resp: Response) -> (StatusCode, Vec<u8>, serde_json::Value) {
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(
        |_| serde_json::json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }),
    );
    (status, bytes.to_vec(), json)
}

/// Register a storage connection in `ws` whose named secret resolves to the dev
/// MinIO credentials; returns the conn id. `key_prefix` namespaces the conn root.
async fn seed_reachable_conn(
    store: &CatalogStore,
    ws_secrets: &egret_secrets::WorkspaceSecretStore,
    ws: Uuid,
    name: &str,
    secret_name: &str,
) -> Uuid {
    let secret_ref = ws_secrets
        .create(ws, secret_name, None, "egretsecret")
        .await
        .expect("mint named secret");
    let input = StorageConnInput {
        name: name.to_string(),
        purpose: None,
        provider: "s3".to_string(),
        endpoint: Some("http://localhost:9000".to_string()),
        region: "local-01".to_string(),
        bucket: "warehouse".to_string(),
        key_prefix: Some(format!("conns/{name}")),
        access_key: "egret".to_string(),
        secret_ref: Some(secret_ref),
        path_style: true,
    };
    store
        .create_storage_conn(ws, &input)
        .await
        .expect("seed storage conn")
        .id
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
    let ws_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secret_store.clone());

    let suffix = Uuid::now_v7().simple().to_string();
    let owner = users
        .insert_local(&format!("whowner-{suffix}@egret.test"), "Wh Owner", "x")
        .await
        .expect("seed owner");
    let slug = format!("whws{suffix}");
    let ws = WorkspaceRepo::new(db.clone())
        .create(&slug, "Warehouse Admin Test", owner)
        .await
        .expect("seed workspace");
    let workspace_id: WorkspaceId = ws.id;

    let conn_id = seed_reachable_conn(
        &store,
        &ws_secrets,
        workspace_id.0,
        "minio-main",
        "minio-creds",
    )
    .await;

    let app = browse_router().with_state(BrowseState {
        engine: engine_state,
        store: store.clone(),
        workspace_secrets: ws_secrets,
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
        conn_id,
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

/// Seed a second workspace Y with its own warehouse ROW, returning the egret
/// warehouse-row id (to attack across the tenant boundary). The lakekeeper UUID
/// is arbitrary — the routes that 404 on a cross-workspace `{wid}` never reach
/// lakekeeper for it.
async fn seed_other_workspace_warehouse(f: &Fixture) -> Uuid {
    let suffix = Uuid::now_v7().simple().to_string();
    let owner = f
        .users
        .insert_local(&format!("why-{suffix}@egret.test"), "Y Owner", "x")
        .await
        .expect("seed Y owner");
    let slug = format!("why{suffix}");
    let ws = WorkspaceRepo::new(f.db.clone())
        .create(&slug, "Warehouse Other", owner)
        .await
        .expect("seed other workspace");
    f.store
        .insert_warehouse(ws.id.0, Uuid::now_v7(), "default", None)
        .await
        .expect("seed Y warehouse row")
        .id
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
async fn admin_creates_warehouse_lists_namespaces_and_deletes() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "admin", "ws_admin").await;
    let base = format!("/ws/{}/catalog/warehouses", f.slug);

    // CREATE on the seeded, reachable storage connection → 201, live lakekeeper
    // create_warehouse (validate_access against MinIO).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &base,
            &token,
            Some(serde_json::json!({
                "name": "analytics",
                "storage_conn_id": f.conn_id,
            })),
        ))
        .await
        .unwrap();
    let (status, _bytes, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create warehouse; body={json}");
    assert_eq!(json["name"], "analytics", "name echoed; body={json}");
    assert_eq!(
        json["storage_conn_id"],
        f.conn_id.to_string(),
        "storage_conn_id echoed; body={json}"
    );
    let wid = json["id"].as_str().expect("wid in body").to_string();

    // The egret row maps to a REAL lakekeeper warehouse id.
    let lk_id = f
        .store
        .lakekeeper_id(f.workspace_id.0, wid.parse().unwrap())
        .await
        .expect("warehouse row resolves to a lakekeeper id");
    assert_ne!(lk_id, Uuid::nil(), "lakekeeper warehouse id is non-nil");

    // LIST — the warehouse appears with the resolved conn name.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", &base, &token, None))
        .await
        .unwrap();
    let (status, _bytes, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list warehouses; body={json}");
    let row = json
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"].as_str() == Some(wid.as_str()))
        .expect("created warehouse listed");
    assert_eq!(row["storage_conn_name"], "minio-main", "conn name resolved");

    // NAMESPACES on the new warehouse → 200, empty (a fresh warehouse).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("{base}/{wid}/namespaces"),
            &token,
            None,
        ))
        .await
        .unwrap();
    let (status, _bytes, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "namespaces; body={json}");
    assert_eq!(
        json,
        serde_json::json!([]),
        "a fresh warehouse has no namespaces; body={json}"
    );

    // DELETE → drives lakekeeper delete_warehouse then drops the row → 204.
    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &format!("{base}/{wid}"), &token, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "delete → 204");

    // Gone from the list.
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
        .any(|w| w["id"].as_str() == Some(wid.as_str()));
    assert!(!still, "deleted warehouse no longer listed; body={json}");
}

#[tokio::test]
async fn cross_workspace_wid_namespaces_is_404() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "admin", "ws_admin").await;
    let foreign_wid = seed_other_workspace_warehouse(&f).await;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!(
                "/ws/{}/catalog/warehouses/{}/namespaces",
                f.slug, foreign_wid
            ),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "another workspace's warehouse-row id must 404 (IDOR closed)"
    );
}

#[tokio::test]
async fn non_admin_member_is_403() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "viewer", "ws_viewer").await;
    let base = format!("/ws/{}/catalog/warehouses", f.slug);

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
        "a non-admin member must not list warehouses"
    );

    // POST (create) → 403 (admin gate runs before lakekeeper).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &base,
            &token,
            Some(serde_json::json!({
                "name": "x",
                "storage_conn_id": f.conn_id,
            })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non-admin member must not create warehouses"
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
        "a non-admin member must not delete warehouses"
    );
}

#[tokio::test]
async fn create_with_unreachable_storage_is_400() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "admin", "ws_admin").await;
    let base = format!("/ws/{}/catalog/warehouses", f.slug);

    // Register a connection pointing at an UNREACHABLE endpoint directly via the
    // store (bypassing the registry's reachability probe), so the FAILURE happens
    // at warehouse-create time (lakekeeper validate_access) → 400.
    let ws_secrets = egret_secrets::WorkspaceSecretStore::new(f.db.clone(), f.secret_store.clone());
    let secret_ref = ws_secrets
        .create(f.workspace_id.0, "dead-creds", None, "egretsecret")
        .await
        .expect("mint named secret");
    let bad_conn = f
        .store
        .create_storage_conn(
            f.workspace_id.0,
            &StorageConnInput {
                name: "dead".to_string(),
                purpose: None,
                provider: "s3".to_string(),
                // A port nothing listens on → connection refused at create time.
                endpoint: Some("http://localhost:9".to_string()),
                region: "local-01".to_string(),
                bucket: "warehouse".to_string(),
                key_prefix: Some("conns/dead".to_string()),
                access_key: "egret".to_string(),
                secret_ref: Some(secret_ref),
                path_style: true,
            },
        )
        .await
        .expect("seed unreachable conn")
        .id;

    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &base,
            &token,
            Some(serde_json::json!({
                "name": "doomed",
                "storage_conn_id": bad_conn,
            })),
        ))
        .await
        .unwrap();
    let (status, _bytes, json) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "unreachable storage fails lakekeeper validate_access → 400; body={json}"
    );

    // No warehouse row was persisted.
    let warehouses = f
        .store
        .list_warehouses(f.workspace_id.0)
        .await
        .expect("list warehouses");
    assert!(
        warehouses.iter().all(|w| w.name != "doomed"),
        "a failed create must not persist a warehouse row"
    );
}
