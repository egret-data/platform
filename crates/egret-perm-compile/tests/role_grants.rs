//! SP4c **Task 2b**: a `role` grant reaches the engine, or stops claiming to.
//!
//! **The defect, found by D0 in already-merged code.** `grants.rs` compiled a
//! `PrincipalKind::Role` straight into the Ranger `roles` bucket, which D0
//! measured is never populated on the Trino path egret provisions: the plugin
//! fetches roles from a `/service/roles/download/<svc>` route egret does not
//! serve, so `lastKnownRoleVersion` never leaves `-1`, and Trino's plugin passes
//! `userRoles` as a hardcoded `null`. So a role grant matched NOBODY in the
//! engine — while `egret_perm::eval` matched it happily, meaning `explain`
//! reported an access the engine did not grant. Same divergence class D7 found
//! for groups, on a path D7 did not look at, and SP4b's picker offers `role` as
//! an ordinary choice.
//!
//! A role grant now compiles to its holders' STORED usernames, through the same
//! expansion SP4c Task 2's clearance uses.
//!
//! **The agreement test is the one that would have caught this originally.** A
//! test that only asserted "the compiled body has a `roles` bucket naming the
//! role" passed against the defect, because the body was correct Ranger — it
//! just named a principal nothing resolves. So the assertions here are which
//! USERNAMES the policy names, that the role name appears NOWHERE in the body,
//! and that `evaluate` (egret's own model oracle) and the compiled policy
//! agree about the same subject.
//!
//! Its own test binary, so `egret-test-db` gives it a private database: a role
//! grant is catalog-scoped, but the model is read whole on every recompile and
//! a concurrently deleted role would race this file's assertions.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test role_grants

use egret_metastore::Db;
use egret_perm::eval::evaluate;
use egret_perm::{
    Action, Decision, Effect, Grant, GrantOrigin, PermStore, Principal, PrincipalKind,
    PrincipalSet, Reason, Scope, Subject as EvalSubject,
};
use egret_perm_compile::recompile_service;
use egret_policy::{PolicyStore, COMPILER_GRANT_PREFIX};
use serde_json::Value;
use uuid::Uuid;

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
    perm: PermStore,
    policy: PolicyStore,
    service_id: i64,
    catalog_id: Uuid,
}

