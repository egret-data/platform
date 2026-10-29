//! SP4a **D3**: assigning a catalog to a workspace grants that workspace read
//! — for real, inside the engine.
//!
//! The implicit grant's only principal is `workspace_members`, and D7 measured
//! (`docs/en/plans/2026-09-24-permission-model-sp4a-d7-result.md`) that a
//! Ranger GROUP item matches nobody on any engine egret provisions: Trino has
//! no group provider, egret writes none, egret runs no usersync. So the
//! principal compiles to **the member list enumerated at compile time**, as
//! the bare usernames the engine actually sees.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test workspace_members
//!
//! Every assertion is scoped to the fixture's OWN service/workspace ids — one
//! database is shared across a binary's concurrently running tests, so an
//! unscoped count is racy — and asserts WHICH username is named, never only
//! how many there are.

use egret_metastore::Db;
use egret_perm::eval::evaluate;
use egret_perm::{Action, Decision, PermStore, Reason, Scope, Subject as EvalSubject};
use egret_perm_compile::{
    affected_services_for_workspace_members, recompile_service, recompile_services_in_tx,
};
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
    workspace_id: Uuid,
    catalog_id: Uuid,
    catalog_name: String,
}

/// A StarRocks profile with one engine bound to it, one registry catalog
/// attached to that engine, and one workspace — but NO grants and no rules.
/// StarRocks rather than Trino so nothing here needs a tag service: D3 is
/// about an access policy, and the tag-service routing has its own tests.
async fn seed(db: &Db) -> Fixture {
    let pool = &db.pool;
    let perm = PermStore::new(pool.clone());
    let policy = PolicyStore::new(db.clone());

    egret_policy::seed_starrocks(&policy)
        .await
        .expect("seed starrocks service-def");
    let service = policy
        .create_service(&unique("sr_wsmem"), "starrocks", &serde_json::json!({}))
        .await
        .expect("create service");

    let workspace_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique("wsmem-ws"))
            .bind("WS Members")
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-wsmem-test','AKIATEST') RETURNING id",
    )
    .bind(unique("wsmem-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("wsmem_wh"))
    .bind(workspace_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'starrocks','egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("wsmem-eng"))
    .bind(service.id)
    .fetch_one(pool)
    .await
    .expect("engine");
    let catalog_name = unique("wsmem_cat");
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
        workspace_id,
        catalog_id,
        catalog_name,
    }
}

/// Insert a user. `username` of `None` leaves the column NULL, which is what
/// EVERY user-insert path in egret actually does — `insert_local`, the OIDC
/// upsert and the kernel's user rows all leave it unset, and the value is
/// minted lazily on first engine use.
async fn new_user(db: &Db, email: &str, username: Option<&str>) -> Uuid {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) VALUES ('local',$1,$2,$3) RETURNING id",
    )
    .bind(email)
    .bind("Test User")
    .bind(username)
    .fetch_one(&db.pool)
    .await
    .expect("user");
    id
}

/// Make `user_id` a member of `ws` the way `MemberRepo` does: one live `allow`
/// workspace-scoped `access_grants` row.
async fn add_member(db: &Db, ws: Uuid, user_id: Uuid) {
    let res = sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $1, r.id, 'workspace', $2 FROM roles r \
          WHERE r.scope = 'workspace' AND r.name = 'ws_viewer'",
    )
    .bind(user_id)
    .bind(ws)
    .execute(&db.pool)
    .await
    .expect("membership");
    // A helper that silently inserts NOTHING (a role name that does not exist,
    // say) turns every assertion below into a test of the empty-workspace path
    // wearing the name of the member path.
    assert_eq!(res.rows_affected(), 1, "the membership row must exist");
}

async fn remove_member(db: &Db, ws: Uuid, user_id: Uuid) {
    let res = sqlx::query(
        "DELETE FROM access_grants WHERE principal_type = 'user' AND principal_id = $1 \
           AND scope_type = 'workspace' AND scope_id = $2",
    )
    .bind(user_id)
    .bind(ws)
    .execute(&db.pool)
    .await
    .expect("remove membership");
    assert_eq!(
        res.rows_affected(),
        1,
        "the membership row must have existed"
    );
}

