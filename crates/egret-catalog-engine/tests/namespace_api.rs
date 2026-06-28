//! Workspace-admin namespace DDL HTTP route tests (Catalog sub-epic 2).
//! DB-gated AND MinIO/lakekeeper-gated; self-skips without `DATABASE_URL`.
//!
//! These prove the two namespace DDL routes added to the browse router
//! (`egret_catalog_engine::browse_router`): a workspace-admin can create (incl.
//! nested) namespaces and drop empty ones in a warehouse, calling lakekeeper's
//! `NamespaceService::create_namespace` / `drop_namespace`:
//!   - a `ws_admin` member (holds `workspace:manage_members`) can create a
//!     top-level namespace → 201, then a nested child → 201; both appear in the
//!     flattened namespace listing,
//!   - a duplicate create → 409 Conflict,
//!   - an invalid name (containing a dot) → 400,
//!   - dropping an empty leaf namespace → 204; it disappears from the listing,
//!   - dropping a NON-empty namespace (still has a child) → 409 Conflict,
//!   - a cross-workspace warehouse-row id `{wid}` → 404 (IDOR closed) for both
//!     create and drop,
//!   - a non-admin member (`ws_viewer`) is Forbidden (403) for both.
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   cargo test -p egret-catalog-engine --test namespace_api -- --test-threads=1
//! (requires the dev MinIO at localhost:9000, creds egret/egretsecret, bucket
//! `warehouse`, path_style, plus the embedded lakekeeper engine).

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

/// Register a storage connection in `ws` whose named secret resolves to the dev
/// MinIO credentials; returns the conn id.
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
        .insert_local(&format!("nsowner-{suffix}@egret.test"), "Ns Owner", "x")
        .await
        .expect("seed owner");
    let slug = format!("nsws{suffix}");
    let ws = WorkspaceRepo::new(db.clone())
        .create(&slug, "Namespace Admin Test", owner)
        .await
        .expect("seed workspace");
    let workspace_id: WorkspaceId = ws.id;

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