/// A StarRocks profile with one engine bound to it and one registry catalog
/// attached — StarRocks so nothing here needs a tag service; a role GRANT is a
/// plain resource access policy.
async fn seed(db: &Db) -> Fixture {
    let pool = &db.pool;
    let perm = PermStore::new(pool.clone());
    let policy = PolicyStore::new(db.clone());

    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks service-def");
    let service = policy
        .create_service(&unique("sr_rolegrant"), "starrocks", &serde_json::json!({}))
        .await
        .expect("create service");

    let workspace_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique("rolegrant-ws"))
            .bind("Role Grant WS")
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-rolegrant-test','AKIATEST') RETURNING id",
    )
    .bind(unique("rolegrant-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("rolegrant_wh"))
    .bind(workspace_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'starrocks','egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("rolegrant-eng"))
    .bind(service.id)
    .fetch_one(pool)
    .await
    .expect("engine");
    let catalog_name = unique("rolegrant_cat");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(&catalog_name)
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
        perm,
        policy,
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

async fn new_role(db: &Db, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id")
        .bind(name)
        .fetch_one(&db.pool)
        .await
        .expect("insert role")
}

async fn grant_role(db: &Db, user_id: Uuid, role_id: Uuid) {
    let res = sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'platform', NULL)",
    )
    .bind(user_id)
    .bind(role_id)
    .execute(&db.pool)
    .await
    .expect("role grant");
    assert_eq!(res.rows_affected(), 1, "the role grant row must exist");
}

fn role_principal(name: &str) -> Principal {
    Principal {
        kind: PrincipalKind::Role,
        r#ref: name.to_string(),
    }
}

fn user_principal(name: &str) -> Principal {
    Principal {
        kind: PrincipalKind::User,
        r#ref: name.to_string(),
    }
}

/// A read grant on the fixture's whole catalog with the given principal set.
fn read_grant(catalog_id: Uuid, principals: PrincipalSet) -> Grant {
    Grant {
        id: Uuid::nil(),
        effect: Effect::Allow,
        actions: vec![Action::Read],
        scope: Scope {
            catalog_id,
            schema: None,
            table: None,
            column: None,
        },
        principals,
        origin: GrantOrigin::Explicit,
    }
}

async fn compiled_policy(db: &Db, service_id: i64, grant_id: Uuid) -> Option<Value> {
    sqlx::query_scalar(
        "SELECT policy_jsonb FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(service_id)
    .bind(format!("{COMPILER_GRANT_PREFIX}{grant_id}"))
    .fetch_optional(&db.pool)
    .await
    .expect("policy read")
}

fn users_of(item: &Value) -> Vec<String> {
    item["users"]
        .as_array()
        .expect("users")
        .iter()
        .map(|v| v.as_str().expect("username").to_string())
        .collect()
}

/// The single allow item of a compiled access policy.
fn only_item(body: &Value) -> &Value {
    let items = body["policyItems"].as_array().expect("policyItems");
    assert_eq!(
        items.len(),
        1,
        "an expanded grant is ONE item with many users: {body:#}"
    );
    &items[0]
}

/// **The headline of Task 2b.** A role grant compiles to its holders' STORED
/// usernames — and the role NAME appears nowhere in the body, because the bucket
/// that would hold it matches nobody. The holder here carries a
/// collision-suffixed username, so a re-derivation from the email would name the
/// wrong person and this test would fail.
#[tokio::test]
async fn a_role_grant_names_its_holders_usernames_and_never_a_roles_bucket() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let local = unique("dupe");
    let first = new_user(&db, &format!("{local}@egret.local"), Some(&local)).await;
    let holder = new_user(
        &db,
        &format!("{local}@other.local"),
        Some(&format!("{local}-2")),
    )
    .await;
    let outsider = new_user(
        &db,
        &format!("{}@egret.local", unique("outsider")),
        Some("rolegrant-outsider"),
    )
    .await;
    let role_name = unique("data-reader");
    let role_id = new_role(&db, &role_name).await;
    grant_role(&db, holder, role_id).await;
    let _ = (first, outsider); // neither holds the role

    let grant_id = f
        .perm
        .create_grant(&read_grant(
            f.catalog_id,
            PrincipalSet {
                include: vec![role_principal(&role_name)],
                exclude: vec![],
            },
        ))
        .await
        .expect("create role grant");

    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile");

    let body = compiled_policy(&db, f.service_id, grant_id)
        .await
        .expect("a role grant must compile to a policy");
    let item = only_item(&body);
    assert_eq!(
        users_of(item),
        vec![format!("{local}-2")],
        "the role's holder is named by their STORED username — a re-derivation would name \
         {local:?}, which is a DIFFERENT person"
    );
    assert!(
        item["roles"].as_array().expect("roles").is_empty(),
        "the Ranger `roles` bucket is never populated on the Trino path egret provisions (D0), \
         so a grant naming the role there matches nobody: {body:#}"
    );
    assert!(
        !serde_json::to_string(&body).unwrap().contains(&role_name),
        "the role NAME must not appear anywhere in the compiled body: {body:#}"
    );
    assert!(
        !users_of(item).iter().any(|u| u == "rolegrant-outsider"),
        "a non-holder must not be named: {body:#}"
    );
}

/// **The agreement test.** egret's own oracle
/// (`egret_perm::eval::evaluate`) matches a `role` principal against
/// `Subject::roles` by name; the compiler matches it by naming usernames.
/// Before Task 2b those two disagreed — the oracle allowed, the engine denied,
/// and `explain` reported an access nobody had. They must now say the SAME
/// thing, for a holder and for a non-holder.
///
/// **Renamed in SP4d-2a Task 5 (was
/// `explain_and_the_compiled_policy_agree_about_a_role_granted_subject`).**
/// `explain` no longer answers with `evaluate` — it evaluates the compiled
/// artifact, where a role has already been expanded into usernames — so the
/// old name named a relationship that no longer exists. The property does not
/// change: the model's own answer and the compiled policy must not diverge.
#[tokio::test]
async fn the_oracle_and_the_compiled_policy_agree_about_a_role_granted_subject() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let holder = new_user(
        &db,
        &format!("{}@egret.local", unique("oracle-h")),
        Some("oracle-holder"),
    )
    .await;
    let stranger = new_user(
        &db,
        &format!("{}@egret.local", unique("oracle-s")),
        Some("oracle-stranger"),
    )
    .await;
    let role_name = unique("oracle-role");
    let role_id = new_role(&db, &role_name).await;
    grant_role(&db, holder, role_id).await;
    let _ = stranger;

    let grant_id = f
        .perm
        .create_grant(&read_grant(
            f.catalog_id,
            PrincipalSet {
                include: vec![role_principal(&role_name)],
                exclude: vec![],
            },
        ))
        .await
        .expect("create role grant");
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile");

    let grants: Vec<Grant> = f
        .perm
        .list_grants()
        .await
        .expect("grants")
        .into_iter()
        .filter(|g| g.id == grant_id)
        .collect();
    assert_eq!(grants.len(), 1, "exactly this fixture's grant");
    let target = Scope {
        catalog_id: f.catalog_id,
        schema: Some("sales".into()),
        table: Some("orders".into()),
        column: Some("id".into()),
    };

    let decision = evaluate(
        &EvalSubject {
            user: "oracle-holder".into(),
            groups: vec![],
            roles: vec![role_name.clone()],
            workspaces: vec![],
        },
        Action::Read,
        &target,
        &grants,
        &[],
        &[],
        &[],
    );
    assert_eq!(
        decision,
        Decision {
            allowed: true,
            reasons: vec![Reason::AllowedByGrant { grant_id }],
        },
        "the oracle allows the role holder"
    );

    let stranger_decision = evaluate(
        &EvalSubject {
            user: "oracle-stranger".into(),
            groups: vec![],
            roles: vec![],
            workspaces: vec![],
        },
        Action::Read,
        &target,
        &grants,
        &[],
        &[],
        &[],
    );
    assert!(
        !stranger_decision.allowed,
        "the oracle refuses a non-holder: {stranger_decision:?}"
    );

    // …and the compiled policy says exactly the same, by naming one and not the
    // other. Disagreement here is the defect: `explain` would report an access
    // the engine does not grant.
    let body = compiled_policy(&db, f.service_id, grant_id)
        .await
        .expect("policy");
    let users = users_of(only_item(&body));
    assert_eq!(
        users,
        vec!["oracle-holder".to_string()],
        "the engine must grant exactly whom the oracle allows: {body:#}"
    );
    assert!(!users.iter().any(|u| u == "oracle-stranger"));
}

