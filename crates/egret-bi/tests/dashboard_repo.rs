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
    db: Db,
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

    Some(Fixture {
        repo: DashboardRepo::new(db.clone()),
        charts: ChartRepo::new(db.clone()),
        ws: wid,
        dataset: ds.id,
        user: uid,
        db,
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
    let listed = f.repo.list_by_workspace(f.ws, false).await.unwrap();
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
    f.repo.replace_layout(d.id, &[], &cards).await.unwrap();

    let got = f.repo.cards(d.id).await.unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].ordinal, 0);
    assert_eq!(got[0].grid_col, 0);
    assert_eq!(got[1].grid_col, 6);
    assert_eq!(got[1].options, serde_json::json!({ "title": "copy" }));

    // card_count now reflects the placed cards
    let listed = f.repo.list_by_workspace(f.ws, false).await.unwrap();
    assert_eq!(listed[0].1, 2);

    // replace_layout is a full swap, not an append
    f.repo
        .replace_layout(
            d.id,
            &[],
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
        .update(d.id, "Ops v2", "", &serde_json::json!([]), "full")
        .await
        .unwrap();
    assert_eq!(f.repo.get(d.id).await.unwrap().unwrap().name, "Ops v2");
    assert_eq!(f.repo.dashboard_workspace(d.id).await.unwrap(), Some(f.ws));
    assert_eq!(f.repo.owner(d.id).await.unwrap(), Some(f.user));

    // parameters round-trip
    let dash_params = serde_json::json!([{ "id": "p1", "name": "Region", "slug": "region", "type": "text", "operator": "is" }]);
    f.repo
        .update(d.id, "Ops v2", "", &dash_params, "full")
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
    let Some(f) = setup().await else {
        eprintln!("skip");
        return;
    };
    let params = serde_json::json!({ "spec": { "metrics": ["revenue"], "row_limit": 100 } });
    let chart = f
        .charts
        .create(f.ws, f.dataset, "Sales", "", "bar", &params, Some(f.user))
        .await
        .unwrap();
    let d = f
        .repo
        .create(f.ws, "Tabbed", "", Some(f.user))
        .await
        .unwrap();

    let tabs = vec![
        NewTab {
            name: "Overview".into(),
            position: 0,
        },
        NewTab {
            name: "Detail".into(),
            position: 1,
        },
    ];
    let cards = vec![
        NewCard {
            chart_id: Some(chart.id),
            card_type: "chart".into(),
            grid_col: 0,
            grid_row: 0,
            size_x: 6,
            size_y: 4,
            options: serde_json::json!({}),
            tab_index: Some(0),
        },
        NewCard {
            chart_id: Some(chart.id),
            card_type: "chart".into(),
            grid_col: 0,
            grid_row: 0,
            size_x: 6,
            size_y: 4,
            options: serde_json::json!({}),
            tab_index: Some(1),
        },
    ];
    f.repo.replace_layout(d.id, &tabs, &cards).await.unwrap();

    let got_tabs = f.repo.tabs(d.id).await.unwrap();
    assert_eq!(
        got_tabs.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        vec!["Overview", "Detail"]
    );
    let got_cards = f.repo.cards(d.id).await.unwrap();
    assert_eq!(got_cards.len(), 2);
    assert_eq!(got_cards[0].tab_id, Some(got_tabs[0].id));
    assert_eq!(got_cards[1].tab_id, Some(got_tabs[1].id));

    f.repo
        .replace_layout(
            d.id,
            &[],
            &[NewCard {
                chart_id: Some(chart.id),
                card_type: "chart".into(),
                grid_col: 0,
                grid_row: 0,
                size_x: 6,
                size_y: 4,
                options: serde_json::json!({}),
                tab_index: None,
            }],
        )
        .await
        .unwrap();
    assert!(f.repo.tabs(d.id).await.unwrap().is_empty());
    assert_eq!(f.repo.cards(d.id).await.unwrap()[0].tab_id, None);

    let bad = f
        .repo
        .replace_layout(
            d.id,
            &[NewTab {
                name: "Only".into(),
                position: 0,
            }],
            &[NewCard {
                chart_id: Some(chart.id),
                card_type: "chart".into(),
                grid_col: 0,
                grid_row: 0,
                size_x: 6,
                size_y: 4,
                options: serde_json::json!({}),
                tab_index: Some(5),
            }],
        )
        .await;
    assert!(matches!(bad, Err(egret_bi::BiError::BadRequest(_))));

    f.repo
        .replace_layout(
            d.id,
            &[NewTab {
                name: "T".into(),
                position: 0,
            }],
            &[NewCard {
                chart_id: Some(chart.id),
                card_type: "chart".into(),
                grid_col: 0,
                grid_row: 0,
                size_x: 6,
                size_y: 4,
                options: serde_json::json!({}),
                tab_index: Some(0),
            }],
        )
        .await
        .unwrap();
    f.repo.replace_layout(d.id, &[], &[]).await.unwrap();
    assert!(f.repo.cards(d.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn dashboard_width_defaults_full_and_round_trips() {
    let Some(f) = setup().await else {
        eprintln!("skip");
        return;
    };
    let d = f.repo.create(f.ws, "Wide", "", Some(f.user)).await.unwrap();
    assert_eq!(f.repo.get(d.id).await.unwrap().unwrap().width, "full");

    f.repo
        .update(d.id, "Wide", "", &serde_json::json!([]), "fixed")
        .await
        .unwrap();
    assert_eq!(f.repo.get(d.id).await.unwrap().unwrap().width, "fixed");
}

#[tokio::test]
async fn trash_hides_from_live_list_and_restore_brings_back() {
    let Some(fx) = setup().await else { return };
    let d = fx
        .repo
        .create(fx.ws, "ToTrash", "", Some(fx.user))
        .await
        .unwrap();

    fx.repo.set_trashed(d.id, true).await.unwrap();
    let live: Vec<_> = fx.repo.list_by_workspace(fx.ws, false).await.unwrap();
    assert!(
        live.iter().all(|(x, _)| x.id != d.id),
        "trashed must be hidden from live list"
    );
    let trashed: Vec<_> = fx.repo.list_by_workspace(fx.ws, true).await.unwrap();
    assert!(
        trashed.iter().any(|(x, _)| x.id == d.id),
        "trash list must include it"
    );

    fx.repo.set_trashed(d.id, false).await.unwrap();
    let live2: Vec<_> = fx.repo.list_by_workspace(fx.ws, false).await.unwrap();
    assert!(
        live2.iter().any(|(x, _)| x.id == d.id),
        "restore returns it to live list"
    );
}

#[tokio::test]
async fn duplicate_copies_tabs_and_cards_with_new_owner() {
    let Some(fx) = setup().await else { return };
    let src = fx
        .repo
        .create(fx.ws, "Orig", "desc", Some(fx.user))
        .await
        .unwrap();
    let tabs = vec![
        NewTab {
            name: "T1".into(),
            position: 0,
        },
        NewTab {
            name: "T2".into(),
            position: 1,
        },
    ];
    let cards = vec![
        NewCard {
            chart_id: None,
            card_type: "text".into(),
            grid_col: 0,
            grid_row: 0,
            size_x: 4,
            size_y: 2,
            options: serde_json::json!({"text":"hi"}),
            tab_index: Some(0),
        },
        NewCard {
            chart_id: None,
            card_type: "heading".into(),
            grid_col: 0,
            grid_row: 2,
            size_x: 4,
            size_y: 1,
            options: serde_json::json!({"text":"h"}),
            tab_index: Some(1),
        },
    ];
    fx.repo.replace_layout(src.id, &tabs, &cards).await.unwrap();

    let other: Uuid =
        sqlx::query_scalar("INSERT INTO users (email, provider) VALUES ($1,'local') RETURNING id")
            .bind(format!("u_{}@t.local", Uuid::new_v4().simple()))
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    let copy = fx
        .repo
        .duplicate(src.id, Some(other))
        .await
        .unwrap()
        .expect("copy");

    assert_ne!(copy.id, src.id);
    assert_eq!(copy.name, "Orig (copy)");
    assert_eq!(copy.created_by, Some(other));
    assert_eq!(fx.repo.tabs(copy.id).await.unwrap().len(), 2);
    let copy_cards = fx.repo.cards(copy.id).await.unwrap();
    assert_eq!(copy_cards.len(), 2);
    let copy_tabs = fx.repo.tabs(copy.id).await.unwrap();
    let copy_tab_ids: std::collections::HashSet<Uuid> = copy_tabs.iter().map(|t| t.id).collect();
    let card_tab_ids: Vec<Uuid> = copy_cards.iter().filter_map(|c| c.tab_id).collect();
    assert_eq!(card_tab_ids.len(), 2, "both copy cards should be on a tab");
    for t in &card_tab_ids {
        assert!(copy_tab_ids.contains(t), "card tab must belong to the copy");
    }
    let distinct: std::collections::HashSet<Uuid> = card_tab_ids.iter().copied().collect();
    assert_eq!(
        distinct.len(),
        2,
        "the two cards must map to two distinct copy tabs (remap not collapsed)"
    );
}

#[tokio::test]
async fn bookmark_toggles_and_shows_in_list_flag() {
    let Some(fx) = setup().await else { return };
    let d = fx
        .repo
        .create(fx.ws, "Fav", "", Some(fx.user))
        .await
        .unwrap();

    fx.repo
        .add_bookmark(fx.user, "dashboard", d.id)
        .await
        .unwrap();
    fx.repo
        .add_bookmark(fx.user, "dashboard", d.id)
        .await
        .unwrap(); // idempotent
    let listed = fx
        .repo
        .list_by_workspace_for(fx.ws, fx.user, false)
        .await
        .unwrap();
    let row = listed
        .iter()
        .find(|(x, _, _)| x.id == d.id)
        .expect("listed");
    assert!(row.2, "bookmarked flag should be true");

    fx.repo
        .remove_bookmark(fx.user, "dashboard", d.id)
        .await
        .unwrap();
    let listed2 = fx
        .repo
        .list_by_workspace_for(fx.ws, fx.user, false)
        .await
        .unwrap();
    let row2 = listed2
        .iter()
        .find(|(x, _, _)| x.id == d.id)
        .expect("listed");
    assert!(!row2.2, "bookmarked flag should be false after removal");
}

#[tokio::test]
async fn permanent_delete_purges_bookmarks() {
    let Some(fx) = setup().await else { return };
    let d = fx
        .repo
        .create(fx.ws, "Doomed", "", Some(fx.user))
        .await
        .unwrap();
    fx.repo
        .add_bookmark(fx.user, "dashboard", d.id)
        .await
        .unwrap();

    // sanity: bookmark exists
    let before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM analytics.bookmarks WHERE entity_type='dashboard' AND entity_id=$1",
    )
    .bind(d.id)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(before, 1, "bookmark should exist before delete");

    fx.repo.delete(d.id).await.unwrap();

    let after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM analytics.bookmarks WHERE entity_type='dashboard' AND entity_id=$1",
    )
    .bind(d.id)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(
        after, 0,
        "permanent delete must purge the dashboard's bookmark rows"
    );
}
