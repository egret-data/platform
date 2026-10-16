//! SP4c **Task 3, input 3 of three: who holds the clearing role** — and the one
//! test that proves ONE trigger genuinely serves BOTH consumers.
//!
//! A role NAME is unresolvable in every engine egret provisions (D0 on Trino, D0b
//! on StarRocks), so egret enumerates a role's HOLDERS at compile time in two
//! places: a clearance's `denyExceptions` (Task 2/2d) and a `role` principal in a
//! grant (Task 2b). The previous agent flagged that the membership trigger must
//! therefore serve two consumers, and the requirement was ONE trigger, not two —
//! two triggers over one enumeration is two chances for one of them to be
//! forgotten, which is the shape of the defect SP4a shipped four times.
//!
//! **Every assertion reads `policy.authz_policies`, never `access_grants`.** The
//! `access_grants` row was always written correctly; that is precisely why this
//! defect class keeps shipping. A test asserting the grant row exists passes
//! against a compiler that never recompiled at all.
//!
//! Its own binary: a clearance is installation-wide, so each write here
//! recompiles every compiler-targeted profile in the private database
//! `egret-test-db` hands this process.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test clearance_triggers

use egret_metastore::Db;
use egret_perm::{
    Action, Effect, Grant, GrantOrigin, PermStore, Principal, PrincipalKind, PrincipalSet, Scope,
};
use egret_perm_compile::{
    assign_platform_role_and_recompile, recompile_service, unassign_platform_role_and_recompile,
};
use egret_policy::{PolicyStore, COMPILER_CLEARANCE_PREFIX, COMPILER_GRANT_PREFIX};
use serde_json::Value;
use uuid::Uuid;

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Fixture {
    perm: PermStore,
    policy: PolicyStore,
    profile_id: i64,
    /// The first profile's tag service. Kept because it is what a reader expects
    /// to find, and because the LONE one's whole point is that it is not this.
    #[allow(dead_code)]
    tag_service_id: i64,
    /// A SECOND Trino profile's tag service. Its profile has an engine (so the
    /// compiler targets it) but NO catalog attachment, so it is **unreachable
    /// through `services_for_catalog_via`** — the grant arm of the membership
    /// trigger can never name it. A clearance still compiles into it, because a
    /// Trino clearance is installation-wide. That asymmetry is what makes the
    /// clearance arm's absence observable: without it, this tag service is never
    /// recompiled and its exceptions stay stale. The first draft of this test
    /// asserted on the FIRST profile's tag service, and a mutation deleting the
    /// clearance arm SURVIVED, because the grant arm happened to name that same
    /// profile.
    lone_tag_service_id: i64,
    lone_profile_id: i64,
    catalog_id: Uuid,
}

/// A Trino profile with a real tag service (so a clearance can be filed) plus an
/// engine, a registry catalog and a warehouse (so a `role` GRANT on that catalog
/// reaches this profile). One fixture for both consumers, because the point of
/// this file is that one write updates both.
async fn seed(db: &Db) -> Fixture {
    let pool = &db.pool;
    let perm = PermStore::new(pool.clone());
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_trino(&policy).await.expect("seed trino");
    egret_policy::seed_tag(&policy).await.expect("seed tag");

    let profile_name = unique("trino_roletrig");
    let tag_service_name = format!("{profile_name}_tag");
    let tag_service = policy
        .create_service(&tag_service_name, "tag", &serde_json::json!({}))
        .await
        .expect("create tag service");
    let profile = policy
        .create_service(
            &profile_name,
            "trino",
            &serde_json::json!({ "tagService": tag_service_name }),
        )
        .await
        .expect("create trino profile");

    let ws_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1,$2) RETURNING id")
            .bind(unique("roletrig-ws"))
            .bind("Role Trigger WS")
            .fetch_one(pool)
            .await
            .expect("workspace");
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key) \
         VALUES ($1,'us-east-1','egret-roletrig','AKIATEST') RETURNING id",
    )
    .bind(unique("roletrig-sc"))
    .fetch_one(pool)
    .await
    .expect("storage connection");
    let warehouse_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(unique("roletrig_wh"))
    .bind(ws_id)
    .bind(sc_id)
    .fetch_one(pool)
    .await
    .expect("warehouse");
    let engine_id: Uuid = sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'trino','egret','{}'::jsonb,$2) RETURNING id",
    )
    .bind(unique("roletrig-eng"))
    .bind(profile.id)
    .fetch_one(pool)
    .await
    .expect("engine");
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config, warehouse_id) \
         VALUES ($1, 'iceberg', jsonb_build_object('metastore', \
                 jsonb_build_object('type', 'egret', 'warehouse_id', $2::uuid::text)), $2) \
         RETURNING id",
    )
    .bind(unique("roletrig_cat"))
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

    // The second profile: an engine, so the compiler targets it; no catalog, so
    // no grant can reach it.
    let lone_name = unique("trino_lonetrig");
    let lone_tag_name = format!("{lone_name}_tag");
    let lone_tag = policy
        .create_service(&lone_tag_name, "tag", &serde_json::json!({}))
        .await
        .expect("create lone tag service");
    let lone_profile = policy
        .create_service(
            &lone_name,
            "trino",
            &serde_json::json!({ "tagService": lone_tag_name }),
        )
        .await
        .expect("create lone trino profile");
    sqlx::query(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'trino','egret','{}'::jsonb,$2)",
    )
    .bind(unique("lonetrig-eng"))
    .bind(lone_profile.id)
    .execute(pool)
    .await
    .expect("lone engine");
    let _ = warehouse_id;

    Fixture {
        perm,
        policy,
        profile_id: profile.id,
        tag_service_id: tag_service.id,
        lone_tag_service_id: lone_tag.id,
        lone_profile_id: lone_profile.id,
        catalog_id,
    }
}

