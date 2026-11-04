//! Username takeover, spec §4.2 (decision D2): the `egret-admin - …` policies,
//! which give every platform admin ALL access on an engine, name exactly the
//! ACTIVE platform admins, on every engine egret seeded, at every commit.
//!
//! Before the fix they were a snapshot written at engine apply only: a demoted,
//! disabled or deleted admin kept engine ALL access, and the next person minted
//! a deleted admin's freed username inherited it. Every test here asserts from
//! the side of that principal, asking the engine (`egret-ranger-eval` over the
//! profile's download) right after the production entry point returns.
//!
//! Its own binary: the admin set is installation-wide, so the tests hold
//! [`SERIAL`] and each starts by demoting every admin a previous test made.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test admin_policy_sync

#[path = "support/engine_eval.rs"]
mod engine_eval;

use egret_auth::groups::{GroupRepo, ReferenceDetail};
use egret_auth::roles::engine_admin_user_ids_via;
use egret_auth::users::UserRepo;
use egret_core::UserId;
use egret_metastore::Db;
use egret_perm::PermStore;
use egret_perm_compile::infra::infra_services;
use egret_perm_compile::{
    assign_platform_role_and_recompile, converge_infra_policies, converge_infra_policy,
    recompile_service, set_user_status_and_recompile, unassign_platform_role_and_recompile,
    PermGroupRecompiler,
};
use egret_policy::default_policies::admin_all_access_bodies;
use egret_policy::PolicyStore;
use engine_eval::{allowed, starrocks_decides, trino_decides};
use serde_json::Value;
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn test_db() -> Option<Db> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 8).await.expect("connect");
    db.migrate().await.expect("migrate");
    // No admin a previous test in this binary made survives into the next.
    sqlx::query(
        "DELETE FROM access_grants WHERE role_id = \
           (SELECT id FROM roles WHERE name = 'platform_admin' AND scope = 'platform')",
    )
    .execute(&db.pool)
    .await
    .expect("demote every admin");
    // Every test starts with no profile whose compile is broken
    // ([`break_compiles_of`]).
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS test_broken_compiles (service_id bigint PRIMARY KEY); \
         DELETE FROM test_broken_compiles; \
         CREATE OR REPLACE FUNCTION test_break_compile() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN \
             IF NEW.service_id IN (SELECT service_id FROM test_broken_compiles) THEN \
                 RAISE EXCEPTION 'the test broke every compile of profile %', NEW.service_id; \
             END IF; \
             RETURN NEW; \
         END $$; \
         CREATE OR REPLACE TRIGGER test_break_compile BEFORE INSERT OR UPDATE ON perm.compile_runs \
             FOR EACH ROW EXECUTE FUNCTION test_break_compile();",
    )
    .execute(&db.pool)
    .await
    .expect("compile breaker");
    Some(db)
}

/// Make every compile of `service_id` fail inside its transaction: the
/// compile's `perm.compile_runs` write raises, as a DB error, a refused model
/// or a failed mint would. The `egret-admin` sync never writes that table.
async fn break_compiles_of(db: &Db, service_id: i64) {
    sqlx::query("INSERT INTO test_broken_compiles (service_id) VALUES ($1)")
        .bind(service_id)
        .execute(&db.pool)
        .await
        .expect("break the profile's compile");
    // The arrangement: its compile does fail.
    recompile_service(
        &PermStore::new(db.pool.clone()),
        &PolicyStore::new(db.clone()),
        service_id,
    )
    .await
    .expect_err("the profile's compile is broken");
}

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

async fn new_user(db: &Db, username: Option<&str>) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) VALUES ('local',$1,'T',$2) RETURNING id",
    )
    .bind(format!("{}@aps.test", unique("u")))
    .bind(username)
    .fetch_one(&db.pool)
    .await
    .expect("insert user")
}

async fn platform_admin_role(db: &Db) -> Uuid {
    sqlx::query_scalar("SELECT id FROM roles WHERE name = 'platform_admin' AND scope = 'platform'")
        .fetch_one(&db.pool)
        .await
        .expect("the seeded platform_admin role")
}

async fn make_admin_directly(db: &Db, user: Uuid) {
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
         VALUES ('user', $1, $2, 'platform')",
    )
    .bind(user)
    .bind(platform_admin_role(db).await)
    .execute(&db.pool)
    .await
    .expect("admin grant");
}

