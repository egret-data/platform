//! SQL snippet repo: CRUD + workspace scoping (no cross-tenant IDOR).
//! DB-gated; self-skips without DATABASE_URL. Exercises migration 0096.

use egret_bi::SnippetRepo;
use egret_core::UserId;
use egret_metastore::Db;
use uuid::Uuid;

async fn seed_ws(db: &Db) -> (Uuid, Uuid) {
    let uid: Uuid =
        sqlx::query_scalar("INSERT INTO users (provider, email) VALUES ('local', $1) RETURNING id")
            .bind(format!("snip_{}@egret.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .expect("insert user");
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by, status) \
         VALUES ($1, 'Snippet Test', $2, 'active') RETURNING id",
    )
    .bind(format!("snip-{}", Uuid::new_v4().simple()))
    .bind(uid)
    .fetch_one(&db.pool)
    .await
    .expect("insert workspace");
    (ws, uid)
}

#[tokio::test]
async fn snippet_crud_is_workspace_scoped() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let repo = SnippetRepo::new(db.clone());

    let (ws, uid) = seed_ws(&db).await;
    let (other_ws, _) = seed_ws(&db).await;

    // Create + list.
    let s = repo
        .create(ws, "greeting", "SELECT 1", "a hello", Some(UserId(uid)))
        .await
        .expect("create");
    assert!(repo
        .list(ws)
        .await
        .unwrap()
        .iter()
        .any(|x| x.id == s.id && x.name == "greeting"));

    // Update in the owning workspace succeeds.
    let up = repo
        .update(ws, s.id, "greeting2", "SELECT 2", "")
        .await
        .expect("update")
        .expect("row returned");
    assert_eq!(up.name, "greeting2");
    assert_eq!(up.body, "SELECT 2");

    // Update/delete from ANOTHER workspace is a no-op (IDOR-safe).
    assert!(repo
        .update(other_ws, s.id, "x", "y", "")
        .await
        .unwrap()
        .is_none());
    repo.delete(other_ws, s.id).await.unwrap();
    assert!(
        repo.list(ws).await.unwrap().iter().any(|x| x.id == s.id),
        "must survive cross-ws delete"
    );

    // Delete in the owning workspace removes it.
    repo.delete(ws, s.id).await.unwrap();
    assert!(!repo.list(ws).await.unwrap().iter().any(|x| x.id == s.id));
}
