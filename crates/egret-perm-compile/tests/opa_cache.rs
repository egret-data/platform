//! Governance SP2b Task 7: the per-profile OPA snapshot cache, its refresher,
//! and the stored-state checks every build runs (C4, C8).
//!   DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
//!   cargo test -p egret-perm-compile --features harness --test opa_cache
//!
//! Every test commits its arrangement (the refresher reads committed state in
//! its own transaction) under its own prefix. The test database is private to
//! the process, but the tests in it share it: a report lists every OPA
//! profile of the database, so each assertion looks at its own profile only.
include!("opa_support/fixtures_host.rs");
#[allow(dead_code)]
#[path = "golden/fixtures.rs"]
mod fixtures;

use std::sync::Arc;

use egret_metastore::Db;
use egret_perm::{Action, Effect, PermStore, Selector};
use egret_perm_compile::opa::cache::{Lookup, RefreshReport, SnapshotCache};
use egret_perm_compile::recompile_service;
use egret_perm_eval::{Decision, Resource, Snapshot};
use egret_policy::PolicyStore;
use fixtures::*;
use serde_json::json;
use uuid::Uuid;

static SEEDED: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
/// One test at a time: every cache refreshes every OPA profile of the shared
/// database, so a concurrent test's refresh would write the `compile_runs`
/// row of another test's profile.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn db() -> Db {
    let dsn = egret_test_db::url().await.expect("a test database");
    let db = Db::connect(&dsn, 5).await.expect("connect");
    SEEDED
        .get_or_init(|| async {
            db.migrate().await.expect("migrate");
            seed_service_defs(&db).await;
        })
        .await;
    db
}

fn table(catalog: &str, table: &str, columns: &[&str]) -> Resource {
    Resource::Table {
        catalog: catalog.to_string(),
        schema: "s".to_string(),
        table: table.to_string(),
        columns: columns.iter().map(|c| c.to_string()).collect(),
    }
}

fn is_allow(d: &Decision) -> bool {
    matches!(d, Decision::Allow { .. })
}

async fn exec(db: &Db, sql: &'static str, id: i64) {
    sqlx::query(sql)
        .bind(id)
        .execute(&db.pool)
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

async fn set_mechanism(db: &Db, profile: i64, mechanism: &str) {
    sqlx::query("UPDATE policy.authz_services SET mechanism = $2 WHERE id = $1")
        .bind(profile)
        .bind(mechanism)
        .execute(&db.pool)
        .await
        .expect("mechanism");
}

async fn bump(db: &Db, service: i64) {
    exec(
        db,
        "UPDATE policy.authz_services SET policy_version = policy_version + 1 WHERE id = $1",
        service,
    )
    .await;
}

async fn key_of(db: &Db, profile: i64) -> (i64, i64) {
    sqlx::query_as("SELECT policy_version, tag_version FROM policy.authz_services WHERE id = $1")
        .bind(profile)
        .fetch_one(&db.pool)
        .await
        .expect("key")
}

async fn catalog_name(db: &Db, id: Uuid) -> String {
    sqlx::query_scalar("SELECT name FROM registry.catalogs WHERE id = $1")
        .bind(id)
        .fetch_one(&db.pool)
        .await
        .expect("catalog name")
}

async fn compile_error(db: &Db, profile: i64) -> Option<String> {
    sqlx::query_scalar("SELECT error FROM perm.compile_runs WHERE service_id = $1")
        .bind(profile)
        .fetch_optional(&db.pool)
        .await
        .expect("compile run")
        .flatten()
}

/// When the profile's last compile run was written (a recorded failure moves it).
async fn attempted_at(db: &Db, profile: i64) -> String {
    sqlx::query_scalar("SELECT attempted_at::text FROM perm.compile_runs WHERE service_id = $1")
        .bind(profile)
        .fetch_one(&db.pool)
        .await
        .expect("a compile run")
}

async fn recompile(db: &Db, profile: i64) {
    recompile_service(
        &PermStore::new(db.pool.clone()),
        &PolicyStore::new(db.clone()),
        profile,
    )
    .await
    .expect("recompile");
}

async fn refresh(cache: &SnapshotCache, db: &Db) -> RefreshReport {
    cache.refresh_once(&db.pool).await.expect("refresh")
}

fn ready(cache: &SnapshotCache, id: i64) -> Arc<Snapshot> {
    match cache.get(id) {
        Lookup::Ready(s) => s,
        other => panic!("profile {id} is not Ready: {other:?}"),
    }
}

fn not_built_error(cache: &SnapshotCache, id: i64) -> String {
    match cache.get(id) {
        Lookup::NotBuilt { error } => error.expect("a NotBuilt entry names its reason"),
        other => panic!("profile {id} is not NotBuilt: {other:?}"),
    }
}

/// Which of the report's lists name `id` (the report covers every OPA
/// profile of the shared database).
fn listed(report: &RefreshReport, id: i64) -> Vec<&'static str> {
    let mut out = Vec::new();
    for (label, ids) in [
        ("rebuilt", &report.rebuilt),
        ("spliced", &report.spliced),
        ("failed", &report.failed),
        ("dropped", &report.dropped),
    ] {
        if ids.contains(&id) {
            out.push(label);
        }
    }
    out
}

