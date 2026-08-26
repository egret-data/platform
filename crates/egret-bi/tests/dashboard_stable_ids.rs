//! DB-gated tests for `replace_layout`'s id reconciliation. Self-skip without DATABASE_URL.
//!
//! Saving used to delete every card and tab and reinsert the payload, so **every id changed on
//! every save**. These pin the replacement behaviour and, just as importantly, the two ways it
//! could be abused: an id belonging to another dashboard, and the same id sent twice.
use egret_bi::{DashboardRepo, NewCard, NewTab};
use egret_metastore::Db;
use uuid::Uuid;

struct Fixture {
    repo: DashboardRepo,
    ws: Uuid,
    user: Uuid,
}

async fn setup() -> Option<Fixture> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let user: Uuid =
        sqlx::query_scalar("INSERT INTO users (email, provider) VALUES ($1,'local') RETURNING id")
            .bind(format!("u_{}@t.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let ws: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (slug,name,created_by) VALUES ($1,'t',$2) RETURNING id",
    )
    .bind(format!("w-{}", Uuid::new_v4().simple()))
    .bind(user)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    Some(Fixture {
        repo: DashboardRepo::new(db.clone()),
        ws,
        user,
    })
}

/// A card on a tabless dashboard, which is most of these tests.
fn card(id: Option<Uuid>, col: i32) -> NewCard {
    on_tab(id, col, None)
}

/// A card carrying a `tab_index`. Separate from [`card`] rather than a third argument everywhere,
/// because only the tab test needs it and `card(None, 0, None)` reads worse at every other site.
fn on_tab(id: Option<Uuid>, col: i32, tab_index: Option<usize>) -> NewCard {
    NewCard {
        id,
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

fn tab(id: Option<Uuid>, name: &str, position: i32) -> NewTab {
    NewTab {
        id,
        name: name.into(),
        position,
    }
}

#[tokio::test]
async fn a_save_that_re_sends_the_ids_keeps_them() {
    let Some(f) = setup().await else {
        eprintln!("skip: no DATABASE_URL");
        return;
    };
    let d = f.repo.create(f.ws, "Keep", "", Some(f.user)).await.unwrap();
    f.repo
        .replace_layout(d.id, &[], &[card(None, 0), card(None, 6)])
        .await
        .unwrap();
    let first = f.repo.cards(d.id).await.unwrap();
    let ids: Vec<Uuid> = first.iter().map(|c| c.id).collect();

    // The second save is what an ordinary edit sends: same cards, one of them moved.
    f.repo
        .replace_layout(d.id, &[], &[card(Some(ids[0]), 0), card(Some(ids[1]), 12)])
        .await
        .unwrap();

    let second = f.repo.cards(d.id).await.unwrap();
    assert_eq!(
        second.iter().map(|c| c.id).collect::<Vec<_>>(),
        ids,
        "ids must survive a save — this is the whole point"
    );
    // and the move actually landed, so "kept" is not "ignored the payload"
    assert_eq!(second[1].grid_col, 12);
}

#[tokio::test]
async fn tabs_keep_their_ids_so_their_cards_are_not_cascaded_away() {
    let Some(f) = setup().await else {
        eprintln!("skip: no DATABASE_URL");
        return;
    };
    let d = f.repo.create(f.ws, "Tabs", "", Some(f.user)).await.unwrap();
    f.repo
        .replace_layout(d.id, &[tab(None, "A", 0)], &[on_tab(None, 0, Some(0))])
        .await
        .unwrap();
    let tab_id = f.repo.tabs(d.id).await.unwrap()[0].id;
    let card_id = f.repo.cards(d.id).await.unwrap()[0].id;

    // Rename the tab — the commonest tab edit there is.
    f.repo
        .replace_layout(
            d.id,
            &[tab(Some(tab_id), "A renamed", 0)],
            &[on_tab(Some(card_id), 0, Some(0))],
        )
        .await
        .unwrap();

    let tabs = f.repo.tabs(d.id).await.unwrap();
    let cards = f.repo.cards(d.id).await.unwrap();
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0].id, tab_id);
    assert_eq!(tabs[0].name, "A renamed");
    assert_eq!(cards.len(), 1, "the card must not have been cascaded away");
    assert_eq!(cards[0].id, card_id);
    assert_eq!(cards[0].tab_id, Some(tab_id));
}

