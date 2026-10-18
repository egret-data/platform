//! F11 Task 5 (spec E16, §4.5): an engine grant counts as a reference
//! everywhere a grant does — the group delete/rename refusal, the group and
//! role membership recompiles, `/perm/unresolved-principals`, and the username
//! allocation's stale-ref 409 — and (spec E21, amended) deleting the user an
//! impersonation grant targets deletes the grant AND reaps its compiled policy
//! in the same transaction, so a re-minted username inherits nothing.
//!
//! Every assertion is scoped to this fixture's own rows: tests in one binary
//! share a database.
//!
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test engine_grants_references

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request as HttpRequest, StatusCode};
use egret_auth::groups::{GroupRepo, ReferenceDetail};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_core::UserId;
use egret_metastore::Db;
use egret_perm::{
    create_engine_grant_in_tx, delete_engine_grant_in_tx, EngineGrant, PermStore, Principal,
    PrincipalKind, PrincipalSet,
};
use egret_perm_compile::{
    assign_platform_role_and_recompile, group_references_via, perm_router, recompile_service,
    PermGroupRecompiler, PermState,
};
use egret_policy::PolicyStore;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Env {
    db: Db,
    app: axum::Router,
    admin: String,
    perm: PermStore,
    policy: PolicyStore,
    groups: GroupRepo,
    profile: i64,
    engine: Uuid,
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
    let admin_id = user(pool, None).await;
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
        .create(UserId(admin_id), "test", "127.0.0.1")
        .await
        .expect("session")
        .token;
    let pdp: Arc<dyn egret_core::authz::Pdp> = Arc::new(
        egret_auth::pdp::CedarPdp::build(db.clone(), &egret_events::EventBus::new())
            .await
            .expect("build CedarPdp"),
    );

    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_starrocks(&policy)
        .await
        .expect("starrocks def");
    let profile = policy
        .create_service(&unique("egref-sr-"), "starrocks", &json!({}))
        .await
        .expect("profile")
        .id;
    let engine: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1, 'starrocks', 'egret', '{}'::jsonb, $2) RETURNING id",
    )
    .bind(unique("egref-eng-"))
    .bind(profile)
    .fetch_one(pool)
    .await
    .expect("engine");

    let app = perm_router().with_state(PermState {
        perm: PermStore::new(pool.clone()),
        policy: policy.clone(),
        sessions,
        pdp,
    });
    Some(Env {
        perm: PermStore::new(pool.clone()),
        policy,
        groups: GroupRepo::new(db.clone()),
        db,
        app,
        admin,
        profile,
        engine,
    })
}

async fn user(pool: &PgPool, username: Option<&str>) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) \
         VALUES ('local', $1, 'Engine grant ref test', $2) RETURNING id",
    )
    .bind(format!("{}@egret.local", unique("egref")))
    .bind(username)
    .fetch_one(pool)
    .await
    .expect("user")
}

async fn person(pool: &PgPool) -> (Uuid, String) {
    let name = unique("egref-u-");
    (user(pool, Some(&name)).await, name)
}

async fn group_with(pool: &PgPool, members: &[Uuid]) -> (Uuid, String) {
    let name = unique("egref-g-");
    let gid: Uuid =
        sqlx::query_scalar("INSERT INTO groups (name, source) VALUES ($1, 'local') RETURNING id")
            .bind(&name)
            .fetch_one(pool)
            .await
            .expect("group");
    for m in members {
        sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
            .bind(m)
            .bind(gid)
            .execute(pool)
            .await
            .expect("member");
    }
    (gid, name)
}

fn p(kind: PrincipalKind, r: &str) -> Principal {
    Principal {
        kind,
        r#ref: r.to_string(),
    }
}

async fn create(
    e: &Env,
    capability: &str,
    values: &[&str],
    target: Option<Uuid>,
    include: Vec<Principal>,
) -> Uuid {
    let g = EngineGrant {
        id: Uuid::nil(),
        engine_id: e.engine,
        capability: capability.into(),
        values: values.iter().map(|v| v.to_string()).collect(),
        target_user_id: target,
        principals: PrincipalSet {
            include,
            exclude: vec![],
        },
    };
    let mut tx = e.db.pool.begin().await.expect("begin");
    let id = create_engine_grant_in_tx(&mut tx, &g)
        .await
        .expect("create");
    tx.commit().await.expect("commit");
    recompile_service(&e.perm, &e.policy, e.profile)
        .await
        .expect("recompile");
    id
}

