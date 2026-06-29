//! DB-gated catalog-token store tests. Self-skip without DATABASE_URL.
mod common;
use common::{seed_warehouse, seed_workspace, store};
use egret_catalog::service::tokens::{hash_token, Grant};

/// A single all-namespaces grant with the given full perm strings — the common
/// "read everything" / "read+write everything" shape.
fn star_grant(perms: &[&str]) -> Vec<Grant> {
    vec![Grant {
        namespace: "*".to_string(),
        table: None,
        permissions: perms.iter().map(|p| p.to_string()).collect(),
    }]
}

/// Full read+write all-namespaces scope, used by most existing tests.
fn rw_scope() -> Vec<Grant> {
    star_grant(&["catalog:read", "catalog:write"])
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
        .create_token(ws, wh, "test-engine", &rw_scope())
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
        .create_token(ws, wh, "revoke-me", &rw_scope())
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
        .create_token(ws, wh, "hash-check", &rw_scope())
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
    let (r1, _) = s.create_token(ws, wh, "alpha", &rw_scope()).await.unwrap();
    let (r2, _) = s.create_token(ws, wh, "beta", &rw_scope()).await.unwrap();

    let tokens = s.list_tokens(ws).await.unwrap();
    let ids: Vec<_> = tokens.iter().map(|t| t.id).collect();
    assert!(ids.contains(&r1.id));
    assert!(ids.contains(&r2.id));
}

// ---------------------------------------------------------------------------
// Grant scope (token scope v2)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_token_with_read_only_scope() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let wh = seed_warehouse(&s, ws).await;
    let scope = star_grant(&["catalog:read"]);
    let (token_row, raw) = s.create_token(ws, wh, "ro", &scope).await.unwrap();
    assert_eq!(token_row.scope, scope);

    let auth = s.authenticate_token(&raw).await.unwrap();
    assert_eq!(auth.workspace_id, ws);
    // The coarse-gate union holds only read.
    assert_eq!(auth.permissions, vec!["catalog:read".to_string()]);
    assert_eq!(auth.grants, scope);
}

/// Round-trip a multi-grant scope: it persists on the row, lists back unchanged,
/// authenticates into `TokenAuth.grants`, and the derived union `permissions`
/// is the deduped union of every grant's perms.
#[tokio::test]
async fn multi_grant_scope_round_trips_with_union_permissions() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let wh = seed_warehouse(&s, ws).await;
    let scope = vec![
        Grant {
            namespace: "sale".into(),
            table: None,
            permissions: vec!["catalog:read".into()],
        },
        Grant {
            namespace: "marketing".into(),
            table: Some("campaigns".into()),
            permissions: vec!["catalog:read".into(), "catalog:write".into()],
        },
    ];
    let (token_row, raw) = s.create_token(ws, wh, "mixed", &scope).await.unwrap();
    assert_eq!(token_row.scope, scope, "scope persists on the row");

    let auth = s.authenticate_token(&raw).await.unwrap();
    assert_eq!(auth.grants, scope, "grants flow into TokenAuth");
    // Deduped union of {read} ∪ {read,write} = {read, write} (sorted).
    assert_eq!(
        auth.permissions,
        vec!["catalog:read".to_string(), "catalog:write".to_string()],
        "permissions is the deduped union for the coarse gate"
    );
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
        .create_token(ws, wh, "x", &star_grant(&["catalog:admin"]))
        .await
        .unwrap_err();
    assert!(
        matches!(err, egret_catalog::CatalogError::BadRequest(_)),
        "expected BadRequest for unknown permission, got: {err:?}"
    );
}

#[tokio::test]
async fn create_token_rejects_empty_scope() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let wh = seed_warehouse(&s, ws).await;
    let err = s.create_token(ws, wh, "x", &[]).await.unwrap_err();
    assert!(
        matches!(err, egret_catalog::CatalogError::BadRequest(_)),
        "expected BadRequest for empty scope, got: {err:?}"
    );
}

#[tokio::test]
async fn create_token_rejects_grant_with_empty_permissions() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let wh = seed_warehouse(&s, ws).await;
    let err = s
        .create_token(
            ws,
            wh,
            "x",
            &[Grant {
                namespace: "*".into(),
                table: None,
                permissions: vec![],
            }],
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, egret_catalog::CatalogError::BadRequest(_)),
        "expected BadRequest for a grant with no permissions, got: {err:?}"
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
        .create_token(ws_a, wh_a, "guard", &rw_scope())
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
// Grant validation (namespace / table shape)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_token_with_namespace_scope() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let wh = seed_warehouse(&s, ws).await;
    let scope = vec![
        Grant {
            namespace: "marts".into(),
            table: None,
            permissions: vec!["catalog:read".into(), "catalog:write".into()],
        },
        Grant {
            namespace: "staging.raw".into(),
            table: None,
            permissions: vec!["catalog:read".into(), "catalog:write".into()],
        },
    ];
    let (token_row, raw) = s.create_token(ws, wh, "scoped", &scope).await.unwrap();
    assert_eq!(token_row.scope, scope);

    let auth = s.authenticate_token(&raw).await.unwrap();
    assert_eq!(auth.workspace_id, ws);
    assert_eq!(auth.grants, scope);
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
        .create_token(
            ws,
            wh,
            "bad",
            &[Grant {
                namespace: "".into(),
                table: None,
                permissions: vec!["catalog:read".into()],
            }],
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, egret_catalog::CatalogError::BadRequest(_)),
        "expected BadRequest for empty namespace, got: {err:?}"
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
        .create_token(
            ws,
            wh,
            "bad",
            &[Grant {
                namespace: ".bad".into(),
                table: None,
                permissions: vec!["catalog:read".into()],
            }],
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, egret_catalog::CatalogError::BadRequest(_)),
        "expected BadRequest for leading-dot namespace, got: {err:?}"
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
        .create_token(
            ws,
            wh,
            "bad",
            &[Grant {
                namespace: "a..b".into(),
                table: None,
                permissions: vec!["catalog:read".into()],
            }],
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, egret_catalog::CatalogError::BadRequest(_)),
        "expected BadRequest for double-dot namespace, got: {err:?}"
    );
}

#[tokio::test]
async fn create_token_rejects_dotted_table_name() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let wh = seed_warehouse(&s, ws).await;
    let err = s
        .create_token(
            ws,
            wh,
            "bad",
            &[Grant {
                namespace: "sale".into(),
                table: Some("a.b".into()),
                permissions: vec!["catalog:read".into()],
            }],
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, egret_catalog::CatalogError::BadRequest(_)),
        "expected BadRequest for a dotted table name, got: {err:?}"
    );
}

#[tokio::test]
async fn create_token_rejects_table_grant_on_star_namespace() {
    let Some(s) = store().await else {
        eprintln!("skip: DATABASE_URL not set");
        return;
    };
    let ws = seed_workspace(&s).await;
    let wh = seed_warehouse(&s, ws).await;
    let err = s
        .create_token(
            ws,
            wh,
            "bad",
            &[Grant {
                namespace: "*".into(),
                table: Some("orders".into()),
                permissions: vec!["catalog:read".into()],
            }],
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, egret_catalog::CatalogError::BadRequest(_)),
        "expected BadRequest for a table grant on the '*' namespace, got: {err:?}"
    );
}