/// A committed Trino profile with a tag service, catalog `lake` on an
/// engine bound to it (`spec`), and `ann` granted Read on the catalog.
struct Store {
    profile: i64,
    tag_service: i64,
    engine: Uuid,
    catalog: Uuid,
    lake: String,
    ann: String,
}

async fn store(db: &Db, prefix: &str, spec: serde_json::Value, names: &mut Names) -> Store {
    let (profile, tag_service, _) = fixtures::profile(db, prefix, "trino", true, names).await;
    let mut tx = db.pool.begin().await.expect("tx");
    let wh = warehouse(&mut tx, prefix, names).await;
    let engine = engine_bound_spec(
        &mut tx,
        &format!("{prefix}.engine"),
        profile,
        "trino",
        spec,
        names,
    )
    .await;
    let cat = catalog(&mut tx, &format!("{prefix}.lake"), wh, &[engine], names).await;
    let ann = format!("{prefix}_ann");
    user(&mut tx, &format!("{prefix}.ann"), Some(&ann), names).await;
    grant(
        &mut tx,
        &format!("{prefix}.read"),
        Effect::Allow,
        &[Action::Read],
        scope(cat, None, None, None),
        set(&[user_ref(&ann)], &[]),
        names,
    )
    .await;
    tx.commit().await.expect("commit the arranged state");
    Store {
        profile,
        tag_service: tag_service.expect("a tag service"),
        engine,
        catalog: cat,
        lake: catalog_name(db, cat).await,
        ann,
    }
}

fn open() -> serde_json::Value {
    json!({"trinoConfig": {"delegationMode": "open"}})
}

fn impersonate() -> serde_json::Value {
    json!({"trinoConfig": {"coordinator": {}, "delegationMode": "impersonate",
                           "auth": {"password": {}}}})
}

/// A committed store, switched to OPA and built once.
async fn ready_store(db: &Db, prefix: &str, cache: &SnapshotCache, names: &mut Names) -> Store {
    let st = store(db, prefix, open(), names).await;
    set_mechanism(db, st.profile, "opa").await;
    let report = refresh(cache, db).await;
    assert_eq!(listed(&report, st.profile), vec!["rebuilt"], "{report:?}");
    ready(cache, st.profile);
    st
}