/// The `exclude` half is the one that fails OPEN when it is got wrong: a
/// subtract that evaporates grants MORE than the model states. A role named in
/// an `exclude` must therefore expand too, into `allowExceptions`.
#[tokio::test]
async fn a_role_in_an_exclude_subtracts_its_holders() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let alice = new_user(
        &db,
        &format!("{}@egret.local", unique("exc-a")),
        Some("exc-alice"),
    )
    .await;
    let bob = new_user(
        &db,
        &format!("{}@egret.local", unique("exc-b")),
        Some("exc-bob"),
    )
    .await;
    let role_name = unique("contractor");
    let role_id = new_role(&db, &role_name).await;
    grant_role(&db, bob, role_id).await;
    let _ = alice;

    let grant_id = f
        .perm
        .create_grant(&read_grant(
            f.catalog_id,
            PrincipalSet {
                include: vec![user_principal("exc-alice"), user_principal("exc-bob")],
                exclude: vec![role_principal(&role_name)],
            },
        ))
        .await
        .expect("create grant");
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile");

    let body = compiled_policy(&db, f.service_id, grant_id)
        .await
        .expect("policy");
    let exceptions = body["allowExceptions"].as_array().expect("allowExceptions");
    assert_eq!(
        exceptions.len(),
        1,
        "the exclude must compile to one exception item: {body:#}"
    );
    assert_eq!(
        users_of(&exceptions[0]),
        vec!["exc-bob".to_string()],
        "the excluded role's holder is named by username — an exception naming the ROLE matches \
         nobody, so the subtract would evaporate and the grant would reach them: {body:#}"
    );
    assert_eq!(
        users_of(only_item(&body)),
        vec!["exc-alice".to_string(), "exc-bob".to_string()],
        "the include side is untouched — Ranger applies the exception, not egret: {body:#}"
    );
}

/// A grant with the given effect over the fixture's whole catalog.
fn grant_with(effect: Effect, catalog_id: Uuid, principals: PrincipalSet) -> Grant {
    Grant {
        effect,
        ..read_grant(catalog_id, principals)
    }
}

fn public_principal() -> Principal {
    Principal {
        kind: PrincipalKind::Group,
        r#ref: egret_perm::PUBLIC_GROUP.to_string(),
    }
}

