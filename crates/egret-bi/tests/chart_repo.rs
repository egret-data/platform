//! DB-gated ChartRepo tests. Self-skip without DATABASE_URL.
use egret_bi::{ChartRepo, DatasetRepo, SourceSpec};
use egret_metastore::Db;
use uuid::Uuid;

async fn setup() -> Option<(ChartRepo, Uuid, Uuid, Uuid, Uuid)> {
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
        "INSERT INTO query.databases (name, engine, host, port, workspace_id) VALUES ($1,'trino','localhost',8080,$2) RETURNING id",
    )
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(wid)
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
    let ds2 = datasets
        .create(
            wid,
            db_id,
            "customers",
            "",
            &SourceSpec::Physical {
                catalog: Some("tpch".into()),
                schema: "sf1".into(),
                table: "customer".into(),
            },
            &[],
            Some(egret_core::UserId(uid)),
        )
        .await
        .unwrap();

    Some((ChartRepo::new(db), wid, ds.id, ds2.id, uid))
}

#[tokio::test]
async fn chart_repo_crud_and_unique_conflict() {
    let Some((repo, ws, ds, ds2, user)) = setup().await else {
        eprintln!("skip");
        return;
    };

    let params = serde_json::json!({ "spec": { "metrics": ["revenue"], "row_limit": 100 } });
    let c = repo
        .create(ws, ds, "Sales", "", "table", &params, Some(user))
        .await
        .unwrap();
    assert_eq!(c.name, "Sales");
    assert_eq!(c.viz_type, "table");

    let got = repo.get(c.id, ws).await.unwrap().unwrap();
    assert_eq!(got.dataset_id, ds);

    let listed = repo.list_by_workspace(ws).await.unwrap();
    assert_eq!(listed.len(), 1);

    // update can also re-point the chart at a different dataset
    repo.update(c.id, ds2, "Sales v2", "desc", "pivot_table", &params)
        .await
        .unwrap();
    let after = repo.get(c.id, ws).await.unwrap().unwrap();
    assert_eq!(after.name, "Sales v2");
    assert_eq!(after.viz_type, "pivot_table");
    assert_eq!(after.dataset_id, ds2);

    assert_eq!(repo.chart_workspace(c.id).await.unwrap(), Some(ws));
    assert_eq!(repo.owner(c.id).await.unwrap(), Some(user));

    // duplicate name → Conflict
    let dup = repo
        .create(ws, ds, "Sales v2", "", "table", &params, Some(user))
        .await;
    assert!(matches!(dup, Err(egret_bi::BiError::Conflict(_))));

    repo.delete(c.id).await.unwrap();
    assert!(repo.get(c.id, ws).await.unwrap().is_none());
}

/// The defect this repo change exists to close, proven directly rather than
/// through a caller: `.superpowers/sdd/chart-target-scoping-probe.md` drove a
/// `ws_admin` of workspace 1, holding no grant at all on workspace 2, to a 39-byte
/// attachment of workspace 2's row because `ChartRepo::get` was `WHERE id = $1` —
/// existence-only, no workspace predicate. `get` now takes the workspace as a
/// second, required parameter so a caller cannot forget it.
#[tokio::test]
async fn get_is_scoped_by_workspace_not_id_alone() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        eprintln!("skip");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");

    let uid: Uuid =
        sqlx::query_scalar("INSERT INTO users (email, provider) VALUES ($1,'local') RETURNING id")
            .bind(format!("u_{}@t.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let ws1: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug,name,created_by) VALUES ($1,'t1',$2) RETURNING id",
    )
    .bind(format!("w1-{}", Uuid::new_v4().simple()))
    .bind(uid)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let ws2: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug,name,created_by) VALUES ($1,'t2',$2) RETURNING id",
    )
    .bind(format!("w2-{}", Uuid::new_v4().simple()))
    .bind(uid)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let db_id: Uuid = sqlx::query_scalar(
        "INSERT INTO query.databases (name, engine, host, port, workspace_id) VALUES ($1,'trino','localhost',8080,$2) RETURNING id",
    )
    .bind(format!("db-{}", Uuid::new_v4().simple()))
    .bind(ws2)
    .fetch_one(&db.pool)
    .await
    .unwrap();

    let datasets = DatasetRepo::new(db.clone());
    let ds2 = datasets
        .create(
            ws2,
            db_id,
            "secret",
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

    let repo = ChartRepo::new(db.clone());
    let params = serde_json::json!({ "spec": { "metrics": ["revenue"], "row_limit": 100 } });
    let chart = repo
        .create(
            ws2,
            ds2.id,
            "ws2 secret chart",
            "",
            "table",
            &params,
            Some(uid),
        )
        .await
        .unwrap();

    // A chart that belongs to workspace 2 must not resolve under workspace 1's
    // scope, even though the id alone is a perfectly valid, existing row.
    assert!(
        repo.get(chart.id, ws1).await.unwrap().is_none(),
        "a workspace-2 chart resolved under workspace 1's scope"
    );
    // The chart's own workspace still resolves it — this is not a blanket
    // refusal, only a scoped one.
    let got = repo
        .get(chart.id, ws2)
        .await
        .unwrap()
        .expect("workspace 2 must still resolve its own chart");
    assert_eq!(got.id, chart.id);
}