/// A group (`source` `local` or `oidc`) holding platform_admin, with
/// `members`. Returns `(id, name)`.
async fn admin_group(db: &Db, source: &str, members: &[Uuid]) -> (Uuid, String) {
    let name = unique("admins");
    let gid: Uuid =
        sqlx::query_scalar("INSERT INTO groups (name, source) VALUES ($1, $2) RETURNING id")
            .bind(&name)
            .bind(source)
            .fetch_one(&db.pool)
            .await
            .expect("group");
    for m in members {
        sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
            .bind(m)
            .bind(gid)
            .execute(&db.pool)
            .await
            .expect("member");
    }
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
         VALUES ('group', $1, $2, 'platform')",
    )
    .bind(gid)
    .bind(platform_admin_role(db).await)
    .execute(&db.pool)
    .await
    .expect("group admin grant");
    (gid, name)
}

async fn set_status(db: &Db, user: Uuid, status: &str) {
    sqlx::query("UPDATE users SET status = $2 WHERE id = $1")
        .bind(user)
        .bind(status)
        .execute(&db.pool)
        .await
        .expect("status");
}

/// A Trino or StarRocks profile with `egret-admin` seeded for `users`, as
/// engine apply seeds it, and a qualifying engine bound to it (convergence
/// keeps `egret-admin` only on such a profile, governance SP2a D2).
async fn seeded_service(db: &Db, service_type: &str, users: &[String]) -> i64 {
    let svc = seeded_rows_only(db, service_type, users).await;
    bind_qualifying_engine(db, svc, service_type).await;
    svc
}

/// A provisioned engine of `service_type` that qualifies for infrastructure
/// policies (a Trino one in the default impersonate mode, a StarRocks one in
/// impersonate mode), bound to `svc`.
async fn bind_qualifying_engine(db: &Db, svc: i64, service_type: &str) {
    let spec = if service_type == "starrocks" {
        serde_json::json!({"starrocksConfig": {"delegation": {"mode": "impersonate"}}})
    } else {
        serde_json::json!({})
    };
    sqlx::query(
        "INSERT INTO engine_mgmt.engines (id, name, type, namespace, spec, authz_service_id) \
         VALUES (gen_random_uuid(), $1, $2, 'egret', $3, $4)",
    )
    .bind(unique("aps-engine").replace('_', "-"))
    .bind(service_type)
    .bind(spec)
    .bind(svc)
    .execute(&db.pool)
    .await
    .expect("engine");
}

/// [`seeded_service`] without the engine.
async fn seeded_rows_only(db: &Db, service_type: &str, users: &[String]) -> i64 {
    let policy = PolicyStore::new(db.clone());
    if service_type == "starrocks" {
        egret_policy::seed_starrocks(&policy).await.expect("def");
    } else {
        egret_policy::seed_trino(&policy).await.expect("def");
    }
    let name = unique(&format!("aps_{service_type}"));
    let svc = policy
        .create_service(&name, service_type, &serde_json::json!({}))
        .await
        .expect("service");
    let n = policy
        .seed_admin_all_access(svc.id, &name, service_type, users)
        .await
        .expect("seed");
    assert!(n > 0, "the seed wrote the egret-admin set");
    svc.id
}

async fn admin_bodies(db: &Db, service_id: i64) -> Vec<(String, Value)> {
    sqlx::query_as(
        "SELECT name, policy_jsonb FROM policy.authz_policies \
          WHERE service_id = $1 AND name LIKE 'egret-admin - %' ORDER BY name",
    )
    .bind(service_id)
    .fetch_all(&db.pool)
    .await
    .expect("bodies")
}

fn named(body: &Value) -> Vec<String> {
    body["policyItems"][0]["users"]
        .as_array()
        .expect("users")
        .iter()
        .map(|u| u.as_str().unwrap().to_string())
        .collect()
}

async fn version_and_updated(
    db: &Db,
    service_id: i64,
) -> (i64, Vec<chrono::DateTime<chrono::Utc>>) {
    let v: i64 =
        sqlx::query_scalar("SELECT policy_version FROM policy.authz_services WHERE id = $1")
            .bind(service_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let u: Vec<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT updated_at FROM policy.authz_policies \
          WHERE service_id = $1 AND name LIKE 'egret-admin - %' ORDER BY name",
    )
    .bind(service_id)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    (v, u)
}

