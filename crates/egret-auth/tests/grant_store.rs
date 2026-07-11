//! DB-gated integration test for `AccessGrantStore` (Effort B / B1a Task 5).
//! Self-skips without DATABASE_URL.
//!
//! Model: `crates/egret-workspace/tests/workspace_db.rs`.

use egret_auth::grants::AccessGrantStore;
use egret_core::{UserId, WorkspaceId};
use egret_metastore::Db;
use uuid::Uuid;

async fn connect() -> Option<Db> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

/// Insert a fresh local user and return its id.
async fn seed_user(db: &Db) -> UserId {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, email, name) VALUES ('local', $1, 'T') RETURNING id",
    )
    .bind(format!("{}@test.local", Uuid::new_v4()))
    .fetch_one(&db.pool)
    .await
    .unwrap();
    UserId(id)
}

async fn seed_workspace(db: &Db) -> WorkspaceId {
    let id: Uuid =
        sqlx::query_scalar("INSERT INTO workspaces (slug, name) VALUES ($1, 'W') RETURNING id")
            .bind(format!("ws-{}", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .unwrap();
    WorkspaceId(id)
}

#[tokio::test]
async fn grants_for_returns_platform_and_scoped_workspace_grants() {
    let Some(db) = connect().await else {
        eprintln!("skip: DATABASE_URL unset");
        return;
    };

    let user_id = seed_user(&db).await;
    let ws_id = seed_workspace(&db).await;
    let other_ws = seed_workspace(&db).await;

    // Platform grant written directly to the unified access_grants store.
    let platform_role_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM roles WHERE scope = 'platform' AND name = 'data_steward'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("seeded platform role `data_steward` must exist");
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user', $1, $2, 'platform', NULL) ON CONFLICT DO NOTHING",
    )
    .bind(user_id.0)
    .bind(platform_role_id)
    .execute(&db.pool)
    .await
    .unwrap();

    // Workspace grant written directly (role NAME -> role id). `ws_admin` is a
    // seeded workspace-scoped role (migration 0003).
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         SELECT 'user', $2, r.id, 'workspace', $1 FROM roles r \
         WHERE r.scope = 'workspace' AND r.name = 'ws_admin' \
         ON CONFLICT DO NOTHING",
    )
    .bind(ws_id.0)
    .bind(user_id.0)
    .execute(&db.pool)
    .await
    .unwrap();

    let store = AccessGrantStore::new(db.clone());
    let g = store.grants_for(user_id, Some(ws_id), &[]).await.unwrap();
    assert!(g.iter().any(|x| x.scope_type == "platform"));
    assert!(g
        .iter()
        .any(|x| x.scope_type == "workspace" && x.scope_id == Some(ws_id.0)));

    // A workspace the user is not in returns no workspace grants.
    let g2 = store
        .grants_for(user_id, Some(other_ws), &[])
        .await
        .unwrap();
    assert!(g2.iter().all(|x| x.scope_type != "workspace"));
    // Platform grants still show up regardless of workspace.
    assert!(g2.iter().any(|x| x.scope_type == "platform"));

    // With no workspace given at all, only platform grants come back.
    let g3 = store.grants_for(user_id, None, &[]).await.unwrap();
    assert!(g3.iter().all(|x| x.scope_type != "workspace"));
    assert!(g3.iter().any(|x| x.scope_type == "platform"));
}
