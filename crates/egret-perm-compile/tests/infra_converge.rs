//! Governance SP2a (D4): a profile's IR-owned infrastructure policies (the
//! `egret-admin - …` set, the four fixed names and, on StarRocks, the query-log
//! loader and guard) are derived from the store
//! and reconciled by name: updated in place (id kept), inserted when missing,
//! deleted when extra, with `policy_version` bumped once only when something
//! changed. The `all - …` default set is never touched.
//!
//! Its own binary: the engine-admin set is installation-wide, so the tests hold
//! [`SERIAL`] and each starts by demoting every admin a previous test made.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS:
//!   export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
//!   env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-perm-compile --test infra_converge

use std::collections::BTreeMap;

use egret_metastore::Db;
use egret_perm::PermStore;
use egret_perm_compile::{converge_infra_policies, converge_infra_policy, recompile_service};
use egret_policy::default_policies::{admin_all_access_bodies, query_log_guard_bodies};
use egret_policy::infra_names::{
    is_ir_infra_name, EXECUTE_QUERY_POLICY_NAME, IMPERSONATE_POLICY_NAME,
    QUERY_LOG_LOADER_POLICY_NAME, QUERY_TIMEOUT_POLICY_NAME, SELF_IMPERSONATE_POLICY_NAME,
};
use egret_policy::PolicyStore;
use serde_json::{json, Value};
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const FIXED: [&str; 4] = [
    IMPERSONATE_POLICY_NAME,
    SELF_IMPERSONATE_POLICY_NAME,
    EXECUTE_QUERY_POLICY_NAME,
    QUERY_TIMEOUT_POLICY_NAME,
];

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
    Some(db)
}

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &Uuid::new_v4().simple().to_string()[..12])
}

async fn new_user(db: &Db, username: Option<&str>) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name, username) VALUES ('local',$1,'T',$2) RETURNING id",
    )
    .bind(format!("{}@ic.test", unique("u")))
    .bind(username)
    .fetch_one(&db.pool)
    .await
    .expect("insert user")
}

async fn make_admin_directly(db: &Db, user: Uuid) {
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type) \
         SELECT 'user', $1, id, 'platform' FROM roles \
          WHERE name = 'platform_admin' AND scope = 'platform'",
    )
    .bind(user)
    .execute(&db.pool)
    .await
    .expect("admin grant");
}

/// One active platform admin with a username. Returns the username.
async fn one_admin(db: &Db) -> String {
    let name = unique("ic_adm");
    let id = new_user(db, Some(&name)).await;
    make_admin_directly(db, id).await;
    name
}

/// A bare profile of `service_type` (its def seeded). Returns `(id, name)`.
async fn profile(db: &Db, service_type: &str) -> (i64, String) {
    let policy = PolicyStore::new(db.clone());
    if service_type == "starrocks" {
        egret_policy::seed_starrocks(&policy).await.expect("def");
    } else {
        egret_policy::seed_trino(&policy).await.expect("def");
    }
    let name = unique(&format!("ic_{service_type}"));
    let svc = policy
        .create_service(&name, service_type, &json!({}))
        .await
        .expect("service");
    (svc.id, name)
}

/// A provisioned engine of `engine_type` bound to `svc` with `spec`.
async fn bind_engine(db: &Db, svc: i64, engine_type: &str, spec: Value) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (id, name, type, namespace, spec, authz_service_id) \
         VALUES (gen_random_uuid(), $1, $2, 'egret', $3, $4) RETURNING id",
    )
    .bind(unique("ic-engine").replace('_', "-"))
    .bind(engine_type)
    .bind(spec)
    .bind(svc)
    .fetch_one(&db.pool)
    .await
    .expect("engine")
}

/// An external engine (egret did not provision it) bound to `svc`.
async fn bind_external_engine(db: &Db, svc: i64, engine_type: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines \
           (id, name, type, namespace, spec, authz_service_id, origin, \
            external_host, external_port, ranger_plugin_version) \
         VALUES (gen_random_uuid(), $1, $2, '', '{}'::jsonb, $3, 'external', \
                 'ext.example', 8080, '2.5.0') RETURNING id",
    )
    .bind(unique("ic-ext").replace('_', "-"))
    .bind(engine_type)
    .bind(svc)
    .fetch_one(&db.pool)
    .await
    .expect("external engine")
}