async fn trino_can_select(db: &Db, service_id: i64, user: &str) -> bool {
    allowed(
        &trino_decides(
            db,
            service_id,
            user,
            "select",
            &[
                ("catalog", "c"),
                ("schema", "s"),
                ("table", "t"),
                ("column", "x"),
            ],
        )
        .await,
    )
}

async fn starrocks_can_select(db: &Db, service_id: i64, user: &str) -> bool {
    allowed(
        &starrocks_decides(
            db,
            service_id,
            user,
            "select",
            &[
                ("catalog", "c"),
                ("database", "s"),
                ("table", "t"),
                ("column", "x"),
            ],
        )
        .await,
    )
}

// ── part 1: one engine-admin set, one in-transaction writer (Task 3) ─────

#[tokio::test]
async fn the_engine_admin_set_excludes_disabled_admins() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    let a = new_user(&db, None).await;
    let b = new_user(&db, None).await;
    let g = new_user(&db, None).await;
    make_admin_directly(&db, a).await;
    make_admin_directly(&db, b).await;
    admin_group(&db, "local", &[g]).await;
    set_status(&db, b, "disabled").await;

    let mut conn = db.pool.acquire().await.unwrap();
    let got = engine_admin_user_ids_via(&mut conn).await.expect("set");
    let mut want = vec![UserId(a), UserId(g)];
    want.sort_by_key(|u| u.0);
    assert_eq!(got, want, "a disabled admin is no engine admin");
}

#[tokio::test]
async fn an_empty_admin_set_removes_the_rows() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    let zed = unique("zed");
    let trino = seeded_service(&db, "trino", std::slice::from_ref(&zed)).await;
    let sr = seeded_service(&db, "starrocks", std::slice::from_ref(&zed)).await;
    assert!(trino_can_select(&db, trino, &zed).await, "the arrangement");
    assert!(starrocks_can_select(&db, sr, &zed).await, "the arrangement");

    // No one is an admin (`test_db` demoted them all).
    for svc in [trino, sr] {
        let seeded: Vec<String> = admin_bodies(&db, svc)
            .await
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert!(!seeded.is_empty(), "the arrangement");
        let (v0, _) = version_and_updated(&db, svc).await;
        let sync = converge_infra_policy(&db.pool, svc)
            .await
            .expect("converge");
        assert_eq!(
            sync.removed, seeded,
            "every egret-admin row goes when there are no admins (D5 (e))"
        );
        assert!(
            admin_bodies(&db, svc).await.is_empty(),
            "an egret-admin row survived an empty admin set"
        );
        assert_eq!(version_and_updated(&db, svc).await.0, v0 + 1, "bumped once");
    }
    assert!(
        !trino_can_select(&db, trino, &zed).await,
        "an ex-admin keeps ALL access on Trino"
    );
    assert!(
        !starrocks_can_select(&db, sr, &zed).await,
        "an ex-admin keeps ALL access on StarRocks"
    );
}

#[tokio::test]
async fn an_unchanged_set_writes_nothing() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    let ann = unique("ann");
    let ann_id = new_user(&db, Some(&ann)).await;
    make_admin_directly(&db, ann_id).await;
    let svc = seeded_service(&db, "trino", std::slice::from_ref(&ann)).await;
    // The arrangement: the first convergence adds the delegation baseline.
    converge_infra_policy(&db.pool, svc)
        .await
        .expect("first convergence");
    let before = version_and_updated(&db, svc).await;
    let sync = converge_infra_policy(&db.pool, svc)
        .await
        .expect("converge");
    assert!(!sync.changed);
    assert_eq!(version_and_updated(&db, svc).await, before);
}

#[tokio::test]
async fn infra_services_are_the_seeded_and_the_qualifying_ones() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    let seeded = seeded_service(&db, "trino", &[unique("x")]).await;
    let rows_only = seeded_rows_only(&db, "trino", &[unique("y")]).await;
    let policy = PolicyStore::new(db.clone());
    let engine_only = policy
        .create_service(&unique("aps_engine_only"), "trino", &serde_json::json!({}))
        .await
        .expect("service")
        .id;
    bind_qualifying_engine(&db, engine_only, "trino").await;
    let bare = policy
        .create_service(&unique("aps_bare"), "trino", &serde_json::json!({}))
        .await
        .expect("service")
        .id;
    let ids = infra_services(&db.pool).await.expect("services");
    assert!(ids.contains(&seeded), "a seeded profile with an engine");
    assert!(ids.contains(&rows_only), "a profile holding infra rows");
    assert!(
        ids.contains(&engine_only),
        "a profile with a qualifying engine"
    );
    assert!(!ids.contains(&bare), "a bare profile");
    assert!(ids.is_sorted(), "ascending: callers lock in this order");
    let mut deduped = ids.clone();
    deduped.dedup();
    assert_eq!(deduped, ids, "deduped");
}