/// `users.username` for `user_id`, NULL as `None`.
async fn stored_username(db: &Db, user_id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT username::text FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&db.pool)
        .await
        .expect("read username")
}

/// **2026-10-18 design R5.** A role holder with no username yet used to be
/// left out of the compiled policy ("absent is the right answer"), so *allow
/// `{public}`, deny role R* reached a holder of R the moment they got a username
/// — on their first query, which recompiles nothing. The deny must name them
/// from the start: the compile mints their username, as group and workspace
/// expansion already did.
#[tokio::test]
async fn a_role_in_a_deny_names_a_holder_who_had_no_username_yet() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let holder = new_user(&db, &format!("{}@egret.local", unique("rmint-deny")), None).await;
    let role_name = unique("blocked");
    let role_id = new_role(&db, &role_name).await;
    grant_role(&db, holder, role_id).await;

    f.perm
        .create_grant(&grant_with(
            Effect::Allow,
            f.catalog_id,
            PrincipalSet {
                include: vec![public_principal()],
                exclude: vec![],
            },
        ))
        .await
        .expect("create allow-public grant");
    let deny_id = f
        .perm
        .create_grant(&grant_with(
            Effect::Deny,
            f.catalog_id,
            PrincipalSet {
                include: vec![role_principal(&role_name)],
                exclude: vec![],
            },
        ))
        .await
        .expect("create role deny");
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile");

    let minted = stored_username(&db, holder)
        .await
        .expect("compiling a model that names the role must mint its holder's username");
    let body = compiled_policy(&db, f.service_id, deny_id)
        .await
        .expect("a role deny with a holder compiles to a policy");
    let deny = body["denyPolicyItems"].as_array().expect("denyPolicyItems");
    assert_eq!(deny.len(), 1, "one deny item: {body:#}");
    assert_eq!(
        users_of(&deny[0]),
        vec![minted],
        "the deny names the holder by the username the compile minted — omitted, the allow on \
         `public` reaches them once they get one: {body:#}"
    );
}

/// R5, the exclude half: *allow `{public}` except role R* must subtract a
/// holder of R who had no username yet.
#[tokio::test]
async fn a_role_in_an_exclude_subtracts_a_holder_who_had_no_username_yet() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let holder = new_user(&db, &format!("{}@egret.local", unique("rmint-exc")), None).await;
    let role_name = unique("contractor");
    let role_id = new_role(&db, &role_name).await;
    grant_role(&db, holder, role_id).await;

    let grant_id = f
        .perm
        .create_grant(&grant_with(
            Effect::Allow,
            f.catalog_id,
            PrincipalSet {
                include: vec![public_principal()],
                exclude: vec![role_principal(&role_name)],
            },
        ))
        .await
        .expect("create grant");
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile");

    let minted = stored_username(&db, holder)
        .await
        .expect("compiling the exclude must mint the holder's username");
    let body = compiled_policy(&db, f.service_id, grant_id)
        .await
        .expect("policy");
    let exceptions = body["allowExceptions"].as_array().expect("allowExceptions");
    assert_eq!(exceptions.len(), 1, "one exception item: {body:#}");
    assert_eq!(
        users_of(&exceptions[0]),
        vec![minted],
        "the excluded role's holder is subtracted by their minted username: {body:#}"
    );
}

/// The stable-allocation property R5 leans on: the name the compile mints is
/// the one every later `ensure_username` — the first query, the engine token
/// minter — hands out, so the policy names the identity the engine will see.
#[tokio::test]
async fn the_name_a_role_compile_mints_is_the_one_ensure_username_returns() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let holder = new_user(
        &db,
        &format!("{}@egret.local", unique("rmint-stable")),
        None,
    )
    .await;
    let role_name = unique("reader");
    let role_id = new_role(&db, &role_name).await;
    grant_role(&db, holder, role_id).await;
    let grant_id = f
        .perm
        .create_grant(&read_grant(
            f.catalog_id,
            PrincipalSet {
                include: vec![role_principal(&role_name)],
                exclude: vec![],
            },
        ))
        .await
        .expect("create role grant");
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile");

    let body = compiled_policy(&db, f.service_id, grant_id)
        .await
        .expect("a role grant with a holder compiles to a policy");
    let named = users_of(only_item(&body));
    let later = egret_auth::users::UserRepo::new(db.clone())
        .ensure_username(egret_core::UserId(holder))
        .await
        .expect("ensure_username");
    assert_eq!(
        named,
        vec![later],
        "the compiled name and the one the engine later sees are the same: {body:#}"
    );
}

