//! Username takeover, spec §4.5 (decision D3): `GET /perm/username-reuse`
//! reports the evidence of takeovers that happened before migration 0209, and
//! changes nothing.
//!
//! Pre-0209 history is arranged with raw SQL: a ref, body or event written
//! (backdated) for a name, then a user created later carrying that name — the
//! state a delete-then-mint left. The ledger only stops new mints, so writing
//! such a user directly here is the pre-0209 shape, not a bypass.
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

/// A user who held `username` before migration 0209: their ledger row is the
/// backfill (`minted_at` NULL) that 0209's step 1 wrote, not a mint. A user
/// INSERTed with a username now is minted (`minted_at` set), a name nobody
/// held before, and so can be no takeover.
async fn pre_0209_holder(db: &Db, username: &str) -> Uuid {
    let id = user(db, None).await;
    sqlx::query(
        "INSERT INTO username_ledger (username, user_id, minted_for, source) \
         VALUES ($1, $2, $2, 'backfill_user')",
    )
    .bind(username)
    .bind(id)
    .execute(&db.pool)
    .await
    .expect("backfilled ledger row");
    sqlx::query("UPDATE users SET username = $2 WHERE id = $1")
        .bind(id)
        .bind(username)
        .execute(&db.pool)
        .await
        .expect("username");
    id
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
    // The next person, minted the freed name later (before 0209).
    pre_0209_holder(&e.db, &with_event).await;
    pre_0209_holder(&e.db, &without).await;

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
    pre_0209_holder(&e.db, &compiled).await;
    pre_0209_holder(&e.db, &hand).await;

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
    let holder = pre_0209_holder(&e.db, &carol).await;
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
    pre_0209_holder(&e.db, &name).await;

    let f = findings_for(&e, &name).await;
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0]["kind"], "ref_predates_holder");
    assert_eq!(f[0]["verdict"], "definite", "{f:?}");
    assert_eq!(f[0]["place"]["type"], "engine_grant");
    // The engine page is where an engine grant is fixed, so the place names it.
    assert_eq!(f[0]["place"]["engineId"], engine.to_string(), "{f:?}");
}

#[tokio::test]
async fn a_name_minted_after_0209_is_no_takeover_however_old_the_place() {
    let Some(e) = env().await else { return };
    // A newcomer gets a fresh name (a mint: nobody ever held it, 0209 seeded
    // every name a ref, body or event knew), then a steward adds them to an
    // old grant and an old hand-written policy. Neither was written for
    // someone else.
    let newbie = format!("newbie_{}", tag());
    user(&e.db, Some(&newbie)).await;
    old_grant(&e, "allow", "include", &newbie).await;
    policy_naming(&e, "operator", "hand-written", &newbie).await;

    let f = findings_for(&e, &newbie).await;
    assert!(f.is_empty(), "{f:?}");
}

/// An `egret-admin` row of the 0209 baseline naming `username`, its body last
/// written at `written`, with `holder` (and their `created_at`) as 0209 saw
/// them.
async fn baseline_admin_row(e: &Env, username: &str, holder: Option<Uuid>, written: &str) {
    sqlx::query(
        "INSERT INTO username_reuse_baseline (service_id, policy_id, policy_name, managed_by, \
           policy_updated_at, username, holder_id, holder_created_at, holder_is_active_admin) \
         VALUES (-1, -1, 'egret-admin - catalog', 'infra', $1::timestamptz, $2, $3, \
                 (SELECT created_at FROM users WHERE id = $3), false)",
    )
    .bind(written)
    .bind(username)
    .bind(holder)
    .execute(&e.db.pool)
    .await
    .expect("baseline row");
}

async fn created_long_ago(e: &Env, user: Uuid) {
    sqlx::query("UPDATE users SET created_at = $2::timestamptz WHERE id = $1")
        .bind(user)
        .bind(LONG_AGO)
        .execute(&e.db.pool)
        .await
        .expect("backdate");
}

async fn grant_platform_admin(e: &Env, user: Uuid) {
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
         SELECT 'user', $1, id, 'platform' FROM roles \
          WHERE scope = 'platform' AND name = 'platform_admin'",
    )
    .bind(user)
    .execute(&e.db.pool)
    .await
    .expect("platform_admin");
}

async fn disable(e: &Env, user: Uuid) {
    sqlx::query("UPDATE users SET status = 'disabled' WHERE id = $1")
        .bind(user)
        .execute(&e.db.pool)
        .await
        .expect("disable");
}

