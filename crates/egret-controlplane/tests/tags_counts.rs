//! DG #4b (spec §4.4): `GET /api/control-plane/tags` carries each tag's Data
//! counts, `catalog_tables` and `catalog_columns`, additively.
//!
//! Fixtures are scoped to their own tags. DB-gated; FAILS
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

async fn send(
    st: &CpState,
    token: &str,
    method: &str,
    uri: &str,
    body: Value,
) -> (StatusCode, Value) {
    let req = HttpRequest::builder()
        .method(method)
        .uri(uri)
        .header("Cookie", format!("egret_session={token}"))
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
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

/// An egret catalog over a new warehouse; returns the warehouse id.
async fn catalog_warehouse(f: &Fixture) -> Uuid {
    let p = &f.db.pool;
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1, $1) RETURNING id")
            .bind(unique("tgc-ws"))
            .fetch_one(p)
            .await
            .unwrap();
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','b','AK') RETURNING id",
    )
    .bind(unique("tgc-sc"))
    .fetch_one(p)
    .await
    .unwrap();
    let wh: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("tgc_wh"))
    .bind(ws)
    .bind(sc)
    .fetch_one(p)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2)",
    )
    .bind(unique("tgc_cat"))
    .bind(wh)
    .execute(p)
    .await
    .unwrap();
    wh
}

/// Assignments in a warehouse no catalog names are not tables or columns of
/// any catalog: the index leaves them out, as Where used (route C) does, which
/// reports them separately as `orphaned` (CL11).
#[tokio::test]
async fn tags_carry_their_data_counts() {
    let f = setup().await;
    let p = &f.db.pool;
    let mk = |k: String| async move {
        sqlx::query_scalar::<_, Uuid>("INSERT INTO tags (key, name) VALUES ($1, $1) RETURNING id")
            .bind(k)
            .fetch_one(p)
            .await
            .unwrap()
    };
    let a = mk(unique("tgc-a")).await;
    let other = mk(unique("tgc-o")).await;
    let wh = catalog_warehouse(&f).await;
    let orphan = Uuid::new_v4();
    for (w, table, column) in [
        (wh, "t", ""),
        (wh, "u", "a"),
        (wh, "u", "b"),
        (orphan, "o", ""),
        (orphan, "o", "c"),
    ] {
        sqlx::query(
            "INSERT INTO catalog_tag_assignments (tag_id, warehouse_id, schema_name, table_name, column_name) \
             VALUES ($1, $2, 's', $3, $4)",
        )
        .bind(a)
        .bind(w)
        .bind(table)
        .bind(column)
        .execute(p)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO tag_assignments (tag_id, object_type, object_id) VALUES ($1, 'dashboard', $2)",
    )
    .bind(a)
    .bind(Uuid::new_v4())
    .execute(p)
    .await
    .unwrap();
    let reader = session(&f, &["tag:read"]).await;
    let st = state(&f).await;
    let (status, body) = get(&st, &reader, "/tags").await;
    assert_eq!(status, StatusCode::OK);
    let find = |id: Uuid| {
        body.as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == json!(id.to_string()))
            .cloned()
            .unwrap()
    };
    let ta = find(a);
    assert_eq!(
        (ta["catalog_tables"].clone(), ta["catalog_columns"].clone()),
        (json!(1), json!(2))
    );
    assert_eq!(ta["assignment_count"], 1);
    // The same numbers as Where used, which counts the orphaned rows apart.
    let (status, usage) = get(&st, &reader, &format!("/classifications/{a}/usage")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (
            usage["data"]["tableCount"].clone(),
            usage["data"]["columnCount"].clone(),
            usage["data"]["orphaned"].clone()
        ),
        (
            ta["catalog_tables"].clone(),
            ta["catalog_columns"].clone(),
            json!(2)
        )
    );
    let to = find(other);
    assert_eq!(
        (to["catalog_tables"].clone(), to["catalog_columns"].clone()),
        (json!(0), json!(0))
    );
    assert_eq!(to["assignment_count"], 0);
}

#[tokio::test]
async fn classification_writes_answer_stable_codes() {
    let f = setup().await;
    let st = state(&f).await;
    let token = session(&f, &["platform_admin"]).await;
    // Keys allow no `_`, which `unique` emits.
    let s = unique("cx").replace('_', "-");
    let (status, first) = send(
        &st,
        &token,
        "POST",
        "/tags",
        json!({"key": format!("pii-{s}"), "name": format!("PII {s}")}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{first}");

    // Same name, other case, other key: tag_name_taken naming the holder.
    let (status, body) = send(
        &st,
        &token,
        "POST",
        "/tags",
        json!({"key": format!("pii2-{s}"), "name": format!("pii {s}")}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "tag_name_taken");
    assert_eq!(body["existing"]["key"], format!("pii-{s}"));
    assert!(body["error"].as_str().unwrap().contains("already exists"));

    // Same key: tag_key_taken.
    let (status, body) = send(
        &st,
        &token,
        "POST",
        "/tags",
        json!({"key": format!("pii-{s}"), "name": format!("Other {s}")}),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("tag_key_taken")),
        "{body}"
    );

    // Bad key: tag_key_invalid.
    let (status, body) = send(
        &st,
        &token,
        "POST",
        "/tags",
        json!({"key": "bad_key", "name": format!("Bad {s}")}),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("tag_key_invalid")),
        "{body}"
    );

    // Rename another classification INTO the taken name: refused.
    let (_, second) = send(
        &st,
        &token,
        "POST",
        "/tags",
        json!({"key": format!("sec-{s}"), "name": format!("Secret {s}")}),
    )
    .await;
    let second_id = second["id"].as_str().unwrap();
    let (status, body) = send(
        &st,
        &token,
        "PATCH",
        &format!("/tags/{second_id}"),
        json!({"name": format!("PII {s}")}),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("tag_name_taken")),
        "{body}"
    );

    // Rename a classification to its own name in another case: allowed.
    let first_id = first["id"].as_str().unwrap();
    let (status, body) = send(
        &st,
        &token,
        "PATCH",
        &format!("/tags/{first_id}"),
        json!({"name": format!("PII {}", s.to_uppercase())}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn only_a_real_rename_is_checked_against_existing_names() {
    let f = setup().await;
    let st = state(&f).await;
    let token = session(&f, &["platform_admin"]).await;
    let s = unique("dup").replace('_', "-");
    let name = format!("PII {s}");
    // A pre-existing duplicate pair, inserted around the handler's check.
    let mut ids = Vec::new();
    for k in ["a", "b"] {
        let id: Uuid =
            sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
                .bind(format!("{k}-{s}"))
                .bind(&name)
                .fetch_one(&f.db.pool)
                .await
                .unwrap();
        ids.push(id);
    }
    let uri = format!("/tags/{}", ids[0]);

    // Its own unchanged name plus a colour change is not a rename.
    let (status, body) = send(
        &st,
        &token,
        "PATCH",
        &uri,
        json!({"name": name, "color": "#112233"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // A rename to a third, unique name is allowed.
    let (status, body) = send(
        &st,
        &token,
        "PATCH",
        &uri,
        json!({"name": format!("Third {s}")}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Back to a duplicate of B differing only in case: a rename, refused.
    let (status, body) = send(
        &st,
        &token,
        "PATCH",
        &uri,
        json!({"name": name.to_lowercase()}),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("tag_name_taken")),
        "{body}"
    );
}
