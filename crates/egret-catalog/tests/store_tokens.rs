//! DB-gated catalog-token store tests. Self-skip without DATABASE_URL.
mod common;
use common::{seed_warehouse, seed_workspace, store};
use egret_catalog::service::tokens::hash_token;

/// Full read+write permissions, used by most existing tests.
fn rw_perms() -> Vec<String> {
    vec!["catalog:read".to_string(), "catalog:write".to_string()]
}

#[tokio::test]
async fn create_token_and_workspace_for_token_returns_workspace_id() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let wh = seed_warehouse(&s, ws).await;
    let (token_row, raw) = s
        .create_token(ws, wh, "test-engine", &rw_perms(), &[], &[])
        .await
        .unwrap();
    assert_eq!(token_row.workspace_id, ws);
    assert_eq!(token_row.label, "test-engine");

    let auth = s.authenticate_token(&raw).await.unwrap();
    assert_eq!(auth.workspace_id, ws);
}

#[tokio::test]
async fn wrong_token_returns_not_found() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let _ws = seed_workspace(&s).await;
    let err = s
        .authenticate_token("garbage-token-that-does-not-exist")
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
    let wh = seed_warehouse(&s, ws).await;
    let (token_row, raw) = s
        .create_token(ws, wh, "revoke-me", &rw_perms(), &[], &[])
        .await
        .unwrap();

    // Before revocation it works.
    s.authenticate_token(&raw).await.unwrap();

    s.revoke_token(token_row.id).await.unwrap();

    let err = s.authenticate_token(&raw).await.unwrap_err();
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
    let wh = seed_warehouse(&s, ws).await;
    let (token_row, raw) = s
        .create_token(ws, wh, "hash-check", &rw_perms(), &[], &[])
        .await
        .unwrap();

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
    let wh = seed_warehouse(&s, ws).await;
    let (r1, _) = s
        .create_token(ws, wh, "alpha", &rw_perms(), &[], &[])
        .await
        .unwrap();
    let (r2, _) = s
        .create_token(ws, wh, "beta", &rw_perms(), &[], &[])
        .await
        .unwrap();

    let tokens = s.list_tokens(ws).await.unwrap();
    let ids: Vec<_> = tokens.iter().map(|t| t.id).collect();
    assert!(ids.contains(&r1.id));
    assert!(ids.contains(&r2.id));
}

// ---------------------------------------------------------------------------
// New CE-3 tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_token_with_read_only_perms() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let wh = seed_warehouse(&s, ws).await;
    let ro_perms = vec!["catalog:read".to_string()];
    let (token_row, raw) = s
        .create_token(ws, wh, "ro", &ro_perms, &[], &[])
        .await
        .unwrap();
    assert_eq!(token_row.permissions, ro_perms);

    let auth = s.authenticate_token(&raw).await.unwrap();
    assert_eq!(auth.workspace_id, ws);
    assert_eq!(auth.permissions, ro_perms);
}

#[tokio::test]
async fn create_token_rejects_unknown_permission() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let wh = seed_warehouse(&s, ws).await;
    let err = s
        .create_token(ws, wh, "x", &["catalog:admin".to_string()], &[], &[])
        .await
        .unwrap_err();
    assert!(
        matches!(err, egret_catalog::CatalogError::BadRequest(_)),
        "expected BadRequest for unknown permission, got: {err:?}"
    );
}