/// The compiled access policy for the catalog's implicit grant, by the
/// compiler's own name prefix, scoped to this fixture's service.
async fn implicit_policy(db: &Db, service_id: i64, grant_id: Uuid) -> Option<Value> {
    sqlx::query_scalar(
        "SELECT policy_jsonb FROM policy.authz_policies \
          WHERE service_id = $1 AND name = $2",
    )
    .bind(service_id)
    .bind(format!("{COMPILER_GRANT_PREFIX}{grant_id}"))
    .fetch_optional(&db.pool)
    .await
    .expect("policy read")
}

/// The `users` array of the policy's single allow item.
fn named_users(body: &Value) -> Vec<String> {
    let items = body["policyItems"].as_array().expect("policyItems");
    assert_eq!(
        items.len(),
        1,
        "an expanded grant is ONE item with many users, not one item per user: {body:#}"
    );
    items[0]["users"]
        .as_array()
        .expect("users")
        .iter()
        .map(|v| v.as_str().expect("username").to_string())
        .collect()
}

/// The id of the one implicit grant the assignment wrote for this catalog.
async fn assign_and_get_grant_id(db: &Db, f: &Fixture) -> Uuid {
    egret_perm::sync_assignment_grants(&f.perm, f.catalog_id, &[f.workspace_id])
        .await
        .expect("assign catalog to workspace");
    sqlx::query_scalar(
        "SELECT id FROM perm.grants WHERE catalog_id = $1 AND origin = 'workspace_assignment'",
    )
    .bind(f.catalog_id)
    .fetch_one(&db.pool)
    .await
    .expect("implicit grant")
}

/// **The headline of D3.** Assigning a catalog to a workspace must produce a
/// policy naming each member by the username the ENGINE sees — the stored
/// `users.username`, which D7 measured is what `SELECT current_user` returns
/// through egret (`admin`, not `admin@egret.local`).
#[tokio::test]
async fn assigning_a_catalog_names_each_members_stored_username() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let alice = new_user(
        &db,
        &format!("{}@egret.local", unique("alice")),
        Some("wsmem-alice"),
    )
    .await;
    let bob = new_user(
        &db,
        &format!("{}@egret.local", unique("bob")),
        Some("wsmem-bob"),
    )
    .await;
    let outsider = new_user(
        &db,
        &format!("{}@egret.local", unique("outsider")),
        Some("wsmem-outsider"),
    )
    .await;
    add_member(&db, f.workspace_id, alice).await;
    add_member(&db, f.workspace_id, bob).await;
    let _ = outsider; // deliberately NOT a member

    let grant_id = assign_and_get_grant_id(&db, &f).await;
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile");

    let body = implicit_policy(&db, f.service_id, grant_id)
        .await
        .expect("the implicit grant must compile to a policy, not be skipped");
    let users = named_users(&body);
    assert_eq!(
        users,
        vec!["wsmem-alice".to_string(), "wsmem-bob".to_string()],
        "every member is named, sorted, and nobody else is"
    );
    assert!(
        !users.iter().any(|u| u == "wsmem-outsider"),
        "a non-member must not be named: {users:?}"
    );
    // The policy must reach the data, not just exist: an access policy names
    // every level the service-def declares (Task 10's live finding).
    assert_eq!(body["resources"]["catalog"]["values"][0], f.catalog_name);
    assert_eq!(body["resources"]["column"]["values"][0], "*");
    assert_eq!(body["policyItems"][0]["accesses"][0]["type"], "select");
}