#[tokio::test]
async fn a_card_missing_from_the_payload_is_deleted_and_a_new_one_is_inserted() {
    let Some(f) = setup().await else {
        eprintln!("skip: no DATABASE_URL");
        return;
    };
    let d = f.repo.create(f.ws, "Diff", "", Some(f.user)).await.unwrap();
    f.repo
        .replace_layout(d.id, &[], &[card(None, 0), card(None, 6)])
        .await
        .unwrap();
    let ids: Vec<Uuid> = f
        .repo
        .cards(d.id)
        .await
        .unwrap()
        .iter()
        .map(|c| c.id)
        .collect();

    // Drop the first, keep the second, add a third.
    f.repo
        .replace_layout(d.id, &[], &[card(Some(ids[1]), 6), card(None, 12)])
        .await
        .unwrap();

    let got = f.repo.cards(d.id).await.unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].id, ids[1], "the kept card keeps its id");
    assert!(
        !got.iter().any(|c| c.id == ids[0]),
        "the dropped card is gone"
    );
    assert!(
        !ids.contains(&got[1].id),
        "the added card is genuinely new, not a resurrected id"
    );
    // ordinal follows payload order, which is what `cards()` sorts by
    assert_eq!(got[0].ordinal, 0);
    assert_eq!(got[1].ordinal, 1);
}

#[tokio::test]
async fn an_id_from_another_dashboard_is_not_adopted() {
    let Some(f) = setup().await else {
        eprintln!("skip: no DATABASE_URL");
        return;
    };
    let victim = f
        .repo
        .create(f.ws, "Victim", "", Some(f.user))
        .await
        .unwrap();
    let attacker = f
        .repo
        .create(f.ws, "Attacker", "", Some(f.user))
        .await
        .unwrap();
    f.repo
        .replace_layout(victim.id, &[], &[card(None, 0)])
        .await
        .unwrap();
    let victim_card = f.repo.cards(victim.id).await.unwrap()[0].id;

    // The attacker's save claims the victim's card id.
    f.repo
        .replace_layout(attacker.id, &[], &[card(Some(victim_card), 3)])
        .await
        .unwrap();

    let victim_cards = f.repo.cards(victim.id).await.unwrap();
    assert_eq!(victim_cards.len(), 1, "the victim still has its card");
    assert_eq!(victim_cards[0].id, victim_card);
    assert_eq!(
        victim_cards[0].grid_col, 0,
        "and it was not moved by the other dashboard's save"
    );

    let attacker_cards = f.repo.cards(attacker.id).await.unwrap();
    assert_eq!(attacker_cards.len(), 1);
    assert_ne!(
        attacker_cards[0].id, victim_card,
        "the borrowed id must have been ignored and a fresh row inserted"
    );
}

#[tokio::test]
async fn the_same_id_sent_twice_updates_one_row_and_inserts_the_other() {
    let Some(f) = setup().await else {
        eprintln!("skip: no DATABASE_URL");
        return;
    };
    let d = f.repo.create(f.ws, "Dup", "", Some(f.user)).await.unwrap();
    f.repo
        .replace_layout(d.id, &[], &[card(None, 0)])
        .await
        .unwrap();
    let id = f.repo.cards(d.id).await.unwrap()[0].id;

    // Without the `claimed` guard both entries would UPDATE the same row, leaving one card where
    // the payload asked for two — a silent loss rather than an error.
    f.repo
        .replace_layout(d.id, &[], &[card(Some(id), 0), card(Some(id), 6)])
        .await
        .unwrap();

    let got = f.repo.cards(d.id).await.unwrap();
    assert_eq!(got.len(), 2, "two cards were asked for, two must exist");
    assert_eq!(got[0].id, id);
    assert_ne!(got[1].id, id);
    assert_eq!(got[1].grid_col, 6);
}

#[tokio::test]
async fn an_empty_payload_clears_the_dashboard() {
    let Some(f) = setup().await else {
        eprintln!("skip: no DATABASE_URL");
        return;
    };
    let d = f
        .repo
        .create(f.ws, "Clear", "", Some(f.user))
        .await
        .unwrap();
    f.repo
        .replace_layout(d.id, &[tab(None, "A", 0)], &[on_tab(None, 0, Some(0))])
        .await
        .unwrap();

    // `id <> ALL('{}')` is TRUE in Postgres, so an empty keep-list deletes everything — the
    // behaviour the delete-and-reinsert version had for free, and easy to lose here.
    f.repo.replace_layout(d.id, &[], &[]).await.unwrap();

    assert!(f.repo.cards(d.id).await.unwrap().is_empty());
    assert!(f.repo.tabs(d.id).await.unwrap().is_empty());
}
