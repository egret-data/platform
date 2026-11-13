//! SP3c-2: clearance migrates into `perm.rules` rows of the `withhold` kind.
//!
//! DB-gated; needs DATABASE_URL (see `egret_test_db`).

use egret_metastore::Db;
use egret_perm::PermStore;
use egret_perm_compile::clearance_migrate::{migrate_clearances_in_tx, undo_clearances_in_tx};
use egret_perm_compile::{recompile_service, PermHttpError};
use egret_policy::PolicyStore;
use sqlx::PgPool;
use uuid::Uuid;

/// The migrate/undo operate across services and recompile every
/// compiler-targeted profile, and clearances are installation-wide. So every
/// test that calls them runs ONE AT A TIME, and each starts from a clean slate
/// of its own clearance state ([`reset`]); the per-process test database makes
/// that safe. Assertions are keyed to the test's OWN tag, never a global count.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn pool() -> Option<PgPool> {
    let dsn = egret_test_db::url().await?;
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db.pool)
}

async fn seed_tag(pool: &PgPool) -> String {
    let key = format!("wh_{}", &Uuid::new_v4().simple().to_string()[..12]);
    sqlx::query("INSERT INTO tags (key, name) VALUES ($1, $1)")
        .bind(&key)
        .execute(pool)
        .await
        .expect("seed tag");
    key
}

async fn insert_withhold(
    pool: &PgPool,
    tag_key: &str,
    mask_type: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO perm.rules (name, kind, mask_type, selector_kind, tag_key) \
         VALUES ($1, 'withhold', $2, 'tag', $3)",
    )
    .bind(format!("withhold-{tag_key}"))
    .bind(mask_type)
    .bind(tag_key)
    .execute(pool)
    .await
    .map(|_| ())
}

#[tokio::test]
async fn a_withhold_rule_with_no_payload_is_accepted() {
    let Some(pool) = pool().await else { return };
    let tag = seed_tag(&pool).await;
    insert_withhold(&pool, &tag, None)
        .await
        .expect("a payload-free withhold rule is valid");
}

/// SQLSTATE 23514 = check_violation.
fn is_check_violation(e: &sqlx::Error) -> bool {
    e.as_database_error()
        .and_then(|d| d.code())
        .is_some_and(|c| c == "23514")
}

#[tokio::test]
async fn a_withhold_rule_with_a_payload_is_rejected() {
    let Some(pool) = pool().await else { return };
    let tag = seed_tag(&pool).await;
    let err = insert_withhold(&pool, &tag, Some("MASK_NULL"))
        .await
        .expect_err("a withhold rule with a mask_type must be refused");
    // The 0154 per-kind CHECKs and `rules_withhold_has_no_payload` overlap on
    // payloads; either refusal is the contract.
    assert!(
        is_check_violation(&err),
        "expected a CHECK violation: {err}"
    );
}

#[tokio::test]
async fn a_withhold_rule_must_be_tag_selected() {
    let Some(pool) = pool().await else { return };
    let catalog_id: Uuid = sqlx::query_scalar(
        "INSERT INTO registry.catalogs (name, kind, config) \
         VALUES ($1, 'iceberg', '{}'::jsonb) RETURNING id",
    )
    .bind(format!(
        "wh_cat_{}",
        &Uuid::new_v4().simple().to_string()[..12]
    ))
    .fetch_one(&pool)
    .await
    .expect("seed catalog");
    let err = sqlx::query(
        "INSERT INTO perm.rules (name, kind, selector_kind, catalog_id) \
         VALUES ($1, 'withhold', 'path', $2)",
    )
    .bind(format!("withhold-path-{}", Uuid::new_v4()))
    .bind(catalog_id)
    .execute(&pool)
    .await
    .expect_err("a path-selected withhold rule must be refused");
    assert!(
        err.to_string().contains("rules_withhold_has_no_payload"),
        "expected the withhold CHECK to fire: {err}"
    );
}

#[tokio::test]
async fn the_clearance_migration_table_records_a_run() {
    let Some(pool) = pool().await else { return };
    let ids = vec![Uuid::new_v4(), Uuid::new_v4()];
    let tags = vec!["pii".to_string(), "secret".to_string()];
    let hashes = vec!["h1".to_string(), "h2".to_string()];
    let rules = serde_json::json!([{"tag": "pii"}]);
    let original = serde_json::json!([{"tag": "pii", "min_clearance": 2}]);
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO perm.clearance_migration \
         (tags, rules, rule_ids, exemption_hashes, original, migrated_by) \
         VALUES ($1, $2, $3, $4, $5, 'test') RETURNING id",
    )
    .bind(&tags)
    .bind(&rules)
    .bind(&ids)
    .bind(&hashes)
    .bind(&original)
    .fetch_one(&pool)
    .await
    .expect("insert");
    let row: (
        Vec<String>,
        serde_json::Value,
        Vec<Uuid>,
        Vec<String>,
        serde_json::Value,
        String,
    ) = sqlx::query_as(
        "SELECT tags, rules, rule_ids, exemption_hashes, original, migrated_by \
             FROM perm.clearance_migration WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .expect("read");
    assert_eq!(
        row,
        (tags, rules, ids, hashes, original, "test".to_string())
    );
    sqlx::query("DELETE FROM perm.clearance_migration WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
}

// ── SP3c-2 Task 4: the migration and its undo ──────────────────────────────

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
    policy
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
        tag_key,
        role_name,
    }
}