/// Seed a second workspace Y with its own warehouse ROW, returning the egret
/// warehouse-row id (to attack across the tenant boundary).
async fn seed_other_workspace_warehouse(f: &Fixture) -> Uuid {
    let suffix = Uuid::now_v7().simple().to_string();
    let owner = f
        .users
        .insert_local(&format!("nsy-{suffix}@egret.test"), "Y Owner", "x")
        .await
        .expect("seed Y owner");
    let slug = format!("nsy{suffix}");
    let ws = WorkspaceRepo::new(f.db.clone())
        .create(&slug, "Namespace Other", owner)
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

/// Create a real warehouse and return its egret warehouse-row id `{wid}`.
///
/// Each warehouse is created on a FRESHLY-seeded storage connection with a
/// UNIQUE `key_prefix`, and the warehouse name carries a per-call suffix. The
/// embedded lakekeeper shares one Postgres DB across runs and rejects a second
/// warehouse whose name or storage profile overlaps an existing one
/// (`CreateWarehouseStorageProfileOverlap`); a unique prefix + name avoids both,
/// so the suite is reproducible without a clean DB between runs.
async fn create_warehouse(f: &Fixture, token: &str, name_tag: &str) -> String {
    let suffix = Uuid::now_v7().simple().to_string();
    let conn_id = seed_reachable_conn(
        &f.store,
        &egret_secrets::WorkspaceSecretStore::new(
            f.db.clone(),
            egret_secrets::SecretStore::new(
                f.db.clone(),
                Some(
                    egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef")
                        .expect("crypto"),
                ),
            ),
        ),
        f.workspace_id.0,
        &format!("{name_tag}-{suffix}"),
        &format!("creds-{name_tag}-{suffix}"),
    )
    .await;

    let base = format!("/ws/{}/catalog/warehouses", f.slug);
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &base,
            token,
            Some(serde_json::json!({
                "name": format!("{name_tag}-{suffix}"),
                "storage_conn_id": conn_id,
            })),
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create warehouse; body={json}");
    json["id"].as_str().expect("wid in body").to_string()
}

/// Fetch the flattened list of namespaces, each as a dotted string.
async fn list_namespaces(f: &Fixture, token: &str, wid: &str) -> Vec<String> {
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/ws/{}/catalog/warehouses/{wid}/namespaces", f.slug),
            token,
            None,
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list namespaces; body={json}");
    json.as_array()
        .unwrap()
        .iter()
        .map(|ns| {
            ns["levels"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| l.as_str().unwrap())
                .collect::<Vec<_>>()
                .join(".")
        })
        .collect()
}

#[tokio::test]
async fn admin_creates_nested_drops_namespaces() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "admin", "ws_admin").await;
    let wid = create_warehouse(&f, &token, "ns-analytics").await;
    let ns_base = format!("/ws/{}/catalog/warehouses/{wid}/namespaces", f.slug);

    // CREATE top-level "sales" → 201.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &ns_base,
            &token,
            Some(serde_json::json!({ "parent": null, "name": "sales" })),
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create sales; body={json}");
    assert_eq!(
        json["levels"],
        serde_json::json!(["sales"]),
        "levels echoed"
    );
    assert!(
        list_namespaces(&f, &token, &wid)
            .await
            .contains(&"sales".to_string()),
        "sales listed"
    );

    // CREATE nested "sales.y2024" → 201.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &ns_base,
            &token,
            Some(serde_json::json!({ "parent": "sales", "name": "y2024" })),
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "create sales.y2024; body={json}"
    );
    assert_eq!(
        json["levels"],
        serde_json::json!(["sales", "y2024"]),
        "nested levels echoed"
    );
    assert!(
        list_namespaces(&f, &token, &wid)
            .await
            .contains(&"sales.y2024".to_string()),
        "sales.y2024 listed"
    );

    // DUPLICATE create → 409.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &ns_base,
            &token,
            Some(serde_json::json!({ "parent": null, "name": "sales" })),
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "dup namespace → 409; body={json}"
    );

    // INVALID name (dot inside) → 400.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &ns_base,
            &token,
            Some(serde_json::json!({ "parent": null, "name": "a.b" })),
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "dot in name → 400; body={json}"
    );

    // DROP non-empty parent "sales" (still has child y2024) → 409.
    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &format!("{ns_base}/sales"), &token, None))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "drop non-empty namespace → 409; body={json}"
    );

    // DROP empty leaf "sales.y2024" → 204.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("{ns_base}/sales.y2024"),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "drop empty leaf → 204"
    );
    assert!(
        !list_namespaces(&f, &token, &wid)
            .await
            .contains(&"sales.y2024".to_string()),
        "sales.y2024 gone after drop"
    );

    // Now "sales" is empty → drop → 204.
    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &format!("{ns_base}/sales"), &token, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "drop now-empty sales → 204"
    );
    assert!(
        !list_namespaces(&f, &token, &wid)
            .await
            .contains(&"sales".to_string()),
        "sales gone after drop"
    );
}

#[tokio::test]
async fn cross_workspace_wid_ddl_is_404() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = add_member(&f, "admin", "ws_admin").await;
    let foreign_wid = seed_other_workspace_warehouse(&f).await;
    let ns_base = format!(
        "/ws/{}/catalog/warehouses/{}/namespaces",
        f.slug, foreign_wid
    );

    // CREATE against a cross-workspace warehouse-row id → 404.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &ns_base,
            &token,
            Some(serde_json::json!({ "parent": null, "name": "x" })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "cross-workspace create must 404 (IDOR closed)"
    );

    // DROP against a cross-workspace warehouse-row id → 404.
    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &format!("{ns_base}/x"), &token, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "cross-workspace drop must 404 (IDOR closed)"
    );
}

#[tokio::test]
async fn non_admin_member_ddl_is_403() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let viewer = add_member(&f, "viewer", "ws_viewer").await;
    // A real warehouse for the path to resolve (made by an admin).
    let admin = add_member(&f, "admin", "ws_admin").await;
    let wid = create_warehouse(&f, &admin, "ns-guarded").await;
    let ns_base = format!("/ws/{}/catalog/warehouses/{wid}/namespaces", f.slug);

    // CREATE → 403 (admin gate runs before lakekeeper).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &ns_base,
            &viewer,
            Some(serde_json::json!({ "parent": null, "name": "nope" })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non-admin member must not create namespaces"
    );

    // DROP → 403.
    let resp = f
        .app
        .clone()
        .oneshot(req("DELETE", &format!("{ns_base}/nope"), &viewer, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non-admin member must not drop namespaces"
    );
}
