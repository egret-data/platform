//! DG #4b routes A, B and D over HTTP (spec §4.2, §4.3, §4.5): permission,
//! refusals and the response shape. What each route matches is tested in
//! `egret-perm-compile`'s `handwritten_*` suites; this binary pins the
//! handlers.
//!
//! Fixtures are scoped to their own ids. DB-gated; FAILS without a database.

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
        .insert_local(&format!("{}@egret.local", unique("hwr")), "T", "x")
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
        .bind(unique("hwr_role"))
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

/// A catalog attached to a Trino engine whose profile holds one hand-written
/// allow on `s.*.*`.
async fn catalog_with_policy(f: &Fixture) -> (Uuid, String) {
    let p = &f.db.pool;
    let ws: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$1) RETURNING id")
            .bind(unique("hwr-ws"))
            .fetch_one(p)
            .await
            .unwrap();
    let sc: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','b','AK') RETURNING id",
    )
    .bind(unique("hwr-sc"))
    .fetch_one(p)
    .await
    .unwrap();
    let wh: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("hwr_wh"))
    .bind(ws)
    .bind(sc)
    .fetch_one(p)
    .await
    .unwrap();
    let name = unique("hwr_cat");
    let cat: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(&name)
    .bind(wh)
    .fetch_one(p)
    .await
    .unwrap();
    let tag = unique("hwr-tag");
    f.policy
        .create_service(&tag, "tag", &json!({}))
        .await
        .unwrap();
    let prof = unique("hwr-tr");
    let sid = f
        .policy
        .create_service(&prof, "trino", &json!({ "tagService": tag }))
        .await
        .unwrap()
        .id;
    let eng: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1, 'trino', 'egret', '{}'::jsonb, $2) RETURNING id",
    )
    .bind(unique("hwr-eng"))
    .bind(sid)
    .fetch_one(p)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(eng)
    .bind(cat)
    .execute(p)
    .await
    .unwrap();
    let body = json!({
        "name": "hw-s", "service": prof, "policyType": 0, "isEnabled": true,
        "resources": { "catalog": { "values": [name] }, "schema": { "values": ["s"] },
                       "table": { "values": ["*"] }, "column": { "values": ["*"] } },
        "policyItems": [{ "users": ["ua"], "groups": ["g1"], "accesses": [{ "type": "select", "isAllowed": true }] }]
    });
    sqlx::query(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, managed_by) \
         VALUES ($1, 'hw-s', $2, 'operator')",
    )
    .bind(sid)
    .bind(body)
    .execute(p)
    .await
    .unwrap();
    (cat, prof)
}