async fn delete(e: &Env, id: Uuid) {
    let mut tx = e.db.pool.begin().await.expect("begin");
    delete_engine_grant_in_tx(&mut tx, id)
        .await
        .expect("delete");
    tx.commit().await.expect("commit");
    recompile_service(&e.perm, &e.policy, e.profile)
        .await
        .expect("recompile");
}

/// The users of the compiled `perm-engine-{id}`, or `None` when absent.
async fn compiled_users(e: &Env, id: Uuid) -> Option<Vec<String>> {
    let body: Option<Value> = sqlx::query_scalar(
        "SELECT policy_jsonb FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(e.profile)
    .bind(format!("perm-engine-{id}"))
    .fetch_optional(&e.db.pool)
    .await
    .expect("read");
    body.map(|b| {
        b["policyItems"][0]["users"]
            .as_array()
            .expect("users")
            .iter()
            .map(|u| u.as_str().unwrap().to_string())
            .collect()
    })
}

fn conflict_message(e: egret_core::Error) -> String {
    match e {
        egret_core::Error::Conflict(m) => m,
        other => panic!("expected Conflict, got {other:?}"),
    }
}

async fn call(e: &Env, method: &str, uri: &str) -> (StatusCode, Value) {
    let resp = e
        .app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method(method)
                .uri(uri)
                .header("Cookie", format!("egret_session={}", e.admin))
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

#[tokio::test]
async fn a_group_named_by_an_engine_grant_is_a_reference_and_is_guarded() {
    let Some(e) = env().await else { return };
    let pool = &e.db.pool;
    let (ua, _) = person(pool).await;
    let (gid, g1) = group_with(pool, &[ua]).await;
    let id = create(
        &e,
        "starrocks.manage_blacklist",
        &[],
        None,
        vec![p(PrincipalKind::Group, &g1)],
    )
    .await;

    let mut conn = pool.acquire().await.expect("conn");
    let refs = group_references_via(&mut conn, std::slice::from_ref(&g1))
        .await
        .expect("references");
    drop(conn);
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].source, "engine_grant");
    assert_eq!(refs[0].id, id);
    assert_eq!(refs[0].effect.as_deref(), Some("allow"));
    assert_eq!(refs[0].position, "include");

    let msg = conflict_message(
        e.groups
            .delete_local(gid, &PermGroupRecompiler, ReferenceDetail::Entries)
            .await
            .expect_err("a delete of a group an engine grant names is refused"),
    );
    assert!(
        msg.contains(&format!("engine grant {id} (allow, include)")),
        "{msg}"
    );
    let msg = conflict_message(
        e.groups
            .rename(
                gid,
                &unique("egref-new-"),
                &PermGroupRecompiler,
                ReferenceDetail::Entries,
            )
            .await
            .expect_err("a rename too"),
    );
    assert!(
        msg.contains(&format!("engine grant {id} (allow, include)")),
        "{msg}"
    );

    delete(&e, id).await;
    e.groups
        .rename(
            gid,
            &unique("egref-new-"),
            &PermGroupRecompiler,
            ReferenceDetail::Entries,
        )
        .await
        .expect("rename once nothing names it");
    e.groups
        .delete_local(gid, &PermGroupRecompiler, ReferenceDetail::Entries)
        .await
        .expect("delete once nothing names it");
}

#[tokio::test]
async fn a_membership_change_recompiles_the_engines_profile() {
    let Some(e) = env().await else { return };
    let pool = &e.db.pool;
    let (ua, a) = person(pool).await;
    let (ub, b) = person(pool).await;
    let (gid, g1) = group_with(pool, &[ua]).await;
    let group_grant = create(
        &e,
        "starrocks.manage_blacklist",
        &[],
        None,
        vec![p(PrincipalKind::Group, &g1)],
    )
    .await;
    assert_eq!(compiled_users(&e, group_grant).await, Some(vec![a.clone()]));

    // No explicit recompile: the membership write must do it.
    e.groups
        .add_member(gid, UserId(ub), &PermGroupRecompiler)
        .await
        .expect("add member");
    let mut want = vec![a.clone(), b.clone()];
    want.sort();
    assert_eq!(compiled_users(&e, group_grant).await, Some(want));

    let role = unique("egref-r-");
    let role_id: Uuid =
        sqlx::query_scalar("INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id")
            .bind(&role)
            .fetch_one(pool)
            .await
            .expect("role");
    let role_grant = create(
        &e,
        "starrocks.use_global_function",
        &["f(INT)"],
        None,
        vec![p(PrincipalKind::Role, &role)],
    )
    .await;
    assert_eq!(
        compiled_users(&e, role_grant).await,
        None,
        "nobody holds it yet"
    );
    assign_platform_role_and_recompile(pool, "user", ua, role_id)
        .await
        .expect("assign role");
    assert_eq!(compiled_users(&e, role_grant).await, Some(vec![a]));
}