/// **The invisible failure.** `ensure_username` resolves a slug collision by
/// appending `-2`, `-3`, … so two humans whose email local parts slug to the
/// same base are `bob` and `bob-2` inside the engine. A compiler that
/// RE-DERIVED the username from the email would name `bob` for both: the
/// policy compiles, deploys, is listed live by egret — and the second person
/// is granted nothing, with no error anywhere to say so.
#[tokio::test]
async fn a_collision_suffixed_username_is_named_not_the_base_slug() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    // Same local part, different domains — exactly the shape that produces a
    // `-2` suffix — and the SECOND one is the member.
    let local = unique("dupe");
    let first = new_user(&db, &format!("{local}@egret.local"), Some(&local)).await;
    let second = new_user(
        &db,
        &format!("{local}@other.local"),
        Some(&format!("{local}-2")),
    )
    .await;
    let _ = first;
    add_member(&db, f.workspace_id, second).await;

    let grant_id = assign_and_get_grant_id(&db, &f).await;
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile");

    let body = implicit_policy(&db, f.service_id, grant_id)
        .await
        .expect("policy");
    assert_eq!(
        named_users(&body),
        vec![format!("{local}-2")],
        "the STORED username must be emitted verbatim — a re-derivation would name {local:?} \
         and grant the wrong person"
    );
}

/// `users.username` is NULLable and no user-insert path sets it; it is minted
/// lazily on first engine use. A compiler that only READ the column would omit
/// every member who has not yet run a query — on a fresh install, that is
/// everyone, including the bootstrap admin, so D3 would compile to a policy
/// naming nobody. The compile mints, and the minted value is the one the
/// engine token minter hands out later.
#[tokio::test]
async fn a_member_with_no_username_yet_is_minted_and_named() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let local = unique("fresh");
    let fresh = new_user(&db, &format!("{local}@egret.local"), None).await;
    add_member(&db, f.workspace_id, fresh).await;

    let grant_id = assign_and_get_grant_id(&db, &f).await;
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile");

    let stored: Option<String> =
        sqlx::query_scalar("SELECT username::text FROM users WHERE id = $1")
            .bind(fresh)
            .fetch_one(&db.pool)
            .await
            .expect("username");
    let stored = stored.expect("the compile must have minted a username, not left it NULL");
    assert_eq!(stored, local, "minted from the email local-part");

    let body = implicit_policy(&db, f.service_id, grant_id)
        .await
        .expect("policy");
    assert_eq!(named_users(&body), vec![stored]);
}

/// A membership write must recompile, or the enumeration is stale. Both
/// directions, because they fail differently: a missed ADD denies someone who
/// should have access, a missed REMOVE keeps granting someone who should not.
#[tokio::test]
async fn adding_and_removing_a_member_recompiles_the_affected_profile() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let alice = new_user(
        &db,
        &format!("{}@egret.local", unique("churn-a")),
        Some("churn-alice"),
    )
    .await;
    let bob = new_user(
        &db,
        &format!("{}@egret.local", unique("churn-b")),
        Some("churn-bob"),
    )
    .await;
    add_member(&db, f.workspace_id, alice).await;

    let grant_id = assign_and_get_grant_id(&db, &f).await;
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile");
    assert_eq!(
        named_users(&implicit_policy(&db, f.service_id, grant_id).await.unwrap()),
        vec!["churn-alice".to_string()]
    );

    // The affected set is what a membership write resolves, and it must name
    // this fixture's profile — that is the wiring `MemberRepo` depends on.
    let mut conn = db.pool.acquire().await.expect("conn");
    let affected = affected_services_for_workspace_members(&mut conn, f.workspace_id)
        .await
        .expect("affected services");
    drop(conn);
    assert!(
        affected.contains(&f.service_id),
        "a membership write in this workspace must recompile this profile; got {affected:?}"
    );
    assert!(
        affected.is_sorted(),
        "ascending is the lock order `lock_services_in_tx` debug_asserts: {affected:?}"
    );

    // ADD: the new member appears, in the same transaction as the write.
    add_member(&db, f.workspace_id, bob).await;
    let mut tx = db.pool.begin().await.expect("tx");
    recompile_services_in_tx(&mut tx, &affected)
        .await
        .expect("recompile after add");
    tx.commit().await.expect("commit");
    assert_eq!(
        named_users(&implicit_policy(&db, f.service_id, grant_id).await.unwrap()),
        vec!["churn-alice".to_string(), "churn-bob".to_string()],
        "the member added since the last compile must be named"
    );

    // REMOVE: the departed member is gone. This is the fail-OPEN direction.
    remove_member(&db, f.workspace_id, alice).await;
    let mut tx = db.pool.begin().await.expect("tx");
    recompile_services_in_tx(&mut tx, &affected)
        .await
        .expect("recompile after remove");
    tx.commit().await.expect("commit");
    let after = named_users(&implicit_policy(&db, f.service_id, grant_id).await.unwrap());
    assert_eq!(
        after,
        vec!["churn-bob".to_string()],
        "a removed member must disappear from the compiled policy"
    );
}

