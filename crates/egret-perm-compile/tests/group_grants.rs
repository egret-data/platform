//! J-F3 **Task 1b** (design R8): a `group` principal reaches the engine as its
//! MEMBERS, and every write to `user_groups` recompiles.
//!
//! **The defect.** `grants.rs` put a `PrincipalKind::Group` straight into the
//! Ranger `groups` bucket and `rules.rs` did the same for a rule exemption. D7
//! (`docs/en/plans/2026-09-24-permission-model-sp4a-d7-result.md`) measured that
//! no engine egret provisions ever sees a group — no `group-provider.properties`,
//! no usersync — so that bucket matched nobody: an allow on a group granted
//! nobody while `explain` said it did, and a group in a deny include or an
//! exclude failed OPEN (the deny or the subtract silently evaporated).
//!
//! A group now compiles to its members' STORED usernames, minted when missing,
//! the way SP4c compiled a role. The assertions are therefore about which
//! USERNAMES each item names and that the group name appears nowhere in the
//! body — a test that only asserted "the body has a `groups` bucket naming the
//! group" passed against the defect, because the body was correct Ranger that
//! named a principal nothing resolves.
//!
//! The second half is the triggers. An enumeration is stale the instant
//! membership changes, and a missed REMOVE is a breach, so each writer of
//! `user_groups` is driven here through its real API with the real
//! `PermGroupRecompiler`, and the compiled policy is read back WITHOUT any
//! explicit recompile: if a writer forgot the recompile, the policy still names
//! the old membership and the test fails.
//!
//! Its own test binary, so `egret-test-db` gives it a private database.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test group_grants

use egret_auth::groups::{GroupMembershipRecompiler, GroupReference, GroupRepo};
use egret_auth::oidc::{provision_and_sync, OidcClaims};
use egret_auth::users::UserRepo;
use egret_core::UserId;
use egret_metastore::Db;
use egret_perm::{
    Action, Effect, Grant, GrantOrigin, PermStore, Principal, PrincipalKind, PrincipalSet, Rule,
    RuleKind, Scope, Selector,
};
use egret_perm_compile::{recompile_service, PermGroupRecompiler};
use egret_policy::{PolicyStore, COMPILER_GRANT_PREFIX, COMPILER_RULE_PREFIX};
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

/// The users of the one item in `bucket` (`policyItems`, `denyPolicyItems`,
/// `allowExceptions`, `denyExceptions`), asserting there is exactly one and that
/// it names no group.
fn only_item_users(body: &Value, bucket: &str) -> Vec<String> {
    let items = body[bucket]
        .as_array()
        .unwrap_or_else(|| panic!("{bucket}: {body:#}"));
    assert_eq!(items.len(), 1, "exactly one {bucket} item: {body:#}");
    assert!(
        names(&items[0], "groups").is_empty(),
        "no {bucket} item may name a group — no engine egret provisions resolves one (D7): \
         {body:#}"
    );
    let mut users = names(&items[0], "users");
    users.sort();
    users
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

/// **The headline.** An allow on a group names its members by their STORED
/// usernames — one of them with a collision-suffixed name a re-derivation from
/// the email would get wrong, one with NO username yet, which is minted rather
/// than skipped (`members.rs`' module doc: on a fresh install that is every
/// member) — and never the group.
#[tokio::test]
async fn an_allow_on_a_group_names_its_members_and_never_the_group() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let local = unique("dupe").replace('_', "-");
    let _first = new_user(&db, &format!("{local}@egret.local"), Some(&local)).await;
    let suffixed = new_user(
        &db,
        &format!("{local}@other.local"),
        Some(&format!("{local}-2")),
    )
    .await;
    let unminted_local = unique("fresh").replace('_', "-");
    let unminted = new_user(&db, &format!("{unminted_local}@egret.local"), None).await;
    let (_, outsider) = person(&db, "outsider").await;

    let group_name = unique("analysts");
    let gid = new_group(&db, &group_name).await;
    seed_member(&db, gid, suffixed).await;
    seed_member(&db, gid, unminted).await;

    let grant_id = create_and_compile(
        &f,
        &grant(&f, Effect::Allow, vec![group(&group_name)], vec![]),
    )
    .await;

    let minted: Option<String> =
        sqlx::query_scalar("SELECT username::text FROM users WHERE id = $1")
            .bind(unminted)
            .fetch_one(&db.pool)
            .await
            .expect("read minted username");
    let minted = minted.expect("compiling the group grant must mint the member's username");

    let body = grant_body(&f, grant_id)
        .await
        .expect("a group grant with members compiles to a policy");
    let users = only_item_users(&body, "policyItems");
    assert_eq!(
        users,
        sorted(vec![format!("{local}-2"), minted]),
        "the members, by STORED username (a re-derivation would name {local:?}, a different \
         person) and the minted one: {body:#}"
    );
    assert!(!users.contains(&outsider), "a non-member is not named");
    assert!(
        !serde_json::to_string(&body).unwrap().contains(&group_name),
        "the group NAME must appear nowhere in the compiled body: {body:#}"
    );
}

