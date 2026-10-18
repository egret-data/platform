//! People & groups Task 5 (spec §4.4, Q1): `POST /perm/people/users/{user_id}/username`
//! allocates a user's engine username from Governance, gated `policy:update`,
//! and REFUSES with 409 when a stored grant principal or rule exemption already
//! names the name it would allocate as a `user` ref — such a ref matches nobody
//! today and would silently start applying to this person.
//!
//! The write path refuses such refs since J-F3, so every stale row here is
//! seeded straight into `perm.*` with SQL — the state a pre-J-F3 write, or a
//! user deleted after a valid write, leaves behind.
//!
//! Its own test binary, so `egret-test-db` gives it a private database; tests
//! in it run concurrently, so each one uses unique names.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test allocate_username

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::session::SessionService;
use egret_metastore::Db;
use egret_perm::PermStore;
use egret_perm_compile::{perm_router, PermState};
use egret_policy::PolicyStore;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt; // oneshot
use uuid::Uuid;

/// A short unique token, lowercase alphanumerics only: a valid email local
/// part that `ensure_username_via` slugifies to itself.
fn tag() -> String {
    Uuid::new_v4().simple().to_string()[..12].to_string()
}

struct Env {
    db: Db,
    app: axum::Router,
    /// `platform_admin`.
    admin: String,
    /// A session holding exactly `policy:update`.
    updater: String,
    /// A session holding exactly `policy:read`.
    reader: String,
    /// A session holding no permission at all.
    nobody: String,
    catalog_id: Uuid,
}

async fn insert_user(pool: &sqlx::PgPool, provider: &str, email: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ($1, $2, 'Test') RETURNING id",
    )
    .bind(provider)
    .bind(email)
    .fetch_one(pool)
    .await
    .expect("insert user")
}

async fn session(db: &Db, sessions: &SessionService, permission: Option<&str>) -> String {
    let t = tag();
    let uid = insert_user(&db.pool, "local", &format!("alloc_gate_{t}@egret.local")).await;
    if let Some(permission) = permission {
        let role_id: Uuid = sqlx::query_scalar(
            "INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id",
        )
        .bind(format!("alloc_gate_role_{t}"))
        .fetch_one(&db.pool)
        .await
        .expect("insert role");
        sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1, $2)")
            .bind(role_id)
            .bind(permission)
            .execute(&db.pool)
            .await
            .expect("grant permission");
        sqlx::query(
            "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
             VALUES ('user', $1, $2, 'platform')",
        )
        .bind(uid)
        .bind(role_id)
        .execute(&db.pool)
        .await
        .expect("insert access grant");
    }
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
    let pool = &db.pool;
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::minutes(60),
    );
    let updater = session(&db, &sessions, Some("policy:update")).await;
    let reader = session(&db, &sessions, Some("policy:read")).await;
    let nobody = session(&db, &sessions, None).await;
    let admin_id = insert_user(pool, "local", &format!("alloc_admin_{}@egret.local", tag())).await;
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $1, id, 'platform', NULL FROM roles \
          WHERE scope='platform' AND name='platform_admin'",
    )
    .bind(admin_id)
    .execute(pool)
    .await
    .expect("grant platform_admin");
    let admin = sessions
        .create(egret_core::UserId(admin_id), "test", "127.0.0.1")
        .await
        .expect("session")
        .token;

    // A grant needs a registry catalog; nothing here compiles.
    let workspace_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(format!("alloc-ws-{}", tag()))
            .bind("Alloc WS")
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-alloc-test','AKIATEST') RETURNING id",
    )
    .bind(format!("alloc-sc-{}", tag()))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(format!("alloc_wh_{}", tag()))
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
    .bind(format!("alloc_cat_{}", tag()))
    .bind(warehouse_id)
    .fetch_one(pool)
    .await
    .expect("registry catalog");

    // Built AFTER every permission above is written: the PDP loads them once.
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );
    let app = perm_router().with_state(PermState {
        perm: PermStore::new(pool.clone()),
        policy: PolicyStore::new(db.clone()),
        sessions,
        pdp,
    });
    Some(Env {
        db,
        app,
        admin,
        updater,
        reader,
        nobody,
        catalog_id,
    })
}

async fn allocate(e: &Env, token: &str, user_id: Uuid) -> (StatusCode, Value) {
    let resp = e
        .app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri(format!("/perm/people/users/{user_id}/username"))
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

async fn stored_username(e: &Env, user_id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT username::text FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&e.db.pool)
        .await
        .expect("read username")
}

/// A grant seeded behind the write path's back: `principals` is `(kind, ref, mode)`.
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

