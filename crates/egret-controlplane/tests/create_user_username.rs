//! A local user created through `POST /users` has an engine username from the
//! moment the row exists (spec `docs/en/specs/2026-10-31-local-user-username.md`).
//!
//! Before the fix the create flow left `users.username` NULL, answered
//! `"username": null`, and a grant naming the new user was refused 400 until
//! that user ran a first query (or a steward allocated the name from
//! Governance). Both halves run here through the real handlers, as a
//! principal holding only `user:create` and `policy:update` — the permissions
//! the two writes check — not as a platform admin.
//!
//! Its own test binary, so `egret-test-db` gives it a private database.
//! DB-gated; FAILS without a database unless `EGRET_ALLOW_DB_SKIP` is set.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_controlplane::{
    control_plane_router, CpState, ExposureEngine, MaterializedViewLister, MvRow,
    UnwiredCatalogLister,
};
use egret_metastore::Db;
use egret_perm::PermStore;
use egret_perm_compile::{perm_router, PermState};
use egret_policy::PolicyStore;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

struct NoMvLister;
#[async_trait::async_trait]
impl MaterializedViewLister for NoMvLister {
    async fn list(&self, _engine: &ExposureEngine) -> Result<Vec<MvRow>, String> {
        panic!("no route here lists materialized views")
    }
}

fn tag() -> String {
    Uuid::new_v4().simple().to_string()[..12].to_string()
}

fn sessions(db: &Db) -> SessionService {
    SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    )
}

struct Env {
    db: Db,
    app: axum::Router,
    /// A session holding exactly `user:create` and `policy:update`.
    steward: String,
    catalog_id: Uuid,
}

async fn env() -> Option<Env> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = &db.pool;

    let t = tag();
    let steward_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ('local', $1, 'Steward') RETURNING id",
    )
    .bind(format!("cu_steward_{t}@egret.local"))
    .fetch_one(pool)
    .await
    .expect("steward");
    let role_id: Uuid =
        sqlx::query_scalar("INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id")
            .bind(format!("cu_steward_role_{t}"))
            .fetch_one(pool)
            .await
            .expect("role");
    for permission in ["user:create", "policy:update"] {
        sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2)")
            .bind(role_id)
            .bind(permission)
            .execute(pool)
            .await
            .expect("permission");
    }
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
         VALUES ('user', $1, $2, 'platform')",
    )
    .bind(steward_id)
    .bind(role_id)
    .execute(pool)
    .await
    .expect("assign role");
    let steward = sessions(&db)
        .create(egret_core::UserId(steward_id), "test", "127.0.0.1")
        .await
        .expect("session")
        .token;

    // A grant needs a registry catalog; nothing here compiles.
    let workspace_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(format!("cu-ws-{t}"))
            .bind("CU WS")
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-cu-test','AKIATEST') RETURNING id",
    )
    .bind(format!("cu-sc-{t}"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(format!("cu_wh_{t}"))
    .bind(workspace_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(format!("cu_cat_{t}"))
    .bind(warehouse_id)
    .fetch_one(pool)
    .await
    .expect("registry catalog");

    // Built AFTER the steward's permissions are written: the PDP loads them once.
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let crypto = egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").unwrap();
    let secrets = egret_secrets::WorkspaceSecretStore::new(
        db.clone(),
        egret_secrets::SecretStore::new(db.clone(), Some(crypto)),
    );
    let cp = control_plane_router().with_state(CpState {
        storage: egret_catalog_engine::CatalogStore::new(db.clone()),
        storage_registry: egret_storage::StorageRegistry::new(db.clone()),
        users: UserRepo::new(db.clone()),
        groups: egret_auth::groups::GroupRepo::new(db.clone()),
        roles: egret_auth::roles::RoleRepo::new(db.clone(), egret_events::EventBus::new()),
        access_resolver: egret_auth::effective::AccessResolver::new(db.clone()),
        secrets,
        sessions: sessions(&db),
        browser: None,
        pdp: pdp.clone(),
        audit: egret_audit::AuditSink::new(16).0,
        audit_store: egret_audit::AuditStore::new(db.clone()),
        tags: egret_bi::TagRepo::new(db.clone()),
        classification_policies: egret_bi::ClassificationPolicyRepo::new(
            db.clone(),
            egret_events::EventBus::new(),
        ),
        db: db.clone(),
        mv_lister: Arc::new(NoMvLister),
        mv_list_timeout: std::time::Duration::from_secs(15),
        lineage_sweeper: Arc::new(egret_catalog_engine::NoCatalogSweeper),
        view_catalog: Arc::new(egret_catalog_engine::NoViewCatalog),
        view_audit_timeout: std::time::Duration::from_secs(15),
        catalog_lister: Arc::new(UnwiredCatalogLister),
        browse_timeout: std::time::Duration::from_secs(15),
        audit_retention_days: Some(90),
        storage_scope: egret_controlplane::StorageScope::s3(
            egret_controlplane::SystemPrefixes::default(),
        ),
    });
    let perm = perm_router().with_state(PermState {
        opa_cache: None,
        perm: PermStore::new(pool.clone()),
        policy: PolicyStore::new(db.clone()),
        sessions: sessions(&db),
        pdp,
    });
    Some(Env {
        db,
        app: cp.merge(perm),
        steward,
        catalog_id,
    })
}

async fn send(e: &Env, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let resp = e
        .app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method(method)
                .uri(uri)
                .header("Cookie", format!("egret_session={}", e.steward))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() }));
    (status, json)
}

