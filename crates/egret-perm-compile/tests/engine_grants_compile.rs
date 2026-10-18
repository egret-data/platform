//! F11 Task 4: an engine grant compiles into its engine's profile as ONE
//! `perm-engine-{id}` access policy, expands its group and role grantees like a
//! grant, skips (never fails) a grant whose target can no longer be compiled,
//! counts toward the profile ceiling, and records no `perm.compiled` row.
//!
//! Every query is scoped to this fixture's own services, engines and names:
//! tests in one binary share a database.
//!
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test engine_grants_compile

use egret_metastore::Db;
use egret_perm::{
    create_engine_grant_in_tx, delete_engine_grant_in_tx, EngineGrant, PermStore, Principal,
    PrincipalKind, PrincipalSet,
};
use egret_perm_compile::{recompile_service, SourceKind, PROFILE_EXPANSION_CEILING};
use egret_policy::PolicyStore;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

async fn test_db() -> Option<Db> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..12])
}

struct Fixture {
    db: Db,
    perm: PermStore,
    policy: PolicyStore,
    trino_profile: i64,
    trino_profile_name: String,
    trino_tag_service: i64,
    trino_engine: Uuid,
    sr_profile: i64,
    sr_engine: Uuid,
}

async fn seed(db: &Db) -> Fixture {
    let pool = &db.pool;
    let policy = PolicyStore::new(db.clone());
    egret_policy::seed_trino(&policy).await.expect("trino def");
    egret_policy::seed_starrocks(&policy)
        .await
        .expect("starrocks def");
    egret_policy::seed_tag(&policy).await.expect("tag def");

    let trino_profile_name = unique("eg-trino");
    let tag_name = format!("{trino_profile_name}_tag");
    let tag = policy
        .create_service(&tag_name, "tag", &serde_json::json!({}))
        .await
        .expect("tag service");
    let trino = policy
        .create_service(
            &trino_profile_name,
            "trino",
            &serde_json::json!({ "tagService": tag_name }),
        )
        .await
        .expect("trino profile");
    let sr = policy
        .create_service(&unique("eg-sr"), "starrocks", &serde_json::json!({}))
        .await
        .expect("starrocks profile");
    let trino_engine = engine(pool, "trino", Some(trino.id)).await;
    let sr_engine = engine(pool, "starrocks", Some(sr.id)).await;
    Fixture {
        db: db.clone(),
        perm: PermStore::new(pool.clone()),
        policy,
        trino_profile: trino.id,
        trino_profile_name,
        trino_tag_service: tag.id,
        trino_engine,
        sr_profile: sr.id,
        sr_engine,
    }
}

async fn engine(pool: &PgPool, ty: &str, profile: Option<i64>) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1, $2, 'egret', '{}'::jsonb, $3) RETURNING id",
    )
    .bind(unique("eg-eng"))
    .bind(ty)
    .bind(profile)
    .fetch_one(pool)
    .await
    .expect("engine")
}

async fn user(pool: &PgPool, username: Option<&str>) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) \
         VALUES ('local', $1, 'Engine grant test', $2) RETURNING id",
    )
    .bind(format!("{}@egret.local", unique("eg")))
    .bind(username)
    .fetch_one(pool)
    .await
    .expect("user")
}

async fn person(pool: &PgPool, prefix: &str) -> (Uuid, String) {
    let name = unique(prefix);
    (user(pool, Some(&name)).await, name)
}

async fn group_with(pool: &PgPool, members: &[Uuid]) -> String {
    let name = unique("eg-group");
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
    name
}

async fn role_held_by(pool: &PgPool, holder: Uuid) -> String {
    let name = unique("eg-role");
    let rid: Uuid =
        sqlx::query_scalar("INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id")
            .bind(&name)
            .fetch_one(pool)
            .await
            .expect("role");
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'platform', NULL)",
    )
    .bind(holder)
    .bind(rid)
    .execute(pool)
    .await
    .expect("role grant");
    name
}

