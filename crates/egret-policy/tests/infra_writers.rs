//! Every NON-human writer of `policy.authz_policies` stamps its own
//! `managed_by`, and the stamp agrees with the name predicates.
//!
//! The migration's backfill is covered by `egret-perm-compile`'s
//! `managed_by_agreement`; this file covers the writers that run AFTER it, which
//! no other test would notice regressing to the column default (`'operator'`).
//! Every read is scoped to this fixture's own `service_id`.

use egret_metastore::Db;
use egret_policy::infra_names::{
    is_egret_infrastructure_policy_name, IMPERSONATE_POLICY_NAME, QUERY_TIMEOUT_POLICY_NAME,
};
use egret_policy::{PolicyStore, PolicyStoreError};
use egret_policy_api::PolicyPort;
use serde_json::json;

async fn store() -> Option<(PolicyStore, sqlx::PgPool)> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    Some((PolicyStore::new(db), pool))
}

async fn trino_service(s: &PolicyStore) -> i64 {
    egret_policy::seed_trino(s).await.expect("seed trino def");
    s.create_service(
        &format!("t-writers-{}", uuid::Uuid::new_v4().simple()),
        "trino",
        &json!({}),
    )
    .await
    .expect("create service")
    .id
}

async fn owners(pool: &sqlx::PgPool, service_id: i64) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT name, managed_by FROM policy.authz_policies WHERE service_id = $1 ORDER BY name",
    )
    .bind(service_id)
    .fetch_all(pool)
    .await
    .expect("read back")
}

/// Every row is `infra`, there is at least `min` of them (the control that makes
/// an empty read fail instead of vacuously pass), and the predicate agrees.
fn assert_all_infra(rows: &[(String, String)], min: usize) {
    assert!(rows.len() >= min, "expected >= {min} rows, got {rows:?}");
    for (name, managed_by) in rows {
        assert_eq!(managed_by, "infra", "{name:?} was not stamped infra");
        assert!(
            is_egret_infrastructure_policy_name(name),
            "{name:?} is stamped infra but the predicate disagrees"
        );
    }
}

#[tokio::test]
async fn the_default_trino_seeder_stamps_infra() {
    let Some((s, pool)) = store().await else {
        return;
    };
    let id = trino_service(&s).await;
    let n = s
        .seed_default_trino_policies(id, "writers-svc", "admin")
        .await
        .expect("seed");
    assert!(n > 0);
    assert_all_infra(&owners(&pool, id).await, n);
}

#[tokio::test]
async fn the_admin_all_access_seeder_stamps_infra() {
    let Some((s, pool)) = store().await else {
        return;
    };
    let id = trino_service(&s).await;
    let n = s
        .seed_admin_all_access(id, "writers-svc", "trino", &["admin".to_string()])
        .await
        .expect("seed");
    assert!(n > 0);
    assert_all_infra(&owners(&pool, id).await, n);
}

/// The delegator seeding (`engines.rs::seed_delegator_policies`) calls exactly
/// `PolicyPort::create_infra_policy` with these two names. It needs a live
/// engine/CpState to reach, so this drives the `PolicyPort` implementation it
/// calls, with the delegator's own name constants.
#[tokio::test]
async fn the_delegator_policy_port_call_stamps_infra_on_both_names() {
    let Some((s, pool)) = store().await else {
        return;
    };
    let id = trino_service(&s).await;
    for name in [IMPERSONATE_POLICY_NAME, QUERY_TIMEOUT_POLICY_NAME] {
        PolicyPort::create_infra_policy(&s, id, name, &json!({"name": name}))
            .await
            .expect("create infra policy");
    }
    let rows = owners(&pool, id).await;
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_all_infra(&rows, 2);

    // Create semantics are preserved: a second create is AlreadyExists.
    let again = PolicyPort::create_infra_policy(&s, id, IMPERSONATE_POLICY_NAME, &json!({})).await;
    assert!(matches!(
        again,
        Err(egret_policy_api::PolicyError::AlreadyExists(_))
    ));
}

#[tokio::test]
async fn the_infra_writers_refuse_a_name_the_predicate_does_not_recognise() {
    let Some((s, pool)) = store().await else {
        return;
    };
    let id = trino_service(&s).await;
    let human = "sales can read orders";
    assert!(!is_egret_infrastructure_policy_name(human));

    let created = s.create_infra_policy(id, human, &json!({})).await;
    assert!(
        matches!(created, Err(PolicyStoreError::BadRequest(_))),
        "{created:?}"
    );
    let upserted = s.upsert_infra_policy(id, human, &json!({})).await;
    assert!(
        matches!(upserted, Err(PolicyStoreError::BadRequest(_))),
        "{upserted:?}"
    );
    assert!(
        owners(&pool, id).await.is_empty(),
        "a refused write left a row"
    );

    // The other cell: an infra name is accepted by both.
    s.create_infra_policy(id, "egret-admin - catalog", &json!({}))
        .await
        .expect("infra name accepted by create");
    s.upsert_infra_policy(id, "all - catalog", &json!({}))
        .await
        .expect("infra name accepted by upsert");
    assert_all_infra(&owners(&pool, id).await, 2);
}