// ── part 2: every admin change converges egret-admin in its own commit (Task 4) ──
//
// Each test seeds `egret-admin` on a Trino profile with a qualifying engine
// bound, makes the change through the production entry point, and asks the
// engine as the demoted principal, with no recompile in between.

/// An admin with a username and a Trino profile whose `egret-admin` names
/// them. Returns `(admin id, admin username, service id)`.
async fn admin_and_service(db: &Db) -> (Uuid, String, i64) {
    let name = unique("adm");
    let admin = new_user(db, Some(&name)).await;
    make_admin_directly(db, admin).await;
    let svc = seeded_service(db, "trino", std::slice::from_ref(&name)).await;
    assert!(trino_can_select(db, svc, &name).await, "the arrangement");
    (admin, name, svc)
}

#[tokio::test]
async fn unassigning_platform_admin_drops_them_from_egret_admin() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    let (admin, name, svc) = admin_and_service(&db).await;
    unassign_platform_role_and_recompile(&db.pool, "user", admin, platform_admin_role(&db).await)
        .await
        .expect("unassign");
    assert!(
        !trino_can_select(&db, svc, &name).await,
        "a demoted admin keeps engine ALL access"
    );
}

#[tokio::test]
async fn assigning_platform_admin_adds_them() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    let (_admin, _name, svc) = admin_and_service(&db).await;
    // No username yet: the sync mints one.
    let newcomer = new_user(&db, None).await;
    assign_platform_role_and_recompile(&db.pool, "user", newcomer, platform_admin_role(&db).await)
        .await
        .expect("assign");
    let username: Option<String> =
        sqlx::query_scalar("SELECT username::text FROM users WHERE id = $1")
            .bind(newcomer)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let username = username.expect("minted at the sync");
    assert!(
        trino_can_select(&db, svc, &username).await,
        "a new admin is not in egret-admin"
    );
}

#[tokio::test]
async fn leaving_a_platform_admin_group_drops_them() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    let (_admin, name, svc) = admin_and_service(&db).await;
    let gname = unique("gadm");
    let member = new_user(&db, Some(&gname)).await;
    let (gid, _) = admin_group(&db, "local", &[member]).await;
    reseed(&db, svc, &[name, gname.clone()]).await;
    assert!(trino_can_select(&db, svc, &gname).await, "the arrangement");

    GroupRepo::new(db.clone())
        .remove_member(gid, UserId(member), &PermGroupRecompiler)
        .await
        .expect("remove member");
    assert!(
        !trino_can_select(&db, svc, &gname).await,
        "an ex-member of the admin group keeps engine ALL access"
    );
}

#[tokio::test]
async fn deleting_a_platform_admin_group_drops_its_members() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    let (_admin, name, svc) = admin_and_service(&db).await;
    let gname = unique("gdel");
    let member = new_user(&db, Some(&gname)).await;
    let (gid, _) = admin_group(&db, "local", &[member]).await;
    reseed(&db, svc, &[name, gname.clone()]).await;
    assert!(trino_can_select(&db, svc, &gname).await, "the arrangement");

    GroupRepo::new(db.clone())
        .delete_local(gid, &PermGroupRecompiler, ReferenceDetail::CountOnly)
        .await
        .expect("delete group");
    assert!(
        !trino_can_select(&db, svc, &gname).await,
        "a member of a deleted admin group keeps engine ALL access"
    );
}

#[tokio::test]
async fn an_oidc_sync_that_removes_the_admin_group_drops_them() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    let (_admin, name, svc) = admin_and_service(&db).await;
    let uname = unique("oidc");
    let member = new_user(&db, Some(&uname)).await;
    admin_group(&db, "oidc", &[member]).await;
    reseed(&db, svc, &[name, uname.clone()]).await;
    assert!(trino_can_select(&db, svc, &uname).await, "the arrangement");

    // The next login claims no group.
    GroupRepo::new(db.clone())
        .sync_user_oidc_groups(UserId(member), &[], &PermGroupRecompiler)
        .await
        .expect("oidc sync");
    assert!(
        !trino_can_select(&db, svc, &uname).await,
        "a user the IdP no longer puts in the admin group keeps engine ALL access"
    );
}