fn p(kind: PrincipalKind, r: &str) -> Principal {
    Principal {
        kind,
        r#ref: r.to_string(),
    }
}

async fn create(
    pool: &PgPool,
    engine_id: Uuid,
    capability: &str,
    values: &[&str],
    target: Option<Uuid>,
    include: Vec<Principal>,
) -> Uuid {
    let g = EngineGrant {
        id: Uuid::nil(),
        engine_id,
        capability: capability.into(),
        values: values.iter().map(|v| v.to_string()).collect(),
        target_user_id: target,
        principals: PrincipalSet {
            include,
            exclude: vec![],
        },
    };
    let mut tx = pool.begin().await.expect("begin");
    let id = create_engine_grant_in_tx(&mut tx, &g)
        .await
        .expect("create");
    tx.commit().await.expect("commit");
    id
}

async fn compiled(pool: &PgPool, service_id: i64, grant: Uuid) -> Option<(Value, String)> {
    sqlx::query_as(
        "SELECT policy_jsonb, managed_by FROM policy.authz_policies \
         WHERE service_id = $1 AND name = $2",
    )
    .bind(service_id)
    .bind(format!("perm-engine-{grant}"))
    .fetch_optional(pool)
    .await
    .expect("read")
}

fn users_of(body: &Value) -> Vec<String> {
    let items = body["policyItems"].as_array().expect("items");
    assert_eq!(items.len(), 1, "one item: {body}");
    items[0]["users"]
        .as_array()
        .expect("users")
        .iter()
        .map(|u| u.as_str().unwrap().to_string())
        .collect()
}

async fn policy_version(pool: &PgPool, service_id: i64) -> i64 {
    sqlx::query_scalar("SELECT policy_version FROM policy.authz_services WHERE id = $1")
        .bind(service_id)
        .fetch_one(pool)
        .await
        .expect("version")
}

#[tokio::test]
async fn a_grant_compiles_once_into_its_engines_profile_and_its_delete_reaps_it() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    let pool = &f.db.pool;
    let (_ua, ua) = person(pool, "ua").await;
    let id = create(
        pool,
        f.trino_engine,
        "trino.set_system_session_property",
        &["query_max_memory"],
        None,
        vec![p(PrincipalKind::User, &ua)],
    )
    .await;

    let v0 = policy_version(pool, f.trino_profile).await;
    let summary = recompile_service(&f.perm, &f.policy, f.trino_profile)
        .await
        .expect("recompile");
    assert_eq!(summary.created, 1, "{summary:?}");
    let (body, managed_by) = compiled(pool, f.trino_profile, id).await.expect("compiled");
    assert_eq!(managed_by, "compiler");
    assert_eq!(users_of(&body), vec![ua.clone()]);
    assert_eq!(
        body["resources"]["systemproperty"]["values"],
        serde_json::json!(["query_max_memory"])
    );
    let origin: String =
        sqlx::query_scalar("SELECT origin FROM policy.authz_services WHERE id = $1")
            .bind(f.trino_profile)
            .fetch_one(pool)
            .await
            .expect("origin");
    assert_eq!(origin, "generated");
    assert_eq!(
        policy_version(pool, f.trino_profile).await,
        v0 + 1,
        "bumped once"
    );

    // Nothing anywhere else: not the tag service, not the StarRocks profile.
    assert!(compiled(pool, f.trino_tag_service, id).await.is_none());
    assert!(compiled(pool, f.sr_profile, id).await.is_none());
    // No provenance row: `perm.compiled` is grant/rule-shaped (spec C10).
    let provenance: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM perm.compiled c JOIN policy.authz_policies p ON p.id = c.policy_id \
         WHERE p.name = $1",
    )
    .bind(format!("perm-engine-{id}"))
    .fetch_one(pool)
    .await
    .expect("provenance");
    assert_eq!(provenance, 0);

    let again = recompile_service(&f.perm, &f.policy, f.trino_profile)
        .await
        .expect("recompile again");
    assert!(!again.wrote_anything(), "{again:?}");

    let mut tx = pool.begin().await.expect("begin");
    assert!(delete_engine_grant_in_tx(&mut tx, id)
        .await
        .expect("delete"));
    tx.commit().await.expect("commit");
    recompile_service(&f.perm, &f.policy, f.trino_profile)
        .await
        .expect("recompile after delete");
    assert!(
        compiled(pool, f.trino_profile, id).await.is_none(),
        "reaped"
    );
}

