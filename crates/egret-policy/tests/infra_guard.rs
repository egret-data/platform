//! A human may not edit, disable or delete a policy egret's own plumbing owns,
//! and may not mint or rename a policy INTO an infrastructure name.
//!
//! Both cells on every path. A guard measured only by its refusals cannot be
//! told apart from `return Err(..)`, and this programme has already shipped one
//! predicate whose allow arm was unfalsifiable.

use egret_metastore::Db;
use egret_policy::{PolicyStore, PolicyStoreError};
use serde_json::json;

async fn fixture() -> Option<(PolicyStore, sqlx::PgPool, i64, i64, i64)> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let store = PolicyStore::new(db);
    egret_policy::seed_trino(&store)
        .await
        .expect("seed trino def");
    let service_id = store
        .create_service(
            &format!("t-infra-{}", uuid::Uuid::new_v4().simple()),
            "trino",
            &json!({}),
        )
        .await
        .expect("service")
        .id;
    let infra: i64 = sqlx::query_scalar(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, managed_by) \
         VALUES ($1, 'egret-delegator-impersonate', '{}'::jsonb, 'infra') RETURNING id",
    )
    .bind(service_id)
    .fetch_one(&pool)
    .await
    .expect("infra policy");
    let operator: i64 = sqlx::query_scalar(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, managed_by) \
         VALUES ($1, 'sales can read orders', '{}'::jsonb, 'operator') RETURNING id",
    )
    .bind(service_id)
    .fetch_one(&pool)
    .await
    .expect("operator policy");
    Some((store, pool, service_id, infra, operator))
}

async fn names_in(pool: &sqlx::PgPool, service_id: i64) -> Vec<String> {
    sqlx::query_scalar("SELECT name FROM policy.authz_policies WHERE service_id = $1 ORDER BY name")
        .bind(service_id)
        .fetch_all(pool)
        .await
        .expect("names")
}

#[tokio::test]
async fn an_infra_policy_cannot_be_updated_disabled_or_deleted_and_an_operator_one_can() {
    let Some((store, pool, _service_id, infra, operator)) = fixture().await else {
        return;
    };
    let body = json!({"name": "x", "policyItems": []});

    // REFUSED — the three paths, on the infra row.
    assert!(matches!(
        store.update_policy(infra, "renamed", &body, true).await,
        Err(PolicyStoreError::InfrastructureOwned(_))
    ));
    assert!(matches!(
        store.set_policy_enabled(infra, false).await,
        Err(PolicyStoreError::InfrastructureOwned(_))
    ));
    assert!(matches!(
        store.delete_policy(infra).await,
        Err(PolicyStoreError::InfrastructureOwned(_))
    ));

    // ALLOWED — the same three paths, on the operator row. Without these the
    // test cannot tell a correct guard from one that refuses everything.
    store
        .update_policy(operator, "sales can read orders v2", &body, true)
        .await
        .expect("an operator policy is still editable");
    store
        .set_policy_enabled(operator, false)
        .await
        .expect("an operator policy is still disableable");
    store
        .delete_policy(operator)
        .await
        .expect("an operator policy is still deletable");

    // And the infra row is untouched by the three refusals — a guard that
    // refused AFTER writing would pass every assertion above.
    let (name, enabled): (String, bool) =
        sqlx::query_as("SELECT name, enabled FROM policy.authz_policies WHERE id = $1")
            .bind(infra)
            .fetch_one(&pool)
            .await
            .expect("the infra row still exists");
    assert_eq!(name, "egret-delegator-impersonate");
    assert!(enabled);
}

/// The reservation: a human cannot mint, or rename a row INTO, a name
/// `is_egret_infrastructure_policy_name` recognises — otherwise the column
/// (`'operator'`) and the predicate drift apart. Allow cells: a near-miss
/// operator name, and the explicit infra writers, which must keep working.
#[tokio::test]
async fn a_human_cannot_create_or_rename_into_an_infra_name_but_the_infra_writers_can() {
    let Some((store, pool, service_id, _infra, operator)) = fixture().await else {
        return;
    };
    let body = json!({"name": "x", "policyItems": []});
    let before = names_in(&pool, service_id).await;

    // REFUSED — human create, a fixed name and a seeded-hierarchy name.
    for name in ["egret-query-timeout", "all - catalog, schema, table"] {
        let err = store
            .create_policy(service_id, name, &body)
            .await
            .expect_err("a human create of an infra name must be refused");
        assert!(
            matches!(err, PolicyStoreError::InfrastructureOwned(_)),
            "{name}: expected InfrastructureOwned, got {err:?}"
        );
    }
    assert_eq!(names_in(&pool, service_id).await, before, "no row minted");

    // REFUSED — human rename of an operator row into an infra name; row unchanged.
    for name in ["egret-query-timeout", "all - catalog, schema, table"] {
        let err = store
            .update_policy(operator, name, &body, true)
            .await
            .expect_err("a human rename into an infra name must be refused");
        assert!(
            matches!(err, PolicyStoreError::InfrastructureOwned(_)),
            "{name}: expected InfrastructureOwned, got {err:?}"
        );
    }
    assert_eq!(names_in(&pool, service_id).await, before, "no row renamed");

    // ALLOWED — a near-miss operator name, on both human paths.
    store
        .create_policy(service_id, "all-hands-reporting", &body)
        .await
        .expect("a near-miss operator name is still creatable");
    store
        .update_policy(operator, "all-hands-reporting-2", &body, true)
        .await
        .expect("a near-miss operator name is still a legal rename");

    // ALLOWED — the explicit infra writers still create an infra name, stamped infra.
    store
        .create_infra_policy(service_id, "egret-query-timeout", &body)
        .await
        .expect("the infra creator is not subject to the human reservation");
    store
        .upsert_infra_policy(service_id, "all - catalog, schema, table", &body)
        .await
        .expect("the infra upserter is not subject to the human reservation");
    let stamped: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, managed_by FROM policy.authz_policies \
         WHERE service_id = $1 AND name IN ('egret-query-timeout', 'all - catalog, schema, table') \
         ORDER BY name",
    )
    .bind(service_id)
    .fetch_all(&pool)
    .await
    .expect("stamped");
    assert_eq!(
        stamped,
        vec![
            (
                "all - catalog, schema, table".to_string(),
                "infra".to_string()
            ),
            ("egret-query-timeout".to_string(), "infra".to_string()),
        ]
    );
}