#[tokio::test]
async fn deleting_an_admin_drops_them() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    let (admin, name, svc) = admin_and_service(&db).await;
    // A second admin, so the set is not empty and the body still matters.
    let other = new_user(&db, Some(&unique("adm2"))).await;
    make_admin_directly(&db, other).await;
    UserRepo::new(db.clone())
        .hard_delete(UserId(admin), &deleter(), &PermGroupRecompiler)
        .await
        .expect("delete")
        .expect("existed");
    assert!(
        !trino_can_select(&db, svc, &name).await,
        "a deleted admin's username keeps engine ALL access"
    );
}

/// The chokepoint converges BEFORE the compile reads the profile.
///
/// `compile_desired` returns early only for a profile type this compiler does
/// not target, where convergence has nothing to do either, so the ordering
/// that matters is the one the compile can see: the engine-grant loader's
/// near-root check scans the profile's stored policies, `egret-admin`
/// included. An impersonation grant whose target a stale `egret-admin` row
/// still names (ALL access) compiles only if that row was converged first;
/// converged after the compile, the target reads as near-root and the grant is
/// skipped.
#[tokio::test]
async fn a_recompile_converges_egret_admin_before_it_compiles() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    let admin = unique("adm");
    let admin_id = new_user(&db, Some(&admin)).await;
    make_admin_directly(&db, admin_id).await;
    let stale = unique("stale");
    let stale_id = new_user(&db, Some(&stale)).await;
    let grantee = unique("ua");
    new_user(&db, Some(&grantee)).await;
    // Seeded for someone who is no admin at all, as an old apply left it.
    let svc = seeded_service(&db, "trino", std::slice::from_ref(&stale)).await;
    assert!(trino_can_select(&db, svc, &stale).await, "the arrangement");
    let engine: Uuid =
        sqlx::query_scalar("SELECT id FROM engine_mgmt.engines WHERE authz_service_id = $1")
            .bind(svc)
            .fetch_one(&db.pool)
            .await
            .expect("the bound engine");
    let grant = {
        let g = egret_perm::EngineGrant {
            id: Uuid::nil(),
            engine_id: engine,
            capability: "trino.impersonate_user".into(),
            values: vec![],
            target_user_id: Some(stale_id),
            principals: egret_perm::PrincipalSet {
                include: vec![egret_perm::Principal {
                    kind: egret_perm::PrincipalKind::User,
                    r#ref: grantee.clone(),
                }],
                exclude: vec![],
            },
        };
        let mut tx = db.pool.begin().await.unwrap();
        let id = egret_perm::create_engine_grant_in_tx(&mut tx, &g)
            .await
            .expect("engine grant");
        tx.commit().await.unwrap();
        id
    };

    let summary = recompile_service(
        &PermStore::new(db.pool.clone()),
        &PolicyStore::new(db.clone()),
        svc,
    )
    .await
    .expect("recompile");

    assert!(
        !summary.skipped.iter().any(|s| s.id == grant),
        "the compile read the stale egret-admin body (its target near-root), so convergence \
         ran after it: {summary:?}"
    );
    let compiled: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(svc)
    .bind(format!("perm-engine-{grant}"))
    .fetch_optional(&db.pool)
    .await
    .unwrap();
    assert!(
        compiled.is_some(),
        "the impersonation grant did not compile"
    );
    assert!(
        !trino_can_select(&db, svc, &stale).await,
        "a recompile kept a stale egret-admin body"
    );
    assert!(
        trino_can_select(&db, svc, &admin).await,
        "the recompile's convergence did not name the real admin"
    );
}

/// Write `egret-admin` for `names` directly, as an apply would have: the
/// bodies of the service's own def, upserted `managed_by = 'infra'`.
async fn reseed(db: &Db, svc: i64, names: &[String]) {
    let mut names = names.to_vec();
    names.sort();
    let (service_name, service_type): (String, String) =
        sqlx::query_as("SELECT name, type FROM policy.authz_services WHERE id = $1")
            .bind(svc)
            .fetch_one(&db.pool)
            .await
            .expect("service");
    for (name, body) in
        admin_all_access_bodies(&service_type, &service_name, &names).expect("bodies")
    {
        sqlx::query(
            "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, managed_by) \
             VALUES ($1, $2, $3, 'infra') \
             ON CONFLICT (service_id, name) DO UPDATE \
               SET policy_jsonb = EXCLUDED.policy_jsonb, managed_by = 'infra', \
                   version = policy.authz_policies.version + 1, updated_at = now()",
        )
        .bind(svc)
        .bind(&name)
        .bind(&body)
        .execute(&db.pool)
        .await
        .expect("seed");
    }
}

