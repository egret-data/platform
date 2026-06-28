//! DB-gated per-warehouse mapping store tests. Self-skip without DATABASE_URL.
//!
//! Catalog sub-epic 1 replaced the legacy 1:1 `upsert_warehouse` /
//! `warehouse_for_workspace` / `warehouse_for_slug` helpers with the
//! per-warehouse API (`insert_warehouse` / `get_warehouse` / `get_warehouse_by_name`
//! / `list_warehouses`). These tests exercise that store surface.
mod common;
use common::{seed_workspace_with_slug, store};
use egret_catalog::CatalogError;
use uuid::Uuid;

#[tokio::test]
async fn insert_and_lookup_by_name() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (ws, _slug) = seed_workspace_with_slug(&s).await;
    let lk = Uuid::new_v4();

    let inserted = s
        .insert_warehouse(ws, lk, "default", None)
        .await
        .expect("insert warehouse");
    assert_eq!(inserted.warehouse_id, lk);
    assert_eq!(inserted.name, "default");

    let found = s
        .get_warehouse_by_name(ws, "default")
        .await
        .expect("lookup by name");
    assert_eq!(found.id, inserted.id);
    assert_eq!(found.warehouse_id, lk);
}

#[tokio::test]
async fn get_warehouse_by_id_is_workspace_scoped() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (ws_a, _slug_a) = seed_workspace_with_slug(&s).await;
    let (ws_b, _slug_b) = seed_workspace_with_slug(&s).await;

    let wh = s
        .insert_warehouse(ws_a, Uuid::new_v4(), "default", None)
        .await
        .expect("insert warehouse");

    // Owner workspace resolves it.
    let found = s.get_warehouse(ws_a, wh.id).await.expect("get by id");
    assert_eq!(found.id, wh.id);

    // A different workspace must NOT resolve another tenant's warehouse row.
    let cross = s.get_warehouse(ws_b, wh.id).await;
    assert!(
        matches!(cross, Err(CatalogError::NotFound(_))),
        "cross-workspace get_warehouse must be NotFound (IDOR closed)"
    );
}

#[tokio::test]
async fn list_warehouses_returns_all_for_workspace() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (ws, _slug) = seed_workspace_with_slug(&s).await;

    s.insert_warehouse(ws, Uuid::new_v4(), "alpha", None)
        .await
        .expect("insert alpha");
    s.insert_warehouse(ws, Uuid::new_v4(), "beta", None)
        .await
        .expect("insert beta");

    let names: Vec<String> = s
        .list_warehouses(ws)
        .await
        .expect("list")
        .into_iter()
        .map(|w| w.name)
        .collect();
    // Ordered by name ASC per the store query.
    assert_eq!(names, vec!["alpha".to_string(), "beta".to_string()]);
}

#[tokio::test]
async fn unknown_name_returns_not_found() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (ws, _slug) = seed_workspace_with_slug(&s).await;
    let missing = s.get_warehouse_by_name(ws, "default").await;
    assert!(
        matches!(missing, Err(CatalogError::NotFound(_))),
        "a workspace with no `default` warehouse must be NotFound"
    );
}

#[tokio::test]
async fn duplicate_name_in_workspace_is_already_exists() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let (ws, _slug) = seed_workspace_with_slug(&s).await;

    s.insert_warehouse(ws, Uuid::new_v4(), "default", None)
        .await
        .expect("first insert");
    let dup = s
        .insert_warehouse(ws, Uuid::new_v4(), "default", None)
        .await;
    assert!(
        matches!(dup, Err(CatalogError::AlreadyExists(_))),
        "a duplicate (workspace, name) must be AlreadyExists; got {dup:?}"
    );
}