#[tokio::test]
async fn an_unresolved_engine_grant_principal_is_reported_and_never_fails_open() {
    let Some(e) = env().await else { return };
    let ghost = unique("egref-ghost-");
    let id = create(
        &e,
        "starrocks.manage_blacklist",
        &[],
        None,
        vec![p(PrincipalKind::User, &ghost)],
    )
    .await;
    let (status, body) = call(&e, "GET", "/perm/unresolved-principals").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rows: Vec<&Value> = body
        .as_array()
        .expect("array")
        .iter()
        .filter(|r| r["id"] == json!(id))
        .collect();
    assert_eq!(rows.len(), 1, "{body}");
    assert_eq!(rows[0]["source"], "engine_grant");
    assert_eq!(rows[0]["ref"], json!(ghost));
    assert_eq!(rows[0]["position"], "include");
    assert_eq!(rows[0]["effect"], "allow");
    assert_eq!(rows[0]["status"], "unresolved");
    assert_eq!(rows[0]["failsOpen"], false);
}

#[tokio::test]
async fn allocating_a_username_an_engine_grant_principal_names_is_refused() {
    let Some(e) = env().await else { return };
    let pool = &e.db.pool;
    let t = unique("");
    let target: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ('local', $1, 'Alloc') RETURNING id",
    )
    .bind(format!("egalloc{t}@egret.local"))
    .fetch_one(pool)
    .await
    .expect("user");
    let name = format!("egalloc{t}");
    let id = create(
        &e,
        "starrocks.manage_blacklist",
        &[],
        None,
        vec![p(PrincipalKind::User, &name)],
    )
    .await;
    let (status, body) = call(&e, "POST", &format!("/perm/people/users/{target}/username")).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        body["refs"],
        json!([{ "source": "engine_grant", "id": id, "position": "include", "ref": name }])
    );
    let stored: Option<String> =
        sqlx::query_scalar("SELECT username::text FROM users WHERE id = $1")
            .bind(target)
            .fetch_one(pool)
            .await
            .expect("read");
    assert_eq!(stored, None, "the allocation rolled back");
}

/// The amended E21. The target is stored by id, so the grant cannot name
/// whoever is next minted the freed username; and the user's hard delete
/// recompiles the profile in its own transaction, so no compiled policy keeps
/// naming the freed username either.
#[tokio::test]
async fn deleting_the_target_user_deletes_the_grant_and_reaps_its_policy_at_once() {
    let Some(e) = env().await else { return };
    let pool = &e.db.pool;
    let (_ua, a) = person(pool).await;
    let (uc, _c) = person(pool).await;
    let id = create(
        &e,
        "starrocks.impersonate_user",
        &[],
        Some(uc),
        vec![p(PrincipalKind::User, &a)],
    )
    .await;
    assert_eq!(compiled_users(&e, id).await, Some(vec![a]));

    UserRepo::new(e.db.clone())
        .hard_delete(UserId(uc), &PermGroupRecompiler)
        .await
        .expect("hard delete");

    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM perm.engine_grants WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("count");
    assert_eq!(left, 0, "the grant went with its target");
    assert_eq!(
        compiled_users(&e, id).await,
        None,
        "no explicit recompile: the delete itself reaped the compiled policy"
    );
}

