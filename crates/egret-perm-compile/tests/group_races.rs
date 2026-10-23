//! J-F3 Task 1b, fix round 1: the RACES between a membership write and a
//! concurrent write, each driven as a deterministic interleaving.
//!
//! Each test parks one membership write at a chosen point with
//! `support/pausing.rs` (holding every lock it has taken), runs the competing
//! write, gives it a bounded chance to finish, releases the first, and then
//! asserts the end state: the compiled policy must name exactly the members the
//! database holds once both have committed. Before the per-user and per-group
//! advisory locks (`egret_auth::groups`' module doc, "Two concurrent writers")
//! each of these ended with a policy that disagreed with the database in the
//! fail-OPEN direction.
//!
//! Its own test binary, so `egret-test-db` gives it a private database, and the
//! tests take [`SERIAL`] so no other test's locks are in play.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test group_races

use std::sync::Arc;
use std::time::Duration;

use egret_auth::groups::GroupRepo;
use egret_auth::users::UserRepo;
use egret_core::UserId;
use egret_metastore::Db;
use egret_perm::{
    Action, Effect, Grant, GrantOrigin, PermStore, Principal, PrincipalKind, PrincipalSet, Scope,
};
use egret_perm_compile::{recompile_service, PermGroupRecompiler};
use egret_policy::{PolicyStore, COMPILER_GRANT_PREFIX};
use serde_json::Value;
use uuid::Uuid;

#[path = "support/pausing.rs"]
mod pausing;
use pausing::{Pausing, Stage};

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// How long the competing write is given to finish while the first is parked.
/// Long enough that, without the fix, it does finish (or reaches its row-lock
/// wait); with the fix it is waiting on the advisory lock either way, and the
/// end state does not depend on this number.
const CHANCE: Duration = Duration::from_millis(800);

async fn test_db() -> Option<Db> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Fixture {
    db: Db,
    perm: PermStore,
    policy: PolicyStore,
    groups: GroupRepo,
    service_id: i64,
    catalog_id: Uuid,
}

/// A StarRocks profile with one engine bound to it and one registry catalog
/// attached — the same shape `tests/role_grants.rs` uses, StarRocks so nothing
/// here needs a tag service.
async fn seed(db: &Db) -> Fixture {
    let pool = &db.pool;
    let perm = PermStore::new(pool.clone());
    let policy = PolicyStore::new(db.clone());

    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks service-def");
    let service = policy
        .create_service(
            &unique("sr_groupgrant"),
            "starrocks",
            &serde_json::json!({}),
        )
        .await
        .expect("create service");

    let workspace_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique("groupgrant-ws"))
            .bind("Group Grant WS")
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-groupgrant-test','AKIATEST') RETURNING id",
    )
    .bind(unique("groupgrant-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("groupgrant_wh"))
    .bind(workspace_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'starrocks','egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("groupgrant-eng"))
    .bind(service.id)
    .fetch_one(pool)
    .await
    .expect("engine");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(unique("groupgrant_cat"))
    .bind(warehouse_id)
    .fetch_one(pool)
    .await
    .expect("registry catalog");
    sqlx::query(
        "INSERT INTO registry.engine_catalog_attachments (engine_id, catalog_id) VALUES ($1,$2)",
    )
    .bind(engine_id)
    .bind(catalog_id)
    .execute(pool)
    .await
    .expect("attach catalog");

    Fixture {
        db: db.clone(),
        perm,
        policy,
        groups: GroupRepo::new(db.clone()),
        service_id: service.id,
        catalog_id,
    }
}

async fn new_user(db: &Db, email: &str, username: Option<&str>) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) VALUES ('local',$1,$2,$3) RETURNING id",
    )
    .bind(email)
    .bind("Test User")
    .bind(username)
    .fetch_one(&db.pool)
    .await
    .expect("insert user")
}

/// A user with a fresh unique username; returns `(id, username)`.
async fn person(db: &Db, prefix: &str) -> (Uuid, String) {
    let name = unique(prefix).replace('_', "-");
    let id = new_user(db, &format!("{name}@egret.local"), Some(&name)).await;
    (id, name)
}

