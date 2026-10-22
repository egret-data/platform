//! DG #4b route C (spec §4.4, Q3, Q5, CL11, CL12):
//! `GET /api/control-plane/classifications/{id}/usage` — where a
//! classification is used, in Data and in BI, with a per-object BI read check
//! through the real `CedarPdp`.
//!
//! Fixtures are scoped to their own tags and workspaces. DB-gated; FAILS
//! without a database.

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
use egret_policy::PolicyStore;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

struct NoMvLister;
#[async_trait::async_trait]
impl MaterializedViewLister for NoMvLister {
    async fn list(&self, _engine: &ExposureEngine) -> Result<Vec<MvRow>, String> {
        panic!("never lists materialized views")
    }
}

struct Fixture {
    db: Db,
    users: UserRepo,
    sessions: SessionService,
    /// Seeds the service-defs; unused afterwards here.
    #[allow(dead_code)]
    policy: PolicyStore,
}

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..10])
}

async fn setup() -> Fixture {
    let dsn = egret_test_db::url()
        .await
        .expect("a database is required (DATABASE_URL)");
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_trino(&policy).await.expect("seed trino");
    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks");
    egret_policy::seed_tag(&policy).await.expect("seed tag");
    Fixture {
        users: UserRepo::new(db.clone()),
        sessions: SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        policy,
        db,
    }
}

/// The router with a REAL `CedarPdp`, built after every role exists.
async fn state(f: &Fixture) -> CpState {
    let crypto = egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").unwrap();
    let secrets = egret_secrets::WorkspaceSecretStore::new(
        f.db.clone(),
        egret_secrets::SecretStore::new(f.db.clone(), Some(crypto)),
    );
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(f.db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    CpState {
        storage: egret_catalog_engine::CatalogStore::new(f.db.clone()),
        storage_registry: egret_storage::StorageRegistry::new(f.db.clone()),
        users: f.users.clone(),
        groups: egret_auth::groups::GroupRepo::new(f.db.clone()),
        roles: egret_auth::roles::RoleRepo::new(f.db.clone(), egret_events::EventBus::new()),
        access_resolver: egret_auth::effective::AccessResolver::new(f.db.clone()),
        secrets,
        sessions: f.sessions.clone(),
        browser: None,
        pdp,
        audit: egret_audit::AuditSink::new(16).0,
        audit_store: egret_audit::AuditStore::new(f.db.clone()),
        tags: egret_bi::TagRepo::new(f.db.clone()),
        classification_policies: egret_bi::ClassificationPolicyRepo::new(
            f.db.clone(),
            egret_events::EventBus::new(),
        ),
        db: f.db.clone(),
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
    }
}

/// A session holding exactly `perms` (`["platform_admin"]` for an admin).
async fn session(f: &Fixture, perms: &[&str]) -> String {
    let uid = f
        .users
        .insert_local(&format!("{}@egret.local", unique("tgu")), "T", "x")
        .await
        .expect("user");
    if perms == ["platform_admin"] {
        sqlx::query(
            "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
             SELECT 'user', $1, id, 'platform' FROM roles \
              WHERE scope = 'platform' AND name = 'platform_admin'",
        )
        .bind(uid.0)
        .execute(&f.db.pool)
        .await
        .expect("platform_admin");
    } else if !perms.is_empty() {
        let role: Uuid = sqlx::query_scalar(
            "INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id",
        )
        .bind(unique("tgu_role"))
        .fetch_one(&f.db.pool)
        .await
        .expect("role");
        for p in perms {
            sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2)")
                .bind(role)
                .bind(p)
                .execute(&f.db.pool)
                .await
                .expect("permission");
        }
        sqlx::query(
            "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
             VALUES ('user', $1, $2, 'platform')",
        )
        .bind(uid.0)
        .bind(role)
        .execute(&f.db.pool)
        .await
        .expect("access grant");
    }
    f.sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .expect("session")
        .token
}

async fn get(st: &CpState, token: &str, uri: &str) -> (StatusCode, Value) {
    let req = HttpRequest::builder()
        .uri(uri)
        .header("Cookie", format!("egret_session={token}"))
        .body(Body::empty())
        .unwrap();
    let resp = control_plane_router()
        .with_state(st.clone())
        .oneshot(req)
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

// ── fixtures ─────────────────────────────────────────────────────────────

async fn tag(f: &Fixture) -> Uuid {
    sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $1) RETURNING id")
        .bind(unique("tgu-k"))
        .fetch_one(&f.db.pool)
        .await
        .unwrap()
}