#[tokio::test]
async fn a_ranger_profile_is_not_cached_and_an_opa_profile_is_built_once_per_version() {
    // Arrange
    let _serial = SERIAL.lock().await;
    let db = db().await;
    let mut names = Names::default();
    let st = store(&db, "oc1", open(), &mut names).await;
    let ranger = store(&db, "oc1r", open(), &mut names).await;
    set_mechanism(&db, st.profile, "opa").await;
    let cache = SnapshotCache::default();

    // Act
    let first = refresh(&cache, &db).await;
    let second = refresh(&cache, &db).await;

    // Assert
    assert_eq!(listed(&first, st.profile), vec!["rebuilt"], "{first:?}");
    assert!(listed(&second, st.profile).is_empty(), "{second:?}");
    let snap = ready(&cache, st.profile);
    assert_eq!(snap.key(), key_of(&db, st.profile).await);
    assert!(matches!(cache.get(ranger.profile), Lookup::NotOpa));
    assert!(listed(&first, ranger.profile).is_empty(), "{first:?}");

    // Arrange: a new grant, compiled (policy_version moves).
    let bob = "oc1_bob".to_string();
    let mut tx = db.pool.begin().await.expect("tx");
    user(&mut tx, "oc1.bob", Some(&bob), &mut names).await;
    grant(
        &mut tx,
        "oc1.read_bob",
        Effect::Allow,
        &[Action::Read],
        scope(st.catalog, None, None, None),
        set(&[user_ref(&bob)], &[]),
        &mut names,
    )
    .await;
    tx.commit().await.expect("commit");
    let before = key_of(&db, st.profile).await;
    recompile(&db, st.profile).await;
    assert_ne!(key_of(&db, st.profile).await, before, "the key moved");
    let d = snap.decide(&bob, "SelectFromColumns", &table(&st.lake, "t", &["id"]));
    assert!(!is_allow(&d), "the old snapshot does not know bob: {d:?}");

    // Act
    let third = refresh(&cache, &db).await;

    // Assert
    assert_eq!(listed(&third, st.profile), vec!["rebuilt"], "{third:?}");
    let snap = ready(&cache, st.profile);
    let d = snap.decide(&bob, "SelectFromColumns", &table(&st.lake, "t", &["id"]));
    assert!(is_allow(&d), "the rebuilt snapshot lets bob read: {d:?}");
    assert_eq!(snap.key(), key_of(&db, st.profile).await);
}

#[tokio::test]
async fn switching_back_to_ranger_drops_the_entry_once_no_engine_runs_opa() {
    // Arrange
    let _serial = SERIAL.lock().await;
    let db = db().await;
    let mut names = Names::default();
    let cache = SnapshotCache::default();
    let st = ready_store(&db, "oc2", &cache, &mut names).await;
    sqlx::query("UPDATE engine_mgmt.engines SET applied_mechanism = 'opa' WHERE id = $1")
        .bind(st.engine)
        .execute(&db.pool)
        .await
        .expect("applied");
    set_mechanism(&db, st.profile, "ranger").await;

    // Act
    let report = refresh(&cache, &db).await;

    // Assert: an engine still runs the OPA plugin.
    assert!(listed(&report, st.profile).is_empty(), "{report:?}");
    ready(&cache, st.profile);
    assert_eq!(cache.binding(st.engine), Some(st.profile));

    // Act: the engine is re-applied with Ranger.
    sqlx::query("UPDATE engine_mgmt.engines SET applied_mechanism = 'ranger' WHERE id = $1")
        .bind(st.engine)
        .execute(&db.pool)
        .await
        .expect("applied");
    let report = refresh(&cache, &db).await;

    // Assert
    assert_eq!(listed(&report, st.profile), vec!["dropped"], "{report:?}");
    assert!(matches!(cache.get(st.profile), Lookup::NotOpa));
    assert_eq!(cache.binding(st.engine), None);
}

#[tokio::test]
async fn bindings_follow_the_engine_rows_every_refresh() {
    // Arrange
    let _serial = SERIAL.lock().await;
    let db = db().await;
    let mut names = Names::default();
    let cache = SnapshotCache::default();
    let st = ready_store(&db, "oc3", &cache, &mut names).await;
    assert_eq!(cache.binding(st.engine), Some(st.profile));
    assert_eq!(cache.binding(Uuid::new_v4()), None);

    // Act
    sqlx::query("UPDATE engine_mgmt.engines SET authz_service_id = NULL WHERE id = $1")
        .bind(st.engine)
        .execute(&db.pool)
        .await
        .expect("unbind");
    refresh(&cache, &db).await;

    // Assert
    assert_eq!(cache.binding(st.engine), None);
    ready(&cache, st.profile);
}

