//! DB-gated ChartRepo trash + bookmark tests. Self-skip without DATABASE_URL.
use egret_bi::{ChartRepo, DatasetRepo, SourceSpec};
use egret_metastore::Db;
use uuid::Uuid;

async fn setup() -> Option<(ChartRepo, Uuid, Uuid, Uuid)> {
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
    let db_id: Uuid = sqlx::query_scalar(
        "INSERT INTO databases (name, engine, host, port) VALUES ($1,'trino','localhost',8080) RETURNING id",
    )
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .unwrap();

    let datasets = DatasetRepo::new(db.clone());
    let ds = datasets
        .create(
            wid,
            db_id,
            "orders",
            "",
            &SourceSpec::Physical {
                catalog: Some("tpch".into()),
                schema: "sf1".into(),
                table: "orders".into(),
            },
            &[],
            Some(egret_core::UserId(uid)),
        )
        .await
        .unwrap();

    Some((ChartRepo::new(db), wid, ds.id, uid))
}

#[tokio::test]
async fn chart_trash_and_bookmark_flow() {
    let Some((repo, ws, ds, user)) = setup().await else {
        eprintln!("skip");
        return;
    };

    let params = serde_json::json!({ "spec": { "metrics": ["revenue"] } });
    let c = repo
        .create(ws, ds, "Sales", "", "table", &params, Some(user))
        .await
        .unwrap();

    // Live list includes it, unbookmarked.
    let live = repo.list_by_workspace_for(ws, user, false).await.unwrap();
    let found = live.iter().find(|(ch, _)| ch.id == c.id).expect("in live");
    assert!(!found.1, "fresh chart is not bookmarked");

    // Trash → excluded from live, present in trash bin.
    repo.set_trashed(c.id, true).await.unwrap();
    let live = repo.list_by_workspace_for(ws, user, false).await.unwrap();
    assert!(
        !live.iter().any(|(ch, _)| ch.id == c.id),
        "trashed chart hidden from live"
    );
    let trash = repo.list_by_workspace_for(ws, user, true).await.unwrap();
    assert!(
        trash.iter().any(|(ch, _)| ch.id == c.id),
        "trashed chart in trash list"
    );

    // Restore → back in live.
    repo.set_trashed(c.id, false).await.unwrap();
    let live = repo.list_by_workspace_for(ws, user, false).await.unwrap();
    assert!(
        live.iter().any(|(ch, _)| ch.id == c.id),
        "restored chart back in live"
    );

    // Bookmark → flag true in live; remove → false.
    repo.add_bookmark(user, c.id).await.unwrap();
    repo.add_bookmark(user, c.id).await.unwrap(); // idempotent (ON CONFLICT DO NOTHING)
    let live = repo.list_by_workspace_for(ws, user, false).await.unwrap();
    assert!(
        live.iter().find(|(ch, _)| ch.id == c.id).unwrap().1,
        "bookmarked after add"
    );

    repo.remove_bookmark(user, c.id).await.unwrap();
    let live = repo.list_by_workspace_for(ws, user, false).await.unwrap();
    assert!(
        !live.iter().find(|(ch, _)| ch.id == c.id).unwrap().1,
        "not bookmarked after remove"
    );
}