fn impersonate_with_auth() -> Value {
    json!({"trinoConfig": {
        "coordinator": {},
        "delegationMode": "impersonate",
        "auth": {"password": {}}
    }})
}

fn jwt_with_auth() -> Value {
    json!({"trinoConfig": {
        "coordinator": {},
        "delegationMode": "jwt",
        "auth": {"password": {}, "jwt": {}}
    }})
}

/// One stored row, as convergence sees it.
#[derive(Debug, Clone, PartialEq)]
struct Row {
    id: i64,
    managed_by: String,
    version: i64,
    body: Value,
}

/// Every row of `svc`, by name.
async fn all_rows(db: &Db, svc: i64) -> BTreeMap<String, Row> {
    let rows: Vec<(i64, String, String, i64, Value)> = sqlx::query_as(
        "SELECT id, name, managed_by, version, policy_jsonb FROM policy.authz_policies \
          WHERE service_id = $1",
    )
    .bind(svc)
    .fetch_all(&db.pool)
    .await
    .expect("rows");
    rows.into_iter()
        .map(|(id, name, managed_by, version, body)| {
            (
                name,
                Row {
                    id,
                    managed_by,
                    version,
                    body,
                },
            )
        })
        .collect()
}

/// The IR-owned infra rows of `svc`, by name (the predicate is type-aware).
async fn infra_rows(db: &Db, svc: i64) -> BTreeMap<String, Row> {
    let service_type: String =
        sqlx::query_scalar("SELECT type FROM policy.authz_services WHERE id = $1")
            .bind(svc)
            .fetch_one(&db.pool)
            .await
            .expect("service type");
    all_rows(db, svc)
        .await
        .into_iter()
        .filter(|(name, _)| is_ir_infra_name(&service_type, name))
        .collect()
}

async fn policy_version(db: &Db, svc: i64) -> i64 {
    sqlx::query_scalar("SELECT policy_version FROM policy.authz_services WHERE id = $1")
        .bind(svc)
        .fetch_one(&db.pool)
        .await
        .expect("policy_version")
}

fn admin_names(service_type: &str, service: &str) -> Vec<String> {
    admin_all_access_bodies(service_type, service, &[])
        .expect("bodies")
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

fn users_of(body: &Value) -> Vec<String> {
    body["policyItems"]
        .as_array()
        .expect("policyItems")
        .iter()
        .flat_map(|item| item["users"].as_array().expect("users").iter())
        .map(|u| u.as_str().expect("a user name").to_string())
        .collect()
}

#[tokio::test]
async fn an_impersonate_trino_engine_gets_all_seventeen_rows() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    let admin = one_admin(&db).await;
    let (svc, name) = profile(&db, "trino").await;
    bind_engine(&db, svc, "trino", impersonate_with_auth()).await;
    let v0 = policy_version(&db, svc).await;

    let sync = converge_infra_policy(&db.pool, svc)
        .await
        .expect("converge");

    let rows = infra_rows(&db, svc).await;
    let admin_rows = admin_names("trino", &name);
    assert_eq!(admin_rows.len(), 13, "the Trino def builds 13 hierarchies");
    let mut want: Vec<String> = admin_rows.clone();
    want.extend(FIXED.iter().map(|s| s.to_string()));
    want.sort();
    assert_eq!(
        rows.keys().cloned().collect::<Vec<_>>(),
        want,
        "13 egret-admin rows plus the four fixed names"
    );
    for (row_name, row) in &rows {
        assert_eq!(
            row.managed_by, "infra",
            "{row_name} is not managed_by infra"
        );
        assert_eq!(row.version, 1, "{row_name} was written more than once");
    }
    for row_name in &admin_rows {
        assert_eq!(
            users_of(&rows[row_name].body),
            vec![admin.clone()],
            "{row_name} names someone other than the one admin"
        );
    }
    assert!(sync.changed, "the first convergence wrote");
    assert_eq!(sync.added.len(), 17, "every row was inserted: {sync:?}");
    assert!(sync.removed.is_empty(), "nothing was deleted: {sync:?}");
    assert_eq!(
        policy_version(&db, svc).await,
        v0 + 1,
        "policy_version moves exactly once"
    );
}

