//! SP3c-2 Task 6: authoring a clearance (`ClassificationPolicyRepo::set_policy` /
//! `clear_policy`) keeps its `perm.rules` withhold rule in step. Lives here, not
//! in `egret-perm-compile`, because it drives the real authoring path and
//! `egret-bi` already depends on `egret-perm-compile`.
//!
//! DB-gated; needs DATABASE_URL (see `egret_test_db`).

use egret_bi::ClassificationPolicyRepo;
use egret_events::EventBus;
use egret_metastore::Db;
use egret_perm::PermStore;
use egret_perm_compile::recompile_service;
use egret_policy::{PolicyStore, COMPILER_CLEARANCE_PREFIX};
use sqlx::PgPool;
use uuid::Uuid;

/// Authoring recompiles every compiler-targeted profile and clearances are
/// installation-wide, so every test here runs ONE AT A TIME, each from a clean
/// slate of its own clearance state ([`reset`]). Assertions are keyed to the
/// test's OWN tag.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn pool() -> Option<PgPool> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db.pool)
}

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

/// Clear this binary's clearance state so a test starts from a known slate:
/// every gate and every `clearance:*` withhold rule. The database is private to
/// this process, and the `clearance:%` name scope leaves the CHECK-only tests'
/// `withhold-*` rules untouched. Provenance rows are left alone — their id is a
/// serial so a stale one never collides, and deleting them would race the
/// non-SERIAL `the_clearance_migration_table_records_a_run`. Call under
/// [`SERIAL`].
async fn reset(pool: &PgPool) {
    sqlx::query("DELETE FROM classification_policies")
        .execute(pool)
        .await
        .expect("reset gates");
    sqlx::query(
        "DELETE FROM perm.rule_exemptions WHERE rule_id IN \
           (SELECT id FROM perm.rules WHERE kind = 'withhold' AND name LIKE 'clearance:%')",
    )
    .execute(pool)
    .await
    .expect("reset exemptions");
    sqlx::query("DELETE FROM perm.rules WHERE kind = 'withhold' AND name LIKE 'clearance:%'")
        .execute(pool)
        .await
        .expect("reset rules");
}

struct RoleClearance {
    perm: PermStore,
    policy: PolicyStore,
    profile_id: i64,
    tag_service_id: i64,
    tag_id: Uuid,
    tag_key: String,
    role_name: String,
}

/// A Trino profile with a bound engine (so it is compiler-targeted), a tag, and
/// a clearance gating that tag on a fresh role with one holder.
async fn seed_role_clearance(db: &Db) -> RoleClearance {
    let policy = PolicyStore::new(db.clone());
    let perm = PermStore::new(db.pool.clone());
    egret_policy::seed_trino(&policy).await.expect("seed trino");
    egret_policy::seed_tag(&policy).await.expect("seed tag");

    let profile_name = unique("clrmig");
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
    sqlx::query(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, authz_service_id) \
         VALUES ($1,'trino','egret','{}'::jsonb,$2)",
    )
    .bind(unique("clrmig-eng"))
    .bind(profile.id)
    .execute(&db.pool)
    .await
    .expect("bind engine");

    let tag_key = unique("clrmig-tag");
    let tag_name = unique("ClrMigTag");
    let tag_id: Uuid =
        sqlx::query_scalar("INSERT INTO tags (key, name) VALUES ($1, $2) RETURNING id")
            .bind(&tag_key)
            .bind(&tag_name)
            .fetch_one(&db.pool)
            .await
            .expect("insert tag");

    let role_name = unique("clrmig-role");
    let role_id: Uuid =
        sqlx::query_scalar("INSERT INTO roles (scope, name) VALUES ('platform', $1) RETURNING id")
            .bind(&role_name)
            .fetch_one(&db.pool)
            .await
            .expect("insert role");
    let holder: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) VALUES ('local',$1,'Carol',$2) \
         RETURNING id",
    )
    .bind(format!("{}@egret.local", unique("carol")))
    .bind(unique("carol"))
    .fetch_one(&db.pool)
    .await
    .expect("insert holder");
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'platform', NULL)",
    )
    .bind(holder)
    .bind(role_id)
    .execute(&db.pool)
    .await
    .expect("role grant");

    sqlx::query(
        "INSERT INTO classification_policies (tag_id, required_roles, required_groups) \
         VALUES ($1, $2, '{}') \
         ON CONFLICT (tag_id) DO UPDATE SET required_roles = EXCLUDED.required_roles",
    )
    .bind(tag_id)
    .bind(vec![role_name.clone()])
    .execute(&db.pool)
    .await
    .expect("gate");

    RoleClearance {
        perm,
        policy,
        profile_id: profile.id,
        tag_service_id: tag_service.id,
        tag_id,
        tag_key,
        role_name,
    }
}

/// Whether a clearance deny for `tag_id` is compiled into the TAG service — a
/// Trino clearance is installation-wide and lands in the tag service, not the
/// resource profile (`a tag policy filed under the profile is served to nobody`,
/// SP4a Task 10 T3). This is the enforcement the read switch must preserve.
async fn clearance_enforced(pool: &PgPool, tag_service_id: i64, tag_id: Uuid) -> bool {
    let name = format!("{COMPILER_CLEARANCE_PREFIX}{tag_id}");
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
    )
    .bind(tag_service_id)
    .bind(&name)
    .fetch_one(pool)
    .await
    .expect("count clearance policy")
        > 0
}