#[tokio::test]
async fn a_failed_build_still_drops_a_disabled_admins_all_access() {
    // Arrange: an engine admin, a qualifying impersonate engine; built once.
    let _serial = SERIAL.lock().await;
    let db = db().await;
    let mut names = Names::default();
    let cache = SnapshotCache::default();
    let st = store(&db, "oc4", impersonate(), &mut names).await;
    let mut tx = db.pool.begin().await.expect("tx");
    let root = user(&mut tx, "oc4.root", Some("oc4_root"), &mut names).await;
    role_held(&mut tx, root, "platform_admin").await;
    tx.commit().await.expect("commit");
    set_mechanism(&db, st.profile, "opa").await;
    let report = refresh(&cache, &db).await;
    assert_eq!(listed(&report, st.profile), vec!["rebuilt"], "{report:?}");
    let create = table(&st.lake, "new_t", &[]);
    let snap = ready(&cache, st.profile);
    let d = snap.decide("oc4_root", "CreateTable", &create);
    assert!(is_allow(&d), "the engine admin holds all-access: {d:?}");
    let ann_reads = table(&st.lake, "t", &["id"]);
    let d = snap.decide(&st.ann, "SelectFromColumns", &ann_reads);
    assert!(is_allow(&d), "ann reads: {d:?}");

    // Break the data lowering: a grant naming an unusable user ref, written
    // by SQL past the authoring checks.
    let mut tx = db.pool.begin().await.expect("tx");
    grant(
        &mut tx,
        "oc4.bad",
        Effect::Allow,
        &[Action::Read],
        scope(st.catalog, None, None, None),
        set(&[user_ref("bad@ref")], &[]),
        &mut names,
    )
    .await;
    tx.commit().await.expect("commit");
    // The kill switch converges infrastructure without a compile.
    let before = key_of(&db, st.profile).await;
    egret_perm_compile::set_user_status_and_recompile(&db.pool, root, "disabled")
        .await
        .expect("the kill switch waits on no compile");
    assert_ne!(
        key_of(&db, st.profile).await,
        before,
        "the kill switch must move the version key"
    );

    // Act
    let report = refresh(&cache, &db).await;

    // Assert
    assert_eq!(listed(&report, st.profile), vec!["spliced"], "{report:?}");
    let snap = ready(&cache, st.profile);
    let d = snap.decide("oc4_root", "CreateTable", &create);
    assert!(!is_allow(&d), "a disabled admin keeps all-access: {d:?}");
    let d = snap.decide(&st.ann, "SelectFromColumns", &ann_reads);
    assert!(is_allow(&d), "the previous data is kept: {d:?}");
    let error = compile_error(&db, st.profile)
        .await
        .expect("a compile error");
    assert!(error.starts_with("OPA snapshot:"), "{error}");

    // Act: nothing moved.
    let again = refresh(&cache, &db).await;

    // Assert: the failing key is remembered.
    assert!(listed(&again, st.profile).is_empty(), "{again:?}");
}

#[tokio::test]
async fn a_hand_written_tag_policy_added_after_the_switch_makes_the_profile_deny() {
    // Arrange
    let _serial = SERIAL.lock().await;
    let db = db().await;
    let mut names = Names::default();
    let cache = SnapshotCache::default();
    let st = ready_store(&db, "oc5", &cache, &mut names).await;
    sqlx::query(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, managed_by) \
         VALUES ($1, 'oc5 hand', $2, 'operator')",
    )
    .bind(st.tag_service)
    .bind(
        json!({"name": "oc5 hand", "policyType": 0, "isEnabled": true,
                 "resources": {"tag": {"values": ["oc5_tag"]}},
                 "policyItems": [], "denyPolicyItems": []}),
    )
    .execute(&db.pool)
    .await
    .expect("a hand-written tag policy");
    // Migration 0182's trigger carries the tag service's bump to the profile.
    bump(&db, st.tag_service).await;

    // Act
    let report = refresh(&cache, &db).await;

    // Assert
    assert_eq!(listed(&report, st.profile), vec!["failed"], "{report:?}");
    let error = not_built_error(&cache, st.profile);
    assert!(error.contains("hand-written"), "{error}");
    assert!(error.contains("gold_oc5_trino_tag holds 1"), "{error}");
    let recorded = compile_error(&db, st.profile).await.expect("recorded");
    assert_eq!(recorded, format!("OPA snapshot: {error}"));
    let attempted = attempted_at(&db, st.profile).await;

    // Act: nothing moved.
    let again = refresh(&cache, &db).await;

    // Assert: the refused key is remembered; nothing is rebuilt or recorded again.
    assert!(listed(&again, st.profile).is_empty(), "{again:?}");
    assert_eq!(attempted_at(&db, st.profile).await, attempted);
    not_built_error(&cache, st.profile);
}

