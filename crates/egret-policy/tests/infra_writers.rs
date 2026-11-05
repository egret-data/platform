//! Every NON-human writer of `policy.authz_policies` stamps its own
//! `managed_by`, and the stamp agrees with the name predicates.
//!
//! The migration's backfill is covered by `egret-perm-compile`'s
//! `managed_by_agreement`; this file covers the writers that run AFTER it, which
//! no other test would notice regressing to the column default (`'operator'`).
//! Every read is scoped to this fixture's own `service_id`.

use egret_metastore::Db;
use egret_policy::infra_names::{is_egret_infrastructure_policy, QUERY_TIMEOUT_POLICY_NAME};
use egret_policy::{PolicyStore, PolicyStoreError};
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

async fn starrocks_service(s: &PolicyStore) -> i64 {
    egret_policy::seed_starrocks(s)
        .await
        .expect("seed starrocks def");
    s.create_service(
        &format!("sr-writers-{}", uuid::Uuid::new_v4().simple()),
        "starrocks",
        &json!({}),
    )
    .await
    .expect("create service")
    .id
}

/// Every row is `infra`, there is at least `min` of them (the control that makes
/// an empty read fail instead of vacuously pass), and the predicate agrees on a
/// Trino profile (every fixture here that calls this is one).
fn assert_all_infra(rows: &[(String, String)], min: usize) {
    assert_all_infra_on("trino", rows, min)
}

fn assert_all_infra_on(service_type: &str, rows: &[(String, String)], min: usize) {
    assert!(rows.len() >= min, "expected >= {min} rows, got {rows:?}");
    for (name, managed_by) in rows {
        assert_eq!(managed_by, "infra", "{name:?} was not stamped infra");
        assert!(
            is_egret_infrastructure_policy(service_type, name),
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
async fn the_infra_writers_refuse_a_name_the_predicate_does_not_recognise() {
    let Some((s, pool)) = store().await else {
        return;
    };
    let id = trino_service(&s).await;
    let human = "sales can read orders";
    assert!(!is_egret_infrastructure_policy("trino", human));

    let upserted = s.upsert_infra_policy(id, human, &json!({})).await;
    assert!(
        matches!(upserted, Err(PolicyStoreError::BadRequest(_))),
        "{upserted:?}"
    );
    assert!(
        owners(&pool, id).await.is_empty(),
        "a refused write left a row"
    );

    // The other cell: an infra name is accepted.
    s.upsert_infra_policy(id, "all - catalog", &json!({}))
        .await
        .expect("infra name accepted by upsert");
    assert_all_infra(&owners(&pool, id).await, 1);
}

/// SP4d-3 final review I-1: the default `all - …` set is Trino-shaped, so on a
/// StarRocks profile neither infra writer may stamp that shape (a pre-0180
/// leftover of it is the operator's to delete). A fixed infra name stays
/// infrastructure there — the narrowed ruling. (The StarRocks-shaped per-admin
/// set is written by convergence, covered in `egret-perm-compile`.)
#[tokio::test]
async fn on_a_starrocks_profile_the_infra_writers_refuse_the_trino_default_shape_only() {
    let Some((s, pool)) = store().await else {
        return;
    };
    let id = starrocks_service(&s).await;

    let upserted = s
        .upsert_infra_policy(id, "all - catalog, database, table", &json!({}))
        .await;
    assert!(
        matches!(upserted, Err(PolicyStoreError::BadRequest(_))),
        "{upserted:?}"
    );
    let seeded = s
        .seed_default_trino_policies(id, "sr-writers", "admin")
        .await;
    assert!(
        matches!(seeded, Err(PolicyStoreError::BadRequest(_))),
        "{seeded:?}"
    );
    assert!(
        owners(&pool, id).await.is_empty(),
        "a refused write left a row"
    );

    // The allow cell: a fixed infra name.
    s.upsert_infra_policy(id, QUERY_TIMEOUT_POLICY_NAME, &json!({}))
        .await
        .expect("delegator name on starrocks");
    assert_all_infra_on("starrocks", &owners(&pool, id).await, 1);
}