/// Requirement 3: egret's own ORACLE (`egret_perm::eval::evaluate`) and the
/// compiled policy must say the SAME thing about the implicit grant. The
/// oracle matches `workspace_members` through `Subject::workspaces`; the
/// compiler matches it by naming usernames. Both are checked here against one
/// model, for a member and for a non-member.
///
/// **Renamed in SP4d-2a Task 5 (was
/// `explain_and_the_compiled_policy_agree_for_the_implicit_grant`).**
/// `/perm/explain` no longer answers with `evaluate` at all — it evaluates the
/// compiled artifact each engine downloads — so the old name described a
/// relationship that no longer exists. What is asserted here has not changed
/// and still matters: the model and the compiled policy must not part company
/// about the one grant SP4a creates automatically. The endpoint side of the
/// same alarm is
/// `routes.rs::the_artifact_answer_and_the_model_oracle_agree_on_the_happy_path`.
#[tokio::test]
async fn the_oracle_and_the_compiled_policy_agree_for_the_implicit_grant() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let member = new_user(
        &db,
        &format!("{}@egret.local", unique("oracle-m")),
        Some("oracle-member"),
    )
    .await;
    let stranger = new_user(
        &db,
        &format!("{}@egret.local", unique("oracle-s")),
        Some("oracle-stranger"),
    )
    .await;
    add_member(&db, f.workspace_id, member).await;
    let _ = stranger;

    let grant_id = assign_and_get_grant_id(&db, &f).await;
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile");

    let grants = f.perm.list_grants().await.expect("grants");
    let grants: Vec<_> = grants.into_iter().filter(|g| g.id == grant_id).collect();
    assert_eq!(grants.len(), 1, "exactly this fixture's implicit grant");
    let target = Scope {
        catalog_id: f.catalog_id,
        schema: Some("sales".into()),
        table: Some("orders".into()),
        column: Some("id".into()),
    };

    let decision = evaluate(
        &EvalSubject {
            user: "oracle-member".into(),
            groups: vec![],
            roles: vec![],
            workspaces: vec![f.workspace_id],
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
        "the oracle allows the member through the implicit grant"
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
        "the oracle refuses a non-member: {stranger_decision:?}"
    );

    // …and the compiled policy says exactly the same, by naming one and not
    // the other. Disagreement here is the defect this test exists to catch:
    // egret's explain screen and the engine would answer differently about the
    // one grant SP4a creates automatically.
    let users = named_users(&implicit_policy(&db, f.service_id, grant_id).await.unwrap());
    assert_eq!(users, vec!["oracle-member".to_string()]);
    assert!(!users.iter().any(|u| u == "oracle-stranger"));
}

