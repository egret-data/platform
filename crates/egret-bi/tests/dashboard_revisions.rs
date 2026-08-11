//! DB-gated dashboard revision tests. Self-skip without DATABASE_URL.
use egret_bi::{DashboardRepo, NewCard, NewTab};
use egret_metastore::Db;
use uuid::Uuid;

async fn setup() -> Option<(DashboardRepo, Uuid, Uuid)> {
    let url = std::env::var("DATABASE_URL").ok()?;
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

    let snap = repo.snapshot_current(d.id).await.unwrap().unwrap();
    assert_eq!(snap["name"], "Snap");
    assert_eq!(snap["description"], "desc");
    // "fixed", not "full": W3's migration 0105 changed the column default to Metabase's fixed
    // canvas. The plan predates that and expected "full".
    assert_eq!(snap["width"], "fixed");
    assert_eq!(
        snap["tabs"],
        serde_json::json!([
            { "name": "A", "position": 0 },
            { "name": "B", "position": 1 },
        ])
    );
    // The point of the whole design: no uuid anywhere in the snapshot, so nothing has to be
    // remapped on the way back in. Serialising the value and searching it is a blunt check and
    // that is exactly why it is used — it catches an id leaking in from ANY field, not just the
    // fields this test thought to name.
    let text = snap.to_string();
    assert!(
        !text.contains(&d.id.to_string()),
        "dashboard id leaked into the snapshot"
    );
    for c in snap["cards"].as_array().unwrap() {
        assert!(
            c.get("id").is_none(),
            "card id leaked into the snapshot: {c}"
        );
        assert!(
            c["tab_index"].is_number(),
            "cards must reference a tab by index: {c}"
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

    let before = repo.snapshot_current(d.id).await.unwrap().unwrap();
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
    let now = repo.snapshot_current(d.id).await.unwrap().unwrap();
    assert_eq!(now["description"], "after");
    assert_eq!(now["width"], "full");
    assert_eq!(now["cards"].as_array().unwrap().len(), 1);
}