/// A deny on a group is the direction that failed OPEN: a deny naming a group
/// no engine resolves denied nobody. It must name the members.
#[tokio::test]
async fn a_deny_on_a_group_names_its_members_in_the_deny_item() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let (m1, n1) = person(&db, "deny-m1").await;
    let (m2, n2) = person(&db, "deny-m2").await;
    let group_name = unique("contractors");
    let gid = new_group(&db, &group_name).await;
    seed_member(&db, gid, m1).await;
    seed_member(&db, gid, m2).await;

    let grant_id = create_and_compile(
        &f,
        &grant(&f, Effect::Deny, vec![group(&group_name)], vec![]),
    )
    .await;
    let body = grant_body(&f, grant_id).await.expect("deny policy");
    assert_eq!(
        only_item_users(&body, "denyPolicyItems"),
        sorted(vec![n1, n2]),
        "{body:#}"
    );
}

/// An exclude on a group is the other fail-open: the subtract evaporated and
/// the allow reached the people it was meant to spare. On an allow it is
/// `allowExceptions`; on a deny, `denyExceptions`.
#[tokio::test]
async fn an_exclude_on_a_group_names_its_members_in_the_exception() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let (_, alice) = person(&db, "exc-alice").await;
    let (bob_id, bob) = person(&db, "exc-bob").await;
    let group_name = unique("temps");
    let gid = new_group(&db, &group_name).await;
    seed_member(&db, gid, bob_id).await;

    let allow_id = create_and_compile(
        &f,
        &grant(
            &f,
            Effect::Allow,
            vec![user(&alice), user(&bob)],
            vec![group(&group_name)],
        ),
    )
    .await;
    let body = grant_body(&f, allow_id).await.expect("allow policy");
    assert_eq!(only_item_users(&body, "allowExceptions"), vec![bob.clone()]);

    let deny_id = create_and_compile(
        &f,
        &grant(
            &f,
            Effect::Deny,
            vec![user(&alice), user(&bob)],
            vec![group(&group_name)],
        ),
    )
    .await;
    let body = grant_body(&f, deny_id).await.expect("deny policy");
    assert_eq!(only_item_users(&body, "denyExceptions"), vec![bob]);
}