/// **How the expansion meets the ceilings.** A grant expanded across N members
/// is ONE policy with ONE item carrying N usernames — not N policies. Both
/// ceilings (`DEFAULT_EXPANSION_CEILING` per rule, `PROFILE_EXPANSION_CEILING`
/// per profile total) count POLICIES, so member expansion contributes exactly
/// 1 to the profile total however many members there are, and 0 to any rule's
/// count. That is recorded here as an assertion rather than as prose, because
/// the alternative shape (one policy per member) would silently make a
/// 1000-member workspace blow a ceiling measured for something else.
#[tokio::test]
async fn an_expanded_grant_is_one_policy_with_one_item_however_many_members() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let mut expected: Vec<String> = Vec::new();
    for i in 0..12 {
        let name = format!("{}-{i:02}", unique("bulk"));
        let id = new_user(&db, &format!("{name}@egret.local"), Some(&name)).await;
        add_member(&db, f.workspace_id, id).await;
        expected.push(name);
    }
    expected.sort();

    let grant_id = assign_and_get_grant_id(&db, &f).await;
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile");

    let policy_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM policy.authz_policies WHERE service_id = $1 AND name LIKE $2 || '%'",
    )
    .bind(f.service_id)
    .bind(COMPILER_GRANT_PREFIX)
    .fetch_one(&db.pool)
    .await
    .expect("count");
    assert_eq!(
        policy_count, 1,
        "12 members must be 1 policy — the ceilings count policies, not principals"
    );

    let body = implicit_policy(&db, f.service_id, grant_id)
        .await
        .expect("policy");
    // `named_users` itself asserts there is exactly ONE item.
    assert_eq!(named_users(&body), expected);
}

/// A group that holds a workspace makes its members workspace members: the
/// implicit grant names them, as the PDP already lets them in (the group arm).
#[tokio::test]
async fn a_group_holding_the_workspace_names_its_members() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;

    let member = new_user(
        &db,
        &format!("{}@egret.local", unique("grpmem")),
        Some("wsgrp-member"),
    )
    .await;
    let outsider = new_user(
        &db,
        &format!("{}@egret.local", unique("grpout")),
        Some("wsgrp-outsider"),
    )
    .await;
    let group_id: Uuid = sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
        .bind(unique("wsgrp"))
        .fetch_one(&db.pool)
        .await
        .expect("group");
    sqlx::query("INSERT INTO user_groups (user_id, group_id) VALUES ($1, $2)")
        .bind(member)
        .bind(group_id)
        .execute(&db.pool)
        .await
        .expect("join group");
    let res = sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'group', $1, r.id, 'workspace', $2 FROM roles r \
          WHERE r.scope = 'workspace' AND r.name = 'ws_viewer'",
    )
    .bind(group_id)
    .bind(f.workspace_id)
    .execute(&db.pool)
    .await
    .expect("group holds the workspace");
    assert_eq!(
        res.rows_affected(),
        1,
        "the group's workspace row must exist"
    );
    let _ = outsider; // not in the group, so not named

    let grant_id = assign_and_get_grant_id(&db, &f).await;
    recompile_service(&f.perm, &f.policy, f.service_id)
        .await
        .expect("recompile");

    let body = implicit_policy(&db, f.service_id, grant_id)
        .await
        .expect("the implicit grant must compile to a policy");
    let users = named_users(&body);
    assert_eq!(
        users,
        vec!["wsgrp-member".to_string()],
        "the group's member is named through the group; the outsider is not: {users:?}"
    );
}

/// A change of who is in a group that holds a workspace reaches the profile
/// that enumerates the workspace's members.
#[tokio::test]
async fn a_group_membership_change_reaches_a_workspace_its_group_holds() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    let group_name = unique("wsgrp-leave");
    let group_id: Uuid = sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
        .bind(&group_name)
        .fetch_one(&db.pool)
        .await
        .expect("group");
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'group', $1, r.id, 'workspace', $2 FROM roles r \
          WHERE r.scope = 'workspace' AND r.name = 'ws_viewer'",
    )
    .bind(group_id)
    .bind(f.workspace_id)
    .execute(&db.pool)
    .await
    .expect("group holds the workspace");
    assign_and_get_grant_id(&db, &f).await;

    let mut conn = db.pool.acquire().await.expect("acquire");
    let services = egret_perm_compile::affected_services_for_group_membership(
        &mut conn,
        std::slice::from_ref(&group_name),
    )
    .await
    .expect("affected services");
    assert!(
        services.contains(&f.service_id),
        "a membership change of {group_name:?} must reach the profile naming its workspace: {services:?}"
    );
}
