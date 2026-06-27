//! DB-gated warehouse mapping store tests. Self-skip without DATABASE_URL.
mod common;
use common::{seed_workspace_with_slug, store};
use uuid::Uuid;

#[tokio::test]
async fn upsert_and_lookup_by_workspace_id() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (ws, _slug) = seed_workspace_with_slug(&s).await;
    let wh = Uuid::new_v4();

    s.upsert_warehouse(ws, wh).await.unwrap();

    let found = s.warehouse_for_workspace(ws).await.unwrap();
    assert_eq!(found, Some(wh));
}

#[tokio::test]
async fn lookup_by_slug() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (ws, slug) = seed_workspace_with_slug(&s).await;
    let wh = Uuid::new_v4();

    s.upsert_warehouse(ws, wh).await.unwrap();

    let found = s.warehouse_for_slug(&slug).await.unwrap();
    assert_eq!(found, Some(wh));
}

#[tokio::test]
async fn unknown_workspace_returns_none() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let missing = Uuid::new_v4();
    let found = s.warehouse_for_workspace(missing).await.unwrap();
    assert_eq!(found, None);
}

#[tokio::test]
async fn unknown_slug_returns_none() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let found = s
        .warehouse_for_slug("slug-that-does-not-exist")
        .await
        .unwrap();
    assert_eq!(found, None);
}

#[tokio::test]
async fn second_upsert_is_noop_first_write_wins() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (ws, slug) = seed_workspace_with_slug(&s).await;
    let wh1 = Uuid::new_v4();
    let wh2 = Uuid::new_v4();

    s.upsert_warehouse(ws, wh1).await.unwrap();
    s.upsert_warehouse(ws, wh2).await.unwrap(); // second call is a no-op

    let found_by_id = s.warehouse_for_workspace(ws).await.unwrap();
    assert_eq!(
        found_by_id,
        Some(wh1),
        "second upsert must not overwrite first"
    );

    let found_by_slug = s.warehouse_for_slug(&slug).await.unwrap();
    assert_eq!(
        found_by_slug,
        Some(wh1),
        "slug lookup must also return first warehouse"
    );
}