async fn seed_rule(e: &Env, exempt: &[(&str, &str, &str)]) -> Uuid {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO perm.rules (name, kind, mask_type, selector_kind, catalog_id, \
                                 schema_name, table_name, column_name) \
         VALUES ($1, 'mask', 'MASK', 'path', $2, 's', 't', 'c') RETURNING id",
    )
    .bind(format!("alloc_rule_{}", tag()))
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
    id
}

#[tokio::test]
async fn allocates_the_name_ensure_username_would_and_a_second_call_returns_it_unchanged() {
    let Some(e) = env().await else { return };
    let t = tag();
    let user = insert_user(&e.db.pool, "local", &format!("alloc_{t}@egret.local")).await;
    // A stored ref to SOMEONE ELSE's name, and a group ref spelled like this
    // user's name: neither names this user as a `user`, so neither refuses.
    seed_grant(
        &e,
        "allow",
        &[
            ("user", &format!("alloc_{t}-other"), "include"),
            ("group", &format!("alloc_{t}"), "include"),
        ],
    )
    .await;

    let (status, body) = allocate(&e, &e.updater, user).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // `ensure_username_via` slugifies the email local part, which is already
    // a slug here, so the allocated name is exactly it.
    let expected = format!("alloc_{t}");
    assert_eq!(body, json!({ "username": expected, "allocated": true }));
    assert_eq!(stored_username(&e, user).await.as_deref(), Some(&*expected));

    let (status, body) = allocate(&e, &e.updater, user).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({ "username": expected, "allocated": false }));
}

#[tokio::test]
async fn a_collision_allocates_the_next_free_name_as_ensure_username_does() {
    let Some(e) = env().await else { return };
    let t = tag();
    let first = insert_user(&e.db.pool, "local", &format!("coll_{t}@egret.local")).await;
    let second = insert_user(&e.db.pool, "local", &format!("coll_{t}@other.local")).await;
    let (status, body) = allocate(&e, &e.admin, first).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["username"], json!(format!("coll_{t}")));
    let (status, body) = allocate(&e, &e.admin, second).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({ "username": format!("coll_{t}-2"), "allocated": true })
    );
}

#[tokio::test]
async fn a_stored_grant_principal_naming_the_new_name_refuses_and_allocates_nothing() {
    let Some(e) = env().await else { return };
    let t = tag();
    let user = insert_user(&e.db.pool, "local", &format!("stale_{t}@egret.local")).await;
    let name = format!("stale_{t}");
    // An allow grant's exclude: today it subtracts nobody; allocating the name
    // would silently subtract this person.
    let grant = seed_grant(
        &e,
        "allow",
        &[("group", "public", "include"), ("user", &name, "exclude")],
    )
    .await;

    let (status, body) = allocate(&e, &e.updater, user).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"].as_str().is_some_and(|s| s.contains(&name)),
        "{body}"
    );
    assert_eq!(
        body["refs"],
        json!([{ "source": "grant", "id": grant, "position": "exclude", "ref": name }])
    );
    assert_eq!(
        stored_username(&e, user).await,
        None,
        "the allocation rolled back"
    );
}

#[tokio::test]
async fn a_stored_rule_exemption_naming_the_new_name_refuses_and_allocates_nothing() {
    let Some(e) = env().await else { return };
    let t = tag();
    let user = insert_user(&e.db.pool, "local", &format!("exempt_{t}@egret.local")).await;
    let name = format!("exempt_{t}");
    let rule = seed_rule(&e, &[("user", &name, "include")]).await;

    let (status, body) = allocate(&e, &e.updater, user).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        body["refs"],
        json!([{ "source": "rule", "id": rule, "position": "include", "ref": name }])
    );
    assert_eq!(
        stored_username(&e, user).await,
        None,
        "the allocation rolled back"
    );
}

/// `users.username` is citext and the write path resolves a `user` ref
/// case-insensitively, so to egret a stored `Case_x` names whoever holds
/// `case_x` (and the next edit's re-resolution would rewrite it to them). The
/// guard therefore refuses on a case variant too, and lists it as stored.
#[tokio::test]
async fn a_stored_case_variant_of_the_new_name_also_refuses() {
    let Some(e) = env().await else { return };
    let t = tag();
    let user = insert_user(&e.db.pool, "local", &format!("case_{t}@egret.local")).await;
    let variant = format!("CASE_{t}");
    let grant = seed_grant(&e, "deny", &[("user", &variant, "include")]).await;
    let rule = seed_rule(&e, &[("user", &variant, "exclude")]).await;

    let (status, body) = allocate(&e, &e.admin, user).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        body["refs"],
        json!([
            { "source": "grant", "id": grant, "position": "include", "ref": variant },
            { "source": "rule", "id": rule, "position": "exclude", "ref": variant },
        ])
    );
    assert_eq!(stored_username(&e, user).await, None);
}

