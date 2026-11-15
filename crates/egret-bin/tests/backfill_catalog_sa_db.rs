//! SA2 task 3: `egret backfill catalog-client-sa` gives each pre-existing catalog
//! client its own workspace-scoped service account, ledger-correctly, once.
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.

use egret_bin::backfill_catalog_sa::{backfill_catalog_client_sa, BackfillReport};
use uuid::Uuid;

/// A warehouse (in its own fresh workspace) plus one NULL-SA client on it.
async fn seed_client(pool: &sqlx::PgPool) -> (Uuid, Uuid) {
    let wh = egret_test_db::seed_warehouse(pool).await;
    let ws: Uuid = sqlx::query_scalar("SELECT workspace_id FROM catalog.warehouses WHERE id = $1")
        .bind(wh)
        .fetch_one(pool)
        .await
        .unwrap();
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.catalog_roles (warehouse_id, name, consumer_type) \
         VALUES ($1, 'r', 'external') RETURNING id",
    )
    .bind(wh)
    .fetch_one(pool)
    .await
    .unwrap();
    let cid = format!("cc-{}", Uuid::new_v4().simple());
    let client: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.catalog_clients \
           (name, consumer_type, workspace_id, warehouse_id, catalog_role_id, \
            client_id, client_secret_hash) \
         VALUES ($1, 'external', $2, $3, $4, $1, 'hash') RETURNING id",
    )
    .bind(cid)
    .bind(ws)
    .bind(wh)
    .bind(role)
    .fetch_one(pool)
    .await
    .unwrap();
    (client, ws)
}

async fn count_sas(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM service_accounts")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn backfill_gives_each_client_its_own_ledgered_workspace_sa_and_is_idempotent() {
    let Some(dsn) = egret_test_db::url().await else {
        eprintln!("DATABASE_URL not set — skipping backfill_catalog_sa_db");
        return;
    };
    let db = egret_metastore::Db::connect(&dsn, 5)
        .await
        .expect("connect");
    db.migrate().await.expect("migrate");
    let pool = sqlx::PgPool::connect(&dsn).await.expect("pool");

    let (c1, ws1) = seed_client(&pool).await;
    let (c2, ws2) = seed_client(&pool).await;
    assert_ne!(ws1, ws2);
    let before = count_sas(&pool).await;

    let report = backfill_catalog_client_sa(&pool).await.expect("backfill");
    // Other tests may leave NULL clients in the shared DB; ours are among them.
    assert!(report.created >= 2, "{report:?}");

    let mut sas = Vec::new();
    for (client, ws) in [(c1, ws1), (c2, ws2)] {
        let sa: Uuid = sqlx::query_scalar(
            "SELECT service_account_id FROM catalog.catalog_clients WHERE id = $1",
        )
        .bind(client)
        .fetch_one(&pool)
        .await
        .unwrap();
        let (scope, sa_ws, name): (String, Option<Uuid>, String) = sqlx::query_as(
            "SELECT scope, workspace_id, name FROM service_accounts WHERE user_id = $1",
        )
        .bind(sa)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(scope, "workspace");
        assert_eq!(sa_ws, Some(ws), "SA belongs to the client's workspace");
        assert!(name.starts_with("catalog:"), "{name}");
        let (provider, username): (String, String) =
            sqlx::query_as("SELECT provider, username::text FROM users WHERE id = $1")
                .bind(sa)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(provider, "service");
        assert!(username.starts_with("svc-"), "{username}");
        let ledgered: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM username_ledger WHERE username = $1 AND user_id = $2",
        )
        .bind(&username)
        .bind(sa)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(ledgered, 1, "username must be in the ledger");
        sas.push(sa);
    }
    assert_ne!(sas[0], sas[1], "distinct SA per client");
    assert_eq!(count_sas(&pool).await - before, report.created as i64);

    // Re-run: a no-op for everything already owned.
    let after_first = count_sas(&pool).await;
    let again = backfill_catalog_client_sa(&pool).await.expect("re-run");
    assert_eq!(again, BackfillReport::default());
    assert_eq!(count_sas(&pool).await, after_first);
    let sa1_again: Uuid =
        sqlx::query_scalar("SELECT service_account_id FROM catalog.catalog_clients WHERE id = $1")
            .bind(c1)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(sa1_again, sas[0]);
}
