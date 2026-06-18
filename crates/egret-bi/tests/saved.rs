//! DB-gated SavedQueryRepo tests. Self-skip without DATABASE_URL.
use egret_bi::SavedQueryRepo;
use egret_metastore::Db;
use uuid::Uuid;

async fn setup() -> Option<(SavedQueryRepo, Uuid, Uuid)> {
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
    Some((SavedQueryRepo::new(db), wid, uid))
}

#[tokio::test]
async fn crud_roundtrip() {
    let Some((r, wid, uid)) = setup().await else {
        eprintln!("skip");
        return;
    };
    let q = r
        .create(
            wid,
            None,
            "q1",
            "SELECT 1",
            "",
            Some(egret_core::UserId(uid)),
        )
        .await
        .unwrap();
    assert_eq!(q.name, "q1");
    assert!(r.list(wid).await.unwrap().iter().any(|s| s.id == q.id));
    let got = r.get(q.id).await.unwrap().unwrap();
    assert_eq!(got.sql, "SELECT 1");
    let upd = r.update(q.id, "q1b", "SELECT 2", "", None).await.unwrap();
    assert_eq!(upd.name, "q1b");
    assert_eq!(upd.sql, "SELECT 2");
    r.delete(q.id).await.unwrap();
    assert!(r.get(q.id).await.unwrap().is_none());
}

#[tokio::test]
async fn owner_returns_creator() {
    let Some((r, wid, uid)) = setup().await else {
        eprintln!("skip");
        return;
    };
    let q = r
        .create(
            wid,
            None,
            "q",
            "SELECT 1",
            "",
            Some(egret_core::UserId(uid)),
        )
        .await
        .unwrap();
    assert_eq!(r.owner(q.id).await.unwrap(), Some(uid));
    r.delete(q.id).await.unwrap();
}

#[tokio::test]
async fn description_roundtrips() {
    let Some((r, wid, uid)) = setup().await else {
        eprintln!("skip");
        return;
    };
    let q = r
        .create(
            wid,
            None,
            "q",
            "SELECT 1",
            "my desc",
            Some(egret_core::UserId(uid)),
        )
        .await
        .unwrap();
    assert_eq!(q.description, "my desc");
    let got = r.get(q.id).await.unwrap().unwrap();
    assert_eq!(got.description, "my desc");
    r.delete(q.id).await.unwrap();
}