/// A rule exemption naming a group exempts its members. Before, the exemption
/// named the group, which no engine resolves, so the mask applied to the very
/// people the author exempted.
#[tokio::test]
async fn a_group_in_a_rule_exemption_exempts_its_members() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let (mid, member) = person(&db, "exempt-m").await;
    let group_name = unique("auditors");
    let gid = new_group(&db, &group_name).await;
    seed_member(&db, gid, mid).await;

    let rule = Rule {
        id: Uuid::nil(),
        name: unique("group-exempt-mask"),
        kind: RuleKind::Mask {
            mask_type: "MASK".to_string(),
            value_expr: None,
        },
        selector: Selector::Path(Scope {
            catalog_id: f.catalog_id,
            schema: Some("sales".into()),
            table: Some("orders".into()),
            column: Some("email".into()),
        }),
        exempt: PrincipalSet {
            include: vec![group(&group_name)],
            exclude: vec![],
        },
    };
    let rule_id = f.perm.create_rule(&rule).await.expect("create rule");
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile");

    let body = compiled(&f, &format!("{COMPILER_RULE_PREFIX}{rule_id}"))
        .await
        .expect("rule policy");
    let items = body["dataMaskPolicyItems"].as_array().expect("mask items");
    assert_eq!(items.len(), 2, "an exempt item, then everyone: {body:#}");
    assert_eq!(names(&items[0], "users"), vec![member], "{body:#}");
    assert!(names(&items[0], "groups").is_empty(), "{body:#}");
    assert_eq!(names(&items[1], "groups"), vec!["public".to_string()]);
    assert!(
        !serde_json::to_string(&body).unwrap().contains(&group_name),
        "{body:#}"
    );
}

/// `public` is the ONE group every engine resolves (Ranger's built-in), so it
/// stays a group.
#[tokio::test]
async fn the_public_group_stays_a_group() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let grant_id =
        create_and_compile(&f, &grant(&f, Effect::Allow, vec![group("public")], vec![])).await;
    let body = grant_body(&f, grant_id).await.expect("public policy");
    let item = &body["policyItems"][0];
    assert_eq!(names(item, "groups"), vec!["public".to_string()]);
    assert!(names(item, "users").is_empty(), "{body:#}");
}

/// A group with no members expands to nobody: an include compiles to no policy
/// at all (fails closed — the same answer, said clearly), and an exclude
/// subtracts nobody, which is right because nobody is in it.
#[tokio::test]
async fn an_empty_group_grants_nobody_and_subtracts_nobody() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let (_, alice) = person(&db, "empty-alice").await;
    let group_name = unique("nobody");
    new_group(&db, &group_name).await;

    let include_id = create_and_compile(
        &f,
        &grant(&f, Effect::Allow, vec![group(&group_name)], vec![]),
    )
    .await;
    assert_eq!(
        grant_body(&f, include_id).await,
        None,
        "an include that expands to nobody compiles to no policy"
    );

    let exclude_id = create_and_compile(
        &f,
        &grant(
            &f,
            Effect::Allow,
            vec![user(&alice)],
            vec![group(&group_name)],
        ),
    )
    .await;
    let body = grant_body(&f, exclude_id).await.expect("policy");
    assert_eq!(only_item_users(&body, "policyItems"), vec![alice]);
    assert!(
        body["allowExceptions"]
            .as_array()
            .expect("array")
            .is_empty(),
        "an exclude that expands to nobody emits no exception block: {body:#}"
    );
}

// ---------------------------------------------------------------------------
// The triggers: every writer of `user_groups`, through its real API, with NO
// explicit recompile afterwards.
// ---------------------------------------------------------------------------

/// `GroupRepo::add_member` and `remove_member` (the admin console's
/// `POST/DELETE /groups/{id}/members`): the compiled policy follows each
/// write in the same transaction. The REMOVE is asserted on a DENY's exception —
/// the case where a missed recompile keeps exempting someone the model no
/// longer exempts.
#[tokio::test]
async fn adding_and_removing_a_member_recompiles_in_the_same_transaction() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let (_, alice) = person(&db, "trg-alice").await;
    let (bob_id, bob) = person(&db, "trg-bob").await;
    let group_name = unique("trg-group");
    let gid = new_group(&db, &group_name).await;

    let allow_id = create_and_compile(
        &f,
        &grant(&f, Effect::Allow, vec![group(&group_name)], vec![]),
    )
    .await;
    let deny_id = create_and_compile(
        &f,
        &grant(
            &f,
            Effect::Deny,
            vec![user(&alice), user(&bob)],
            vec![group(&group_name)],
        ),
    )
    .await;
    assert_eq!(grant_body(&f, allow_id).await, None, "empty group to start");

    f.groups
        .add_member(gid, UserId(bob_id), &PermGroupRecompiler)
        .await
        .expect("add member");
    let allow = grant_body(&f, allow_id).await.expect("allow after ADD");
    assert_eq!(only_item_users(&allow, "policyItems"), vec![bob.clone()]);
    let deny = grant_body(&f, deny_id).await.expect("deny after ADD");
    assert_eq!(only_item_users(&deny, "denyExceptions"), vec![bob.clone()]);

    f.groups
        .remove_member(gid, UserId(bob_id), &PermGroupRecompiler)
        .await
        .expect("remove member");
    assert_eq!(
        grant_body(&f, allow_id).await,
        None,
        "the removed member is no longer granted"
    );
    let deny = grant_body(&f, deny_id).await.expect("deny after REMOVE");
    assert!(
        deny["denyExceptions"].as_array().expect("array").is_empty(),
        "the removed member is no longer exempt from the deny — a missed REMOVE recompile \
         is exactly this exception surviving: {deny:#}"
    );
    assert_eq!(
        only_item_users(&deny, "denyPolicyItems"),
        sorted(vec![alice, bob])
    );
}

