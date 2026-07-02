//! Control-plane Storage + Secrets HTTP route tests (platform-admin gate).
//! DB-gated; self-skips without `DATABASE_URL`.
//!
//! Proves `egret_controlplane::control_plane_router`:
//!   - a non-platform-admin session is Forbidden (403) on the global routes,
//!   - a `platform_admin` session can list storage (200), create a global named
//!     secret (201) and create a global storage connection (201).
//!
//! Run with:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret
//!   cargo test -p egret-controlplane

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use axum::response::Response;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_controlplane::{control_plane_router, CpState};
use egret_metastore::Db;
use http_body_util::BodyExt;
use tower::ServiceExt; // oneshot
use uuid::Uuid;

struct Fixture {
    app: axum::Router,
    users: UserRepo,
    sessions: SessionService,
    db: Db,
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

    let crypto =
        egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secrets = egret_secrets::SecretStore::new(db.clone(), Some(crypto));
    let workspace_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets);

    let users = UserRepo::new(db.clone());
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );

    let app = control_plane_router().with_state(CpState {
        storage: egret_catalog::CatalogStore::new(db.clone()),
        secrets: workspace_secrets,
        sessions: sessions.clone(),
        polaris_conn: egret_controlplane::PolarisConnStore::new(db.clone()),
        assignments: egret_controlplane::AssignmentStore::new(db.clone()),
    });

    Some(Fixture {
        app,
        users,
        sessions,
        db,
    })
}

/// Create a user (optionally a platform_admin) and return its session token.
async fn session_for(f: &Fixture, tag: &str, platform_admin: bool) -> String {
    let email = format!("{tag}_{}@egret.local", Uuid::new_v4().simple());
    let uid = f
        .users
        .insert_local(&email, "Test", "x")
        .await
        .expect("insert user");
    if platform_admin {
        sqlx::query(
            "INSERT INTO platform_role_assignments (principal_type, principal_id, role_id) \
             SELECT 'user', $1, id FROM roles WHERE scope='platform' AND name='platform_admin'",
        )
        .bind(uid.0)
        .execute(&f.db.pool)
        .await
        .expect("grant platform_admin");
    }
    f.sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("create session")
        .token
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
async fn non_admin_is_forbidden() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "plain", false).await;

    // GET /storage → 403.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/storage", &token, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non platform-admin must not list storage"
    );

    // POST /secrets → 403 (gate runs before persistence).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/secrets",
            &token,
            Some(serde_json::json!({ "name": "nope", "value": "x" })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non platform-admin must not create secrets"
    );

    // GET /polaris/catalogs → 403 (gate runs before any Polaris client is built).
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/polaris/catalogs", &token, None))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non platform-admin must not list catalogs"
    );

    // POST /polaris/principals → 403 (gate runs before any Polaris client is built).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/polaris/principals",
            &token,
            Some(serde_json::json!({ "name": "svc-nope" })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non platform-admin must not create principals"
    );

    // POST /workspaces/{ws}/catalog-assignments → 403 (gate runs before any
    // Polaris client is built or the catalog is provisioned).
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/workspaces/{}/catalog-assignments", Uuid::new_v4()),
            &token,
            Some(serde_json::json!({ "catalog": "lakehouse" })),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non platform-admin must not assign a catalog to a workspace"
    );
}

#[tokio::test]
async fn platform_admin_can_manage_storage_and_secrets() {
    let Some(f) = setup().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let token = session_for(&f, "admin", true).await;
    let suffix = Uuid::new_v4().simple().to_string();

    // GET /storage → 200.
    let resp = f
        .app
        .clone()
        .oneshot(req("GET", "/storage", &token, None))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::OK, "list storage; body={json}");
    assert!(json.is_array(), "storage list is an array; body={json}");

    // POST /secrets → 201, global named secret.
    let secret_name = format!("cp-secret-{suffix}");
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/secrets",
            &token,
            Some(serde_json::json!({ "name": secret_name, "value": "s3cr3t" })),
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create secret; body={json}");
    let secret_id = json["id"].as_str().expect("secret id in body").to_string();
    assert_eq!(json["name"], serde_json::json!(secret_name));

    // POST /storage → 201, global storage connection referencing the secret.
    let resp = f
        .app
        .clone()
        .oneshot(req(
            "POST",
            "/storage",
            &token,
            Some(serde_json::json!({
                "name": format!("cp-store-{suffix}"),
                "provider": "s3",
                "region": "us-east-1",
                "bucket": "egret-cp-test",
                "access_key": "AKIATEST",
                "secret_ref": secret_id,
                "path_style": true,
            })),
        ))
        .await
        .unwrap();
    let (status, json) = into_json(resp).await;
    assert_eq!(status, StatusCode::CREATED, "create storage; body={json}");
    assert_eq!(json["bucket"], serde_json::json!("egret-cp-test"));
    assert_eq!(json["secret_ref"], serde_json::json!(secret_id));
}