// ── route A ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn route_a_needs_policy_read() {
    let f = setup().await;
    let (cat, _) = catalog_with_policy(&f).await;
    let tag_reader = session(&f, &["tag:read"]).await;
    let nobody = session(&f, &[]).await;
    let st = state(&f).await;
    for token in [&tag_reader, &nobody] {
        let (status, _) = get(
            &st,
            token,
            &format!("/governance/objects/{cat}/handwritten"),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
async fn route_a_answers_the_object_with_policy_read() {
    let f = setup().await;
    let (cat, prof) = catalog_with_policy(&f).await;
    let reader = session(&f, &["policy:read"]).await;
    let st = state(&f).await;
    let (status, body) = get(
        &st,
        &reader,
        &format!("/governance/objects/{cat}/handwritten?schema=s&table=t"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["catalog"]["id"], json!(cat));
    assert_eq!(
        body["target"],
        json!({ "schema": "s", "table": "t", "column": null })
    );
    assert_eq!(body["state"], "listed");
    assert_eq!(body["reason"], Value::Null);
    assert_eq!(body["profiles"][0]["name"], json!(prof));
    let p = &body["policies"][0];
    assert_eq!(p["name"], "hw-s");
    assert_eq!(p["match"], "applies");
    assert_eq!(p["throughPattern"], true);
    assert_eq!(p["inTagService"], false);
    assert_eq!(p["policyType"], "access");
    assert_eq!(p["enabled"], true);
    assert_eq!(
        p["items"][0]["groups"],
        json!([{ "name": "g1", "onEngines": false }])
    );
    assert_eq!(body["seeded"], json!([]));
}

#[tokio::test]
async fn route_a_refusals() {
    let f = setup().await;
    let (cat, _) = catalog_with_policy(&f).await;
    let admin = session(&f, &["platform_admin"]).await;
    let st = state(&f).await;
    for q in ["table=t", "schema=s&column=c", "schema="] {
        let (status, _) = get(
            &st,
            &admin,
            &format!("/governance/objects/{cat}/handwritten?{q}"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{q}");
    }
    let (status, _) = get(
        &st,
        &admin,
        &format!("/governance/objects/{}/handwritten", Uuid::new_v4()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── route B ──────────────────────────────────────────────────────────────

/// A user `ua…` with a username, in a new group, named by a hand-written
/// allow directly and by the group.
async fn user_named(f: &Fixture) -> (Uuid, String) {
    let p = &f.db.pool;
    let username = unique("ua");
    let user: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) VALUES ('local', $1, 'Ua', $2) RETURNING id",
    )
    .bind(format!("{}@egret.local", unique("hwrb")))
    .bind(&username)
    .fetch_one(p)
    .await
    .unwrap();
    let group = unique("g");
    let gid: Uuid =
        sqlx::query_scalar("INSERT INTO groups (name, source) VALUES ($1, 'local') RETURNING id")
            .bind(&group)
            .fetch_one(p)
            .await
            .unwrap();
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(user)
        .bind(gid)
        .execute(p)
        .await
        .unwrap();
    let prof = unique("hwrb-tr");
    let sid = f
        .policy
        .create_service(&prof, "trino", &json!({}))
        .await
        .unwrap()
        .id;
    let body = json!({
        "name": "hw-b", "service": prof, "policyType": 0, "isEnabled": true,
        "resources": { "catalog": { "values": ["c"] } },
        "policyItems": [
            { "users": [username], "accesses": [{ "type": "select", "isAllowed": true }] },
            { "groups": [group], "accesses": [{ "type": "select", "isAllowed": true }] }
        ]
    });
    sqlx::query(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, managed_by) \
         VALUES ($1, 'hw-b', $2, 'operator')",
    )
    .bind(sid)
    .bind(body)
    .execute(p)
    .await
    .unwrap();
    (user, group)
}

#[tokio::test]
async fn route_b_needs_policy_read_and_answers_with_it() {
    let f = setup().await;
    let (user, group) = user_named(&f).await;
    let tag_reader = session(&f, &["tag:read"]).await;
    let reader = session(&f, &["policy:read"]).await;
    let st = state(&f).await;
    let uri = format!("/governance/principals/user/{user}/handwritten");
    let (status, _) = get(&st, &tag_reader, &uri).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = get(&st, &reader, &uri).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"], "listed");
    assert_eq!(body["principal"]["kind"], "user");
    let p = &body["policies"][0];
    assert_eq!(p["name"], "hw-b");
    assert_eq!(
        p["naming"],
        json!([
            { "bucket": "allow", "how": "direct", "via": null, "onEngines": true },
            { "bucket": "allow", "how": "group", "via": group, "onEngines": false }
        ])
    );
    assert!(body["everyone"]["count"].as_i64().is_some());
}

#[tokio::test]
async fn route_b_refusals() {
    let f = setup().await;
    let admin = session(&f, &["platform_admin"]).await;
    let st = state(&f).await;
    for (uri, want) in [
        (
            format!("/governance/principals/user/{}/handwritten", Uuid::new_v4()),
            StatusCode::NOT_FOUND,
        ),
        (
            format!("/governance/principals/group/{}/handwritten", unique("nog")),
            StatusCode::NOT_FOUND,
        ),
        (
            format!("/governance/principals/role/{}/handwritten", unique("nor")),
            StatusCode::NOT_FOUND,
        ),
        (
            "/governance/principals/workspace/x/handwritten".to_string(),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let (status, _) = get(&st, &admin, &uri).await;
        assert_eq!(status, want, "{uri}");
    }
}

// ── route D ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn route_d_needs_policy_read_and_answers_with_it() {
    let f = setup().await;
    let (_, prof) = catalog_with_policy(&f).await;
    let tag_service: String = sqlx::query_scalar(
        "SELECT config_jsonb ->> 'tagService' FROM policy.authz_services WHERE name = $1",
    )
    .bind(&prof)
    .fetch_one(&f.db.pool)
    .await
    .unwrap();
    let tsid: i64 = sqlx::query_scalar("SELECT id FROM policy.authz_services WHERE name = $1")
        .bind(&tag_service)
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    let name = unique("PII");
    let tag: Uuid = sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
        .bind(unique("hwr-k"))
        .bind(&name)
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    let body = json!({
        "name": "hw-d", "service": tag_service, "serviceType": "tag", "policyType": 0, "isEnabled": true,
        "resources": { "tag": { "values": [name] } },
        "denyPolicyItems": [{ "users": ["ua"], "accesses": [{ "type": "select", "isAllowed": true }] }]
    });
    sqlx::query(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, managed_by) \
         VALUES ($1, 'hw-d', $2, 'operator')",
    )
    .bind(tsid)
    .bind(body)
    .execute(&f.db.pool)
    .await
    .unwrap();
    let tag_reader = session(&f, &["tag:read"]).await;
    let reader = session(&f, &["policy:read"]).await;
    let st = state(&f).await;
    let uri = format!("/classifications/{tag}/handwritten");
    assert_eq!(get(&st, &tag_reader, &uri).await.0, StatusCode::FORBIDDEN);
    let (status, body) = get(&st, &reader, &uri).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["state"], "listed");
    assert_eq!(body["tag"]["name"], json!(name));
    assert_eq!(body["sameName"], json!([]));
    let row = body["policies"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "hw-d")
        .cloned()
        .unwrap();
    assert_eq!(row["service"], json!(tag_service));
    assert_eq!(row["profiles"][0]["name"], json!(prof));
    assert_eq!(row["items"][0]["bucket"], "deny");
    let (status, _) = get(
        &st,
        &reader,
        &format!("/classifications/{}/handwritten", Uuid::new_v4()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