#[tokio::test]
async fn converging_twice_writes_nothing_the_second_time() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    one_admin(&db).await;
    let (svc, _) = profile(&db, "trino").await;
    bind_engine(&db, svc, "trino", impersonate_with_auth()).await;
    converge_infra_policy(&db.pool, svc)
        .await
        .expect("first convergence");
    let rows = infra_rows(&db, svc).await;
    let v1 = policy_version(&db, svc).await;
    assert_eq!(rows.len(), 17, "the arrangement");

    let sync = converge_infra_policy(&db.pool, svc)
        .await
        .expect("second convergence");

    assert!(!sync.changed, "the second convergence wrote: {sync:?}");
    assert_eq!(
        infra_rows(&db, svc).await,
        rows,
        "ids, versions or bodies moved on an unchanged store"
    );
    assert_eq!(policy_version(&db, svc).await, v1, "policy_version moved");
}

#[tokio::test]
async fn impersonate_to_jwt_removes_the_delegator_grant_and_keeps_the_other_ids() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    one_admin(&db).await;
    let (svc, _) = profile(&db, "trino").await;
    let engine = bind_engine(&db, svc, "trino", impersonate_with_auth()).await;
    converge_infra_policy(&db.pool, svc)
        .await
        .expect("first convergence");
    let before = infra_rows(&db, svc).await;
    assert!(
        before.contains_key(IMPERSONATE_POLICY_NAME),
        "the arrangement"
    );
    sqlx::query("UPDATE engine_mgmt.engines SET spec = $2 WHERE id = $1")
        .bind(engine)
        .bind(jwt_with_auth())
        .execute(&db.pool)
        .await
        .expect("to jwt");
    let v0 = policy_version(&db, svc).await;

    let sync = converge_infra_policy(&db.pool, svc)
        .await
        .expect("converge");

    let after = infra_rows(&db, svc).await;
    assert!(
        !after.contains_key(IMPERSONATE_POLICY_NAME),
        "a jwt engine still carries the delegator's impersonation grant"
    );
    assert_eq!(sync.removed, vec![IMPERSONATE_POLICY_NAME.to_string()]);
    assert_eq!(after.len(), before.len() - 1, "only the delegator row went");
    for (row_name, row) in &after {
        assert_eq!(
            row, &before[row_name],
            "{row_name} was rewritten, though only the delegator grant changed"
        );
    }
    assert_eq!(policy_version(&db, svc).await, v0 + 1, "bumped once");
}

#[tokio::test]
async fn unbinding_the_last_qualifying_engine_removes_every_ir_infra_row_but_not_all_defaults() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    one_admin(&db).await;
    let (svc, name) = profile(&db, "trino").await;
    // The `all - …` set first: its seeder refuses a profile holding policies.
    PolicyStore::new(db.clone())
        .seed_default_trino_policies(svc, &name, "ic_admins")
        .await
        .expect("seed defaults");
    let engine = bind_engine(&db, svc, "trino", impersonate_with_auth()).await;
    converge_infra_policy(&db.pool, svc)
        .await
        .expect("first convergence");
    assert_eq!(infra_rows(&db, svc).await.len(), 17, "the arrangement");
    let defaults_before: BTreeMap<String, Row> = all_rows(&db, svc)
        .await
        .into_iter()
        .filter(|(n, _)| n.starts_with("all - "))
        .collect();
    assert_eq!(
        defaults_before.len(),
        13,
        "the arrangement: 13 `all - …` rows"
    );

    sqlx::query("UPDATE engine_mgmt.engines SET authz_service_id = NULL WHERE id = $1")
        .bind(engine)
        .execute(&db.pool)
        .await
        .expect("unbind");
    let sync = converge_infra_policy(&db.pool, svc)
        .await
        .expect("converge");

    assert!(
        infra_rows(&db, svc).await.is_empty(),
        "a profile with no qualifying engine keeps IR-owned infra rows"
    );
    assert_eq!(sync.removed.len(), 17, "{sync:?}");
    let defaults_after: BTreeMap<String, Row> = all_rows(&db, svc)
        .await
        .into_iter()
        .filter(|(n, _)| n.starts_with("all - "))
        .collect();
    assert_eq!(
        defaults_after, defaults_before,
        "convergence touched the `all - …` default set"
    );
}