/// The withhold rule id for a tag key, and its `role:<name> holders` exemptions.
async fn withhold_rule(pool: &PgPool, tag_key: &str) -> Option<(Uuid, Vec<String>)> {
    let rule_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM perm.rules WHERE kind = 'withhold' AND selector_kind = 'tag' AND tag_key = $1",
    )
    .bind(tag_key)
    .fetch_optional(pool)
    .await
    .expect("read rule");
    let rule_id = rule_id?;
    let refs: Vec<String> = sqlx::query_scalar(
        "SELECT ref FROM perm.rule_exemptions WHERE rule_id = $1 \
           AND kind = 'group'::perm.principal_kind AND mode = 'include'::perm.principal_mode \
         ORDER BY ref",
    )
    .bind(rule_id)
    .fetch_all(pool)
    .await
    .expect("read exemptions");
    Some((rule_id, refs))
}

// ── SP3c-2 Task 6: authoring keeps the withhold rule in step ───────────────

async fn test_actor(pool: &PgPool) -> egret_core::Principal {
    let id: Uuid =
        sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
            .bind(format!("actor_{}@egret.local", Uuid::new_v4().simple()))
            .fetch_one(pool)
            .await
            .expect("insert actor");
    egret_core::Principal {
        user_id: egret_core::UserId(id),
        email: "actor@egret.local".into(),
        platform_roles: vec![],
        permissions: vec![],
        group_ids: vec![],
        impersonator: None,
    }
}

/// A second role with a holder-less definition, to edit a clearance onto.
async fn extra_role(pool: &PgPool) -> String {
    let name = unique("clrmig-role2");
    sqlx::query("INSERT INTO roles (scope, name) VALUES ('platform', $1)")
        .bind(&name)
        .execute(pool)
        .await
        .expect("insert role");
    name
}

/// Author a clearance through the real write path on a fixture whose seeded
/// table row is first removed, so the rule is born of the authoring call alone.
async fn authoring_fixture() -> Option<(
    PgPool,
    Db,
    RoleClearance,
    ClassificationPolicyRepo,
    egret_core::Principal,
)> {
    let pool = pool().await?;
    reset(&pool).await;
    let db = Db::connect(&egret_test_db::url().await.unwrap(), 5)
        .await
        .unwrap();
    let f = seed_role_clearance(&db).await;
    sqlx::query("DELETE FROM classification_policies WHERE tag_id = $1")
        .bind(f.tag_id)
        .execute(&pool)
        .await
        .unwrap();
    let repo = ClassificationPolicyRepo::new(db.clone(), EventBus::new());
    let actor = test_actor(&pool).await;
    Some((pool, db, f, repo, actor))
}

#[tokio::test]
async fn authoring_a_clearance_creates_its_withhold_rule_and_enforces_from_it() {
    let _serial = SERIAL.lock().await;
    let Some((pool, _db, f, repo, actor)) = authoring_fixture().await else {
        return;
    };
    assert!(withhold_rule(&pool, &f.tag_key).await.is_none());

    repo.set_policy(f.tag_id, std::slice::from_ref(&f.role_name), &[], &actor)
        .await
        .expect("author clearance");

    let (_, refs) = withhold_rule(&pool, &f.tag_key)
        .await
        .expect("authoring created the withhold rule");
    assert_eq!(refs, vec![format!("role:{} holders", f.role_name)]);
    assert!(clearance_enforced(&pool, f.tag_service_id, f.tag_id).await);

    // Enforced FROM THE RULE: drop the table row behind the rule's back and a
    // recompile still produces the deny.
    sqlx::query("DELETE FROM classification_policies WHERE tag_id = $1")
        .bind(f.tag_id)
        .execute(&pool)
        .await
        .unwrap();
    recompile_service(&f.perm, &f.policy, f.profile_id)
        .await
        .expect("recompile");
    assert!(
        clearance_enforced(&pool, f.tag_service_id, f.tag_id).await,
        "enforced from the rule, not the fallback"
    );
}

#[tokio::test]
async fn editing_a_clearance_replaces_the_rules_exemptions() {
    let _serial = SERIAL.lock().await;
    let Some((pool, _db, f, repo, actor)) = authoring_fixture().await else {
        return;
    };
    let other = extra_role(&pool).await;

    repo.set_policy(f.tag_id, std::slice::from_ref(&f.role_name), &[], &actor)
        .await
        .expect("author");
    repo.set_policy(f.tag_id, std::slice::from_ref(&other), &[], &actor)
        .await
        .expect("edit");

    let (_, refs) = withhold_rule(&pool, &f.tag_key).await.expect("rule");
    assert_eq!(
        refs,
        vec![format!("role:{other} holders")],
        "the edit replaced the exemptions rather than appending"
    );
    assert!(clearance_enforced(&pool, f.tag_service_id, f.tag_id).await);
}

#[tokio::test]
async fn clearing_a_clearance_removes_its_rule_and_enforcement() {
    let _serial = SERIAL.lock().await;
    let Some((pool, _db, f, repo, actor)) = authoring_fixture().await else {
        return;
    };

    repo.set_policy(f.tag_id, std::slice::from_ref(&f.role_name), &[], &actor)
        .await
        .expect("author");
    assert!(withhold_rule(&pool, &f.tag_key).await.is_some());

    repo.clear_policy(f.tag_id, &actor).await.expect("clear");

    assert!(withhold_rule(&pool, &f.tag_key).await.is_none());
    assert!(!clearance_enforced(&pool, f.tag_service_id, f.tag_id).await);

    // Authoring an empty clearance is also "no gate": it removes the rule.
    repo.set_policy(f.tag_id, std::slice::from_ref(&f.role_name), &[], &actor)
        .await
        .expect("re-author");
    repo.set_policy(f.tag_id, &[], &[], &actor)
        .await
        .expect("empty");
    assert!(withhold_rule(&pool, &f.tag_key).await.is_none());
}
