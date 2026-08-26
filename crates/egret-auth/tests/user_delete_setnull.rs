//! Migration 0093: deleting a user SET-NULLs attribution columns (dashboard
//! created_by here), so content survives the user's removal.
#[sqlx::test(migrations = "../../migrations")]
async fn deleting_user_nulls_dashboard_created_by(pool: sqlx::PgPool) {
    let uid = sqlx::query_scalar::<_, uuid::Uuid>(
        "INSERT INTO users (provider, email, name) VALUES ('local','u@egret.local','U') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let wid = sqlx::query_scalar::<_, uuid::Uuid>(
        "INSERT INTO workspaces (slug, name, created_by) VALUES ('udr-test-ws','UDR Test WS', $1) RETURNING id",
    )
    .bind(uid)
    .fetch_one(&pool)
    .await
    .unwrap();

    let did = sqlx::query_scalar::<_, uuid::Uuid>(
        "INSERT INTO analytics.dashboards (workspace_id, name, created_by) VALUES ($1, 'UDR Test Dashboard', $2) RETURNING id",
    )
    .bind(wid)
    .bind(uid)
    .fetch_one(&pool)
    .await
    .unwrap();

    // The user owns no notebook, so the NOT-NULL notebooks.owner_user_id FK
    // (handled separately in Task 2 via reassignment) does not block this delete.
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(uid)
        .execute(&pool)
        .await
        .unwrap();

    let created_by: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT created_by FROM analytics.dashboards WHERE id = $1")
            .bind(did)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        created_by.is_none(),
        "dashboard.created_by must be SET NULL after owning user is deleted"
    );
}

/// `#[sqlx::test]` resolves `DATABASE_URL` through dotenvy, so an unexported
/// variable does not skip these tests — it silently points them at the database
/// `.env` names, which is the DEV one. See
/// `egret_test_db::assert_database_url_is_exported`.
#[test]
fn database_url_is_exported_rather_than_read_from_dotenv() {
    egret_test_db::assert_database_url_is_exported();
}
