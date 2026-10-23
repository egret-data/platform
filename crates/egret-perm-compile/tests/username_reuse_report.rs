//! Username takeover, spec §4.5 (decision D3): `GET /perm/username-reuse`
//! reports the evidence of takeovers that happened before migration 0208, and
//! changes nothing.
//!
//! Pre-0208 history is arranged with raw SQL: a ref, body or event written
//! (backdated) for a name, then a user created later carrying that name — the
//! state a delete-then-mint left. The ledger only stops new mints, so writing
//! such a user directly here is the pre-0208 shape, not a bypass.
//!
//! Its own binary; tests run concurrently and use unique names, and each
//! assertion picks its own username's findings out of the report.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test username_reuse_report

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::session::SessionService;
use egret_metastore::Db;
use egret_perm::PermStore;
use egret_perm_compile::{perm_router, PermState};
use egret_policy::PolicyStore;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

fn tag() -> String {
    Uuid::new_v4().simple().to_string()[..12].to_string()
}

const LONG_AGO: &str = "2020-01-01T00:00:00Z";

struct Env {
    db: Db,
    app: axum::Router,
    admin: String,
    reader: String,
    catalog_id: Uuid,
}

async fn user(db: &Db, username: Option<&str>) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) VALUES ('local', $1, 'T', $2) \
         RETURNING id",
    )
    .bind(format!(
        "{}@reuse.test",
        username.map_or_else(tag, str::to_string)
    ))
    .bind(username)
    .fetch_one(&db.pool)
    .await
    .expect("user")
}

async fn session_with(db: &Db, sessions: &SessionService, permissions: &[&str]) -> String {
    let uid = user(db, None).await;
    let role: Uuid =
        sqlx::query_scalar("INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id")
            .bind(format!("reuse_role_{}", tag()))
            .fetch_one(&db.pool)
            .await
            .expect("role");
    for p in permissions {
        sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2)")
            .bind(role)
            .bind(p)
            .execute(&db.pool)
            .await
            .expect("permission");
    }
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
         VALUES ('user', $1, $2, 'platform')",
    )
    .bind(uid)
    .bind(role)
    .execute(&db.pool)
    .await
    .expect("grant");
    sessions
        .create(egret_core::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("session")
        .token
}

async fn env() -> Option<Env> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 10).await.expect("connect");
    db.migrate().await.expect("migrate");
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let reader = session_with(&db, &sessions, &["policy:read", "user:read"]).await;
    let admin_id = user(&db, None).await;
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
         SELECT 'user', $1, id, 'platform' FROM roles \
          WHERE scope = 'platform' AND name = 'platform_admin'",
    )
    .bind(admin_id)
    .execute(&db.pool)
    .await
    .expect("platform_admin");
    let admin = sessions
        .create(egret_core::UserId(admin_id), "test", "127.0.0.1")
        .await
        .expect("session")
        .token;
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config) VALUES ($1, 'iceberg', '{}') \
         RETURNING id",
    )
    .bind(format!("reuse_cat_{}", tag()))
    .fetch_one(&db.pool)
    .await
    .expect("catalog");
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let app = perm_router().with_state(PermState {
        perm: PermStore::new(db.pool.clone()),
        policy: PolicyStore::new(db.clone()),
        sessions,
        pdp,
    });
    Some(Env {
        db,
        app,
        admin,
        reader,
        catalog_id,
    })
}

