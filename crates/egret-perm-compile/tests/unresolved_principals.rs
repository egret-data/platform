//! J-F3 Task 2: `GET /perm/unresolved-principals` lists every STORED grant
//! principal and rule exemption whose ref matches nobody, and flags the ones
//! that fail OPEN (design R5/R6).
//!
//! The write path refuses such refs since J-F3 Task 1, so every bad row here is
//! seeded straight into `perm.*` with SQL — the state a pre-J-F3 write, or a
//! principal deleted after a valid write, leaves behind.
//!
//! Its own test binary, so `egret-test-db` gives it a private database; rows of
//! other tests are still filtered out by id, never assumed absent.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test unresolved_principals

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_metastore::Db;
use egret_perm::PermStore;
use egret_perm_compile::{perm_router, PermState};
use egret_policy::PolicyStore;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt; // oneshot
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Env {
    db: Db,
    app: axum::Router,
    token: String,
    catalog_id: Uuid,
}

async fn env() -> Option<Env> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = &db.pool;
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let uid = UserRepo::new(db.clone())
        .insert_local(
            &format!("unresolved_{}@egret.local", Uuid::new_v4().simple()),
            "Test",
            "x",
        )
        .await
        .expect("insert user")
        .0;
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $1, id, 'platform', NULL FROM roles \
          WHERE scope='platform' AND name='platform_admin'",
    )
    .bind(uid)
    .execute(pool)
    .await
    .expect("grant platform_admin");
    let token = sessions
        .create(egret_core::UserId(uid), "test", "127.0.0.1")
        .await
        .expect("session")
        .token;

    // A grant needs a registry catalog; nothing here compiles, so no profile,
    // engine or attachment is needed.
    let workspace_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique("unres-ws"))
            .bind("Unresolved WS")
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-unresolved-test','AKIATEST') RETURNING id",
    )
    .bind(unique("unres-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("unres_wh"))
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
    .bind(unique("unres_cat"))
    .bind(warehouse_id)
    .fetch_one(pool)
    .await
    .expect("registry catalog");

    let app = perm_router().with_state(PermState {
        perm: PermStore::new(pool.clone()),
        policy: PolicyStore::new(db.clone()),
        sessions,
        pdp,
    });
    Some(Env {
        db,
        app,
        token,
        catalog_id,
    })
}

async fn get(e: &Env, token: &str) -> (StatusCode, Value) {
    let resp = e
        .app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("GET")
                .uri("/perm/unresolved-principals")
                .header("Cookie", format!("egret_session={token}"))
                .body(Body::empty())
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

/// The report's rows for `id` only, each reduced to the fields a test checks.
async fn rows_for(e: &Env, id: Uuid) -> Vec<Value> {
    let (status, body) = get(e, &e.token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body.as_array()
        .expect("an array")
        .iter()
        .filter(|r| r["id"] == json!(id))
        .cloned()
        .collect()
}

/// A grant seeded behind the write path's back: `principals` is
/// `(kind, ref, mode)`.
async fn seed_grant(e: &Env, effect: &str, principals: &[(&str, &str, &str)]) -> Uuid {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO perm.grants (effect, actions, catalog_id) \
         VALUES ($1::perm.effect, ARRAY['read'], $2) RETURNING id",
    )
    .bind(effect)
    .bind(e.catalog_id)
    .fetch_one(&e.db.pool)
    .await
    .expect("grant");
    for (kind, r, mode) in principals {
        sqlx::query(
            "INSERT INTO perm.grant_principals (grant_id, kind, ref, mode) \
             VALUES ($1, $2::perm.principal_kind, $3, $4::perm.principal_mode)",
        )
        .bind(id)
        .bind(kind)
        .bind(r)
        .bind(mode)
        .execute(&e.db.pool)
        .await
        .expect("grant principal");
    }
    id
}

async fn seed_rule(e: &Env, exempt: &[(&str, &str, &str)]) -> (Uuid, String) {
    let name = unique("unres_rule");
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO perm.rules (name, kind, mask_type, selector_kind, catalog_id, \
                                 schema_name, table_name, column_name) \
         VALUES ($1, 'mask', 'MASK', 'path', $2, 's', 't', 'c') RETURNING id",
    )
    .bind(&name)
    .bind(e.catalog_id)
    .fetch_one(&e.db.pool)
    .await
    .expect("rule");
    for (kind, r, mode) in exempt {
        sqlx::query(
            "INSERT INTO perm.rule_exemptions (rule_id, kind, ref, mode) \
             VALUES ($1, $2::perm.principal_kind, $3, $4::perm.principal_mode)",
        )
        .bind(id)
        .bind(kind)
        .bind(r)
        .bind(mode)
        .execute(&e.db.pool)
        .await
        .expect("rule exemption");
    }
    (id, name)
}

/// A user with a stored username; returns `(id, username)`.
async fn user_with_username(e: &Env) -> (Uuid, String) {
    let username = unique("unres-user").replace('_', "-");
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, username) VALUES ('local', $1, $2) RETURNING id",
    )
    .bind(format!("{username}@egret.local"))
    .bind(&username)
    .fetch_one(&e.db.pool)
    .await
    .expect("user");
    (id, username)
}