/// The query-log rows a StarRocks profile `service` gets: the loader and
/// one guard row per hierarchy under `database`.
fn query_log_names(service: &str) -> Vec<String> {
    let mut names = vec![QUERY_LOG_LOADER_POLICY_NAME.to_string()];
    names.extend(
        query_log_guard_bodies(service)
            .expect("guard bodies")
            .into_iter()
            .map(|(name, _)| name),
    );
    names
}

#[tokio::test]
async fn a_starrocks_profile_gets_egret_admin_and_the_query_log() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    let admin = one_admin(&db).await;
    let (svc, name) = profile(&db, "starrocks").await;
    bind_engine(
        &db,
        svc,
        "starrocks",
        json!({"starrocksConfig": {"delegation": {"mode": "impersonate"}}}),
    )
    .await;

    converge_infra_policy(&db.pool, svc)
        .await
        .expect("converge");

    let rows = infra_rows(&db, svc).await;
    let admin_rows = admin_names("starrocks", &name);
    assert_eq!(
        admin_rows.len(),
        13,
        "the StarRocks def builds 13 hierarchies"
    );
    let mut want = admin_rows.clone();
    want.extend(query_log_names(&name));
    want.push(IMPERSONATE_POLICY_NAME.to_string());
    want.sort();
    assert_eq!(
        rows.keys().cloned().collect::<Vec<_>>(),
        want,
        "a StarRocks impersonate profile gets the StarRocks-def egret-admin rows, the query-log rows and the delegator's impersonation"
    );
    for (row_name, row) in &rows {
        assert_eq!(row.managed_by, "infra", "{row_name}");
        assert_eq!(row.body["serviceType"], "starrocks", "{row_name}");
    }
    for row_name in &admin_rows {
        assert_eq!(
            users_of(&rows[row_name].body),
            vec![admin.clone()],
            "{row_name}"
        );
    }
}

/// Package D, D1b: a StarRocks engine in impersonate mode brings the
/// delegator's impersonation, StarRocks-shaped, excluding root, the
/// delegator and the query-log loader; moving it to jwt takes only that row
/// away.
#[tokio::test]
async fn a_starrocks_impersonate_engine_brings_the_delegator_row_and_jwt_takes_it_away() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    one_admin(&db).await;
    let (svc, name) = profile(&db, "starrocks").await;
    let engine = bind_engine(
        &db,
        svc,
        "starrocks",
        json!({"starrocksConfig": {"delegation": {"mode": "impersonate"}}}),
    )
    .await;
    converge_infra_policy(&db.pool, svc)
        .await
        .expect("converge");

    let before = infra_rows(&db, svc).await;
    let row = before
        .get(IMPERSONATE_POLICY_NAME)
        .expect("the delegator's impersonation row");
    assert_eq!(row.managed_by, "infra");
    let excluded: Vec<String> = vec![
        "egret_delegator".into(),
        "egret_query_log".into(),
        "root".into(),
    ];
    assert_eq!(
        row.body,
        egret_policy_api::infra_bodies::starrocks_delegator_impersonate_policy(&name, &excluded)
    );

    sqlx::query("UPDATE engine_mgmt.engines SET spec = $2 WHERE id = $1")
        .bind(engine)
        .bind(json!({"starrocksConfig": {"delegation": {"mode": "jwt"}}}))
        .execute(&db.pool)
        .await
        .expect("to jwt");
    let sync = converge_infra_policy(&db.pool, svc)
        .await
        .expect("converge");

    assert_eq!(sync.removed, vec![IMPERSONATE_POLICY_NAME.to_string()]);
    let after = infra_rows(&db, svc).await;
    assert!(!after.contains_key(IMPERSONATE_POLICY_NAME));
    assert_eq!(after.len(), before.len() - 1, "only the delegator row went");
}

