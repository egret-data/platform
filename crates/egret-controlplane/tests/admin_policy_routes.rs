//! Username takeover, spec §4.2 (decision D2), the control-plane half: the
//! routes that change who is an engine admin without going through a role
//! write converge the `egret-admin - …` policies in the same request.
//!
//! - `PATCH /users/{id}` with `status`: a disabled admin is no engine admin;
//! - `DELETE /groups/{id}/roles/{role_id}` (and `POST`): before the fix these
//!   wrote `access_grants` with no recompile at all (spec E10).
//!
//! The engine-level decisions for the same functions are asserted in
//! `egret-perm-compile/tests/admin_policy_sync.rs`; here the body read right
//! after the response is enough to show the route took the recompiling path.
//!
//! Serial (the admin set is installation-wide); DB-gated, FAILS without a
//! database unless `EGRET_ALLOW_DB_SKIP` is set.

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
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct NoMvLister;
#[async_trait::async_trait]
impl MaterializedViewLister for NoMvLister {
    async fn list(&self, _engine: &ExposureEngine) -> Result<Vec<MvRow>, String> {
        panic!("no route here lists materialized views")
    }
}

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..10])
}

async fn setup() -> Option<Db> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

async fn state(db: &Db) -> CpState {
    let crypto = egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef").unwrap();
    let secrets = egret_secrets::WorkspaceSecretStore::new(
        db.clone(),
        egret_secrets::SecretStore::new(db.clone(), Some(crypto)),
    );
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    CpState {
        storage: egret_catalog_engine::CatalogStore::new(db.clone()),
        storage_registry: egret_storage::StorageRegistry::new(db.clone()),
        users: UserRepo::new(db.clone()),
        groups: egret_auth::groups::GroupRepo::new(db.clone()),
        roles: egret_auth::roles::RoleRepo::new(db.clone(), egret_events::EventBus::new()),
        access_resolver: egret_auth::effective::AccessResolver::new(db.clone()),
        secrets,
        sessions: sessions(db),
        browser: None,
        pdp,
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
    }
}

fn sessions(db: &Db) -> SessionService {
    SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    )
}

async fn platform_admin_role(db: &Db) -> Uuid {
    sqlx::query_scalar("SELECT id FROM roles WHERE name = 'platform_admin' AND scope = 'platform'")
        .fetch_one(&db.pool)
        .await
        .expect("platform_admin")
}

/// A user with a username who holds platform_admin directly.
async fn admin(db: &Db) -> (Uuid, String) {
    let name = unique("cpadm");
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) VALUES ('local',$1,'A',$2) RETURNING id",
    )
    .bind(format!("{name}@cp.test"))
    .bind(&name)
    .fetch_one(&db.pool)
    .await
    .expect("user");
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
         VALUES ('user', $1, $2, 'platform')",
    )
    .bind(id)
    .bind(platform_admin_role(db).await)
    .execute(&db.pool)
    .await
    .expect("admin grant");
    (id, name)
}

/// A Trino profile whose `egret-admin` names `users`.
async fn seeded_service(db: &Db, users: &[String]) -> i64 {
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_trino(&policy).await.expect("def");
    let name = unique("cp_aps");
    let svc = policy
        .create_service(&name, "trino", &json!({}))
        .await
        .expect("service");
    let mut users = users.to_vec();
    users.sort();
    policy
        .seed_admin_all_access(svc.id, &name, "trino", &users)
        .await
        .expect("seed");
    svc.id
}

async fn named(db: &Db, service_id: i64) -> Vec<String> {
    let body: Value = sqlx::query_scalar(
        "SELECT policy_jsonb FROM policy.authz_policies \
          WHERE service_id = $1 AND name LIKE 'egret-admin - %' ORDER BY name LIMIT 1",
    )
    .bind(service_id)
    .fetch_one(&db.pool)
    .await
    .expect("body");
    body["policyItems"][0]["users"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u.as_str().unwrap().to_string())
        .collect()
}

async fn send(
    st: &CpState,
    token: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> StatusCode {
    let mut req = HttpRequest::builder()
        .method(method)
        .uri(uri)
        .header("Cookie", format!("egret_session={token}"));
    let body = match body {
        Some(b) => {
            req = req.header("content-type", "application/json");
            Body::from(b.to_string())
        }
        None => Body::empty(),
    };
    control_plane_router()
        .with_state(st.clone())
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap()
        .status()
}

async fn token_for(db: &Db, user: Uuid) -> String {
    sessions(db)
        .create(egret_core::UserId(user), "test", "127.0.0.1")
        .await
        .expect("session")
        .token
}

#[tokio::test]
async fn disabling_an_admin_through_the_route_drops_them_and_enabling_restores() {
    let _serial = SERIAL.lock().await;
    let Some(db) = setup().await else { return };
    let (caller, caller_name) = admin(&db).await;
    let (target, target_name) = admin(&db).await;
    let svc = seeded_service(&db, &[caller_name.clone(), target_name.clone()]).await;
    let st = state(&db).await;
    let token = token_for(&db, caller).await;

    let status = send(
        &st,
        &token,
        "PATCH",
        &format!("/users/{target}"),
        Some(json!({"status": "disabled"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !named(&db, svc).await.contains(&target_name),
        "a disabled admin is still in egret-admin"
    );

    let status = send(
        &st,
        &token,
        "PATCH",
        &format!("/users/{target}"),
        Some(json!({"status": "active"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        named(&db, svc).await.contains(&target_name),
        "an enabled admin is not back"
    );
}

#[tokio::test]
async fn a_group_role_route_converges_egret_admin() {
    let _serial = SERIAL.lock().await;
    let Some(db) = setup().await else { return };
    let (caller, caller_name) = admin(&db).await;
    let member_name = unique("cpmem");
    let member: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) VALUES ('local',$1,'M',$2) RETURNING id",
    )
    .bind(format!("{member_name}@cp.test"))
    .bind(&member_name)
    .fetch_one(&db.pool)
    .await
    .expect("member");
    let gid: Uuid =
        sqlx::query_scalar("INSERT INTO groups (name, source) VALUES ($1, 'local') RETURNING id")
            .bind(unique("cpgrp"))
            .fetch_one(&db.pool)
            .await
            .expect("group");
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(member)
        .bind(gid)
        .execute(&db.pool)
        .await
        .expect("member");
    let svc = seeded_service(&db, std::slice::from_ref(&caller_name)).await;
    let st = state(&db).await;
    let token = token_for(&db, caller).await;
    let role = platform_admin_role(&db).await;

    let status = send(
        &st,
        &token,
        "POST",
        &format!("/groups/{gid}/roles"),
        Some(json!({"roleId": role})),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        named(&db, svc).await.contains(&member_name),
        "a group given platform_admin: its member is not in egret-admin"
    );

    let status = send(
        &st,
        &token,
        "DELETE",
        &format!("/groups/{gid}/roles/{role}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        !named(&db, svc).await.contains(&member_name),
        "a group that lost platform_admin: its member keeps engine ALL access"
    );
}