#[tokio::test]
async fn a_hand_written_profile_policy_makes_the_profile_deny() {
    // Arrange
    let _serial = SERIAL.lock().await;
    let db = db().await;
    let mut names = Names::default();
    let cache = SnapshotCache::default();
    let st = ready_store(&db, "oc7", &cache, &mut names).await;
    sqlx::query(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, managed_by) \
         VALUES ($1, 'oc7 hand', $2, 'operator')",
    )
    .bind(st.profile)
    .bind(
        json!({"name": "oc7 hand", "policyType": 0, "isEnabled": true,
                 "resources": {"catalog": {"values": ["x"]}},
                 "policyItems": [], "denyPolicyItems": []}),
    )
    .execute(&db.pool)
    .await
    .expect("a hand-written policy");
    bump(&db, st.profile).await;

    // Act
    let report = refresh(&cache, &db).await;

    // Assert
    assert_eq!(listed(&report, st.profile), vec!["failed"], "{report:?}");
    let error = not_built_error(&cache, st.profile);
    assert_eq!(
        error,
        "gold_oc7_trino holds 1 hand-written policies, which OPA would ignore; delete them first"
    );
}

#[tokio::test]
async fn an_all_defaults_row_or_a_stored_overlap_makes_the_profile_deny() {
    let _serial = SERIAL.lock().await;
    let db = db().await;
    let mut names = Names::default();
    let cache = SnapshotCache::default();

    // Case 1: an `all - …` default row.
    let st = ready_store(&db, "oc6a", &cache, &mut names).await;
    sqlx::query(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, managed_by) \
         VALUES ($1, 'all - catalog', $2, 'infra')",
    )
    .bind(st.profile)
    .bind(
        json!({"name": "all - catalog", "policyType": 0, "isEnabled": true,
                 "resources": {"catalog": {"values": ["*"]}},
                 "policyItems": [], "denyPolicyItems": []}),
    )
    .execute(&db.pool)
    .await
    .expect("an all-defaults row");
    bump(&db, st.profile).await;
    let report = refresh(&cache, &db).await;
    assert_eq!(listed(&report, st.profile), vec!["failed"], "{report:?}");
    assert_eq!(
        not_built_error(&cache, st.profile),
        "the profile holds the `all - …` default policies (1); delete them first"
    );

    // Case 2: a stored pair of differing masks on one column.
    let st = store(&db, "oc6b", open(), &mut names).await;
    let mut tx = db.pool.begin().await.expect("tx");
    rule(
        &mut tx,
        "oc6b.mask_a",
        mask("MASK", None),
        Selector::Path(scope(st.catalog, Some("s"), Some("t"), Some("email"))),
        set(&[], &[]),
        &mut names,
    )
    .await;
    let b = rule(
        &mut tx,
        "oc6b.mask_b",
        mask("MASK_NULL", None),
        Selector::Path(scope(st.catalog, Some("s"), Some("t"), Some("phone"))),
        set(&[], &[]),
        &mut names,
    )
    .await;
    tx.commit().await.expect("commit");
    recompile(&db, st.profile).await;
    set_mechanism(&db, st.profile, "opa").await;
    let report = refresh(&cache, &db).await;
    assert_eq!(listed(&report, st.profile), vec!["rebuilt"], "{report:?}");
    // Move B onto `email`, model row and compiled body together: the state
    // an older compile, or a `psql` write, leaves behind.
    sqlx::query("UPDATE perm.rules SET column_name = 'email' WHERE id = $1")
        .bind(b)
        .execute(&db.pool)
        .await
        .expect("move the rule");
    let moved = sqlx::query(
        "UPDATE policy.authz_policies \
            SET policy_jsonb = jsonb_set(policy_jsonb, '{resources,column,values}', \
                                         to_jsonb(ARRAY['email'::text])) \
          WHERE name LIKE $1",
    )
    .bind(format!("perm-rule-{b}%"))
    .execute(&db.pool)
    .await
    .expect("move the compiled body")
    .rows_affected();
    assert!(moved >= 1, "the rule's compiled policy was moved");
    bump(&db, st.profile).await;

    let report = refresh(&cache, &db).await;
    assert_eq!(listed(&report, st.profile), vec!["failed"], "{report:?}");
    let error = not_built_error(&cache, st.profile);
    assert_eq!(
        error,
        "the profile serves 1 mask/row-filter pairs whose winner the artifact does not express \
         (see GET /perm/overlaps)"
    );
}