/// Security review M1: the user delete reads which profile each targeting
/// grant compiles into, so it holds those engines' rows `FOR SHARE` (group
/// locks, then engine rows, then profiles: the write path's order). An
/// uncommitted rebind of the engine (an UPDATE of its row) makes the delete
/// wait, so it recompiles the profile the engine is bound to once the rebind
/// lands, never the one it left.
#[tokio::test]
async fn a_user_delete_waits_on_an_uncommitted_rebind_of_a_targeting_grants_engine() {
    let Some(e) = env().await else { return };
    let pool = &e.db.pool;
    let (_ua, a) = person(pool).await;
    let (uc, _c) = person(pool).await;
    create(
        &e,
        "starrocks.impersonate_user",
        &[],
        Some(uc),
        vec![p(PrincipalKind::User, &a)],
    )
    .await;
    let mut rebind = pool.begin().await.expect("begin");
    sqlx::query("UPDATE engine_mgmt.engines SET authz_service_id = authz_service_id WHERE id = $1")
        .bind(e.engine)
        .execute(&mut *rebind)
        .await
        .expect("rebind");
    let db = e.db.clone();
    let h = tokio::spawn(async move {
        UserRepo::new(db)
            .hard_delete(UserId(uc), &PermGroupRecompiler)
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    assert!(!h.is_finished(), "the delete must wait on the engine row");
    rebind.commit().await.expect("commit");
    h.await.expect("join").expect("hard delete");
}

/// Infra-seeding gap: egret seeds every platform admin as an engine admin
/// (`egret-admin - …`) at the next engine apply. So becoming platform admin is
/// becoming near-root: the target's impersonation grant must lose its policy in
/// the same commit as the role assignment, not at some later apply.
#[tokio::test]
async fn a_target_who_becomes_platform_admin_loses_the_grant_in_the_same_commit() {
    let Some(e) = env().await else { return };
    let pool = &e.db.pool;
    let (_ua, a) = person(pool).await;
    let (uc, _c) = person(pool).await;
    let id = create(
        &e,
        "starrocks.impersonate_user",
        &[],
        Some(uc),
        vec![p(PrincipalKind::User, &a)],
    )
    .await;
    assert_eq!(compiled_users(&e, id).await, Some(vec![a]));
    let admin_role: Uuid = sqlx::query_scalar(
        "SELECT id FROM roles WHERE scope = 'platform' AND name = 'platform_admin'",
    )
    .fetch_one(pool)
    .await
    .expect("platform_admin role");
    assign_platform_role_and_recompile(pool, "user", uc, admin_role)
        .await
        .expect("assign platform_admin");
    assert_eq!(
        compiled_users(&e, id).await,
        None,
        "no explicit recompile: the role assignment reaps it"
    );
}

/// A platform admin through a GROUP (an OIDC `platform-admins` group, which
/// `platform_admin_user_ids` expands and `seed_admin_all_access` seeds) is a
/// platform admin too: refused as a target, and joining such a group reaps the
/// member's impersonation grant in the membership write's own transaction.
#[tokio::test]
async fn a_platform_admin_through_a_group_is_refused_and_joining_one_reaps() {
    let Some(e) = env().await else { return };
    let pool = &e.db.pool;
    let (_ua, a) = person(pool).await;
    let (ub, b) = person(pool).await;
    let (uc, _c) = person(pool).await;
    let (gid, _g) = group_with(pool, &[ub]).await;
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'group', $1, id, 'platform', NULL FROM roles \
          WHERE scope='platform' AND name='platform_admin'",
    )
    .bind(gid)
    .execute(pool)
    .await
    .expect("group holds platform_admin");

    // b is a platform admin through the group: refused at write.
    let (status, body) = call_json(
        &e,
        "POST",
        &format!("/perm/engines/{}/grants", e.engine),
        json!({"capability": "starrocks.impersonate_user", "values": [b],
               "principals": [{"kind": "user", "ref": a}]}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // c is not yet: the grant compiles; joining the group reaps it.
    let id = create(
        &e,
        "starrocks.impersonate_user",
        &[],
        Some(uc),
        vec![p(PrincipalKind::User, &a)],
    )
    .await;
    assert!(compiled_users(&e, id).await.is_some());
    e.groups
        .add_member(gid, UserId(uc), &PermGroupRecompiler)
        .await
        .expect("add member");
    assert_eq!(compiled_users(&e, id).await, None, "joining reaps it");
}

async fn call_json(e: &Env, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let resp = e
        .app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method(method)
                .uri(uri)
                .header("Cookie", format!("egret_session={}", e.admin))
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
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
