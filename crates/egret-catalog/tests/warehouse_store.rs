//! DB-gated per-warehouse store tests (migration 0034). Self-skip without
//! DATABASE_URL. Exercises `WarehouseStore` CRUD + per-warehouse tokens, with
//! a focus on workspace-scoping (cross-tenant rows must read as NotFound).
mod common;
use common::{seed_workspace, store};
use egret_catalog::CatalogError;
use uuid::Uuid;

fn rw_perms() -> Vec<String> {
    vec!["catalog:read".to_string(), "catalog:write".to_string()]
}

#[tokio::test]
async fn insert_list_is_workspace_scoped() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws_a = seed_workspace(&s).await;
    let ws_b = seed_workspace(&s).await;
    let lk = Uuid::new_v4();

    let wh = s.insert_warehouse(ws_a, lk, "wh1", None).await.unwrap();
    assert_eq!(wh.warehouse_id, lk);
    assert_eq!(wh.name, "wh1");
    assert_eq!(wh.storage_conn_id, None);

    let a_list = s.list_warehouses(ws_a).await.unwrap();
    assert!(a_list.iter().any(|w| w.id == wh.id), "ws_a sees its wh");

    let b_list = s.list_warehouses(ws_b).await.unwrap();
    assert!(
        !b_list.iter().any(|w| w.id == wh.id),
        "ws_b must NOT see ws_a's warehouse"
    );
}

#[tokio::test]
async fn get_warehouse_cross_workspace_is_not_found() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws_a = seed_workspace(&s).await;
    let ws_b = seed_workspace(&s).await;
    let wh = s
        .insert_warehouse(ws_a, Uuid::new_v4(), "wh1", None)
        .await
        .unwrap();

    // Owning workspace can fetch it.
    let got = s.get_warehouse(ws_a, wh.id).await.unwrap();
    assert_eq!(got, wh);

    // A different workspace gets NotFound (closes IDOR).
    let err = s.get_warehouse(ws_b, wh.id).await.unwrap_err();
    assert!(
        matches!(err, CatalogError::NotFound(_)),
        "cross-ws get must be NotFound, got: {err:?}"
    );
}

#[tokio::test]
async fn rename_warehouse_updates_and_scopes() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws_a = seed_workspace(&s).await;
    let ws_b = seed_workspace(&s).await;
    let wh = s
        .insert_warehouse(ws_a, Uuid::new_v4(), "before", None)
        .await
        .unwrap();

    s.rename_warehouse(ws_a, wh.id, "after").await.unwrap();
    let got = s.get_warehouse(ws_a, wh.id).await.unwrap();
    assert_eq!(got.name, "after");

    // Cross-workspace rename → NotFound (and leaves the row unchanged).
    let err = s.rename_warehouse(ws_b, wh.id, "hijack").await.unwrap_err();
    assert!(
        matches!(err, CatalogError::NotFound(_)),
        "cross-ws rename must be NotFound, got: {err:?}"
    );
    assert_eq!(s.get_warehouse(ws_a, wh.id).await.unwrap().name, "after");
}

#[tokio::test]
async fn duplicate_name_is_already_exists() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    s.insert_warehouse(ws, Uuid::new_v4(), "dup", None)
        .await
        .unwrap();

    // Second insert with the same name in the same workspace collides.
    let err = s
        .insert_warehouse(ws, Uuid::new_v4(), "dup", None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, CatalogError::AlreadyExists(_)),
        "duplicate insert must be AlreadyExists, got: {err:?}"
    );

    // Renaming a second warehouse onto an existing name also collides.
    let other = s
        .insert_warehouse(ws, Uuid::new_v4(), "other", None)
        .await
        .unwrap();
    let err = s.rename_warehouse(ws, other.id, "dup").await.unwrap_err();
    assert!(
        matches!(err, CatalogError::AlreadyExists(_)),
        "duplicate rename must be AlreadyExists, got: {err:?}"
    );
}

#[tokio::test]
async fn delete_returns_lakekeeper_id_and_removes_row() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws_a = seed_workspace(&s).await;
    let ws_b = seed_workspace(&s).await;
    let lk = Uuid::new_v4();
    let wh = s.insert_warehouse(ws_a, lk, "wh1", None).await.unwrap();

    // Cross-workspace delete is a no-op → NotFound, row survives.
    let err = s.delete_warehouse_row(ws_b, wh.id).await.unwrap_err();
    assert!(
        matches!(err, CatalogError::NotFound(_)),
        "cross-ws delete must be NotFound, got: {err:?}"
    );
    assert!(s.get_warehouse(ws_a, wh.id).await.is_ok());

    // Owning workspace delete returns the lakekeeper id and removes the row.
    let returned = s.delete_warehouse_row(ws_a, wh.id).await.unwrap();
    assert_eq!(returned, lk, "delete returns the lakekeeper warehouse uuid");

    let err = s.get_warehouse(ws_a, wh.id).await.unwrap_err();
    assert!(
        matches!(err, CatalogError::NotFound(_)),
        "row must be gone after delete, got: {err:?}"
    );
}

#[tokio::test]
async fn lakekeeper_id_resolves_and_scopes() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws_a = seed_workspace(&s).await;
    let ws_b = seed_workspace(&s).await;
    let lk = Uuid::new_v4();
    let wh = s.insert_warehouse(ws_a, lk, "wh1", None).await.unwrap();

    assert_eq!(s.lakekeeper_id(ws_a, wh.id).await.unwrap(), lk);

    let err = s.lakekeeper_id(ws_b, wh.id).await.unwrap_err();
    assert!(
        matches!(err, CatalogError::NotFound(_)),
        "cross-ws lakekeeper_id must be NotFound, got: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Per-warehouse tokens
// ---------------------------------------------------------------------------

#[tokio::test]
async fn token_is_scoped_to_its_warehouse() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let wh1 = s
        .insert_warehouse(ws, Uuid::new_v4(), "wh1", None)
        .await
        .unwrap();
    let wh2 = s
        .insert_warehouse(ws, Uuid::new_v4(), "wh2", None)
        .await
        .unwrap();

    let (token, raw) = s
        .create_token(ws, wh1.id, "t", &rw_perms(), &[])
        .await
        .unwrap();
    assert_eq!(token.warehouse_id, wh1.id);

    // Listed under its own warehouse.
    let listed = s.list_tokens_for_warehouse(ws, wh1.id).await.unwrap();
    assert!(listed.iter().any(|t| t.id == token.id));

    // NOT listed under a different warehouse.
    let other = s.list_tokens_for_warehouse(ws, wh2.id).await.unwrap();
    assert!(
        !other.iter().any(|t| t.id == token.id),
        "token must not appear under a different warehouse"
    );

    // authenticate_token carries the warehouse id.
    let auth = s.authenticate_token(&raw).await.unwrap();
    assert_eq!(auth.workspace_id, ws);
    assert_eq!(auth.warehouse_id, wh1.id);
}