async fn new_role(db: &Db, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id")
        .bind(name)
        .fetch_one(&db.pool)
        .await
        .expect("insert role")
}

async fn new_user(db: &Db, username: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) VALUES ('local',$1,'U',$2) RETURNING id",
    )
    .bind(format!("{username}@egret.local"))
    .bind(username)
    .fetch_one(&db.pool)
    .await
    .expect("insert user")
}

async fn new_tag(db: &Db) -> (Uuid, String) {
    let name = unique("RoleTrigTag");
    let id: Uuid = sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1,$2) RETURNING id")
        .bind(unique("roletrig-tag"))
        .bind(&name)
        .fetch_one(&db.pool)
        .await
        .expect("insert tag");
    (id, name)
}

async fn gate(db: &Db, tag_id: Uuid, role: &str) {
    sqlx::query(
        "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
         VALUES ($1, ARRAY[$2], ARRAY[]::uuid[]) \
         ON CONFLICT (tag_id) DO UPDATE SET required_roles = EXCLUDED.required_roles",
    )
    .bind(tag_id)
    .bind(role)
    .execute(&db.pool)
    .await
    .expect("classification policy");
}

/// The `denyExceptions` usernames of the clearance for `tag_id`.
async fn cleared_users(db: &Db, tag_service_id: i64, tag_id: Uuid) -> Vec<String> {
    let body: Value = sqlx::query_scalar(
        "SELECT policy_jsonb FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(tag_service_id)
    .bind(format!("{COMPILER_CLEARANCE_PREFIX}{tag_id}"))
    .fetch_one(&db.pool)
    .await
    .expect("the clearance policy must exist");
    body["denyExceptions"][0]["users"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|v| v.as_str().expect("username").to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The `policyItems` usernames of the compiled policy for `grant_id`, or `None`
/// when no policy exists at all.
///
/// The `None` case is a real state, not a harness convenience: a `role` grant
/// whose role has NO holders expands to an empty principal include, and the
/// compiler records that as a reported skip and emits nothing — one fewer policy
/// that would have matched nobody.
async fn granted_users(db: &Db, profile_id: i64, grant_id: Uuid) -> Option<Vec<String>> {
    let body: Option<Value> = sqlx::query_scalar(
        "SELECT policy_jsonb FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(profile_id)
    .bind(format!("{COMPILER_GRANT_PREFIX}{grant_id}"))
    .fetch_optional(&db.pool)
    .await
    .expect("policy read");
    body.map(|b| {
        b["policyItems"][0]["users"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|v| v.as_str().expect("username").to_string())
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// **The one test this file exists for.** A single role-membership write must
/// update BOTH consumers of the enumeration: the clearance's `denyExceptions` and
/// the `role` grant's `policyItems`. A test that checked only one of them would
/// pass against two separate triggers of which one was forgotten — the exact
/// defect shape the requirement "build ONE trigger, not two" exists to rule out.
#[tokio::test]
async fn one_membership_write_updates_both_the_clearance_and_the_role_grant() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let f = seed(&db).await;

    let role_name = unique("dual-consumer");
    let role_id = new_role(&db, &role_name).await;
    let user_id = new_user(&db, &unique("dualholder")).await;
    let username: String = sqlx::query_scalar("SELECT username FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&db.pool)
        .await
        .expect("username");

    // Consumer 1: a clearance gated on the role.
    let (tag_id, _tag_name) = new_tag(&db).await;
    gate(&db, tag_id, &role_name).await;
    // Consumer 2: a read grant whose principal IS the role.
    let grant_id = f
        .perm
        .create_grant(&Grant {
            id: Uuid::nil(),
            effect: Effect::Allow,
            actions: vec![Action::Read],
            scope: Scope {
                catalog_id: f.catalog_id,
                schema: None,
                table: None,
                column: None,
            },
            principals: PrincipalSet {
                include: vec![Principal {
                    kind: PrincipalKind::Role,
                    r#ref: role_name.clone(),
                }],
                exclude: Vec::new(),
            },
            origin: GrantOrigin::Explicit,
        })
        .await
        .expect("create role grant");

    // Compile the starting state once, explicitly, so the write under test is the
    // only thing between the two observations.
    recompile_service(&f.perm, &f.policy, f.profile_id)
        .await
        .expect("baseline recompile");
    recompile_service(&f.perm, &f.policy, f.lone_profile_id)
        .await
        .expect("baseline recompile of the lone profile");
    assert!(
        cleared_users(&db, f.lone_tag_service_id, tag_id)
            .await
            .is_empty(),
        "nobody holds the role yet, so nobody is cleared"
    );
    assert_eq!(
        granted_users(&db, f.profile_id, grant_id).await,
        None,
        "nobody holds the role yet, so the grant expands to nobody and emits no policy"
    );

    // ONE write. No recompile call follows it.
    assign_platform_role_and_recompile(f.perm.pool(), "user", user_id, role_id)
        .await
        .expect("grant the role");

    assert_eq!(
        cleared_users(&db, f.lone_tag_service_id, tag_id).await,
        vec![username.clone()],
        "consumer 1 — the clearance's denyExceptions must gain the new holder on THAT write, in a \
         tag service NO grant can reach, so only the trigger's clearance arm can have brought it"
    );
    assert_eq!(
        granted_users(&db, f.profile_id, grant_id).await,
        Some(vec![username.clone()]),
        "consumer 2 — the role grant's policyItems must gain them on the SAME write. If this \
         passes only when the clearance assertion is removed, there are two triggers and one was \
         forgotten"
    );

    // The REVOKE direction, which is the one that fails OPEN when missed: the
    // revoked holder stays cleared and stays granted.
    unassign_platform_role_and_recompile(f.perm.pool(), "user", user_id, role_id)
        .await
        .expect("revoke the role");
    assert!(
        cleared_users(&db, f.lone_tag_service_id, tag_id)
            .await
            .is_empty(),
        "revoking must remove them from the clearance exceptions — otherwise they keep reading a \
         classified column after their clearance was taken away"
    );
    assert_eq!(
        granted_users(&db, f.profile_id, grant_id).await,
        None,
        "and from the role grant — otherwise the engine keeps granting access the model no longer \
         gives"
    );
}

/// **The trigger's GRANT arm, isolated.** When NO gated tag names the role the
/// clearance arm does not fire at all, so a `role` grant's recompile can only
/// come from the grant arm. Without this test, deleting that arm survives every
/// other assertion here — the clearance arm returns every compiler-targeted
/// profile, which happens to include the grant's own.
#[tokio::test]
async fn a_role_grant_with_no_clearance_naming_the_role_still_recompiles() {
    let Some(dsn) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let f = seed(&db).await;

    // Deliberately NO `classification_policies` row anywhere names this role.
    let role_name = unique("grant-arm-only");
    let role_id = new_role(&db, &role_name).await;
    let user_id = new_user(&db, &unique("grantarm")).await;
    let username: String = sqlx::query_scalar("SELECT username FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&db.pool)
        .await
        .expect("username");

    let grant_id = f
        .perm
        .create_grant(&Grant {
            id: Uuid::nil(),
            effect: Effect::Allow,
            actions: vec![Action::Read],
            scope: Scope {
                catalog_id: f.catalog_id,
                schema: None,
                table: None,
                column: None,
            },
            principals: PrincipalSet {
                include: vec![Principal {
                    kind: PrincipalKind::Role,
                    r#ref: role_name.clone(),
                }],
                exclude: Vec::new(),
            },
            origin: GrantOrigin::Explicit,
        })
        .await
        .expect("create role grant");
    recompile_service(&f.perm, &f.policy, f.profile_id)
        .await
        .expect("baseline recompile");
    assert_eq!(
        granted_users(&db, f.profile_id, grant_id).await,
        None,
        "nobody holds the role yet"
    );

    assign_platform_role_and_recompile(f.perm.pool(), "user", user_id, role_id)
        .await
        .expect("grant the role");
    assert_eq!(
        granted_users(&db, f.profile_id, grant_id).await,
        Some(vec![username.clone()]),
        "the grant arm alone must reach this profile — no gated tag names this role, so the \
         clearance arm contributes nothing"
    );

    unassign_platform_role_and_recompile(f.perm.pool(), "user", user_id, role_id)
        .await
        .expect("revoke the role");
    assert_eq!(
        granted_users(&db, f.profile_id, grant_id).await,
        None,
        "and the revoke direction too — the one that fails OPEN when missed"
    );
}