async fn policy_version(db: &Db, service_id: i64) -> i64 {
    sqlx::query_scalar("SELECT policy_version FROM policy.authz_services WHERE id = $1")
        .bind(service_id)
        .fetch_one(&db.pool)
        .await
        .expect("policy_version")
}

/// **2026-10-18 design R7.** A body the old compiler wrote left out a role
/// holder who had no username, and nothing recompiles it: the holder's first
/// query mints a username without a recompile. The boot pass recompiles every
/// profile whose model names a role, so the stored deny comes to name them —
/// and, run again, writes nothing.
#[tokio::test]
async fn the_boot_repair_names_a_holder_the_old_compiler_left_out() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let kept = new_user(
        &db,
        &format!("{}@egret.local", unique("repair-kept")),
        Some("repair-kept"),
    )
    .await;
    let left_out = new_user(&db, &format!("{}@egret.local", unique("repair-out")), None).await;
    let role_name = unique("repair-blocked");
    let role_id = new_role(&db, &role_name).await;
    grant_role(&db, kept, role_id).await;
    grant_role(&db, left_out, role_id).await;

    f.perm
        .create_grant(&grant_with(
            Effect::Allow,
            f.catalog_id,
            PrincipalSet {
                include: vec![public_principal()],
                exclude: vec![],
            },
        ))
        .await
        .expect("create allow-public grant");
    let deny_id = f
        .perm
        .create_grant(&grant_with(
            Effect::Deny,
            f.catalog_id,
            PrincipalSet {
                include: vec![role_principal(&role_name)],
                exclude: vec![],
            },
        ))
        .await
        .expect("create role deny");
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile");

    // The old path: the deny names only the holder who had a username, and the
    // other one still has none.
    sqlx::query(
        "UPDATE policy.authz_policies \
            SET policy_jsonb = jsonb_set(policy_jsonb, '{denyPolicyItems,0,users}', \
                                         '[\"repair-kept\"]'::jsonb) \
          WHERE service_id = $1 AND name = $2",
    )
    .bind(f.service_id)
    .bind(format!("{COMPILER_GRANT_PREFIX}{deny_id}"))
    .execute(&db.pool)
    .await
    .expect("write the old body");
    sqlx::query("UPDATE users SET username = NULL WHERE id = $1")
        .bind(left_out)
        .execute(&db.pool)
        .await
        .expect("unmint");
    let before = policy_version(&db, f.service_id).await;

    let repaired =
        egret_perm_compile::role_membership::recompile_role_referencing_profiles(&db.pool)
            .await
            .expect("boot repair");

    assert!(
        repaired.contains(&f.service_id),
        "a profile whose model names a role is recompiled: {repaired:?}"
    );
    let minted = stored_username(&db, left_out)
        .await
        .expect("the repair mints the left-out holder's username");
    let body = compiled_policy(&db, f.service_id, deny_id)
        .await
        .expect("policy");
    let mut expected = vec!["repair-kept".to_string(), minted];
    expected.sort();
    assert_eq!(
        users_of(&body["denyPolicyItems"][0]),
        expected,
        "the repaired deny names the holder the old compiler left out: {body:#}"
    );
    let after_first = policy_version(&db, f.service_id).await;
    assert!(
        after_first > before,
        "rewriting the body bumps the profile's policy_version"
    );

    egret_perm_compile::role_membership::recompile_role_referencing_profiles(&db.pool)
        .await
        .expect("second boot repair");
    assert_eq!(
        policy_version(&db, f.service_id).await,
        after_first,
        "a second run changes nothing, so it writes nothing"
    );
}

/// The ascending pre-mint sweep `recompile_services_in_tx` runs (Task 3
/// review) mints only people the model names: a mint is a write, and a
/// compile must not make it for a holder of a role nothing names.
#[tokio::test]
async fn the_pre_mint_sweep_mints_only_holders_of_a_role_the_model_names() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let named_holder = new_user(&db, &format!("{}@egret.local", unique("pm-named")), None).await;
    let other_holder = new_user(&db, &format!("{}@egret.local", unique("pm-other")), None).await;
    let named_role = unique("pm-named-role");
    let other_role = unique("pm-unnamed-role");
    let named_id = new_role(&db, &named_role).await;
    let other_id = new_role(&db, &other_role).await;
    grant_role(&db, named_holder, named_id).await;
    grant_role(&db, other_holder, other_id).await;
    f.perm
        .create_grant(&read_grant(
            f.catalog_id,
            PrincipalSet {
                include: vec![role_principal(&named_role)],
                exclude: vec![],
            },
        ))
        .await
        .expect("create role grant");

    let mut tx = db.pool.begin().await.expect("begin");
    egret_perm_compile::recompile_services_in_tx(&mut tx, &[f.service_id])
        .await
        .expect("recompile");
    tx.commit().await.expect("commit");

    assert!(
        stored_username(&db, named_holder).await.is_some(),
        "a holder of a role the model names is minted"
    );
    assert_eq!(
        stored_username(&db, other_holder).await,
        None,
        "a holder of a role nothing names is not minted"
    );
}