/// `GroupRepo::sync_user_oidc_groups` (every OIDC login): a claim that drops a
/// group removes the member from what that group compiles to, and one that adds
/// a group adds them.
#[tokio::test]
async fn an_oidc_sync_that_moves_a_membership_recompiles() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let (uid, name) = person(&db, "sso").await;
    let g_old = f
        .groups
        .upsert_oidc_group(&unique("sso-old"))
        .await
        .expect("group");
    let g_new = f
        .groups
        .upsert_oidc_group(&unique("sso-new"))
        .await
        .expect("group");
    let old_name: String = sqlx::query_scalar("SELECT name FROM groups WHERE id = $1")
        .bind(g_old)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let new_name: String = sqlx::query_scalar("SELECT name FROM groups WHERE id = $1")
        .bind(g_new)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    seed_member(&db, g_old, uid).await;

    let old_id = create_and_compile(
        &f,
        &grant(&f, Effect::Allow, vec![group(&old_name)], vec![]),
    )
    .await;
    let new_id = create_and_compile(
        &f,
        &grant(&f, Effect::Allow, vec![group(&new_name)], vec![]),
    )
    .await;
    assert_eq!(
        only_item_users(&grant_body(&f, old_id).await.expect("old"), "policyItems"),
        vec![name.clone()]
    );

    f.groups
        .sync_user_oidc_groups(
            UserId(uid),
            std::slice::from_ref(&new_name),
            &PermGroupRecompiler,
        )
        .await
        .expect("sync");
    assert_eq!(
        grant_body(&f, old_id).await,
        None,
        "the IdP dropped the group, so the login must drop the access it gave"
    );
    assert_eq!(
        only_item_users(&grant_body(&f, new_id).await.expect("new"), "policyItems"),
        vec![name]
    );
}