async fn group(e: &Env, members: &[Uuid]) -> String {
    let name = unique("unres_group");
    let gid: Uuid = sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
        .bind(&name)
        .fetch_one(&e.db.pool)
        .await
        .expect("group");
    for m in members {
        sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
            .bind(m)
            .bind(gid)
            .execute(&e.db.pool)
            .await
            .expect("membership");
    }
    name
}

/// `(kind, ref, position, status, failsOpen)` per row, sorted for comparison.
fn summary(rows: &[Value]) -> Vec<(String, String, String, String, bool)> {
    let mut out: Vec<_> = rows
        .iter()
        .map(|r| {
            (
                r["kind"].as_str().unwrap().to_string(),
                r["ref"].as_str().unwrap().to_string(),
                r["position"].as_str().unwrap().to_string(),
                r["status"].as_str().unwrap().to_string(),
                r["failsOpen"].as_bool().unwrap(),
            )
        })
        .collect();
    out.sort();
    out
}

fn row(
    k: &str,
    r: &str,
    pos: &str,
    status: &str,
    open: bool,
) -> (String, String, String, String, bool) {
    (k.into(), r.into(), pos.into(), status.into(), open)
}

/// The finding itself: a deny naming a user by UUID denies nobody, so the
/// report flags it as failing OPEN; a valid username on the same grant is not
/// listed at all.
#[tokio::test]
async fn a_uuid_user_ref_on_a_deny_fails_open_and_a_valid_ref_is_not_listed() {
    let Some(e) = env().await else { return };
    let (uid, username) = user_with_username(&e).await;
    let id = seed_grant(
        &e,
        "deny",
        &[
            ("user", &uid.to_string(), "include"),
            ("user", &username, "include"),
        ],
    )
    .await;

    let rows = rows_for(&e, id).await;

    assert_eq!(
        summary(&rows),
        vec![row("user", &uid.to_string(), "include", "unresolved", true)]
    );
    let r = &rows[0];
    assert_eq!(r["source"], "grant");
    assert_eq!(r["effect"], "deny");
    assert!(
        r["reason"].as_str().unwrap().contains(&username),
        "the reason names the username to use: {r}"
    );
}

/// An allow include matching nobody fails CLOSED (grants nobody): listed, not
/// flagged. Any exclude matching nobody fails OPEN, on an allow as on a deny.
#[tokio::test]
async fn an_allow_include_fails_closed_and_any_exclude_fails_open() {
    let Some(e) = env().await else { return };
    let ghost_user = unique("ghost-user");
    let ghost_group = unique("ghost_group");
    let ghost_role = unique("ghost_role");
    let allow = seed_grant(
        &e,
        "allow",
        &[
            ("user", &ghost_user, "include"),
            ("group", &ghost_group, "exclude"),
        ],
    )
    .await;
    let deny = seed_grant(&e, "deny", &[("role", &ghost_role, "exclude")]).await;

    let allow_rows = rows_for(&e, allow).await;
    let deny_rows = rows_for(&e, deny).await;

    assert_eq!(
        summary(&allow_rows),
        vec![
            row("group", &ghost_group, "exclude", "unresolved", true),
            row("user", &ghost_user, "include", "unresolved", false),
        ]
    );
    assert_eq!(
        summary(&deny_rows),
        vec![row("role", &ghost_role, "exclude", "unresolved", true)]
    );
}