/// A fingerprint of a profile's compiled bodies, as the migration gates on.
async fn profile_fingerprint(pool: &PgPool, service_id: i64) -> String {
    let rows: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT name, policy_jsonb FROM policy.authz_policies WHERE service_id = $1 ORDER BY name",
    )
    .bind(service_id)
    .fetch_all(pool)
    .await
    .expect("read compiled");
    serde_json::to_string(&rows).expect("serialize")
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

#[tokio::test]
async fn migrating_a_role_clearance_is_byte_identical() {
    let Some(pool) = pool().await else { return };
    let _serial = SERIAL.lock().await;
    reset(&pool).await;
    let db = Db::connect(&egret_test_db::url().await.unwrap(), 5)
        .await
        .unwrap();
    let f = seed_role_clearance(&db).await;

    // Compile the clearance deny first, then capture the baseline.
    recompile_service(&f.perm, &f.policy, f.profile_id)
        .await
        .expect("baseline recompile");
    let before = profile_fingerprint(&pool, f.profile_id).await;
    assert!(
        withhold_rule(&pool, &f.tag_key).await.is_none(),
        "no withhold rule exists before the migration"
    );

    let mut tx = pool.begin().await.unwrap();
    let out = migrate_clearances_in_tx(&mut tx, "test")
        .await
        .expect("migrate");
    tx.commit().await.unwrap();

    // (a) the withhold rule and its `role:<name> holders` exemption now exist.
    let (_rule_id, refs) = withhold_rule(&pool, &f.tag_key)
        .await
        .expect("a withhold rule for the gated tag");
    assert_eq!(refs, vec![format!("role:{} holders", f.role_name)]);
    assert!(out.tags.contains(&f.tag_key));
    assert!(out.rules_created >= 1);

    // (b) the compiled output is byte-identical to before.
    let after = profile_fingerprint(&pool, f.profile_id).await;
    assert_eq!(after, before, "the role clearance compile must not change");
}

#[tokio::test]
async fn undo_restores_classification_policies_and_removes_the_rule() {
    let Some(pool) = pool().await else { return };
    let _serial = SERIAL.lock().await;
    reset(&pool).await;
    let db = Db::connect(&egret_test_db::url().await.unwrap(), 5)
        .await
        .unwrap();
    let f = seed_role_clearance(&db).await;
    recompile_service(&f.perm, &f.policy, f.profile_id)
        .await
        .expect("baseline recompile");

    let mut tx = pool.begin().await.unwrap();
    let out = migrate_clearances_in_tx(&mut tx, "test")
        .await
        .expect("migrate");
    tx.commit().await.unwrap();
    assert!(withhold_rule(&pool, &f.tag_key).await.is_some());

    let mut tx = pool.begin().await.unwrap();
    undo_clearances_in_tx(&mut tx, out.id).await.expect("undo");
    tx.commit().await.unwrap();

    // The withhold rule is gone and the gate is still there.
    assert!(
        withhold_rule(&pool, &f.tag_key).await.is_none(),
        "the withhold rule is removed by the undo"
    );
    let still_gated: Vec<String> = sqlx::query_scalar(
        "SELECT unnest(required_roles) FROM classification_policies cp \
           JOIN tags t ON t.id = cp.tag_id WHERE t.key = $1",
    )
    .bind(&f.tag_key)
    .fetch_all(&pool)
    .await
    .expect("read gate");
    assert_eq!(still_gated, vec![f.role_name.clone()]);
    // The provenance row is gone.
    let provenance: Option<i64> =
        sqlx::query_scalar("SELECT id FROM perm.clearance_migration WHERE id = $1")
            .bind(out.id)
            .fetch_optional(&pool)
            .await
            .expect("read provenance");
    assert!(provenance.is_none(), "provenance deleted by the undo");
}

#[tokio::test]
async fn undo_409s_when_a_rule_changed() {
    let Some(pool) = pool().await else { return };
    let _serial = SERIAL.lock().await;
    reset(&pool).await;
    let db = Db::connect(&egret_test_db::url().await.unwrap(), 5)
        .await
        .unwrap();
    let f = seed_role_clearance(&db).await;
    recompile_service(&f.perm, &f.policy, f.profile_id)
        .await
        .expect("baseline recompile");

    let mut tx = pool.begin().await.unwrap();
    let out = migrate_clearances_in_tx(&mut tx, "test")
        .await
        .expect("migrate");
    tx.commit().await.unwrap();

    // Tamper: change the created rule's exemption ref under the migration.
    let (rule_id, _) = withhold_rule(&pool, &f.tag_key).await.expect("rule");
    sqlx::query("UPDATE perm.rule_exemptions SET ref = $2 WHERE rule_id = $1")
        .bind(rule_id)
        .bind("role:tampered holders")
        .execute(&pool)
        .await
        .expect("tamper");

    let mut tx = pool.begin().await.unwrap();
    let err = undo_clearances_in_tx(&mut tx, out.id)
        .await
        .expect_err("undo must refuse a tampered rule");
    drop(tx);
    assert!(
        matches!(err, PermHttpError::Conflict(_)),
        "expected a 409 Conflict, got {err:?}"
    );
}