/// A ref naming a username the user ALREADY holds is not stale: it applies to
/// them today, and nothing is allocated.
#[tokio::test]
async fn a_user_who_already_has_a_username_is_answered_unchanged_whatever_names_it() {
    let Some(e) = env().await else { return };
    let t = tag();
    let user = insert_user(&e.db.pool, "local", &format!("has_{t}@egret.local")).await;
    sqlx::query("UPDATE users SET username = $2 WHERE id = $1")
        .bind(user)
        .bind(format!("has_{t}"))
        .execute(&e.db.pool)
        .await
        .unwrap();
    seed_grant(&e, "allow", &[("user", &format!("has_{t}"), "include")]).await;

    let (status, body) = allocate(&e, &e.updater, user).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({ "username": format!("has_{t}"), "allocated": false })
    );
}

/// A service account is a `users` row engines enforce grants on like any user,
/// and it gets the same name from `ensure_username` at its first engine token.
#[tokio::test]
async fn a_service_account_is_allocated_like_any_user() {
    let Some(e) = env().await else { return };
    let t = tag();
    let sa = insert_user(&e.db.pool, "service", &format!("svc_{t}@service.local")).await;
    let (status, body) = allocate(&e, &e.updater, sa).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({ "username": format!("svc_{t}"), "allocated": true })
    );
}

#[tokio::test]
async fn an_unknown_user_is_404() {
    let Some(e) = env().await else { return };
    let (status, body) = allocate(&e, &e.admin, Uuid::new_v4()).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(body["error"].is_string(), "{body}");
}

#[tokio::test]
async fn needs_policy_update_and_policy_read_is_not_enough() {
    let Some(e) = env().await else { return };
    let user = insert_user(&e.db.pool, "local", &format!("gate_{}@egret.local", tag())).await;
    for token in [&e.reader, &e.nobody] {
        let (status, body) = allocate(&e, token, user).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    }
    assert_eq!(
        stored_username(&e, user).await,
        None,
        "a refusal allocates nothing"
    );
    // Unknown user: still 403 first, so the gate reveals nothing about ids.
    let (status, _) = allocate(&e, &e.reader, Uuid::new_v4()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// Two stewards picking the same user at once: one allocates, the other is
/// answered the same name with `allocated: false` — never two names, never
/// two `allocated: true`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_allocations_for_one_user_allocate_once() {
    let Some(e) = env().await else { return };
    let t = tag();
    let user = insert_user(&e.db.pool, "local", &format!("race_{t}@egret.local")).await;
    let e = Arc::new(e);
    let mut set = tokio::task::JoinSet::new();
    for _ in 0..6 {
        let e = e.clone();
        set.spawn(async move { allocate(&e, &e.updater, user).await });
    }
    let answers = set.join_all().await;
    let mut allocated = 0;
    for (status, body) in &answers {
        assert_eq!(*status, StatusCode::OK, "{body}");
        assert_eq!(body["username"], json!(format!("race_{t}")), "{body}");
        if body["allocated"] == json!(true) {
            allocated += 1;
        }
    }
    assert_eq!(allocated, 1, "{answers:?}");
}

/// Another allocation of the same user in flight (a first query's
/// `ensure_username`, a compile minting a group member) when the route reads:
/// the route waits for it and answers ITS name, `allocated: false` — it never
/// claims an allocation it did not make, nor runs its guard against a name
/// someone else chose. Deterministic: the other writer is held open by hand.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_allocation_in_flight_elsewhere_is_waited_for_and_not_claimed() {
    let Some(e) = env().await else { return };
    let t = tag();
    let user = insert_user(&e.db.pool, "local", &format!("inflight_{t}@egret.local")).await;
    let theirs = format!("inflight_{t}-theirs");
    let mut other = e.db.pool.begin().await.unwrap();
    sqlx::query("UPDATE users SET username = $2 WHERE id = $1 AND username IS NULL")
        .bind(user)
        .bind(&theirs)
        .execute(&mut *other)
        .await
        .unwrap();

    let e = Arc::new(e);
    let call = {
        let e = e.clone();
        tokio::spawn(async move { allocate(&e, &e.updater, user).await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(
        !call.is_finished(),
        "the route must wait for the in-flight writer"
    );
    other.commit().await.unwrap();

    let (status, body) = call.await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({ "username": theirs, "allocated": false }));
}