async fn workspace(f: &Fixture) -> (Uuid, String) {
    let slug = unique("tgu-ws");
    let id = sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1, $1) RETURNING id")
        .bind(&slug)
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    (id, slug)
}

async fn dashboard(f: &Fixture, ws: Uuid, tag: Uuid) -> (Uuid, String) {
    let name = unique("dash");
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO analytics.dashboards (workspace_id, name) VALUES ($1, $2) RETURNING id",
    )
    .bind(ws)
    .bind(&name)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    assign(f, tag, "dashboard", id).await;
    (id, name)
}

async fn assign(f: &Fixture, tag: Uuid, kind: &str, id: Uuid) {
    sqlx::query("INSERT INTO tag_assignments (tag_id, object_type, object_id) VALUES ($1, $2, $3)")
        .bind(tag)
        .bind(kind)
        .bind(id)
        .execute(&f.db.pool)
        .await
        .unwrap();
}

/// A user holding `tag:read` on the platform, `ws_viewer` in `member_of`, and
/// the platform role `role` when given.
async fn member(f: &Fixture, member_of: &[Uuid], role: Option<&str>) -> String {
    let uid = f
        .users
        .insert_local(&format!("{}@egret.local", unique("tgum")), "T", "x")
        .await
        .unwrap();
    let reader: Uuid =
        sqlx::query_scalar("INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id")
            .bind(unique("tgu_reader"))
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, 'tag:read')")
        .bind(reader)
        .execute(&f.db.pool)
        .await
        .unwrap();
    let mut roles = vec![reader];
    if let Some(r) = role {
        roles.push(
            sqlx::query_scalar("SELECT id FROM roles WHERE scope = 'platform' AND name = $1")
                .bind(r)
                .fetch_one(&f.db.pool)
                .await
                .unwrap(),
        );
    }
    for r in roles {
        sqlx::query(
            "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
             VALUES ('user', $1, $2, 'platform')",
        )
        .bind(uid.0)
        .bind(r)
        .execute(&f.db.pool)
        .await
        .unwrap();
    }
    for ws in member_of {
        sqlx::query(
            "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
             SELECT 'user', $1, id, 'workspace', $2 FROM roles WHERE name = 'ws_viewer' LIMIT 1",
        )
        .bind(uid.0)
        .bind(ws)
        .execute(&f.db.pool)
        .await
        .unwrap();
    }
    f.sessions
        .create(uid, "test", "127.0.0.1")
        .await
        .unwrap()
        .token
}

async fn usage(st: &CpState, token: &str, tag: Uuid) -> (StatusCode, Value) {
    get(st, token, &format!("/classifications/{tag}/usage")).await
}

// ── BI ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_member_reads_its_workspaces_objects_and_counts_the_rest() {
    let f = setup().await;
    let t = tag(&f).await;
    let (w1, slug1) = workspace(&f).await;
    let (w2, _) = workspace(&f).await;
    let (d1, n1) = dashboard(&f, w1, t).await;
    dashboard(&f, w2, t).await;
    let token = member(&f, &[w1], None).await;
    let st = state(&f).await;
    let (status, body) = usage(&st, &token, t).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["bi"]["readable"],
        json!([{ "kind": "dashboard", "id": d1, "name": n1, "workspaceSlug": slug1 }])
    );
    // Q5: a count and nothing else.
    assert_eq!(body["bi"]["unreadable"], json!({ "count": 1 }));
    assert_eq!(body["bi"]["checked"], 2);
    assert_eq!(body["bi"]["remaining"], 0);
    assert_eq!(body["bi"]["deleted"], 0);
}