/// The switch to OPA (`egret_policy::opa_switch_check`, over the production
/// `PermRecompiler`) of `profile`, its one provisioned engine on Trino 483.
async fn switch_refusals(db: &Db, profile: i64, engine: Uuid) -> Vec<egret_policy::SwitchRefusal> {
    let svc = PolicyStore::new(db.clone())
        .get_service_by_id(profile)
        .await
        .expect("the profile");
    let bindings = [egret_policy::EngineBinding {
        id: engine,
        name: "engine".into(),
        status: "running".into(),
        origin: "provisioned".into(),
        r#type: "trino".into(),
        authz_service_id: Some(profile),
        warehouse_ids: Vec::new(),
        image: Some("trinodb/trino:483".into()),
    }];
    let mut tx = db.pool.begin().await.expect("tx");
    let check = egret_policy::opa_switch_check(
        &mut tx,
        true,
        &svc,
        &bindings,
        &egret_perm_compile::PermRecompiler,
    )
    .await
    .expect("the switch check reads");
    tx.rollback().await.expect("rollback");
    check.refusals
}

/// Task 11, carry 2: the switch and the refresher judge stored state with
/// the same predicates. A state the switch allows builds Ready; a state it
/// refuses (a hand-written row, a stored overlap pair) is NotBuilt with the
/// same text, and undoing it clears both.
#[tokio::test]
async fn the_switch_and_the_refresher_agree() {
    let _serial = SERIAL.lock().await;
    let db = db().await;
    let mut names = Names::default();
    let cache = SnapshotCache::default();

    // Allowed: a generated Trino profile with nothing stored outside the model.
    let st = store(&db, "oc11", open(), &mut names).await;
    recompile(&db, st.profile).await;
    assert_eq!(switch_refusals(&db, st.profile, st.engine).await, vec![]);
    set_mechanism(&db, st.profile, "opa").await;
    let report = refresh(&cache, &db).await;
    assert_eq!(listed(&report, st.profile), vec!["rebuilt"], "{report:?}");
    ready(&cache, st.profile);

    // Refused: a hand-written row in the profile.
    let hand: i64 = sqlx::query_scalar(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, managed_by) \
         VALUES ($1, 'oc11 hand', $2, 'operator') RETURNING id",
    )
    .bind(st.profile)
    .bind(
        json!({"name": "oc11 hand", "policyType": 0, "isEnabled": true,
                 "resources": {"catalog": {"values": ["x"]}},
                 "policyItems": [], "denyPolicyItems": []}),
    )
    .fetch_one(&db.pool)
    .await
    .expect("a hand-written policy");
    bump(&db, st.profile).await;
    let refused = switch_refusals(&db, st.profile, st.engine).await;
    assert!(
        matches!(
            refused.as_slice(),
            [egret_policy::SwitchRefusal::Stored(
                egret_policy::opa_guard::StoredStateRefusal::HandWritten { count: 1, .. }
            )]
        ),
        "{refused:?}"
    );
    let report = refresh(&cache, &db).await;
    assert_eq!(listed(&report, st.profile), vec!["failed"], "{report:?}");
    assert_eq!(not_built_error(&cache, st.profile), refused[0].message());

    // Undone: both allow again.
    exec(&db, "DELETE FROM policy.authz_policies WHERE id = $1", hand).await;
    bump(&db, st.profile).await;
    assert_eq!(switch_refusals(&db, st.profile, st.engine).await, vec![]);
    let report = refresh(&cache, &db).await;
    assert_eq!(listed(&report, st.profile), vec!["rebuilt"], "{report:?}");
    ready(&cache, st.profile);

    // Refused: a stored pair of differing masks on one column (the state an
    // older compile, or a `psql` write, leaves behind).
    let mut tx = db.pool.begin().await.expect("tx");
    rule(
        &mut tx,
        "oc11.mask_a",
        mask("MASK", None),
        Selector::Path(scope(st.catalog, Some("s"), Some("t"), Some("email"))),
        set(&[], &[]),
        &mut names,
    )
    .await;
    let b = rule(
        &mut tx,
        "oc11.mask_b",
        mask("MASK_NULL", None),
        Selector::Path(scope(st.catalog, Some("s"), Some("t"), Some("phone"))),
        set(&[], &[]),
        &mut names,
    )
    .await;
    tx.commit().await.expect("commit");
    recompile(&db, st.profile).await;
    assert_eq!(switch_refusals(&db, st.profile, st.engine).await, vec![]);
    sqlx::query("UPDATE perm.rules SET column_name = 'email' WHERE id = $1")
        .bind(b)
        .execute(&db.pool)
        .await
        .expect("move the rule");
    let moved = sqlx::query(
        "UPDATE policy.authz_policies \
            SET policy_jsonb = jsonb_set(policy_jsonb, '{resources,column,values}', \
                                         to_jsonb(ARRAY['email'::text])) \
          WHERE name LIKE $1",
    )
    .bind(format!("perm-rule-{b}%"))
    .execute(&db.pool)
    .await
    .expect("move the compiled body")
    .rows_affected();
    assert!(moved >= 1, "the rule's compiled policy was moved");
    bump(&db, st.profile).await;

    let refused = switch_refusals(&db, st.profile, st.engine).await;
    let [egret_policy::SwitchRefusal::StoredOverlaps { pairs }] = refused.as_slice() else {
        panic!("one StoredOverlaps refusal: {refused:?}");
    };
    assert_eq!(pairs.len(), 1, "{pairs:?}");
    // `{kind} on {coordinate}: {rule names}`, the coordinate and names as
    // `GET /perm/overlaps` shows them; the pair's order is the policy names'.
    let prefix = format!(
        "mask on catalog=[{}] column=[email] schema=[s] table=[t]: ",
        st.lake
    );
    let rules = pairs[0]
        .strip_prefix(&prefix)
        .unwrap_or_else(|| panic!("{pairs:?}"));
    let mut rules: Vec<&str> = rules.split(", ").collect();
    rules.sort();
    assert_eq!(rules, vec!["oc11.mask_a", "oc11.mask_b"], "{pairs:?}");
    let report = refresh(&cache, &db).await;
    assert_eq!(listed(&report, st.profile), vec!["failed"], "{report:?}");
    assert_eq!(
        not_built_error(&cache, st.profile),
        "the profile serves 1 mask/row-filter pairs whose winner the artifact does not express \
         (see GET /perm/overlaps)"
    );
}