fn deleter() -> egret_core::Principal {
    egret_core::Principal {
        user_id: UserId(Uuid::nil()),
        email: "deleter@aps.test".into(),
        platform_roles: vec![],
        permissions: vec![],
        group_ids: vec![],
        impersonator: None,
    }
}

// ── part 3: status and the boot pass (Task 5) ───────────────────────────

#[tokio::test]
async fn disabling_an_admin_drops_them_and_enabling_restores() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    let (admin, name, svc) = admin_and_service(&db).await;
    set_user_status_and_recompile(&db.pool, admin, "disabled")
        .await
        .expect("disable");
    assert!(
        !trino_can_select(&db, svc, &name).await,
        "a disabled admin keeps engine ALL access"
    );
    set_user_status_and_recompile(&db.pool, admin, "active")
        .await
        .expect("enable");
    assert!(
        trino_can_select(&db, svc, &name).await,
        "an enabled admin did not get engine ALL access back"
    );
}

#[tokio::test]
async fn disabling_a_group_admin_drops_them() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    let (_admin, name, svc) = admin_and_service(&db).await;
    let gname = unique("gdis");
    let member = new_user(&db, Some(&gname)).await;
    admin_group(&db, "local", &[member]).await;
    reseed(&db, svc, &[name, gname.clone()]).await;
    set_user_status_and_recompile(&db.pool, member, "disabled")
        .await
        .expect("disable");
    assert!(
        !trino_can_select(&db, svc, &gname).await,
        "a disabled group admin keeps engine ALL access"
    );
}

#[tokio::test]
async fn unassigning_platform_admin_from_a_group_drops_its_members() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    let (_admin, name, svc) = admin_and_service(&db).await;
    let gname = unique("grole");
    let member = new_user(&db, Some(&gname)).await;
    let (gid, _) = admin_group(&db, "local", &[member]).await;
    reseed(&db, svc, &[name, gname.clone()]).await;
    unassign_platform_role_and_recompile(&db.pool, "group", gid, platform_admin_role(&db).await)
        .await
        .expect("unassign from group");
    assert!(
        !trino_can_select(&db, svc, &gname).await,
        "a member of a group that lost platform_admin keeps engine ALL access"
    );
}

#[tokio::test]
async fn the_boot_pass_repairs_a_stale_body() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    let (_admin, name, svc) = admin_and_service(&db).await;
    let stranger = unique("stranger");
    // Written directly, as a pre-fix apply or a raced one left it.
    reseed(&db, svc, &[name.clone(), stranger.clone()]).await;
    assert!(
        trino_can_select(&db, svc, &stranger).await,
        "the arrangement"
    );

    assert!(converge_infra_policies(&db.pool).await.expect("boot pass") >= 1);
    assert!(
        !trino_can_select(&db, svc, &stranger).await,
        "the boot pass left a non-admin in egret-admin"
    );
    assert!(
        trino_can_select(&db, svc, &name).await,
        "the boot pass dropped a real admin"
    );

    let before = version_and_updated(&db, svc).await;
    converge_infra_policies(&db.pool)
        .await
        .expect("boot pass again");
    assert_eq!(
        version_and_updated(&db, svc).await,
        before,
        "a second pass wrote"
    );
}

// ── review follow-ups: status and boot pass never wait on a compile ──────

#[tokio::test]
async fn disabling_an_admin_does_not_depend_on_a_profile_compiling() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    let (admin, name, svc) = admin_and_service(&db).await;
    let other = unique("adm2");
    let other_id = new_user(&db, Some(&other)).await;
    make_admin_directly(&db, other_id).await;
    reseed(&db, svc, &[name.clone(), other.clone()]).await;
    break_compiles_of(&db, svc).await;

    set_user_status_and_recompile(&db.pool, admin, "disabled")
        .await
        .expect("the kill switch waits on no compile");
    let status: String = sqlx::query_scalar("SELECT status FROM users WHERE id = $1")
        .bind(admin)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(status, "disabled");
    assert!(
        !trino_can_select(&db, svc, &name).await,
        "a disabled admin keeps engine ALL access"
    );
    assert!(
        trino_can_select(&db, svc, &other).await,
        "the other admin lost engine ALL access"
    );
}