#[tokio::test]
async fn a_gated_object_is_read_only_with_clearance() {
    let f = setup().await;
    let t = tag(&f).await;
    let role = unique("tgu_cleared");
    sqlx::query("INSERT INTO roles (scope, name) VALUES ('platform', $1)")
        .bind(&role)
        .execute(&f.db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO classification_policies (tag_id, required_roles) VALUES ($1, $2)")
        .bind(t)
        .bind(vec![role.clone()])
        .execute(&f.db.pool)
        .await
        .unwrap();
    let (w1, _) = workspace(&f).await;
    let (d1, _) = dashboard(&f, w1, t).await;
    let uncleared = member(&f, &[w1], None).await;
    let cleared = member(&f, &[w1], Some(&role)).await;
    let st = state(&f).await;
    let (_, body) = usage(&st, &uncleared, t).await;
    assert_eq!(body["bi"]["readable"], json!([]));
    assert_eq!(body["bi"]["unreadable"]["count"], 1);
    let (_, body) = usage(&st, &cleared, t).await;
    assert_eq!(body["bi"]["readable"][0]["id"], json!(d1));
    assert_eq!(body["bi"]["unreadable"]["count"], 0);
}

#[tokio::test]
async fn checks_stop_at_500_and_say_how_many_remain() {
    let f = setup().await;
    let t = tag(&f).await;
    let (w1, _) = workspace(&f).await;
    sqlx::query(
        "WITH d AS (INSERT INTO analytics.dashboards (workspace_id, name) \
                    SELECT $1, 'cap-' || g FROM generate_series(1, 501) g RETURNING id) \
         INSERT INTO tag_assignments (tag_id, object_type, object_id) SELECT $2, 'dashboard', id FROM d",
    )
    .bind(w1)
    .bind(t)
    .execute(&f.db.pool)
    .await
    .unwrap();
    let admin = session(&f, &["platform_admin"]).await;
    let st = state(&f).await;
    let (status, body) = usage(&st, &admin, t).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["bi"]["checked"], 500);
    assert_eq!(body["bi"]["remaining"], 1);
    assert_eq!(body["bi"]["readable"].as_array().unwrap().len(), 500);
}

#[tokio::test]
async fn a_deleted_objects_assignment_is_counted() {
    let f = setup().await;
    let t = tag(&f).await;
    assign(&f, t, "dashboard", Uuid::new_v4()).await;
    let admin = session(&f, &["platform_admin"]).await;
    let st = state(&f).await;
    let (_, body) = usage(&st, &admin, t).await;
    assert_eq!(body["bi"]["deleted"], 1);
    assert_eq!(body["bi"]["checked"], 0);
    assert_eq!(body["bi"]["readable"], json!([]));
}

// ── Data ─────────────────────────────────────────────────────────────────

async fn catalog(f: &Fixture) -> (Uuid, String, Uuid) {
    let (ws, _) = workspace(f).await;
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','b','AK') RETURNING id",
    )
    .bind(unique("tgu-sc"))
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    let wh: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("tgu_wh"))
    .bind(ws)
    .bind(sc)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    let name = unique("tgu_cat");
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(&name)
    .bind(wh)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    (id, name, wh)
}

async fn data_tag(f: &Fixture, tag: Uuid, wh: Uuid, table: &str, column: &str) {
    sqlx::query(
        "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, column_name) \
         VALUES ($1, $2, 's', $3, $4)",
    )
    .bind(tag)
    .bind(wh)
    .bind(table)
    .bind(column)
    .execute(&f.db.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn data_is_grouped_by_table_and_orphans_are_counted() {
    let f = setup().await;
    let t = tag(&f).await;
    let (cat, name, wh) = catalog(&f).await;
    data_tag(&f, t, wh, "u", "b").await;
    data_tag(&f, t, wh, "t", "phone").await;
    data_tag(&f, t, wh, "t", "").await;
    data_tag(&f, t, wh, "t", "email").await;
    // A warehouse no catalog names (no foreign key, premise 1).
    data_tag(&f, t, Uuid::new_v4(), "x", "").await;
    let admin = session(&f, &["platform_admin"]).await;
    let st = state(&f).await;
    let (status, body) = usage(&st, &admin, t).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["data"]["tables"],
        json!([
            { "catalogId": cat, "catalogName": name, "warehouseId": wh, "schema": "s",
              "table": "t", "onTable": true, "columns": ["email", "phone"] },
            { "catalogId": cat, "catalogName": name, "warehouseId": wh, "schema": "s",
              "table": "u", "onTable": false, "columns": ["b"] }
        ])
    );
    assert_eq!(body["data"]["tableCount"], 1);
    assert_eq!(body["data"]["columnCount"], 3);
    assert_eq!(body["data"]["orphaned"], 1);
    assert_eq!(body["tag"]["id"], json!(t));
}

// ── permissions ──────────────────────────────────────────────────────────

#[tokio::test]
async fn tag_read_or_policy_read_and_a_known_tag() {
    let f = setup().await;
    let t = tag(&f).await;
    let nobody = session(&f, &[]).await;
    let tag_reader = session(&f, &["tag:read"]).await;
    let policy_reader = session(&f, &["policy:read"]).await;
    let st = state(&f).await;
    assert_eq!(usage(&st, &nobody, t).await.0, StatusCode::FORBIDDEN);
    assert_eq!(usage(&st, &tag_reader, t).await.0, StatusCode::OK);
    assert_eq!(usage(&st, &policy_reader, t).await.0, StatusCode::OK);
    assert_eq!(
        usage(&st, &tag_reader, Uuid::new_v4()).await.0,
        StatusCode::NOT_FOUND
    );
}