/// Package A, Task 2: binding a provisioned StarRocks engine, delegating or
/// not, adds the query-log loader and guard rows `managed_by = 'infra'`, the
/// boot pass sees the profile, and unbinding removes the rows again.
#[tokio::test]
async fn a_provisioned_starrocks_engine_brings_the_query_log_rows_and_takes_them_away() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    one_admin(&db).await;
    let (svc, name) = profile(&db, "starrocks").await;
    // Not delegating: the rows must not wait on delegation (correction 5).
    let engine = bind_engine(&db, svc, "starrocks", json!({})).await;
    assert!(
        egret_perm_compile::infra::infra_services(&db.pool)
            .await
            .expect("services")
            .contains(&svc),
        "the boot pass must visit a profile bound to a non-delegating StarRocks engine"
    );

    let sync = converge_infra_policy(&db.pool, svc)
        .await
        .expect("converge");

    let rows = infra_rows(&db, svc).await;
    let mut want = query_log_names(&name);
    assert_eq!(want.len(), 7, "the loader and six guard rows");
    assert_eq!(sync.added, want, "inserted in render order");
    want.sort();
    assert_eq!(
        rows.keys().cloned().collect::<Vec<_>>(),
        want,
        "no egret-admin rows without delegation; the query-log rows only"
    );
    for (row_name, row) in &rows {
        assert_eq!(row.managed_by, "infra", "{row_name}");
    }

    sqlx::query("UPDATE engine_mgmt.engines SET authz_service_id = NULL WHERE id = $1")
        .bind(engine)
        .execute(&db.pool)
        .await
        .expect("unbind");
    let sync = converge_infra_policy(&db.pool, svc)
        .await
        .expect("converge after unbind");

    assert!(
        infra_rows(&db, svc).await.is_empty(),
        "the query-log rows outlived the engine"
    );
    assert_eq!(sync.removed, want, "{sync:?}");
}

#[tokio::test]
async fn an_external_starrocks_engine_gets_no_query_log_rows() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    one_admin(&db).await;
    let (svc, _) = profile(&db, "starrocks").await;
    bind_external_engine(&db, svc, "starrocks").await;
    let v0 = policy_version(&db, svc).await;

    let sync = converge_infra_policy(&db.pool, svc)
        .await
        .expect("converge");

    assert!(
        infra_rows(&db, svc).await.is_empty(),
        "an external StarRocks engine got infra rows"
    );
    assert!(!sync.changed, "{sync:?}");
    assert_eq!(policy_version(&db, svc).await, v0, "bumped");
}

#[tokio::test]
async fn an_external_or_open_engine_does_not_qualify() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    one_admin(&db).await;
    let (external, _) = profile(&db, "trino").await;
    bind_external_engine(&db, external, "trino").await;
    let (open, _) = profile(&db, "trino").await;
    bind_engine(
        &db,
        open,
        "trino",
        json!({"trinoConfig": {"delegationMode": "open"}}),
    )
    .await;

    for (what, svc) in [("an external engine", external), ("an open engine", open)] {
        let v0 = policy_version(&db, svc).await;
        let sync = converge_infra_policy(&db.pool, svc)
            .await
            .expect("converge");
        assert!(
            infra_rows(&db, svc).await.is_empty(),
            "a profile bound only to {what} got infra rows"
        );
        assert!(!sync.changed, "{what}: {sync:?}");
        assert_eq!(policy_version(&db, svc).await, v0, "{what}: bumped");
    }
}

#[tokio::test]
async fn a_hand_written_profile_stays_manual() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    one_admin(&db).await;
    let (svc, _) = profile(&db, "trino").await;
    bind_engine(&db, svc, "trino", impersonate_with_auth()).await;

    converge_infra_policy(&db.pool, svc)
        .await
        .expect("converge");
    recompile_service(
        &PermStore::new(db.pool.clone()),
        &PolicyStore::new(db.clone()),
        svc,
    )
    .await
    .expect("recompile");

    assert_eq!(infra_rows(&db, svc).await.len(), 17, "the arrangement");
    let origin: String =
        sqlx::query_scalar("SELECT origin FROM policy.authz_services WHERE id = $1")
            .bind(svc)
            .fetch_one(&db.pool)
            .await
            .expect("origin");
    assert_eq!(
        origin, "manual",
        "infra rows turned a hand-written profile into a generated one"
    );
}