/// A rule exemption: an exclude matching nobody fails OPEN (the exemption is
/// wider than written); an include matching nobody fails closed (the mask
/// applies to someone meant to be exempt). A rule has no effect.
#[tokio::test]
async fn a_rule_exemption_exclude_fails_open_and_its_include_does_not() {
    let Some(e) = env().await else { return };
    let ghost_ws = Uuid::new_v4().to_string();
    let ghost_user = unique("ghost-user");
    let (id, name) = seed_rule(
        &e,
        &[
            ("user", &ghost_user, "include"),
            ("workspace_members", &ghost_ws, "exclude"),
        ],
    )
    .await;

    let rows = rows_for(&e, id).await;

    assert_eq!(
        summary(&rows),
        vec![
            row("user", &ghost_user, "include", "unresolved", false),
            row(
                "workspace_members",
                &ghost_ws,
                "exclude",
                "unresolved",
                true
            ),
        ]
    );
    for r in &rows {
        assert_eq!(r["source"], "rule");
        assert_eq!(r["name"], json!(name));
        assert_eq!(r["effect"], Value::Null);
    }
}

/// Groups resolve through membership (Task 1b): a group with members and
/// `public` are not listed; an existing group with no members is listed as
/// `empty` and NOT flagged even on a deny — nobody is in it, so matching
/// nobody is exactly right.
#[tokio::test]
async fn an_empty_group_is_listed_as_empty_and_never_fails_open() {
    let Some(e) = env().await else { return };
    let (member, _) = user_with_username(&e).await;
    let staffed = group(&e, &[member]).await;
    let empty = group(&e, &[]).await;
    let id = seed_grant(
        &e,
        "deny",
        &[
            ("group", &staffed, "include"),
            ("group", "public", "exclude"),
            ("group", &empty, "include"),
        ],
    )
    .await;

    let rows = rows_for(&e, id).await;

    assert_eq!(
        summary(&rows),
        vec![row("group", &empty, "include", "empty", false)]
    );
}

/// A user ref that matches a username only case-insensitively: egret's
/// lookup (citext) finds the person, but the compiled policy names the ref
/// byte-for-byte and the engine compares it so — on a deny it denies nobody.
#[tokio::test]
async fn a_username_in_the_wrong_case_is_unresolved() {
    let Some(e) = env().await else { return };
    let (_, username) = user_with_username(&e).await;
    let shouted = username.to_uppercase();
    let id = seed_grant(&e, "deny", &[("user", &shouted, "include")]).await;

    let rows = rows_for(&e, id).await;

    assert_eq!(
        summary(&rows),
        vec![row("user", &shouted, "include", "unresolved", true)]
    );
    assert!(rows[0]["reason"].as_str().unwrap().contains(&username));
}

/// The startup pass logs the report's fail-open rows and only those: the
/// seeded deny on a ghost user is among them, its fail-closed twin is not.
#[tokio::test]
async fn the_startup_pass_logs_exactly_the_fail_open_rows() {
    let Some(e) = env().await else { return };
    let open = seed_grant(&e, "deny", &[("user", &unique("ghost-user"), "include")]).await;
    let closed = seed_grant(&e, "allow", &[("user", &unique("ghost-user"), "include")]).await;

    let logged = egret_perm_compile::principal_refs::warn_fail_open_principals(&e.db.pool)
        .await
        .expect("startup pass");

    assert!(logged.iter().all(|r| r.fails_open), "only fail-open rows");
    assert!(logged.iter().any(|r| r.id == open), "the deny is logged");
    assert!(!logged.iter().any(|r| r.id == closed), "the allow is not");
}

/// Same permission as `/perm/overlaps`: `policy:read`.
#[tokio::test]
async fn a_caller_without_policy_read_is_refused() {
    let Some(e) = env().await else { return };
    let uid = UserRepo::new(e.db.clone())
        .insert_local(
            &format!("nopolicy_{}@egret.local", Uuid::new_v4().simple()),
            "Nobody",
            "x",
        )
        .await
        .expect("insert user")
        .0;
    let token = SessionService::new(
        e.db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    )
    .create(egret_core::UserId(uid), "test", "127.0.0.1")
    .await
    .expect("session")
    .token;

    let (status, _) = get(&e, &token).await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = get(&e, &e.token).await;
    assert_eq!(status, StatusCode::OK, "the control: the admin may");
}
