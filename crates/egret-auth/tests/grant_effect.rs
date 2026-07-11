//! grants_for surfaces the effect column. Requires DATABASE_URL.
use egret_auth::grants::AccessGrantStore;
use egret_core::ids::{UserId, WorkspaceId};
use egret_metastore::Db;
use uuid::Uuid;

async fn db() -> Db {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    db
}

#[tokio::test]
async fn grants_for_returns_effect() {
    let db = db().await;
    let store = AccessGrantStore::new(db.clone());
    let user = UserId::new();
    let ws = WorkspaceId::new();
    let role: Uuid =
        sqlx::query_scalar("SELECT id FROM roles WHERE scope='workspace' AND name='ws_editor'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    sqlx::query("INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id, effect) VALUES ('user',$1,$2,'workspace',$3,'deny')")
        .bind(user.0).bind(role).bind(ws.0).execute(&db.pool).await.unwrap();
    let grants = store.grants_for(user, Some(ws), &[]).await.unwrap();
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].effect, "deny");
    sqlx::query("DELETE FROM access_grants WHERE principal_id=$1")
        .bind(user.0)
        .execute(&db.pool)
        .await
        .unwrap();
}