/// Two concurrent recompiles of DIFFERENT profiles that must both mint the same
/// two unnamed people do not deadlock (2026-10-18 design R5 rebase).
///
/// The shape that used to: a compile mints per pass — the member pass (here a
/// group's member, the HIGHER id) before the role pass (a role's holder, the
/// LOWER id) — while `recompile_services_in_tx`'s pre-mint sweep mints
/// ascending, lower first. Opposite orders over one pair of `users` rows. A
/// third transaction holds the lower row so both recompiles are mid-mint when
/// it lets go; before `recompile_service_in_tx` pre-minted too, whichever of
/// them then took the lower row waited on the higher one the other held, and
/// Postgres aborted one about half the time per round. Now both sweep
/// ascending first and simply queue.
#[tokio::test]
async fn concurrent_recompiles_minting_the_same_people_in_opposite_orders_both_succeed() {
    let Some(db) = test_db().await else { return };
    let a = seed(&db).await;
    let b = seed(&db).await;

    for round in 0..4 {
        let (lo, hi) = {
            let (x, y) = (Uuid::new_v4(), Uuid::new_v4());
            if x < y {
                (x, y)
            } else {
                (y, x)
            }
        };
        for id in [lo, hi] {
            sqlx::query("INSERT INTO users (id, provider, email, name) VALUES ($1,'local',$2,'T')")
                .bind(id)
                .bind(format!("{}@egret.local", unique("dl")))
                .execute(&db.pool)
                .await
                .expect("insert user");
        }
        let group = unique("dl-group");
        let gid: Uuid = sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
            .bind(&group)
            .fetch_one(&db.pool)
            .await
            .expect("group");
        sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1,$2)")
            .bind(hi)
            .bind(gid)
            .execute(&db.pool)
            .await
            .expect("member");
        let role = unique("dl-role");
        let rid = new_role(&db, &role).await;
        grant_role(&db, lo, rid).await;
        a.perm
            .create_grant(&read_grant(
                a.catalog_id,
                PrincipalSet {
                    include: vec![
                        Principal {
                            kind: PrincipalKind::Group,
                            r#ref: group.clone(),
                        },
                        role_principal(&role),
                    ],
                    exclude: vec![],
                },
            ))
            .await
            .expect("create grant");

        let mut holder = db.pool.begin().await.expect("holder tx");
        sqlx::query("SELECT 1 FROM users WHERE id = $1 FOR UPDATE")
            .bind(lo)
            .execute(&mut *holder)
            .await
            .expect("hold the lower row");

        let single = {
            let (perm, policy, sid) = (a.perm.clone(), a.policy.clone(), a.service_id);
            tokio::spawn(async move { recompile_service(&perm, &policy, sid).await })
        };
        let set = {
            let (pool, sid) = (db.pool.clone(), b.service_id);
            tokio::spawn(async move {
                let mut tx = pool.begin().await.expect("begin");
                egret_perm_compile::recompile_services_in_tx(&mut tx, &[sid]).await?;
                tx.commit()
                    .await
                    .map_err(|e| egret_perm::CompileError::Db(e.to_string()))?;
                Ok::<_, egret_perm::CompileError>(())
            })
        };
        // Let both reach the held row before it is released.
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        holder.rollback().await.expect("release");

        let single = single.await.expect("join");
        let set = set.await.expect("join");
        assert!(
            single.is_ok(),
            "round {round}: recompile_service: {single:?}"
        );
        assert!(
            set.is_ok(),
            "round {round}: recompile_services_in_tx: {set:?}"
        );
        for id in [lo, hi] {
            assert!(
                stored_username(&db, id).await.is_some(),
                "round {round}: {id} minted"
            );
        }
    }
}