async fn report(e: &Env, token: &str) -> (StatusCode, Value) {
    let resp = e
        .app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri("/perm/username-reuse")
                .header("Cookie", format!("egret_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// The findings for `username`.
async fn findings_for(e: &Env, username: &str) -> Vec<Value> {
    let (status, body) = report(e, &e.admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .filter(|f| f["username"] == username)
        .cloned()
        .collect()
}

/// A grant naming `user:<name>` at `position`, created long ago.
async fn old_grant(e: &Env, effect: &str, position: &str, name: &str) -> Uuid {
    let g: Uuid = sqlx::query_scalar(
        "INSERT INTO perm.grants (effect, actions, catalog_id, created_at) \
         VALUES ($1::perm.effect, ARRAY['read'], $2, $3::timestamptz) RETURNING id",
    )
    .bind(effect)
    .bind(e.catalog_id)
    .bind(LONG_AGO)
    .fetch_one(&e.db.pool)
    .await
    .expect("grant");
    sqlx::query(
        "INSERT INTO perm.grant_principals (grant_id, kind, ref, mode) \
         VALUES ($1, 'user', $2, $3::perm.principal_mode)",
    )
    .bind(g)
    .bind(name)
    .bind(position)
    .execute(&e.db.pool)
    .await
    .expect("principal");
    g
}

#[tokio::test]
async fn a_ref_older_than_its_holder_is_definite_with_an_event_and_possible_without() {
    let Some(e) = env().await else { return };
    let with_event = format!("ev_{}", tag());
    let without = format!("noev_{}", tag());
    let g1 = old_grant(&e, "allow", "include", &with_event).await;
    old_grant(&e, "allow", "include", &without).await;
    sqlx::query(
        "INSERT INTO audit.audit_log (ts, kind, action, resource_type, resource_id, detail) \
         VALUES ($1::timestamptz, 'mutation', 'perm_grant:create', 'perm_grant', $2, $3)",
    )
    .bind(LONG_AGO)
    .bind(g1.to_string())
    .bind(json!({"before": null, "after": {"include": [format!("user:{with_event}")]}}))
    .execute(&e.db.pool)
    .await
    .expect("event");
    // The next person, minted the freed name later.
    user(&e.db, Some(&with_event)).await;
    user(&e.db, Some(&without)).await;

    let f = findings_for(&e, &with_event).await;
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0]["kind"], "ref_predates_holder");
    assert_eq!(f[0]["verdict"], "definite");
    assert_eq!(f[0]["place"]["type"], "grant");
    assert_eq!(f[0]["place"]["id"], json!(g1));
    assert!(f[0]["holder"]["email"].is_string());

    let f = findings_for(&e, &without).await;
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0]["kind"], "ref_predates_holder");
    assert_eq!(f[0]["verdict"], "possible");
}

async fn policy_naming(e: &Env, managed_by: &str, name: &str, user: &str) -> i64 {
    let policy = PolicyStore::new(e.db.clone());
    egret_policy::seed_trino(&policy).await.expect("def");
    let svc = policy
        .create_service(&format!("reuse_svc_{}", tag()), "trino", &json!({}))
        .await
        .expect("service")
        .id;
    sqlx::query_scalar(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, managed_by, updated_at) \
         VALUES ($1, $2, $3, $4, $5::timestamptz) RETURNING id",
    )
    .bind(svc)
    .bind(name)
    .bind(json!({"name": name, "policyItems": [{"users": [user], "accesses": []}]}))
    .bind(managed_by)
    .bind(LONG_AGO)
    .fetch_one(&e.db.pool)
    .await
    .expect("policy")
}

#[tokio::test]
async fn a_compiled_body_older_than_its_holder_is_definite_and_a_hand_written_one_possible() {
    let Some(e) = env().await else { return };
    let compiled = format!("cmp_{}", tag());
    let hand = format!("hand_{}", tag());
    let pc = policy_naming(
        &e,
        "compiler",
        &format!("perm-grant-{}", Uuid::new_v4()),
        &compiled,
    )
    .await;
    policy_naming(&e, "operator", "hand-written", &hand).await;
    user(&e.db, Some(&compiled)).await;
    user(&e.db, Some(&hand)).await;

    let f = findings_for(&e, &compiled).await;
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0]["kind"], "policy_predates_holder");
    assert_eq!(f[0]["verdict"], "definite");
    assert_eq!(f[0]["place"]["policyId"], json!(pc));
    let f = findings_for(&e, &hand).await;
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0]["verdict"], "possible");
}