/// `GroupRepo::delete_local` and `rename` (2026-10-18 design R1): while a grant
/// names the group (by its old name, or by the name a rename would give it),
/// both are refused with `Conflict` listing the entry, and no compiled body
/// changes. Once the grants are gone, the delete and the rename succeed.
///
/// Before R1 this test pinned the opposite — "the old name now names nobody"
/// — which is exactly the leftover reference an IdP claim or a later group of
/// that name could inherit.
#[tokio::test]
async fn deleting_or_renaming_a_referenced_group_is_refused_until_the_grants_are_gone() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let (uid, name) = person(&db, "grp-life").await;
    let old_name = unique("before");
    let new_name = unique("after");
    let gid = new_group(&db, &old_name).await;
    seed_member(&db, gid, uid).await;

    let old_id = create_and_compile(
        &f,
        &grant(&f, Effect::Allow, vec![group(&old_name)], vec![]),
    )
    .await;
    // A grant on the NEW name, stored by `PermStore` directly — the authoring
    // boundary would refuse a name no group has. A leftover reference.
    let new_id = create_and_compile(
        &f,
        &grant(&f, Effect::Allow, vec![group(&new_name)], vec![]),
    )
    .await;
    let old_body = grant_body(&f, old_id).await.expect("old body");
    assert_eq!(only_item_users(&old_body, "policyItems"), vec![name]);
    assert_eq!(grant_body(&f, new_id).await, None);

    let msg = conflict_message(
        f.groups
            .rename(
                gid,
                &new_name,
                &PermGroupRecompiler,
                egret_auth::groups::ReferenceDetail::Entries,
            )
            .await
            .expect_err("a rename of a referenced group is refused"),
    );
    assert!(
        msg.contains(&format!(
            "grant {old_id} (allow, include) naming `{old_name}`"
        )),
        "{msg}"
    );
    assert!(
        msg.contains(&format!(
            "grant {new_id} (allow, include) naming `{new_name}`"
        )),
        "{msg}"
    );
    let msg = conflict_message(
        f.groups
            .delete_local(
                gid,
                &PermGroupRecompiler,
                egret_auth::groups::ReferenceDetail::Entries,
            )
            .await
            .expect_err("a delete of a referenced group is refused"),
    );
    assert_eq!(
        msg,
        format!(
            "group `{old_name}` is named by 1 permission entry; remove them first: \
             grant {old_id} (allow, include)"
        )
    );
    assert_eq!(
        grant_body(&f, old_id).await,
        Some(old_body),
        "nothing recompiled"
    );
    assert_eq!(grant_body(&f, new_id).await, None);
    assert_eq!(group_rows(&db, &old_name).await, 1, "nothing written");

    delete_grant(&f, new_id).await;
    let msg = conflict_message(
        f.groups
            .rename(
                gid,
                &new_name,
                &PermGroupRecompiler,
                egret_auth::groups::ReferenceDetail::Entries,
            )
            .await
            .expect_err("still refused: the OLD name is named"),
    );
    assert!(
        msg.contains(&old_id.to_string()) && !msg.contains(&new_id.to_string()),
        "{msg}"
    );

    delete_grant(&f, old_id).await;
    let rows = f
        .groups
        .rename(
            gid,
            &new_name,
            &PermGroupRecompiler,
            egret_auth::groups::ReferenceDetail::Entries,
        )
        .await
        .expect("rename once nothing names either name");
    assert_eq!(rows, 1);
    let rows = f
        .groups
        .delete_local(
            gid,
            &PermGroupRecompiler,
            egret_auth::groups::ReferenceDetail::Entries,
        )
        .await
        .expect("delete once nothing names the group");
    assert_eq!(rows, 1);
    assert_eq!(group_rows(&db, &new_name).await, 0);
}

/// R1, the half a delete does not have: renaming a group INTO a name that only
/// a leftover grant names would hand its members that grant — with
/// `group:update` alone. Refused, and the grant's body does not gain them.
#[tokio::test]
async fn renaming_into_a_name_a_leftover_grant_names_is_refused() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let (uid, member) = person(&db, "rn-into").await;
    let (_, bystander) = person(&db, "rn-by").await;
    let current = unique("rn-current");
    let target = unique("rn-target");
    let gid = new_group(&db, &current).await;
    seed_member(&db, gid, uid).await;
    let grant_id = create_and_compile(
        &f,
        &grant(
            &f,
            Effect::Allow,
            vec![group(&target), user(&bystander)],
            vec![],
        ),
    )
    .await;
    let before = grant_body(&f, grant_id).await.expect("body");
    assert_eq!(only_item_users(&before, "policyItems"), vec![bystander]);

    let msg = conflict_message(
        f.groups
            .rename(
                gid,
                &target,
                &PermGroupRecompiler,
                egret_auth::groups::ReferenceDetail::Entries,
            )
            .await
            .expect_err("renaming into a referenced name is refused"),
    );
    assert!(
        msg.contains(&format!(
            "grant {grant_id} (allow, include) naming `{target}`"
        )),
        "{msg}"
    );
    assert_eq!(
        group_rows(&db, &target).await,
        0,
        "the rename was not written"
    );
    let after = grant_body(&f, grant_id).await.expect("body");
    assert!(
        !only_item_users(&after, "policyItems").contains(&member),
        "the members must not reach the leftover grant: {after:#}"
    );
    assert_eq!(after, before);
}