#[tokio::test]
async fn a_baseline_egret_admin_row_for_a_disabled_admin_is_no_takeover() {
    let Some(e) = env().await else { return };
    // The same person, an admin before the body was written, merely disabled
    // when 0209 ran: the body named them because they were an admin.
    let dana = format!("dana_{}", tag());
    let holder = pre_0209_holder(&e.db, &dana).await;
    created_long_ago(&e, holder).await;
    grant_platform_admin(&e, holder).await;
    disable(&e, holder).await;
    baseline_admin_row(&e, &dana, Some(holder), "2021-01-01T00:00:00Z").await;

    let f = findings_for(&e, &dana).await;
    assert!(f.is_empty(), "{f:?}");
}

#[tokio::test]
async fn a_baseline_egret_admin_row_naming_nobody_is_no_finding() {
    let Some(e) = env().await else { return };
    // A deleted admin's name: retired, it matches nobody and nobody can hold
    // it again.
    let gone = format!("goneadmin_{}", tag());
    baseline_admin_row(&e, &gone, None, LONG_AGO).await;

    let f = findings_for(&e, &gone).await;
    assert!(f.is_empty(), "{f:?}");
}

#[tokio::test]
async fn a_baseline_egret_admin_row_for_a_non_admin_who_predates_it_is_possible() {
    let Some(e) = env().await else { return };
    // Created before the body was written and no admin now: a demoted admin,
    // or someone given a deleted admin's name later (a username is minted at
    // first use, after `created_at`). The timestamps cannot tell.
    let erin = format!("erin_{}", tag());
    let holder = pre_0209_holder(&e.db, &erin).await;
    created_long_ago(&e, holder).await;
    baseline_admin_row(&e, &erin, Some(holder), "2021-01-01T00:00:00Z").await;

    let f = findings_for(&e, &erin).await;
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0]["kind"], "admin_policy_named_non_admin");
    assert_eq!(f[0]["verdict"], "possible", "{f:?}");
}

#[tokio::test]
async fn a_live_egret_admin_body_naming_a_disabled_admin_is_stale_not_reuse() {
    let Some(e) = env().await else { return };
    let fred = format!("fred_{}", tag());
    let holder = user(&e.db, Some(&fred)).await;
    grant_platform_admin(&e, holder).await;
    disable(&e, holder).await;
    policy_naming(&e, "infra", "egret-admin - catalog", &fred).await;

    let f = findings_for(&e, &fred).await;
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0]["kind"], "admin_policy_named_non_admin");
    assert_eq!(f[0]["verdict"], "stale", "{f:?}");
}

#[tokio::test]
async fn a_hand_written_name_0209_retired_says_how_to_fix_it_and_a_free_one_is_no_finding() {
    let Some(e) = env().await else { return };
    // 0209 step 3 retired a name a hand-written policy listed for someone who
    // had not signed in: they were given another name.
    let alice = format!("alice_{}", tag());
    sqlx::query(
        "INSERT INTO username_ledger (username, source, retired_at) \
         VALUES ($1, 'backfill_policy', now())",
    )
    .bind(&alice)
    .execute(&e.db.pool)
    .await
    .expect("retired by 0209");
    policy_naming(&e, "operator", "pre-provisioned", &alice).await;
    // Written after 0209 for a name nobody holds yet: still free, so the next
    // person whose email slugs to it is given it.
    let zed = format!("zed_{}", tag());
    policy_naming(&e, "operator", "pre-provisioned", &zed).await;

    let f = findings_for(&e, &alice).await;
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0]["verdict"], "dead");
    let evidence = f[0]["evidence"].as_str().unwrap();
    assert!(
        evidence.contains("rewrite it to the name they were given"),
        "{evidence}"
    );
    assert!(findings_for(&e, &zed).await.is_empty());
}

