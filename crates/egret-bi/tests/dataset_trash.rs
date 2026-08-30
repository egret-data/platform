//! DB-gated DatasetRepo trash + bookmark tests. Self-skip without DATABASE_URL ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAIL.
use egret_bi::{DatasetRepo, NewColumn, SourceSpec};
use egret_metastore::Db;
use uuid::Uuid;

/// Returns (repo, workspace_id, database_id, user_id).
async fn setup() -> Option<(DatasetRepo, Uuid, Uuid, Uuid)> {
    let url = egret_test_db::url().await?;
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
        "INSERT INTO query.databases (name, engine, host, port, workspace_id) VALUES ($1,'trino','localhost',8080,$2) RETURNING id",
    )
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(wid)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    Some((DatasetRepo::new(db), wid, db_id, uid))
}

fn col(name: &str) -> NewColumn {
    NewColumn {
        column_name: name.into(),
        data_type: "bigint".into(),
        type_generic: "numeric".into(),
        is_temporal: false,
    }
}

#[tokio::test]
async fn dataset_trash_and_bookmark_flow() {
    let Some((repo, ws, db_id, user)) = setup().await else {
        eprintln!("skip");
        return;
    };

    // Create a dataset with two columns so column_count > 0.
    let ds = repo
        .create(
            ws,
            db_id,
            "orders",
            "",
            &SourceSpec::Physical {
                catalog: Some("tpch".into()),
                schema: "sf1".into(),
                table: "orders".into(),
            },
            &[col("o_orderkey"), col("o_custkey")],
            Some(egret_core::UserId(user)),
        )
        .await
        .unwrap();

    // Live list includes it, unbookmarked, with the right column count.
    let live = repo.list_by_workspace_for(ws, user, false).await.unwrap();
    let found = live
        .iter()
        .find(|(d, _, _)| d.id == ds.id)
        .expect("in live");
    assert!(!found.2, "fresh dataset is not bookmarked");
    assert_eq!(found.1, 2, "column_count reflects the two columns");

    // Trash → excluded from live, present in trash bin.
    repo.set_trashed(ds.id, true).await.unwrap();
    let live = repo.list_by_workspace_for(ws, user, false).await.unwrap();
    assert!(
        !live.iter().any(|(d, _, _)| d.id == ds.id),
        "trashed dataset hidden from live"
    );
    let trash = repo.list_by_workspace_for(ws, user, true).await.unwrap();
    assert!(
        trash.iter().any(|(d, _, _)| d.id == ds.id),
        "trashed dataset in trash list"
    );

    // Restore → back in live.
    repo.set_trashed(ds.id, false).await.unwrap();
    let live = repo.list_by_workspace_for(ws, user, false).await.unwrap();
    assert!(
        live.iter().any(|(d, _, _)| d.id == ds.id),
        "restored dataset back in live"
    );

    // Bookmark → flag true in live; remove → false.
    repo.add_bookmark(user, ds.id).await.unwrap();
    repo.add_bookmark(user, ds.id).await.unwrap(); // idempotent (ON CONFLICT DO NOTHING)
    let live = repo.list_by_workspace_for(ws, user, false).await.unwrap();
    assert!(
        live.iter().find(|(d, _, _)| d.id == ds.id).unwrap().2,
        "bookmarked after add"
    );

    repo.remove_bookmark(user, ds.id).await.unwrap();
    let live = repo.list_by_workspace_for(ws, user, false).await.unwrap();
    assert!(
        !live.iter().find(|(d, _, _)| d.id == ds.id).unwrap().2,
        "not bookmarked after remove"
    );
}