async fn new_group(db: &Db, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO groups (name, source) VALUES ($1, 'local') RETURNING id")
        .bind(name)
        .fetch_one(&db.pool)
        .await
        .expect("insert group")
}

/// A membership row written behind every writer's back — for SEEDING only, so
/// a test of a trigger never seeds through the trigger it is testing.
async fn seed_member(db: &Db, group_id: Uuid, user_id: Uuid) {
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(user_id)
        .bind(group_id)
        .execute(&db.pool)
        .await
        .expect("seed membership");
}

fn principal(kind: PrincipalKind, r: &str) -> Principal {
    Principal {
        kind,
        r#ref: r.to_string(),
    }
}

fn group(name: &str) -> Principal {
    principal(PrincipalKind::Group, name)
}

fn user(name: &str) -> Principal {
    principal(PrincipalKind::User, name)
}

/// A read grant on the fixture's whole catalog.
fn grant(f: &Fixture, effect: Effect, include: Vec<Principal>, exclude: Vec<Principal>) -> Grant {
    Grant {
        id: Uuid::nil(),
        effect,
        actions: vec![Action::Read],
        scope: Scope {
            catalog_id: f.catalog_id,
            schema: None,
            table: None,
            column: None,
        },
        principals: PrincipalSet { include, exclude },
        origin: GrantOrigin::Explicit,
    }
}

async fn create_and_compile(f: &Fixture, g: &Grant) -> Uuid {
    let id = f.perm.create_grant(g).await.expect("create grant");
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile");
    id
}

