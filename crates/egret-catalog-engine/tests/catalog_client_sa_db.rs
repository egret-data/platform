//! SA2 task 1: `catalog.catalog_clients` gains a nullable owning service account
//! plus `revoked_at` / `expires_at` (migration 0230).
//!
//! DB-gated; self-skips only under `EGRET_ALLOW_DB_SKIP`, and otherwise FAILS.

use uuid::Uuid;

async fn insert_client(
    pool: &sqlx::PgPool,
    ws: Uuid,
    wh: Uuid,
    role: Uuid,
    sa: Option<Uuid>,
) -> Result<Uuid, sqlx::Error> {
    let cid = format!("cc-{}", Uuid::new_v4().simple());
    sqlx::query_scalar(
        "INSERT INTO catalog.catalog_clients \
           (name, consumer_type, workspace_id, warehouse_id, catalog_role_id, \
            client_id, client_secret_hash, service_account_id) \
         VALUES ($1, 'external', $2, $3, $4, $1, 'hash', $5) RETURNING id",
    )
    .bind(cid)
    .bind(ws)
    .bind(wh)
    .bind(role)
    .bind(sa)
    .fetch_one(pool)
    .await
}

#[tokio::test]
async fn catalog_client_owner_and_lifecycle_columns() {
    let Some(dsn) = egret_test_db::url().await else {
        eprintln!("DATABASE_URL not set — skipping catalog_client_owner_and_lifecycle_columns");
        return;
    };
    let db = egret_metastore::Db::connect(&dsn, 5)
        .await
        .expect("connect");
    db.migrate().await.expect("migrate");
    let pool = sqlx::PgPool::connect(&dsn).await.expect("pool");

    let wh = egret_test_db::seed_warehouse(&pool).await;
    let ws: Uuid = sqlx::query_scalar("SELECT workspace_id FROM catalog.warehouses WHERE id = $1")
        .bind(wh)
        .fetch_one(&pool)
        .await
        .unwrap();
    let role: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.catalog_roles (warehouse_id, name, consumer_type) \
         VALUES ($1, 'r', 'external') RETURNING id",
    )
    .bind(wh)
    .fetch_one(&pool)
    .await
    .unwrap();

    // A client with no owner is allowed (backfilled later; verifier fails closed).
    let orphan = insert_client(&pool, ws, wh, role, None)
        .await
        .expect("NULL service_account_id is allowed");
    let (sa_col, revoked, expires): (Option<Uuid>, Option<String>, Option<String>) =
        sqlx::query_as(
            "SELECT service_account_id, revoked_at::text, expires_at::text \
             FROM catalog.catalog_clients WHERE id = $1",
        )
        .bind(orphan)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(sa_col.is_none() && revoked.is_none() && expires.is_none());

    // Seed a service account: a users row with provider='service', then the SA row.
    let suffix = Uuid::new_v4().simple().to_string();
    let sa: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ('service', $1, $2) RETURNING id",
    )
    .bind(format!("sa-{suffix}@service.invalid"))
    .bind(format!("sa-{suffix}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO service_accounts (user_id, scope, workspace_id, name) \
         VALUES ($1, 'workspace', $2, $3)",
    )
    .bind(sa)
    .bind(ws)
    .bind(format!("sa-{suffix}"))
    .execute(&pool)
    .await
    .expect("seed service account");

    let owned = insert_client(&pool, ws, wh, role, Some(sa))
        .await
        .expect("client owned by a real SA");
    let owner: Option<Uuid> =
        sqlx::query_scalar("SELECT service_account_id FROM catalog.catalog_clients WHERE id = $1")
            .bind(owned)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(owner, Some(sa));

    // A bogus owner violates the FK.
    let err = insert_client(&pool, ws, wh, role, Some(Uuid::new_v4()))
        .await
        .expect_err("bogus service_account_id must be refused");
    let code = err
        .as_database_error()
        .and_then(|e| e.code().map(|c| c.to_string()));
    assert_eq!(code.as_deref(), Some("23503"), "foreign_key_violation");

    // Deleting the SA cascades to its clients.
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(sa)
        .execute(&pool)
        .await
        .unwrap();
    let left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM catalog.catalog_clients WHERE id = $1")
            .bind(owned)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(left, 0, "ON DELETE CASCADE");
}
