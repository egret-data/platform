//! DB-gated dashboard revision tests. Self-skip without DATABASE_URL.
use egret_bi::{DashboardRepo, NewCard, NewTab};
use egret_metastore::Db;
use uuid::Uuid;

async fn setup() -> Option<(DashboardRepo, Uuid, Uuid)> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let uid: Uuid =
        sqlx::query_scalar("INSERT INTO users (email, provider) VALUES ($1,'local') RETURNING id")
            .bind(format!("u_{}@t.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug,name,created_by) VALUES ($1,'t',$2) RETURNING id",
    )
    .bind(format!("w-{}", Uuid::new_v4().simple()))
    .bind(uid)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    Some((DashboardRepo::new(db.clone()), ws, uid))
}

fn card(tab_index: Option<usize>, col: i32) -> NewCard {
    NewCard {
        id: None,
        chart_id: None,
        card_type: "text".into(),
        grid_col: col,
        grid_row: 0,
        size_x: 6,
        size_y: 4,
        options: serde_json::json!({ "text": "hi" }),
        tab_index,
    }
}

#[tokio::test]
async fn snapshot_current_is_a_put_body_with_tab_indices_not_tab_ids() {
    let Some((repo, ws, uid)) = setup().await else {
        eprintln!("skip");
        return;
    };
    let d = repo.create(ws, "Snap", "desc", Some(uid)).await.unwrap();
    repo.replace_layout(
        d.id,
        &[
            NewTab {
                id: None,
                name: "A".into(),
                position: 0,
            },
            NewTab {
                id: None,
                name: "B".into(),
                position: 1,
            },
        ],
        &[card(Some(1), 3), card(Some(0), 0)],
    )
    .await
    .unwrap();

    let snap = repo.snapshot_current(d.id, ws).await.unwrap().unwrap();
    assert_eq!(snap["name"], "Snap");
    assert_eq!(snap["description"], "desc");
    // "fixed", not "full": W3's migration 0105 changed the column default to Metabase's fixed
    // canvas. The plan predates that and expected "full".
    assert_eq!(snap["width"], "fixed");

    // TABS CARRY THEIR ID AND THEIR INDEX. The index is what cards reference; the id is what keeps
    // a tab from being recreated on the way back in, which would cascade its cards away. This test
    // previously asserted the opposite — that the snapshot held no id at all — because
    // `replace_layout` used to renumber everything on every save, which made ids worthless. Once a
    // save keeps them, a revert that dropped them would be the one operation left that silently
    // renumbers a dashboard.
    let tabs = repo.tabs(d.id).await.unwrap();
    assert_eq!(
        snap["tabs"],
        serde_json::json!([
            { "id": tabs[0].id, "name": "A", "position": 0 },
            { "id": tabs[1].id, "name": "B", "position": 1 },
        ])
    );

    // THE DASHBOARD's OWN id still must not appear. A snapshot is fed back into a PUT addressed to
    // a dashboard by URL; an id inside the body would be a second, disagreeing answer to which
    // dashboard is being written. The blunt string search stays for that reason — it catches a
    // leak from ANY field, not just the ones this test thought to name.
    assert!(
        !snap.to_string().contains(&d.id.to_string()),
        "dashboard id leaked into the snapshot"
    );

    let card_ids: Vec<String> = repo
        .cards(d.id)
        .await
        .unwrap()
        .iter()
        .map(|c| c.id.to_string())
        .collect();
    for (i, c) in snap["cards"].as_array().unwrap().iter().enumerate() {
        assert_eq!(
            c["id"].as_str().unwrap(),
            card_ids[i],
            "each card must carry the id it was snapshotted with: {c}"
        );
        assert!(
            c["tab_index"].is_number(),
            "cards must still reference a tab by INDEX, not by tab id: {c}"
        );
    }
}