#[tokio::test]
async fn the_boot_pass_repairs_healthy_engines_when_one_cannot_compile() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    let (_admin, name, broken) = admin_and_service(&db).await;
    break_compiles_of(&db, broken).await;
    let stranger = unique("strg");
    new_user(&db, Some(&stranger)).await;
    let healthy = seeded_service(&db, "trino", &[name.clone(), stranger.clone()]).await;

    converge_infra_policies(&db.pool).await.expect("boot pass");
    assert!(
        !trino_can_select(&db, healthy, &stranger).await,
        "one broken profile kept a healthy engine's stale egret-admin"
    );
    assert!(trino_can_select(&db, healthy, &name).await);
}

/// Waits until `handle` finished or `waiters` backends of this database wait
/// on a lock, whichever comes first (a bounded poll, so a wrong guess about
/// who blocks fails the assertion, not the suite).
async fn finished_or_blocked(db: &Db, handle: &tokio::task::JoinHandle<()>, waiters: i64) {
    for _ in 0..200 {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity \
              WHERE datname = current_database() AND wait_event_type = 'Lock'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        if handle.is_finished() || n >= waiters {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn a_disable_racing_an_admin_grant_leaves_them_out_of_egret_admin() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    let (_admin, _name, svc) = admin_and_service(&db).await;
    let target_name = unique("race");
    let target = new_user(&db, Some(&target_name)).await;

    // Hold the target's row, so the disable stops at its status write.
    let mut holder = db.pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM users WHERE id = $1 FOR NO KEY UPDATE")
        .bind(target)
        .execute(&mut *holder)
        .await
        .unwrap();
    let disable = {
        let pool = db.pool.clone();
        tokio::spawn(async move {
            set_user_status_and_recompile(&pool, target, "disabled")
                .await
                .expect("disable");
        })
    };
    finished_or_blocked(&db, &disable, 1).await;
    // An admin grant for the target, concurrently: it reads the target as
    // active unless the disable has locked the admin-policy profiles first.
    let assign = {
        let pool = db.pool.clone();
        let role = platform_admin_role(&db).await;
        tokio::spawn(async move {
            assign_platform_role_and_recompile(&pool, "user", target, role)
                .await
                .expect("assign");
        })
    };
    finished_or_blocked(&db, &assign, 2).await;
    holder.rollback().await.unwrap();
    disable.await.unwrap();
    assign.await.unwrap();

    assert!(
        !trino_can_select(&db, svc, &target_name).await,
        "a disabled user holds engine ALL access"
    );
}

#[tokio::test]
async fn convergence_deletes_egret_admin_rows_its_def_no_longer_builds() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    let admin = unique("adm");
    let admin_id = new_user(&db, Some(&admin)).await;
    make_admin_directly(&db, admin_id).await;
    let stranger = unique("strg");
    // A Trino row carrying StarRocks-shaped `egret-admin` bodies (an engine
    // adopted through the StarRocks path, or an older def's hierarchies).
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_trino(&policy).await.expect("def");
    egret_policy::seed_starrocks(&policy).await.expect("def");
    let name = unique("aps_foreign");
    let svc = policy
        .create_service(&name, "trino", &serde_json::json!({}))
        .await
        .expect("service")
        .id;
    let mut both = vec![admin.clone(), stranger.clone()];
    both.sort();
    policy
        .seed_admin_all_access(svc, &name, "starrocks", &both)
        .await
        .expect("foreign seed");
    bind_qualifying_engine(&db, svc, "trino").await;

    converge_infra_policy(&db.pool, svc)
        .await
        .expect("converge");
    let mut want: Vec<String> = admin_all_access_bodies("trino", &name, &[])
        .expect("trino bodies")
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    want.sort();
    let bodies = admin_bodies(&db, svc).await;
    assert_eq!(
        bodies.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
        want,
        "a row the Trino def does not build survived convergence (D5 (d))"
    );
    for (policy_name, body) in &bodies {
        assert_eq!(named(body), vec![admin.clone()], "{policy_name}");
        assert_eq!(body["serviceType"], "trino", "{policy_name}");
    }
    assert!(
        !trino_can_select(&db, svc, &stranger).await,
        "a stranger named by a foreign egret-admin row keeps ALL access"
    );
}
