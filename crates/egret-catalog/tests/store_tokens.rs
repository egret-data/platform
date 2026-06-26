//! DB-gated catalog-token store tests. Self-skip without DATABASE_URL.
mod common;
use common::{seed_workspace, store};
use egret_catalog::service::tokens::hash_token;

#[tokio::test]
async fn create_token_and_workspace_for_token_returns_workspace_id() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let (token_row, raw) = s.create_token(ws, "test-engine").await.unwrap();
    assert_eq!(token_row.workspace_id, ws);
    assert_eq!(token_row.label, "test-engine");

    let found_ws = s.workspace_for_token(&raw).await.unwrap();
    assert_eq!(found_ws, ws);
}

#[tokio::test]
async fn wrong_token_returns_not_found() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let _ws = seed_workspace(&s).await;
    let err = s
        .workspace_for_token("garbage-token-that-does-not-exist")
        .await
        .unwrap_err();
    assert!(
        matches!(err, egret_catalog::CatalogError::NotFound(_)),
        "expected NotFound, got: {err:?}"
    );
}

#[tokio::test]
async fn revoked_token_returns_not_found() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let (token_row, raw) = s.create_token(ws, "revoke-me").await.unwrap();

    // Before revocation it works.
    s.workspace_for_token(&raw).await.unwrap();

    s.revoke_token(token_row.id).await.unwrap();

    let err = s.workspace_for_token(&raw).await.unwrap_err();
    assert!(
        matches!(err, egret_catalog::CatalogError::NotFound(_)),
        "expected NotFound after revoke, got: {err:?}"
    );
}

#[tokio::test]
async fn stored_value_is_hash_not_raw_token() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let (token_row, raw) = s.create_token(ws, "hash-check").await.unwrap();

    let stored_hash: String =
        sqlx::query_scalar("SELECT token_hash FROM catalog_tokens WHERE id = $1")
            .bind(token_row.id)
            .fetch_one(s.pool())
            .await
            .unwrap();

    assert_ne!(stored_hash, raw, "raw token must NOT be stored");
    assert_eq!(
        stored_hash,
        hash_token(&raw),
        "stored hash must equal hash_token(raw)"
    );
}

#[tokio::test]
async fn list_tokens_for_workspace() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let (r1, _) = s.create_token(ws, "alpha").await.unwrap();
    let (r2, _) = s.create_token(ws, "beta").await.unwrap();

    let tokens = s.list_tokens(ws).await.unwrap();
    let ids: Vec<_> = tokens.iter().map(|t| t.id).collect();
    assert!(ids.contains(&r1.id));
    assert!(ids.contains(&r2.id));
}