#[tokio::test]
async fn group_and_role_grantees_compile_to_their_members() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    let pool = &f.db.pool;
    let (ua_id, ua) = person(pool, "ua").await;
    let ub_id = user(pool, None).await;
    let g1 = group_with(pool, &[ua_id, ub_id]).await;
    let (ud_id, ud) = person(pool, "ud").await;
    let r1 = role_held_by(pool, ud_id).await;

    let group_grant = create(
        pool,
        f.sr_engine,
        "starrocks.use_global_function",
        &["my_udf(INT)"],
        None,
        vec![p(PrincipalKind::Group, &g1)],
    )
    .await;
    let role_grant = create(
        pool,
        f.sr_engine,
        "starrocks.manage_blacklist",
        &[],
        None,
        vec![p(PrincipalKind::Role, &r1)],
    )
    .await;
    recompile_service(&f.perm, &f.policy, f.sr_profile)
        .await
        .expect("recompile");

    let ub: String = sqlx::query_scalar("SELECT username::text FROM users WHERE id = $1")
        .bind(ub_id)
        .fetch_one(pool)
        .await
        .expect("ub was minted a username");
    let mut want = vec![ua, ub];
    want.sort();
    let (body, _) = compiled(pool, f.sr_profile, group_grant)
        .await
        .expect("group grant");
    assert_eq!(users_of(&body), want);
    assert_eq!(body["serviceType"], "starrocks");
    let (body, _) = compiled(pool, f.sr_profile, role_grant)
        .await
        .expect("role grant");
    assert_eq!(users_of(&body), vec![ud]);
    assert_eq!(
        body["resources"]["system"]["values"],
        serde_json::json!(["*"])
    );
}

#[tokio::test]
async fn a_grant_to_an_empty_group_is_skipped_not_compiled() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    let pool = &f.db.pool;
    let empty = group_with(pool, &[]).await;
    let id = create(
        pool,
        f.trino_engine,
        "trino.set_system_session_property",
        &["query_max_memory"],
        None,
        vec![p(PrincipalKind::Group, &empty)],
    )
    .await;
    let summary = recompile_service(&f.perm, &f.policy, f.trino_profile)
        .await
        .expect("recompile");
    assert!(
        summary
            .skipped
            .iter()
            .any(|s| s.id == id && s.source_kind == SourceKind::EngineGrant),
        "{summary:?}"
    );
    assert!(compiled(pool, f.trino_profile, id).await.is_none());
}