#[tokio::test]
async fn insert_revision_keeps_only_the_newest_twenty_per_dashboard() {
    let Some((repo, ws, uid)) = setup().await else {
        eprintln!("skip");
        return;
    };
    let d = repo.create(ws, "Trim", "", Some(uid)).await.unwrap();
    for i in 0..25 {
        repo.insert_revision(
            d.id,
            Some(uid),
            "edit",
            &serde_json::json!({ "name": format!("v{i}") }),
            None,
        )
        .await
        .unwrap();
    }
    let revs = repo.list_revisions(d.id, 100).await.unwrap();
    assert_eq!(revs.len(), 20, "retention is a cap, not a suggestion");
    // Newest first, and it is the OLDEST five that were dropped.
    assert_eq!(revs[0].snapshot["name"], "v24");
    assert_eq!(revs[19].snapshot["name"], "v5");
}

#[tokio::test]
async fn a_revision_is_only_reachable_through_its_own_dashboard() {
    let Some((repo, ws, uid)) = setup().await else {
        eprintln!("skip");
        return;
    };
    let a = repo.create(ws, "A", "", Some(uid)).await.unwrap();
    let b = repo.create(ws, "B", "", Some(uid)).await.unwrap();
    let rev = repo
        .insert_revision(
            a.id,
            Some(uid),
            "edit",
            &serde_json::json!({ "name": "A" }),
            None,
        )
        .await
        .unwrap();
    assert!(repo.revision(a.id, rev.id).await.unwrap().is_some());
    // Scoped lookup, not a bare `WHERE id = $1`. An unscoped read is the IDOR shape: a caller
    // authorised on dashboard B could revert B to a snapshot of A.
    assert!(repo.revision(b.id, rev.id).await.unwrap().is_none());
}

#[tokio::test]
async fn revisions_cascade_when_the_dashboard_is_deleted() {
    let Some((repo, ws, uid)) = setup().await else {
        eprintln!("skip");
        return;
    };
    let d = repo.create(ws, "Gone", "", Some(uid)).await.unwrap();
    repo.insert_revision(d.id, Some(uid), "edit", &serde_json::json!({}), None)
        .await
        .unwrap();
    assert_eq!(repo.revision_count(d.id).await.unwrap(), 1);
    repo.delete(d.id).await.unwrap();
    assert_eq!(repo.revision_count(d.id).await.unwrap(), 0);
}

#[tokio::test]
async fn a_baseline_captures_the_state_the_first_edit_overwrites() {
    // The repo-level half of the capture rule: `pending_baseline` reads the CURRENT state, and
    // that state must survive being overwritten. (The routing half — that nothing is written when
    // the PUT is rejected — is asserted by the route test, because it needs a real 400.)
    let Some((repo, ws, uid)) = setup().await else {
        eprintln!("skip");
        return;
    };
    let d = repo.create(ws, "Base", "before", Some(uid)).await.unwrap();
    repo.replace_layout(d.id, &[], &[card(None, 0), card(None, 6)])
        .await
        .unwrap();

    let before = repo.snapshot_current(d.id, ws).await.unwrap().unwrap();
    repo.update(
        d.id,
        "Base",
        "after",
        &serde_json::json!([]),
        "full",
        Some(uid),
    )
    .await
    .unwrap();
    repo.replace_layout(d.id, &[], &[card(None, 0)])
        .await
        .unwrap();
    repo.insert_revision(d.id, Some(uid), "baseline", &before, None)
        .await
        .unwrap();

    let revs = repo.list_revisions(d.id, 10).await.unwrap();
    assert_eq!(revs.len(), 1);
    assert_eq!(revs[0].kind, "baseline");
    assert_eq!(revs[0].snapshot["description"], "before");
    assert_eq!(revs[0].snapshot["width"], "fixed");
    assert_eq!(revs[0].snapshot["cards"].as_array().unwrap().len(), 2);
    // And the live dashboard has genuinely moved on, so the assertions above are about the
    // snapshot rather than about a dashboard that never changed.
    let now = repo.snapshot_current(d.id, ws).await.unwrap().unwrap();
    assert_eq!(now["description"], "after");
    assert_eq!(now["width"], "full");
    assert_eq!(now["cards"].as_array().unwrap().len(), 1);
}