#[tokio::test]
async fn polaris_conn_store_upsert_and_get() {
    let Some(dsn) = std::env::var("DATABASE_URL").ok() else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    // Seed a GLOBAL named secret to satisfy the `kc_secret_ref` FK.
    let crypto =
        egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secrets = egret_secrets::SecretStore::new(db.clone(), Some(crypto));
    let workspace_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets);
    let suffix = Uuid::new_v4().simple().to_string();
    let secret_id = workspace_secrets
        .create(None, &format!("polaris-kc-{suffix}"), None, "kc-secret")
        .await
        .expect("create global secret");

    let store = egret_controlplane::PolarisConnStore::new(db.clone());

    // First upsert: inserts the singleton row.
    store
        .upsert(&egret_controlplane::PolarisConnInput {
            base_url: "http://polaris-1:8181".into(),
            kc_token_url: "http://kc/token".into(),
            kc_client_id: "polaris-admin".into(),
            kc_secret_ref: secret_id,
            kc_scope: Some("catalog".into()),
        })
        .await
        .expect("first upsert");
    let conn = store.get().await.expect("get").expect("row present");
    assert_eq!(conn.base_url, "http://polaris-1:8181");
    assert_eq!(conn.kc_secret_ref, secret_id);
    assert_eq!(conn.kc_scope.as_deref(), Some("catalog"));

    // Second upsert: updates in place (singleton — still exactly one row).
    store
        .upsert(&egret_controlplane::PolarisConnInput {
            base_url: "http://polaris-2:8181".into(),
            kc_token_url: "http://kc/token".into(),
            kc_client_id: "polaris-admin".into(),
            kc_secret_ref: secret_id,
            kc_scope: None,
        })
        .await
        .expect("second upsert");
    let conn = store.get().await.expect("get").expect("row present");
    assert_eq!(conn.base_url, "http://polaris-2:8181");
    assert_eq!(conn.kc_scope, None);

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM polaris_connection")
        .fetch_one(&db.pool)
        .await
        .expect("count");
    assert_eq!(count, 1, "polaris_connection is a singleton");
}

#[tokio::test]
async fn assignment_store_insert_list_get_delete() {
    let Some(dsn) = std::env::var("DATABASE_URL").ok() else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let suffix = Uuid::new_v4().simple().to_string();

    // Seed a workspace to satisfy the `workspace_id` FK.
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name) VALUES ($1, 'assign-test') RETURNING id",
    )
    .bind(format!("assign-{suffix}"))
    .fetch_one(&db.pool)
    .await
    .expect("seed workspace");

    // Seed a GLOBAL named secret to satisfy the `credential_ref` FK.
    let crypto =
        egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").expect("build test crypto");
    let secrets = egret_secrets::SecretStore::new(db.clone(), Some(crypto));
    let workspace_secrets = egret_secrets::WorkspaceSecretStore::new(db.clone(), secrets);
    let cred_ref = workspace_secrets
        .create(None, &format!("polaris/lakehouse/{suffix}"), None, "{}")
        .await
        .expect("create credential secret");

    let store = egret_controlplane::AssignmentStore::new(db.clone());
    let catalog = format!("lakehouse-{suffix}");

    // insert.
    let row = store
        .insert(ws, &catalog, "egret-ws-x", cred_ref)
        .await
        .expect("insert assignment");
    assert_eq!(row.workspace_id, ws);
    assert_eq!(row.catalog_name, catalog);
    assert_eq!(row.principal_name, "egret-ws-x");
    assert_eq!(row.credential_ref, cred_ref);

    // list_for_workspace returns it.
    let listed = store.list_for_workspace(ws).await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].catalog_name, catalog);

    // get returns it.
    let got = store.get(ws, &catalog).await.expect("get");
    assert!(got.is_some(), "get returns the assignment");

    // Duplicate insert (same ws+catalog) → AlreadyExists.
    let dup = store.insert(ws, &catalog, "egret-ws-x", cred_ref).await;
    assert!(
        matches!(dup, Err(egret_controlplane::CpError::AlreadyExists(_))),
        "duplicate (ws, catalog) must be a conflict, got {dup:?}"
    );

    // delete returns the deleted row; a second get is None.
    let deleted = store.delete(ws, &catalog).await.expect("delete");
    assert!(deleted.is_some(), "delete returns the removed row");
    assert_eq!(deleted.unwrap().catalog_name, catalog);
    let gone = store.get(ws, &catalog).await.expect("get after delete");
    assert!(gone.is_none(), "assignment is gone after delete");
}