/// The amended E21: the target is a user id, rendered to the CURRENT username
/// at every compile; a target that can no longer be compiled is skipped with
/// the reason and its policy reaped.
#[tokio::test]
async fn an_impersonation_target_is_rendered_from_its_id_and_skipped_when_it_fails() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    let pool = &f.db.pool;
    let (_ua, ua) = person(pool, "ua").await;
    let (uc_id, uc) = person(pool, "uc").await;
    let id = create(
        pool,
        f.trino_engine,
        "trino.impersonate_user",
        &[],
        Some(uc_id),
        vec![p(PrincipalKind::User, &ua)],
    )
    .await;
    recompile_service(&f.perm, &f.policy, f.trino_profile)
        .await
        .expect("recompile");
    let (body, _) = compiled(pool, f.trino_profile, id).await.expect("compiled");
    assert_eq!(
        body["resources"]["trinouser"]["values"],
        serde_json::json!([uc.clone()])
    );

    // The username goes (the user keeps their row): skipped, reaped.
    sqlx::query("UPDATE users SET username = NULL WHERE id = $1")
        .bind(uc_id)
        .execute(pool)
        .await
        .expect("drop username");
    let summary = recompile_service(&f.perm, &f.policy, f.trino_profile)
        .await
        .expect("recompile");
    let skip = summary
        .skipped
        .iter()
        .find(|s| s.id == id)
        .unwrap_or_else(|| panic!("skipped: {summary:?}"));
    assert_eq!(skip.source_kind, SourceKind::EngineGrant);
    assert!(skip.reason.contains("no user carries"), "{}", skip.reason);
    assert!(
        compiled(pool, f.trino_profile, id).await.is_none(),
        "reaped"
    );

    // A new username is followed, because the grant names the person.
    let renamed = unique("uc2");
    sqlx::query("UPDATE users SET username = $2 WHERE id = $1")
        .bind(uc_id)
        .bind(&renamed)
        .execute(pool)
        .await
        .expect("new username");
    recompile_service(&f.perm, &f.policy, f.trino_profile)
        .await
        .expect("recompile");
    let (body, _) = compiled(pool, f.trino_profile, id).await.expect("compiled");
    assert_eq!(
        body["resources"]["trinouser"]["values"],
        serde_json::json!([renamed.clone()])
    );

    // The target becomes an engine admin of the profile: skipped, reaped.
    f.policy
        .seed_admin_all_access(
            f.trino_profile,
            &f.trino_profile_name,
            "trino",
            std::slice::from_ref(&renamed),
        )
        .await
        .expect("seed admin all-access");
    let summary = recompile_service(&f.perm, &f.policy, f.trino_profile)
        .await
        .expect("recompile");
    let skip = summary
        .skipped
        .iter()
        .find(|s| s.id == id)
        .unwrap_or_else(|| panic!("skipped: {summary:?}"));
    assert!(skip.reason.contains("engine admin"), "{}", skip.reason);
    assert!(
        compiled(pool, f.trino_profile, id).await.is_none(),
        "reaped"
    );
}

/// One engine grant is one policy, so it counts toward the profile ceiling
/// like any other: at the ceiling, one more is refused.
#[tokio::test]
async fn engine_grants_count_toward_the_profile_ceiling() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    let pool = &f.db.pool;
    let (_ua, ua) = person(pool, "ua").await;
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "INSERT INTO perm.engine_grants (engine_id, capability, resource_values) \
         SELECT $1, 'trino.set_system_session_property', ARRAY['prop_' || gs::text] \
           FROM generate_series(1, $2) gs RETURNING id",
    )
    .bind(f.trino_engine)
    .bind(PROFILE_EXPANSION_CEILING as i64)
    .fetch_all(pool)
    .await
    .expect("bulk grants");
    sqlx::query(
        "INSERT INTO perm.engine_grant_principals (engine_grant_id, kind, ref) \
         SELECT id, 'user', $2 FROM unnest($1::uuid[]) id",
    )
    .bind(&ids)
    .bind(&ua)
    .execute(pool)
    .await
    .expect("bulk principals");
    let at = recompile_service(&f.perm, &f.policy, f.trino_profile)
        .await
        .expect("exactly at the ceiling compiles");
    assert_eq!(at.created, PROFILE_EXPANSION_CEILING);

    create(
        pool,
        f.trino_engine,
        "trino.set_system_session_property",
        &["one_more"],
        None,
        vec![p(PrincipalKind::User, &ua)],
    )
    .await;
    match recompile_service(&f.perm, &f.policy, f.trino_profile).await {
        Err(egret_perm::CompileError::ProfileExpansionCeiling { total, .. }) => {
            assert_eq!(total, PROFILE_EXPANSION_CEILING + 1)
        }
        other => panic!("expected ProfileExpansionCeiling, got {other:?}"),
    }
}