async fn compiled(f: &Fixture, name: &str) -> Option<Value> {
    sqlx::query_scalar(
        "SELECT policy_jsonb FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(f.service_id)
    .bind(name)
    .fetch_optional(&f.db.pool)
    .await
    .expect("policy read")
}

async fn grant_body(f: &Fixture, grant_id: Uuid) -> Option<Value> {
    compiled(f, &format!("{COMPILER_GRANT_PREFIX}{grant_id}")).await
}

fn names(v: &Value, key: &str) -> Vec<String> {
    v[key]
        .as_array()
        .unwrap_or_else(|| panic!("{key} in {v}"))
        .iter()
        .map(|s| s.as_str().expect("string").to_string())
        .collect()
}

/// Every username the compiled policy names in `bucket`, sorted; empty when the
/// policy is absent (an include that expanded to nobody compiles to none).
fn named(body: &Option<Value>, bucket: &str) -> Vec<String> {
    let mut out: Vec<String> = body
        .as_ref()
        .and_then(|b| b[bucket].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .flat_map(|item| names(item, "users"))
        .collect();
    out.sort();
    out
}

/// The usernames `user_groups` says belong to `group_id` right now.
async fn db_members(db: &Db, group_id: Uuid) -> Vec<String> {
    let mut v: Vec<String> = sqlx::query_scalar(
        "SELECT u.username::text FROM user_groups ug JOIN users u ON u.id = ug.user_id \
          WHERE ug.group_id = $1",
    )
    .bind(group_id)
    .fetch_all(&db.pool)
    .await
    .expect("members");
    v.sort();
    v
}

/// **Scenario 1 (sync vs remove).** `U` is in OIDC group `G`, and a deny names
/// `G`. An admin's `remove_member(G, U)` is parked before its recompile, its
/// DELETE written and uncommitted. `U` logs in with a claim that still says
/// `[G]`.
///
/// Without a per-user lock the login read `U`'s groups unlocked, saw `{G}`
/// (the remove was not committed), predicted "nothing moved", and so took no
/// lock and compiled nothing. The remove then committed and recompiled (the deny
/// no longer names `U`), and the login re-INSERTed `(U, G)` and committed with
/// no recompile: the database says `U ∈ G` and the deny does not name `U` —
/// fail-OPEN. With the lock, the login waits for the remove, reads `{}`, sees
/// `G` move, and recompiles.
#[tokio::test(flavor = "multi_thread")]
async fn a_login_during_an_admin_remove_leaves_the_policy_matching_the_database() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let (uid, uname) = person(&db, "race-sso").await;
    let (_, other) = person(&db, "race-other").await;
    let g_name = unique("race-oidc");
    let gid = f.groups.upsert_oidc_group(&g_name).await.expect("group");
    seed_member(&db, gid, uid).await;
    let deny_id = create_and_compile(
        &f,
        &grant(&f, Effect::Deny, vec![group(&g_name), user(&other)], vec![]),
    )
    .await;
    assert!(named(&grant_body(&f, deny_id).await, "denyPolicyItems").contains(&uname));

    let (pausing, h) = Pausing::new(Stage::BeforeRecompile);
    let pausing = Arc::new(pausing);
    let remove = {
        let groups = f.groups.clone();
        let pausing = pausing.clone();
        tokio::spawn(async move { groups.remove_member(gid, UserId(uid), &*pausing).await })
    };
    h.reached.await.expect("remove parked before its recompile");

    let mut login = {
        let groups = f.groups.clone();
        let claim = vec![g_name.clone()];
        tokio::spawn(async move {
            groups
                .sync_user_oidc_groups(UserId(uid), &claim, &PermGroupRecompiler)
                .await
        })
    };
    let early = tokio::time::timeout(CHANCE, &mut login).await.ok();
    let login_waited = early.is_none();
    let _ = h.go.send(());
    remove.await.expect("join").expect("remove");
    match early {
        Some(done) => done,
        None => login.await,
    }
    .expect("join")
    .expect("login sync");

    assert_eq!(
        db_members(&db, gid).await,
        vec![uname.clone()],
        "the login re-added U"
    );
    let deny = named(&grant_body(&f, deny_id).await, "denyPolicyItems");
    assert!(
        deny.contains(&uname),
        "the database says U is in G, so the deny on G must name U — a deny that omits a \
         member is fail-OPEN; deny names {deny:?}"
    );
    assert!(
        login_waited,
        "the login must wait on U's membership lock while the remove holds it"
    );
}

/// **Scenario 1b (hard delete vs add).** `U` is in an unreferenced group `H`;
/// an allow names `G` (member `K`). `hard_delete(U)` is parked after computing
/// its affected set from the groups it read — `{H}`. An admin then adds `U` to
/// `G`.
///
/// Without a per-user lock the add committed and recompiled (the allow names
/// `U`), then the delete removed `(U, G)` and recompiled only `H`'s profiles:
/// the deleted user's username stayed in `G`'s allow, ready to be minted again
/// for the next person whose email slugs to it — fail-OPEN. With the lock the
/// add waits for the delete, and then finds no such user.
#[tokio::test(flavor = "multi_thread")]
async fn an_add_during_a_user_delete_leaves_no_stale_username() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let (uid, uname) = person(&db, "race-gone").await;
    let (kid, kname) = person(&db, "race-keep").await;
    let h_id = new_group(&db, &unique("race-unref")).await;
    seed_member(&db, h_id, uid).await;
    let g_name = unique("race-allow");
    let gid = new_group(&db, &g_name).await;
    seed_member(&db, gid, kid).await;
    let allow_id =
        create_and_compile(&f, &grant(&f, Effect::Allow, vec![group(&g_name)], vec![])).await;
    assert_eq!(
        named(&grant_body(&f, allow_id).await, "policyItems"),
        vec![kname.clone()]
    );

    let (pausing, h) = Pausing::new(Stage::AfterAffected);
    let pausing = Arc::new(pausing);
    let delete = {
        let users = UserRepo::new(db.clone());
        let pausing = pausing.clone();
        tokio::spawn(async move { users.hard_delete(UserId(uid), &deleter(), &*pausing).await })
    };
    h.reached
        .await
        .expect("delete parked after its affected set");

    let mut add = {
        let groups = f.groups.clone();
        tokio::spawn(async move {
            groups
                .add_member(gid, UserId(uid), &PermGroupRecompiler)
                .await
        })
    };
    let early = tokio::time::timeout(CHANCE, &mut add).await.ok();
    let add_waited = early.is_none();
    let _ = h.go.send(());
    delete.await.expect("join").expect("hard delete");
    // With the lock the add finds the user gone (its INSERT's FK refuses it);
    // without it the add had already succeeded. Either is a legitimate outcome
    // for the add itself — what may not happen is the policy below.
    let _ = match early {
        Some(done) => done,
        None => add.await,
    }
    .expect("join");

    assert_eq!(db_members(&db, gid).await, vec![kname.clone()]);
    assert_eq!(
        named(&grant_body(&f, allow_id).await, "policyItems"),
        vec![kname],
        "the deleted user's username must not survive in G's allow (it was {uname:?})"
    );
    assert!(
        add_waited,
        "the add must wait on U's membership lock while the delete holds it"
    );
}

