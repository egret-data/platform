//! DB-gated DashboardRepo tests. Self-skip without DATABASE_URL.
use egret_bi::{ChartRepo, DashboardRepo, DatasetRepo, NewCard, NewTab, SourceSpec};
use egret_metastore::Db;
use uuid::Uuid;

struct Fixture {
    repo: DashboardRepo,
    charts: ChartRepo,
    ws: Uuid,
    dataset: Uuid,
    user: Uuid,
}

async fn setup() -> Option<Fixture> {
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

    Some(Fixture {
        repo: DashboardRepo::new(db.clone()),
        charts: ChartRepo::new(db),
        ws: wid,
        dataset: ds.id,
        user: uid,
    })
}

#[tokio::test]
async fn dashboard_crud_cards_and_unique_conflict() {
    let Some(f) = setup().await else {
        eprintln!("skip");
        return;
    };

    // create a chart to place on the dashboard
    let params = serde_json::json!({ "spec": { "metrics": ["revenue"], "row_limit": 100 } });
    let chart = f
        .charts
        .create(f.ws, f.dataset, "Sales", "", "bar", &params, Some(f.user))
        .await
        .unwrap();

    let d = f
        .repo
        .create(f.ws, "Ops", "overview", Some(f.user))
        .await
        .unwrap();
    assert_eq!(d.name, "Ops");
    assert_eq!(d.description, "overview");

    // list reports a card count of 0 before any cards are placed
    let listed = f.repo.list_by_workspace(f.ws).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].1, 0);

    // place two cards (one referencing the chart)
    let cards = vec![
        NewCard {
            chart_id: Some(chart.id),
            card_type: "chart".into(),
            grid_col: 0,
            grid_row: 0,
            size_x: 6,
            size_y: 4,
            options: serde_json::json!({}),
            tab_index: None,
        },
        NewCard {
            chart_id: Some(chart.id),
            card_type: "chart".into(),
            grid_col: 6,
            grid_row: 0,
            size_x: 6,
            size_y: 4,
            options: serde_json::json!({ "title": "copy" }),
            tab_index: None,
        },
    ];
    f.repo.replace_cards(d.id, &cards).await.unwrap();

    let got = f.repo.cards(d.id).await.unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].ordinal, 0);
    assert_eq!(got[0].grid_col, 0);
    assert_eq!(got[1].grid_col, 6);
    assert_eq!(got[1].options, serde_json::json!({ "title": "copy" }));

    // card_count now reflects the placed cards
    let listed = f.repo.list_by_workspace(f.ws).await.unwrap();
    assert_eq!(listed[0].1, 2);

    // replace_cards is a full swap, not an append
    f.repo
        .replace_cards(
            d.id,
            &[NewCard {
                chart_id: Some(chart.id),
                card_type: "chart".into(),
                grid_col: 0,
                grid_row: 0,
                size_x: 12,
                size_y: 8,
                options: serde_json::json!({}),
                tab_index: None,
            }],
        )
        .await
        .unwrap();
    assert_eq!(f.repo.cards(d.id).await.unwrap().len(), 1);

    // metadata update + guards
    f.repo
        .update(d.id, "Ops v2", "", &serde_json::json!([]))
        .await
        .unwrap();
    assert_eq!(f.repo.get(d.id).await.unwrap().unwrap().name, "Ops v2");
    assert_eq!(f.repo.dashboard_workspace(d.id).await.unwrap(), Some(f.ws));
    assert_eq!(f.repo.owner(d.id).await.unwrap(), Some(f.user));

    // parameters round-trip
    let dash_params = serde_json::json!([{ "id": "p1", "name": "Region", "slug": "region", "type": "text", "operator": "is" }]);
    f.repo
        .update(d.id, "Ops v2", "", &dash_params)
        .await
        .unwrap();
    assert_eq!(
        f.repo.get(d.id).await.unwrap().unwrap().parameters,
        dash_params
    );

    // duplicate name → Conflict
    let dup = f.repo.create(f.ws, "Ops v2", "", Some(f.user)).await;
    assert!(matches!(dup, Err(egret_bi::BiError::Conflict(_))));

    // deleting the dashboard cascades its cards
    f.repo.delete(d.id).await.unwrap();
    assert!(f.repo.get(d.id).await.unwrap().is_none());
    assert!(f.repo.cards(d.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn replace_layout_tabs_and_card_assignment() {
    let Some(f) = setup().await else { eprintln!("skip"); return; };
    let params = serde_json::json!({ "spec": { "metrics": ["revenue"], "row_limit": 100 } });
    let chart = f.charts.create(f.ws, f.dataset, "Sales", "", "bar", &params, Some(f.user)).await.unwrap();
    let d = f.repo.create(f.ws, "Tabbed", "", Some(f.user)).await.unwrap();

    let tabs = vec![
        NewTab { name: "Overview".into(), position: 0 },
        NewTab { name: "Detail".into(),   position: 1 },
    ];
    let cards = vec![
        NewCard { chart_id: Some(chart.id), card_type: "chart".into(), grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: serde_json::json!({}), tab_index: Some(0) },
        NewCard { chart_id: Some(chart.id), card_type: "chart".into(), grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: serde_json::json!({}), tab_index: Some(1) },
    ];
    f.repo.replace_layout(d.id, &tabs, &cards).await.unwrap();

    let got_tabs = f.repo.tabs(d.id).await.unwrap();
    assert_eq!(got_tabs.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), vec!["Overview", "Detail"]);
    let got_cards = f.repo.cards(d.id).await.unwrap();
    assert_eq!(got_cards.len(), 2);
    assert_eq!(got_cards[0].tab_id, Some(got_tabs[0].id));
    assert_eq!(got_cards[1].tab_id, Some(got_tabs[1].id));

    f.repo.replace_layout(d.id, &[], &[
        NewCard { chart_id: Some(chart.id), card_type: "chart".into(), grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: serde_json::json!({}), tab_index: None },
    ]).await.unwrap();
    assert!(f.repo.tabs(d.id).await.unwrap().is_empty());
    assert_eq!(f.repo.cards(d.id).await.unwrap()[0].tab_id, None);

    let bad = f.repo.replace_layout(d.id, &[NewTab { name: "Only".into(), position: 0 }], &[
        NewCard { chart_id: Some(chart.id), card_type: "chart".into(), grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: serde_json::json!({}), tab_index: Some(5) },
    ]).await;
    assert!(matches!(bad, Err(egret_bi::BiError::BadRequest(_))));

    f.repo.replace_layout(d.id, &[NewTab { name: "T".into(), position: 0 }], &[
        NewCard { chart_id: Some(chart.id), card_type: "chart".into(), grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: serde_json::json!({}), tab_index: Some(0) },
    ]).await.unwrap();
    f.repo.replace_layout(d.id, &[], &[]).await.unwrap();
    assert!(f.repo.cards(d.id).await.unwrap().is_empty());
}