/// Task 18 fix round 1 (I1): the snapshot the refresher serves (built by
/// `cache.rs`'s `outcome_from_parts`, not `build_snapshot_via`) names the
/// reserved lineage principal, so a `public` write grant does not let
/// egret-lineage write, while it lets everyone else.
#[tokio::test]
async fn the_served_snapshot_refuses_the_lineage_principal_a_public_write() {
    // Arrange
    let _serial = SERIAL.lock().await;
    let db = db().await;
    let mut names = Names::default();
    let st = store(&db, "oc12", open(), &mut names).await;
    let mut tx = db.pool.begin().await.expect("tx");
    grant(
        &mut tx,
        "oc12.public_write",
        Effect::Allow,
        &[Action::Write],
        scope(st.catalog, None, None, None),
        set(&[group_ref("public")], &[]),
        &mut names,
    )
    .await;
    tx.commit().await.expect("commit");
    recompile(&db, st.profile).await;
    set_mechanism(&db, st.profile, "opa").await;
    let cache = SnapshotCache::default();

    // Act
    let report = refresh(&cache, &db).await;

    // Assert
    assert_eq!(listed(&report, st.profile), vec!["rebuilt"], "{report:?}");
    let snap = ready(&cache, st.profile);
    let lineage = egret_core::engine_principals::LINEAGE_PRINCIPAL;
    assert_eq!(snap.lineage_principal(), lineage);
    let t = table(&st.lake, "t", &[]);
    for op in ["TruncateTable", "InsertIntoTable"] {
        let d = snap.decide(lineage, op, &t);
        assert!(
            matches!(&d, Decision::Deny { explicit: true, reason } if reason.contains("lineage principal")),
            "{op}: {d:?}"
        );
        let d = snap.decide(&st.ann, op, &t);
        assert!(is_allow(&d), "{op} by ann through the public grant: {d:?}");
    }
}