fn conflict_message(e: egret_core::Error) -> String {
    match e {
        egret_core::Error::Conflict(m) => m,
        other => panic!("expected Conflict, got {other:?}"),
    }
}

/// Delete a grant through `PermStore` and recompile, as the route does.
async fn delete_grant(f: &Fixture, id: Uuid) {
    f.perm.delete_grant(id).await.expect("delete grant");
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile");
}

/// `UserRepo::hard_delete` (the control plane's `DELETE /users/{id}` and the
/// Data API's service-account delete): the deleted user leaves every policy
/// their groups reach.
#[tokio::test]
async fn hard_deleting_a_member_recompiles() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let (keep_id, keep) = person(&db, "hd-keep").await;
    let (gone_id, gone) = person(&db, "hd-gone").await;
    let group_name = unique("hd-group");
    let gid = new_group(&db, &group_name).await;
    seed_member(&db, gid, keep_id).await;
    seed_member(&db, gid, gone_id).await;

    let grant_id = create_and_compile(
        &f,
        &grant(&f, Effect::Allow, vec![group(&group_name)], vec![]),
    )
    .await;
    assert_eq!(
        only_item_users(&grant_body(&f, grant_id).await.expect("p"), "policyItems"),
        sorted(vec![keep.clone(), gone])
    );

    UserRepo::new(db.clone())
        .hard_delete(UserId(gone_id), &deleter(), &PermGroupRecompiler)
        .await
        .expect("hard delete");
    assert_eq!(
        only_item_users(&grant_body(&f, grant_id).await.expect("p"), "policyItems"),
        vec![keep]
    );
}

/// A write that reaches no profile — a group no grant or rule names — locks
/// and compiles nothing, so an unrelated membership change cannot be refused by
/// an unrelated profile's compile.
#[tokio::test]
async fn a_membership_change_nothing_references_changes_no_policy_version() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    let (uid, _) = person(&db, "unref").await;
    let gid = new_group(&db, &unique("unreferenced")).await;

    let version = |db: Db, id: i64| async move {
        sqlx::query_scalar::<_, i64>(
            "SELECT policy_version FROM policy.authz_services WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&db.pool)
        .await
        .expect("version")
    };
    let before = version(db.clone(), f.service_id).await;
    f.groups
        .add_member(gid, UserId(uid), &PermGroupRecompiler)
        .await
        .expect("add");
    assert_eq!(version(db.clone(), f.service_id).await, before);
}

// ── 2026-10-18 design R2/R8: the IdP cannot take over a dangling group name ──

/// An OIDC login (`provision_and_sync`, the callback's whole write path) by a
/// fresh subject claiming `groups`, with the real port. Returns the new user.
async fn login(db: &Db, groups: &[&str]) -> Uuid {
    let tag = unique("idp").replace('_', "-");
    let claims = OidcClaims {
        subject: format!("sub-{tag}"),
        email: format!("{tag}@idp.local"),
        email_verified: true,
        name: "IdP User".into(),
        groups: groups.iter().map(|g| g.to_string()).collect(),
    };
    provision_and_sync(
        &UserRepo::new(db.clone()),
        &GroupRepo::new(db.clone()),
        &claims,
        &PermGroupRecompiler,
    )
    .await
    .expect("login")
    .0
}

async fn group_rows(db: &Db, name: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM groups WHERE name = $1")
        .bind(name)
        .fetch_one(&db.pool)
        .await
        .expect("groups count")
}

async fn group_names_of(db: &Db, user_id: Uuid) -> Vec<String> {
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT g.name FROM user_groups ug JOIN groups g ON g.id = ug.group_id \
         WHERE ug.user_id = $1",
    )
    .bind(user_id)
    .fetch_all(&db.pool)
    .await
    .expect("memberships");
    sorted(names)
}

async fn username_of(db: &Db, user_id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT username FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&db.pool)
        .await
        .expect("username")
}