/// **2026-10-18 design R5 rebase, review I1.** A membership add of a user with
/// no username, racing a compile that mints that same user, must not deadlock.
///
/// `U` has no username, holds role `R` (named by a grant on profile A) and is
/// being added to group `G` (named by a grant on profile B). The add parks
/// after its `user_groups` insert, which holds `FOR KEY SHARE` on `U`. A
/// recompile of A then mints `U`; a mint updates `users.username`, a unique
/// column, so it needs `FOR UPDATE` on that row. Before the add took the
/// compile-mint lock with its profile locks (`lock_affected_in_tx`), the
/// recompile held that lock and waited on the add's row lock, the add's own
/// recompile then waited on the lock, and Postgres aborted one of them: a
/// failed login or admin write. Now the recompile waits on the lock the add
/// already holds, and both commit.
#[tokio::test(flavor = "multi_thread")]
async fn a_membership_add_racing_a_compile_that_mints_the_same_user_both_commit() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    let a = seed(&db).await;
    let b = seed(&db).await;

    let uid = new_user(&db, &format!("{}@egret.local", unique("mintrace")), None).await;
    let role = unique("mintrace-role");
    let rid: Uuid =
        sqlx::query_scalar("INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id")
            .bind(&role)
            .fetch_one(&db.pool)
            .await
            .expect("role");
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
         VALUES ('user', $1, $2, 'platform')",
    )
    .bind(uid)
    .bind(rid)
    .execute(&db.pool)
    .await
    .expect("role grant");
    let g_name = unique("mintrace-group");
    let gid = new_group(&db, &g_name).await;
    // Stored, not compiled: a compile now would mint `U` before the race.
    let role_grant = a
        .perm
        .create_grant(&grant(
            &a,
            Effect::Allow,
            vec![principal(PrincipalKind::Role, &role)],
            vec![],
        ))
        .await
        .expect("role grant on A");
    b.perm
        .create_grant(&grant(&b, Effect::Allow, vec![group(&g_name)], vec![]))
        .await
        .expect("group grant on B");

    let (pausing, h) = Pausing::new(Stage::BeforeRecompile);
    let pausing = Arc::new(pausing);
    let add = {
        let groups = b.groups.clone();
        let pausing = pausing.clone();
        tokio::spawn(async move { groups.add_member(gid, UserId(uid), &*pausing).await })
    };
    h.reached
        .await
        .expect("add parked after its membership insert");

    let compile = {
        let (perm, policy, sid) = (a.perm.clone(), a.policy.clone(), a.service_id);
        tokio::spawn(async move { recompile_service(&perm, &policy, sid).await })
    };
    // Let the compile reach its mint (the old order) or the lock (the new).
    tokio::time::sleep(CHANCE).await;
    let _ = h.go.send(());

    let added = add.await.expect("join");
    let compiled_a = compile.await.expect("join");
    assert!(added.is_ok(), "the membership add: {added:?}");
    assert!(compiled_a.is_ok(), "the recompile of A: {compiled_a:?}");

    let username: String = sqlx::query_scalar("SELECT username::text FROM users WHERE id = $1")
        .bind(uid)
        .fetch_one(&db.pool)
        .await
        .expect("U is minted");
    assert_eq!(
        named(&grant_body(&a, role_grant).await, "policyItems"),
        vec![username],
        "A's role grant names U by the minted username"
    );
}

/// Who deletes in these tests: `hard_delete` records it on the `user:delete`
/// event.
fn deleter() -> egret_core::Principal {
    egret_core::Principal {
        user_id: egret_core::UserId(uuid::Uuid::nil()),
        email: "deleter@test.local".into(),
        platform_roles: vec![],
        permissions: vec![],
        group_ids: vec![],
        impersonator: None,
    }
}