/// The username pre-mint sweep reads engine-grant principals too: a member of
/// a group that ONLY an engine grant names is minted by a recompile of any
/// profile, before the passes run.
#[tokio::test]
async fn the_premint_sweep_mints_a_member_of_a_group_only_an_engine_grant_names() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    let pool = &f.db.pool;
    let ub_id = user(pool, None).await;
    let g = group_with(pool, &[ub_id]).await;
    // On the StarRocks engine; the Trino profile is what gets recompiled, so
    // only the sweep can reach this group.
    create(
        pool,
        f.sr_engine,
        "starrocks.manage_blacklist",
        &[],
        None,
        vec![p(PrincipalKind::Group, &g)],
    )
    .await;
    recompile_service(&f.perm, &f.policy, f.trino_profile)
        .await
        .expect("recompile another profile");
    let minted: Option<String> =
        sqlx::query_scalar("SELECT username::text FROM users WHERE id = $1")
            .bind(ub_id)
            .fetch_one(pool)
            .await
            .expect("read");
    assert!(
        minted.is_some(),
        "the sweep must mint the member's username"
    );
}

/// Security fix C1: egret runs as `root` on StarRocks, so a grant targeting a
/// user who carries that username (stored before the route refused it, or
/// renamed into it) is skipped at compile.
#[tokio::test]
async fn a_target_whose_username_is_root_is_skipped() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    let pool = &f.db.pool;
    let (_ua, ua) = person(pool, "ua").await;
    let existing: Option<Uuid> = sqlx::query_scalar("SELECT id FROM users WHERE username = 'root'")
        .fetch_optional(pool)
        .await
        .expect("read");
    let root = match existing {
        Some(id) => id,
        None => user(pool, Some("root")).await,
    };
    let id = create(
        pool,
        f.sr_engine,
        "starrocks.impersonate_user",
        &[],
        Some(root),
        vec![p(PrincipalKind::User, &ua)],
    )
    .await;
    let summary = recompile_service(&f.perm, &f.policy, f.sr_profile)
        .await
        .expect("recompile");
    let skip = summary
        .skipped
        .iter()
        .find(|s| s.id == id)
        .unwrap_or_else(|| panic!("skipped: {summary:?}"));
    assert!(skip.reason.contains("reserved"), "{}", skip.reason);
    assert!(compiled(pool, f.sr_profile, id).await.is_none());
}

/// Security fix I1, the compile half: a target who LATER gains a near-root
/// permission through an operator's hand-written policy is skipped.
#[tokio::test]
async fn a_target_who_gains_a_near_root_permission_is_skipped() {
    let Some(db) = test_db().await else { return };
    let f = seed(&db).await;
    let pool = &f.db.pool;
    let (_ua, ua) = person(pool, "ua").await;
    let (uc_id, uc) = person(pool, "uc").await;
    let id = create(
        pool,
        f.sr_engine,
        "starrocks.impersonate_user",
        &[],
        Some(uc_id),
        vec![p(PrincipalKind::User, &ua)],
    )
    .await;
    recompile_service(&f.perm, &f.policy, f.sr_profile)
        .await
        .expect("recompile");
    assert!(compiled(pool, f.sr_profile, id).await.is_some());

    sqlx::query(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb) VALUES ($1, $2, $3)",
    )
    .bind(f.sr_profile)
    .bind("uc may drop resource groups")
    .bind(serde_json::json!({
        "name": "uc may drop resource groups", "policyType": 0, "isEnabled": true,
        "resources": {"resource_group": {"values": ["rg1"]}},
        "policyItems": [{"users": [uc], "groups": [], "roles": [],
                         "accesses": [{"type": "drop", "isAllowed": true}]}],
    }))
    .execute(pool)
    .await
    .expect("hand-written policy");
    let summary = recompile_service(&f.perm, &f.policy, f.sr_profile)
        .await
        .expect("recompile");
    let skip = summary
        .skipped
        .iter()
        .find(|s| s.id == id)
        .unwrap_or_else(|| panic!("skipped: {summary:?}"));
    assert!(
        skip.reason.contains("uc may drop resource groups"),
        "{}",
        skip.reason
    );
    assert!(compiled(pool, f.sr_profile, id).await.is_none(), "reaped");
}