#[tokio::test]
async fn an_engine_grant_event_naming_the_user_as_an_object_is_definite() {
    let Some(e) = env().await else { return };
    let name = format!("egev_{}", tag());
    let engine: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec) \
         VALUES ($1, 'starrocks', 'egret', '{}'::jsonb) RETURNING id",
    )
    .bind(format!("reuse-eng-{}", tag()))
    .fetch_one(&e.db.pool)
    .await
    .expect("engine");
    // Changed since (so its `updated_at` proves nothing); only the event can.
    let g: Uuid = sqlx::query_scalar(
        "INSERT INTO perm.engine_grants (engine_id, capability, resource_values, created_at) \
         VALUES ($1, 'starrocks.manage_blacklist', '{}', $2::timestamptz) RETURNING id",
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
    for detail in [
        // An object of another shape, with no `ref`, is skipped.
        json!({"before": null, "after": {"principals": [{"kind": "user"}]}}),
        json!({"before": null, "after": {"principals": [{"kind": "user", "ref": name}]}}),
    ] {
        sqlx::query(
            "INSERT INTO audit.audit_log (ts, kind, action, resource_type, resource_id, detail) \
             VALUES ($1::timestamptz, 'mutation', 'engine_grant:create', 'engine_grant', $2, $3)",
        )
        .bind(LONG_AGO)
        .bind(g.to_string())
        .bind(detail)
        .execute(&e.db.pool)
        .await
        .expect("event");
    }
    pre_0209_holder(&e.db, &name).await;

    let f = findings_for(&e, &name).await;
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0]["verdict"], "definite", "{f:?}");
    assert!(
        f[0]["evidence"]
            .as_str()
            .unwrap()
            .starts_with("governance event engine_grant:create"),
        "{f:?}"
    );
}

/// A row of the 0209 baseline for the live policy `policy_id`, as 0209 saw it:
/// its body last written at `written`, naming `username` held by `holder`.
async fn baseline_row_of(e: &Env, policy_id: i64, username: &str, holder: Uuid, written: &str) {
    sqlx::query(
        "INSERT INTO username_reuse_baseline (service_id, policy_id, policy_name, managed_by, \
           policy_updated_at, username, holder_id, holder_created_at, holder_is_active_admin) \
         SELECT p.service_id, p.id, p.name, p.managed_by, $2::timestamptz, $3, $4, u.created_at, \
                false \
           FROM policy.authz_policies p, users u WHERE p.id = $1 AND u.id = $4",
    )
    .bind(policy_id)
    .bind(written)
    .bind(username)
    .bind(holder)
    .execute(&e.db.pool)
    .await
    .expect("baseline row");
}

#[tokio::test]
async fn a_policy_both_live_and_in_the_baseline_is_one_finding() {
    let Some(e) = env().await else { return };
    // A compiled body naming `jdoe_…`, written before its holder existed and
    // unchanged since: the live body and the 0209 baseline row say the same
    // thing about the same place.
    let jdoe = format!("jdoe_{}", tag());
    let pc = policy_naming(
        &e,
        "compiler",
        &format!("perm-grant-{}", Uuid::new_v4()),
        &jdoe,
    )
    .await;
    let holder = pre_0209_holder(&e.db, &jdoe).await;
    baseline_row_of(&e, pc, &jdoe, holder, LONG_AGO).await;

    let f = findings_for(&e, &jdoe).await;
    assert_eq!(f.len(), 1, "one place, one finding: {f:?}");
    assert_eq!(f[0]["kind"], "policy_predates_holder");
    assert_eq!(f[0]["verdict"], "definite");
    assert_eq!(f[0]["place"]["policyId"], json!(pc));
    // The live body wins a tie: its evidence carries no baseline suffix.
    assert_eq!(
        f[0]["evidence"], "the policy body was last written before its holder existed",
        "{f:?}"
    );
}

#[tokio::test]
async fn the_stronger_verdict_wins_when_live_and_baseline_disagree() {
    let Some(e) = env().await else { return };
    // An egret-admin body naming a non-admin: when 0209 ran it had last been
    // written before its holder existed (definite); it was rewritten since,
    // after they joined, so the live body alone reads `possible`. The baseline
    // still proves it was written for someone else.
    let kim = format!("kim_{}", tag());
    let holder = pre_0209_holder(&e.db, &kim).await;
    sqlx::query("UPDATE users SET created_at = '2021-01-01T00:00:00Z' WHERE id = $1")
        .bind(holder)
        .execute(&e.db.pool)
        .await
        .expect("backdate");
    let pc = policy_naming(&e, "infra", "egret-admin - catalog", &kim).await;
    baseline_row_of(&e, pc, &kim, holder, LONG_AGO).await;
    sqlx::query("UPDATE policy.authz_policies SET updated_at = now() WHERE id = $1")
        .bind(pc)
        .execute(&e.db.pool)
        .await
        .expect("rewritten since");

    let f = findings_for(&e, &kim).await;
    assert_eq!(f.len(), 1, "one place, one finding: {f:?}");
    assert_eq!(f[0]["kind"], "admin_policy_named_non_admin");
    assert_eq!(f[0]["verdict"], "definite", "{f:?}");
    assert!(
        f[0]["evidence"]
            .as_str()
            .unwrap()
            .contains("(at migration 0209)"),
        "{f:?}"
    );
}