#[tokio::test]
async fn create_token_rejects_empty_permissions() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let wh = seed_warehouse(&s, ws).await;
    let err = s
        .create_token(ws, wh, "x", &[], &[], &[])
        .await
        .unwrap_err();
    assert!(
        matches!(err, egret_catalog::CatalogError::BadRequest(_)),
        "expected BadRequest for empty permissions, got: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Workspace-scoped revoke (IDOR guard)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn revoke_token_for_workspace_guards_by_workspace() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws_a = seed_workspace(&s).await;
    let ws_b = seed_workspace(&s).await;
    let wh_a = seed_warehouse(&s, ws_a).await;
    let (token, _raw) = s
        .create_token(ws_a, wh_a, "guard", &rw_perms(), &[], &[])
        .await
        .unwrap();

    // Wrong workspace cannot revoke it (IDOR closed).
    assert!(
        !s.revoke_token_for_workspace(token.id, ws_b).await.unwrap(),
        "a different workspace must not be able to revoke"
    );
    // Still present in workspace A's list.
    let ids: Vec<_> = s
        .list_tokens(ws_a)
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert!(
        ids.contains(&token.id),
        "token still active after foreign revoke"
    );

    // Owning workspace revokes it.
    assert!(
        s.revoke_token_for_workspace(token.id, ws_a).await.unwrap(),
        "owning workspace revokes successfully"
    );
    let ids: Vec<_> = s
        .list_tokens(ws_a)
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert!(!ids.contains(&token.id), "token absent after revoke");

    // A second revoke is a no-op (already revoked).
    assert!(
        !s.revoke_token_for_workspace(token.id, ws_a).await.unwrap(),
        "double-revoke returns false"
    );
}

// ---------------------------------------------------------------------------
// CE-4: namespace_scope tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_token_with_namespace_scope() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let wh = seed_warehouse(&s, ws).await;
    let ns: Vec<String> = vec!["marts".to_string(), "staging.raw".to_string()];
    let (token_row, raw) = s
        .create_token(ws, wh, "scoped", &rw_perms(), &ns, &[])
        .await
        .unwrap();
    assert_eq!(token_row.namespace_scope, ns);

    let auth = s.authenticate_token(&raw).await.unwrap();
    assert_eq!(auth.workspace_id, ws);
    assert_eq!(auth.permissions, rw_perms());
    assert_eq!(auth.namespace_scope, ns);
}

#[tokio::test]
async fn create_token_empty_namespace_scope_means_all() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let wh = seed_warehouse(&s, ws).await;
    let (token_row, raw) = s
        .create_token(ws, wh, "all", &rw_perms(), &[], &[])
        .await
        .unwrap();
    assert_eq!(token_row.namespace_scope, Vec::<String>::new());

    let auth = s.authenticate_token(&raw).await.unwrap();
    assert_eq!(auth.namespace_scope, Vec::<String>::new());
}

#[tokio::test]
async fn create_token_rejects_malformed_namespace_empty_string() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let wh = seed_warehouse(&s, ws).await;
    let err = s
        .create_token(ws, wh, "bad", &rw_perms(), &["".to_string()], &[])
        .await
        .unwrap_err();
    assert!(
        matches!(err, egret_catalog::CatalogError::BadRequest(_)),
        "expected BadRequest for empty namespace entry, got: {err:?}"
    );
}

#[tokio::test]
async fn create_token_rejects_malformed_namespace_leading_dot() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let wh = seed_warehouse(&s, ws).await;
    let err = s
        .create_token(ws, wh, "bad", &rw_perms(), &[".bad".to_string()], &[])
        .await
        .unwrap_err();
    assert!(
        matches!(err, egret_catalog::CatalogError::BadRequest(_)),
        "expected BadRequest for leading-dot namespace, got: {err:?}"
    );
}

#[tokio::test]
async fn create_token_rejects_malformed_namespace_trailing_dot() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let wh = seed_warehouse(&s, ws).await;
    let err = s
        .create_token(ws, wh, "bad", &rw_perms(), &["bad.".to_string()], &[])
        .await
        .unwrap_err();
    assert!(
        matches!(err, egret_catalog::CatalogError::BadRequest(_)),
        "expected BadRequest for trailing-dot namespace, got: {err:?}"
    );
}

#[tokio::test]
async fn create_token_rejects_malformed_namespace_double_dot() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let wh = seed_warehouse(&s, ws).await;
    let err = s
        .create_token(ws, wh, "bad", &rw_perms(), &["a..b".to_string()], &[])
        .await
        .unwrap_err();
    assert!(
        matches!(err, egret_catalog::CatalogError::BadRequest(_)),
        "expected BadRequest for double-dot namespace, got: {err:?}"
    );
}
