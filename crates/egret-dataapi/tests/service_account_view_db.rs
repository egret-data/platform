//! SA1: `dataapi.service_accounts` is a compatibility view over the
//! `service_accounts` principal table. DB-gated like `clients_store.rs`.
use uuid::Uuid;

#[tokio::test]
async fn the_view_is_a_drop_in_over_the_principal_table() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let db = egret_metastore::Db::connect(&url, 5)
        .await
        .expect("connect");
    let pool = &db.pool;

    let admin: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, name, provider, status, password_hash) \
         VALUES ($1,'admin','local','active','x') RETURNING id",
    )
    .bind(format!("admin_{}@t.local", Uuid::new_v4().simple()))
    .fetch_one(pool)
    .await
    .unwrap();
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by) VALUES ($1,'t',$2) RETURNING id",
    )
    .bind(format!("w-{}", Uuid::new_v4().simple()))
    .bind(admin)
    .fetch_one(pool)
    .await
    .unwrap();
    let sa: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, name, provider, status) \
         VALUES ($1,'svc','service','active') RETURNING id",
    )
    .bind(format!("svc_{}@t.local", Uuid::new_v4().simple()))
    .fetch_one(pool)
    .await
    .unwrap();

    // 1. INSERT through the view works and RETURNING created_at is filled.
    let created_at: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "INSERT INTO dataapi.service_accounts (user_id, workspace_id, name, description, created_by) \
         VALUES ($1,$2,'svc','d',$3) RETURNING created_at",
    )
    .bind(sa)
    .bind(ws)
    .bind(admin)
    .fetch_one(pool)
    .await
    .expect("insert via view");
    assert!(created_at.is_some(), "trigger must fill created_at");

    // 2. Visible via the view and via the base table as a workspace SA.
    let via_view: i64 =
        sqlx::query_scalar("SELECT count(*) FROM dataapi.service_accounts WHERE workspace_id=$1")
            .bind(ws)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(via_view, 1);
    let (scope, base_ws): (String, Option<Uuid>) =
        sqlx::query_as("SELECT scope, workspace_id FROM service_accounts WHERE user_id=$1")
            .bind(sa)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(scope, "workspace");
    assert_eq!(base_ws, Some(ws));

    // 3. A dataapi.clients row can FK-reference it (FK now targets the base table).
    sqlx::query(
        "INSERT INTO dataapi.clients (service_account_id, name, client_id, client_secret_hash) \
         VALUES ($1,'c',$2,'h')",
    )
    .bind(sa)
    .bind(format!("cid-{}", Uuid::new_v4().simple()))
    .execute(pool)
    .await
    .expect("client insert");

    // 4. Deleting the users row cascades the service account away.
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(sa)
        .execute(pool)
        .await
        .unwrap();
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM service_accounts WHERE user_id=$1")
        .bind(sa)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
}
