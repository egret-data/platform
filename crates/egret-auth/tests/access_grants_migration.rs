//! DB-gated integration test for the `access_grants` migration (Effort B / B1a
//! Task 4). Self-skips without DATABASE_URL. Verifies the expand-phase sync
//! triggers mirror legacy platform + workspace role writes into `access_grants`.
//!
//! Model: `crates/egret-workspace/tests/workspace_db.rs`.

use egret_metastore::Db;
use uuid::Uuid;

async fn connect() -> Option<Db> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

/// Insert a fresh local user and return its id.
async fn seed_user(db: &Db) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ('local', $1, 'T') RETURNING id",
    )
    .bind(format!("{}@test.local", Uuid::new_v4()))
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn triggers_mirror_platform_and_workspace_grants() {
    let Some(db) = connect().await else {
        eprintln!("skip: DATABASE_URL unset");
        return;
    };

    let user_id = seed_user(&db).await;

    // ── Platform grant via the legacy write path (trigger should mirror it) ──
    // Reuse a seeded platform-scoped role.
    let platform_role_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM roles WHERE scope = 'platform' AND name = 'data_steward'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("seeded platform role `data_steward` must exist");
    sqlx::query(
        "INSERT INTO platform_role_assignments (principal_type, principal_id, role_id) \
         VALUES ('user', $1, $2)",
    )
    .bind(user_id)
    .bind(platform_role_id)
    .execute(&db.pool)
    .await
    .unwrap();

    // ── Workspace grant via the legacy write path (role NAME -> role id) ──
    let ws_id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1, 'W') RETURNING id")
            .bind(format!("ws-{}", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .unwrap();
    // `ws_admin` is a seeded workspace-scoped role (migration 0003).
    let ws_admin_exists: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM roles WHERE scope = 'workspace' AND name = 'ws_admin'",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        ws_admin_exists, 1,
        "seeded workspace role `ws_admin` must exist"
    );
    sqlx::query(
        "INSERT INTO workspace_members (workspace_id, principal_type, principal_id, role) \
         VALUES ($1, 'user', $2, 'ws_admin')",
    )
    .bind(ws_id)
    .bind(user_id)
    .execute(&db.pool)
    .await
    .unwrap();

    // ── Assert the triggers mirrored both writes into access_grants ──
    let plat: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM access_grants WHERE principal_id = $1 AND scope_type = 'platform'",
    )
    .bind(user_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        plat, 1,
        "platform grant must be mirrored by sync_platform_grant"
    );

    let ws: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM access_grants \
         WHERE principal_id = $1 AND scope_type = 'workspace' AND scope_id = $2",
    )
    .bind(user_id)
    .bind(ws_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        ws, 1,
        "workspace grant must be mirrored by sync_workspace_grant"
    );

    // The mirrored workspace grant resolved the role NAME to the workspace-scoped
    // role id (the backfill JOIN + trigger lookup both rely on this mapping).
    let ws_role_id: Uuid = sqlx::query_scalar(
        "SELECT role_id FROM access_grants \
         WHERE principal_id = $1 AND scope_type = 'workspace' AND scope_id = $2",
    )
    .bind(user_id)
    .bind(ws_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let expected_role_id: Uuid =
        sqlx::query_scalar("SELECT id FROM roles WHERE scope = 'workspace' AND name = 'ws_admin'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        ws_role_id, expected_role_id,
        "role NAME must map to the ws-scoped role id"
    );

    // ── DELETE path: removing the legacy rows removes the mirror rows ──
    sqlx::query("DELETE FROM platform_role_assignments WHERE principal_id = $1 AND role_id = $2")
        .bind(user_id)
        .bind(platform_role_id)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM workspace_members WHERE workspace_id = $1 AND principal_id = $2")
        .bind(ws_id)
        .bind(user_id)
        .execute(&db.pool)
        .await
        .unwrap();
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM access_grants WHERE principal_id = $1")
            .bind(user_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(
        remaining, 0,
        "DELETE triggers must remove the mirrored grants"
    );
}