#[tokio::test]
async fn an_existing_row_is_adopted_in_place() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    one_admin(&db).await;
    let (svc, name) = profile(&db, "trino").await;
    // Today's exact body, written by hand the way engine apply wrote it.
    let id = egret_policy::store::insert_policy_owned_by_via(
        &mut db.pool.acquire().await.expect("conn"),
        svc,
        EXECUTE_QUERY_POLICY_NAME,
        &egret_policy_api::infra_bodies::execute_query_policy(&name),
        "infra",
    )
    .await
    .expect("hand-written baseline row");
    bind_engine(&db, svc, "trino", impersonate_with_auth()).await;
    let before = all_rows(&db, svc).await[EXECUTE_QUERY_POLICY_NAME].clone();
    assert_eq!(before.id, id, "the arrangement");

    let sync = converge_infra_policy(&db.pool, svc)
        .await
        .expect("converge");

    let after = infra_rows(&db, svc).await;
    assert_eq!(
        after[EXECUTE_QUERY_POLICY_NAME], before,
        "an equal body was rewritten (id, version or body moved)"
    );
    assert!(
        !sync.added.contains(&EXECUTE_QUERY_POLICY_NAME.to_string()),
        "the existing row was inserted again: {sync:?}"
    );
    assert_eq!(after.len(), 17, "the other rows were added around it");
}

/// Fix round 1, Important 1: the query-log names are reserved on StarRocks
/// only, so on a Trino profile `egret-query-log-loader` is a human's policy.
/// Convergence (on its own and in the boot pass) must neither delete nor adopt
/// it, and the boot pass must not pull in a Trino profile for holding it.
#[tokio::test]
async fn a_human_query_log_named_policy_on_a_trino_profile_survives_convergence() {
    let _serial = SERIAL.lock().await;
    let Some(db) = test_db().await else { return };
    one_admin(&db).await;
    let store = PolicyStore::new(db.clone());
    let body = json!({"name": QUERY_LOG_LOADER_POLICY_NAME, "policyItems": []});
    // A Trino profile with a qualifying engine: convergence has work there.
    let (bound, _) = profile(&db, "trino").await;
    bind_engine(&db, bound, "trino", impersonate_with_auth()).await;
    let human = store
        .create_policy(bound, QUERY_LOG_LOADER_POLICY_NAME, &body)
        .await
        .expect("a human name on trino");
    // A Trino profile holding only that human row, no engine.
    let (idle, _) = profile(&db, "trino").await;
    let idle_human = store
        .create_policy(idle, QUERY_LOG_LOADER_POLICY_NAME, &body)
        .await
        .expect("a human name on trino");
    let before = all_rows(&db, bound).await[QUERY_LOG_LOADER_POLICY_NAME].clone();
    assert_eq!(before.id, human, "the arrangement");
    assert_eq!(before.managed_by, "operator", "the arrangement");

    let sync = converge_infra_policy(&db.pool, bound)
        .await
        .expect("converge");
    assert!(
        !egret_perm_compile::infra::infra_services(&db.pool)
            .await
            .expect("services")
            .contains(&idle),
        "the boot pass pulled in a Trino profile for a human query-log-named row"
    );
    converge_infra_policies(&db.pool).await.expect("boot pass");

    assert!(
        !sync
            .removed
            .contains(&QUERY_LOG_LOADER_POLICY_NAME.to_string()),
        "convergence deleted a human policy: {sync:?}"
    );
    assert_eq!(
        all_rows(&db, bound).await.get(QUERY_LOG_LOADER_POLICY_NAME),
        Some(&before),
        "the human row was deleted, adopted or rewritten"
    );
    let idle_rows = all_rows(&db, idle).await;
    assert_eq!(
        idle_rows
            .get(QUERY_LOG_LOADER_POLICY_NAME)
            .map(|r| (r.id, r.managed_by.as_str())),
        Some((idle_human, "operator")),
        "the boot pass touched the idle profile's human row"
    );
}