/// A mask on the fixture's `sales.orders.email` exempting `exempt`.
fn exempting_rule(f: &Fixture, exempt: PrincipalSet) -> Rule {
    Rule {
        id: Uuid::nil(),
        name: unique("dangling-exempt"),
        kind: RuleKind::Mask {
            mask_type: "MASK".to_string(),
            value_expr: None,
        },
        selector: Selector::Path(Scope {
            catalog_id: f.catalog_id,
            schema: Some("sales".into()),
            table: Some("orders".into()),
            column: Some("email".into()),
        }),
        exempt,
    }
}

/// **The headline (R2).** A grant still names `G` after `G`'s row is gone, and
/// a rule exemption names `R` likewise. A login whose IdP claims both must
/// not create either group: before R2 it created `G` as an OIDC group, joined
/// the user, and recompiled the grant to name them — the grant written for
/// the deleted group now granted whoever the IdP put in a group of that name.
#[tokio::test]
async fn a_login_does_not_create_a_group_a_grant_still_names() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let (_, bystander) = person(&db, "dangling-by").await;
    let g = unique("deleted-grp");
    let r = unique("deleted-exempt");
    // `G` existed when the grant was written, then went away. Deleted behind
    // every writer's back, so this test is about the login, not the delete.
    let gid = new_group(&db, &g).await;
    let grant_id = create_and_compile(
        &f,
        &grant(&f, Effect::Allow, vec![group(&g), user(&bystander)], vec![]),
    )
    .await;
    f.perm
        .create_rule(&exempting_rule(
            &f,
            PrincipalSet {
                include: vec![group(&r)],
                exclude: vec![],
            },
        ))
        .await
        .expect("create rule");
    sqlx::query("DELETE FROM groups WHERE id = $1")
        .bind(gid)
        .execute(&db.pool)
        .await
        .expect("delete group");
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile");
    let before = grant_body(&f, grant_id).await.expect("grant body");
    assert_eq!(only_item_users(&before, "policyItems"), vec![bystander]);

    let uid = login(&db, &[&g, &r]).await;

    assert_eq!(group_rows(&db, &g).await, 0, "no `groups` row for {g:?}");
    assert_eq!(group_rows(&db, &r).await, 0, "no `groups` row for {r:?}");
    assert_eq!(group_names_of(&db, uid).await, Vec::<String>::new());
    assert_eq!(
        grant_body(&f, grant_id).await,
        Some(before),
        "the grant written for the deleted group must not reach the IdP's user"
    );
}

/// The same login when nothing names the claimed name: the group is created
/// and joined, as before R2.
#[tokio::test]
async fn a_login_creates_and_joins_a_group_nothing_names() {
    let Some(db) = test_db().await else { return };
    let _f = seed(&db).await;

    let g = unique("idp-new");
    let uid = login(&db, &[&g]).await;

    let source: String = sqlx::query_scalar("SELECT source FROM groups WHERE name = $1")
        .bind(&g)
        .fetch_one(&db.pool)
        .await
        .expect("the group is created");
    assert_eq!(source, "oidc");
    assert_eq!(group_names_of(&db, uid).await, vec![g]);
}

/// A claim naming an EXISTING OIDC group that a grant names is joined, and the
/// grant recompiled to name the user, as before R2. A reference to a name that
/// has a row is a reference to that group, so it never blocks.
#[tokio::test]
async fn a_login_joins_an_existing_referenced_oidc_group_and_recompiles() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let g = unique("idp-existing");
    f.groups.upsert_oidc_group(&g).await.expect("group");
    let grant_id = create_and_compile(&f, &grant(&f, Effect::Allow, vec![group(&g)], vec![])).await;
    assert_eq!(grant_body(&f, grant_id).await, None, "no member yet");

    let uid = login(&db, &[&g]).await;

    assert_eq!(group_names_of(&db, uid).await, vec![g]);
    let name = username_of(&db, uid).await.expect("minted by the compile");
    assert_eq!(
        only_item_users(&grant_body(&f, grant_id).await.expect("p"), "policyItems"),
        vec![name]
    );
}