#[tokio::test]
async fn a_created_local_user_has_a_username_and_a_grant_can_name_them_at_once() {
    let Some(e) = env().await else { return };
    let t = tag();

    let (status, body) = send(
        &e,
        "POST",
        "/users",
        json!({ "email": format!("cu_new_{t}@egret.local"), "name": "New", "password": "Passw0rd!long" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    // The name the one mint path derives: the email local part, already a slug.
    let expected = format!("cu_new_{t}");
    assert_eq!(body["username"], json!(expected), "{body}");
    let id: Uuid = body["id"].as_str().unwrap().parse().unwrap();
    let stored: Option<String> =
        sqlx::query_scalar("SELECT username::text FROM users WHERE id = $1")
            .bind(id)
            .fetch_one(&e.db.pool)
            .await
            .unwrap();
    assert_eq!(stored.as_deref(), Some(&*expected));
    // Written through the ledger (migration 0209's trigger), as every mint is.
    let ledgered: Option<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM username_ledger WHERE username = $1::citext")
            .bind(&expected)
            .fetch_optional(&e.db.pool)
            .await
            .unwrap()
            .flatten();
    assert_eq!(ledgered, Some(id));

    let (status, body) = send(
        &e,
        "POST",
        "/perm/grants",
        json!({
            "effect": "allow",
            "actions": ["read"],
            "scope": { "catalog_id": e.catalog_id, "schema": "sales", "table": "orders", "column": null },
            "principals": { "include": [{ "kind": "user", "ref": expected }], "exclude": [] }
        }),
    )
    .await;
    assert!(
        status.is_success(),
        "a grant naming the new user: {status} {body}"
    );
}

#[tokio::test]
async fn a_second_user_whose_email_slugs_to_a_taken_name_gets_the_next_one() {
    let Some(e) = env().await else { return };
    let t = tag();
    let (status, first) = send(
        &e,
        "POST",
        "/users",
        json!({ "email": format!("cu_dup_{t}@a.local"), "name": "A", "password": "Passw0rd!long" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    let (status, second) = send(
        &e,
        "POST",
        "/users",
        json!({ "email": format!("cu_dup_{t}@b.local"), "name": "B", "password": "Passw0rd!long" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{second}");
    assert_eq!(first["username"], json!(format!("cu_dup_{t}")));
    assert_eq!(second["username"], json!(format!("cu_dup_{t}-2")));
}
