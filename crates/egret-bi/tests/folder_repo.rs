//! DB-gated FolderRepo tests. Self-skip without DATABASE_URL.
use egret_metastore::Db;
use uuid::Uuid;

async fn setup() -> Option<(Db, Uuid, Uuid)> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let uid: Uuid =
        sqlx::query_scalar("INSERT INTO users (email, provider) VALUES ($1,'local') RETURNING id")
            .bind(format!("u_{}@t.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let wid: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug,name,created_by) VALUES ($1,'t',$2) RETURNING id",
    )
    .bind(format!("w-{}", Uuid::new_v4().simple()))
    .bind(uid)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    Some((db, wid, uid))
}

#[tokio::test]
async fn folder_crud_and_unique_conflict() {
    let Some((db, ws, uid)) = setup().await else {
        eprintln!("skip");
        return;
    };

    let repo = egret_bi::folder::FolderRepo::new(db.clone());
    let f = repo
        .create(ws, "chart", "Revenue", Some(uid))
        .await
        .unwrap();
    assert_eq!(f.name, "Revenue");
    let dup = repo.create(ws, "chart", "Revenue", Some(uid)).await;
    assert!(matches!(dup, Err(egret_bi::BiError::Conflict(_))));
    repo.create(ws, "dashboard", "Revenue", Some(uid))
        .await
        .unwrap(); // same name, other type OK
    let charts = repo.list(ws, "chart").await.unwrap();
    assert_eq!(charts.len(), 1);
    let meta = repo.meta(f.id).await.unwrap().unwrap();
    assert_eq!(meta, (ws, "chart".to_string()));
    repo.rename(f.id, "Revenue 2024").await.unwrap();
    assert_eq!(
        repo.list(ws, "chart").await.unwrap()[0].name,
        "Revenue 2024"
    );
    repo.delete(f.id).await.unwrap();
    assert!(repo.list(ws, "chart").await.unwrap().is_empty());
}