/// A claim of `public` with an allow on `public` stored: not refused, and the
/// row exists afterwards, as before. A reference to `public` does not depend
/// on a row (every engine resolves it), so it is not counted. The refusal list
/// is asserted directly, so the test does not depend on whether another test
/// already created the shared `public` row, and it deletes nothing.
#[tokio::test]
async fn a_login_claiming_public_is_not_refused_by_a_public_allow() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    create_and_compile(&f, &grant(&f, Effect::Allow, vec![group("public")], vec![])).await;

    let refused = f
        .groups
        .upsert_claimed_oidc_groups(&["public".to_string()], &PermGroupRecompiler)
        .await
        .expect("upsert");
    assert_eq!(refused, Vec::<String>::new(), "`public` is never refused");
    let uid = login(&db, &["public"]).await;
    assert_eq!(group_rows(&db, "public").await, 1);
    assert_eq!(group_names_of(&db, uid).await, vec!["public".to_string()]);
}

/// R8: the port reports every grant principal and rule exemption naming one of
/// the given groups, with the grant's effect, the rule's name and the
/// position — and never `public`, nor a group it was not asked about.
#[tokio::test]
async fn the_port_reports_grant_and_rule_references_to_a_group() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let (_, someone) = person(&db, "refs-u").await;
    let g = unique("refs-grp");
    let other = unique("refs-other");
    let allow_id = f
        .perm
        .create_grant(&grant(
            &f,
            Effect::Allow,
            vec![group(&g), group("public"), group(&other)],
            vec![],
        ))
        .await
        .expect("allow");
    let deny_id = f
        .perm
        .create_grant(&grant(
            &f,
            Effect::Deny,
            vec![user(&someone)],
            vec![group(&g)],
        ))
        .await
        .expect("deny");
    let rule = exempting_rule(
        &f,
        PrincipalSet {
            include: vec![group(&g)],
            exclude: vec![],
        },
    );
    let rule_id = f.perm.create_rule(&rule).await.expect("rule");
    // A TAG-selector rule exempting `someone` EXCEPT `G`: the exclude position,
    // and a selector with no catalog, are found too.
    let tag_key = unique("refs-tag");
    sqlx::query("INSERT INTO tags (key, name) VALUES ($1, $1)")
        .bind(&tag_key)
        .execute(&db.pool)
        .await
        .expect("tag");
    let tag_rule = Rule {
        id: Uuid::nil(),
        name: unique("refs-tag-rule"),
        kind: RuleKind::RowFilter {
            filter_expr: "1 = 1".to_string(),
        },
        selector: Selector::Tag {
            key: tag_key.clone(),
        },
        exempt: PrincipalSet {
            include: vec![user(&someone)],
            exclude: vec![group(&g)],
        },
    };
    let tag_rule_id = f.perm.create_rule(&tag_rule).await.expect("tag rule");

    let mut tx = db.pool.begin().await.expect("begin");
    let got = PermGroupRecompiler
        .group_references_in_tx(&mut tx, &[g.clone(), "public".to_string()])
        .await
        .expect("references");
    tx.rollback().await.expect("rollback");

    let reference =
        |source: &str, id: Uuid, rule_name: Option<&str>, effect: Option<&str>, position: &str| {
            GroupReference {
                group_name: g.clone(),
                source: source.into(),
                id,
                rule_name: rule_name.map(str::to_string),
                effect: effect.map(str::to_string),
                position: position.into(),
            }
        };
    let mut grants = vec![
        reference("grant", allow_id, None, Some("allow"), "include"),
        reference("grant", deny_id, None, Some("deny"), "exclude"),
    ];
    grants.sort_by_key(|r| r.id);
    let mut rules = vec![
        reference("rule", rule_id, Some(&rule.name), None, "include"),
        reference("rule", tag_rule_id, Some(&tag_rule.name), None, "exclude"),
    ];
    rules.sort_by_key(|r| r.id);
    let mut expected = grants;
    expected.extend(rules);
    assert_eq!(got, expected);
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