#[tokio::test]
async fn a_baseline_egret_admin_row_naming_a_non_admin_is_reported() {
    let Some(e) = env().await else { return };
    let carol = format!("carol_{}", tag());
    let holder = user(&e.db, Some(&carol)).await;
    sqlx::query(
        "INSERT INTO username_reuse_baseline (service_id, policy_id, policy_name, managed_by, \
           policy_updated_at, username, holder_id, holder_created_at, holder_is_active_admin) \
         VALUES (-1, -1, 'egret-admin - catalog', 'infra', $1::timestamptz, $2, $3, now(), false)",
    )
    .bind(LONG_AGO)
    .bind(&carol)
    .bind(holder)
    .execute(&e.db.pool)
    .await
    .expect("baseline row");

    let f = findings_for(&e, &carol).await;
    assert!(
        f.iter()
            .any(|x| x["kind"] == "admin_policy_named_non_admin" && x["verdict"] == "definite"),
        "{f:?}"
    );
}

#[tokio::test]
async fn a_holderless_ref_is_dead_and_says_if_it_fails_open() {
    let Some(e) = env().await else { return };
    let gone_deny = format!("gd_{}", tag());
    let gone_allow = format!("ga_{}", tag());
    old_grant(&e, "deny", "include", &gone_deny).await;
    old_grant(&e, "allow", "include", &gone_allow).await;

    let f = findings_for(&e, &gone_deny).await;
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0]["kind"], "holderless_ref");
    assert_eq!(f[0]["verdict"], "dead");
    assert_eq!(
        f[0]["retired"]["failsOpen"], true,
        "a deny naming nobody fails open"
    );
    let f = findings_for(&e, &gone_allow).await;
    assert_eq!(
        f[0]["retired"]["failsOpen"], false,
        "an allow naming nobody fails closed"
    );
}

#[tokio::test]
async fn only_platform_admins_may_read_it() {
    let Some(e) = env().await else { return };
    let (status, _) = report(&e, &e.reader).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "policy:read + user:read is not enough"
    );
    let (status, body) = report(&e, &e.admin).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["generatedAt"].is_string());
}

#[tokio::test]
async fn an_engine_grant_unchanged_since_before_the_holder_is_definite() {
    let Some(e) = env().await else { return };
    let name = format!("eg_{}", tag());
    let engine: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec) \
         VALUES ($1, 'starrocks', 'egret', '{}'::jsonb) RETURNING id",
    )
    .bind(format!("reuse-eng-{}", tag()))
    .fetch_one(&e.db.pool)
    .await
    .expect("engine");
    let g: Uuid = sqlx::query_scalar(
        "INSERT INTO perm.engine_grants (engine_id, capability, resource_values, created_at, \
           updated_at) \
         VALUES ($1, 'starrocks.manage_blacklist', '{}', $2::timestamptz, $2::timestamptz) \
         RETURNING id",
    )
    .bind(engine)
    .bind(LONG_AGO)
    .fetch_one(&e.db.pool)
    .await
    .expect("engine grant");
    sqlx::query(
        "INSERT INTO perm.engine_grant_principals (engine_grant_id, kind, ref) VALUES ($1, 'user', $2)",
    )
    .bind(g)
    .bind(&name)
    .execute(&e.db.pool)
    .await
    .expect("principal");
    user(&e.db, Some(&name)).await;

    let f = findings_for(&e, &name).await;
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0]["kind"], "ref_predates_holder");
    assert_eq!(f[0]["verdict"], "definite", "{f:?}");
    assert_eq!(f[0]["place"]["type"], "engine_grant");
    // The engine page is where an engine grant is fixed, so the place names it.
    assert_eq!(f[0]["place"]["engineId"], engine.to_string(), "{f:?}");
}
