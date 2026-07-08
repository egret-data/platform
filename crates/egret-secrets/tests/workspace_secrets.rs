//! DB-gated WorkspaceSecretStore tests. Self-skip without DATABASE_URL.
use egret_metastore::Db;
use egret_secrets::{Crypto, SecretStore, SecretsError, WorkspaceSecretStore};
use uuid::Uuid;

fn crypto() -> Crypto {
    Crypto::new(b"0123456789abcdef0123456789abcdef").unwrap()
}

struct Ctx {
    db: Db,
    secrets: SecretStore,
    store: WorkspaceSecretStore,
}

async fn ctx() -> Option<Ctx> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let secrets = SecretStore::new(db.clone(), Some(crypto()));
    let store = WorkspaceSecretStore::new(db.clone(), secrets.clone());
    Some(Ctx { db, secrets, store })
}

async fn seed_workspace(db: &Db) -> Uuid {
    let id = Uuid::new_v4();
    let suffix = id.simple().to_string();
    sqlx::query("INSERT INTO workspaces (id, slug, name) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(format!("ws-{suffix}"))
        .bind(format!("ws {suffix}"))
        .execute(&db.pool)
        .await
        .expect("seed workspace");
    id
}

async fn seed_storage(db: &Db, secret_ref: Uuid) {
    // Storage moved to the control plane (migration 0050): `workspace_storages`
    // was renamed to `storage_connections` and its `workspace_id` column dropped.
    sqlx::query(
        "INSERT INTO storage.storage_connections (name, region, bucket, access_key, secret_ref) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(format!("storage-{}", Uuid::new_v4().simple()))
    .bind("us-east-1")
    .bind("bucket")
    .bind("AKIA")
    .bind(secret_ref)
    .execute(&db.pool)
    .await
    .expect("seed storage");
}

#[tokio::test]
async fn create_resolve_roundtrip_and_cross_workspace_isolation() {
    let Some(c) = ctx().await else {
        eprintln!("skip");
        return;
    };
    let ws = seed_workspace(&c.db).await;
    let other_ws = seed_workspace(&c.db).await;

    let id = c
        .store
        .create(Some(ws), "prod-db", Some("primary"), "p@ss")
        .await
        .unwrap();
    assert_eq!(
        c.store.resolve_plaintext(Some(ws), id).await.unwrap(),
        "p@ss"
    );

    // Cross-workspace access resolves to NotFound.
    let err = c
        .store
        .resolve_plaintext(Some(other_ws), id)
        .await
        .unwrap_err();
    assert!(matches!(err, SecretsError::NotFound));
    let err = c.store.get_meta(Some(other_ws), id).await.unwrap_err();
    assert!(matches!(err, SecretsError::NotFound));

    // list reflects used_by == 0 and the metadata exposes no value.
    let list = c.store.list(Some(ws)).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "prod-db");
    assert_eq!(list[0].description.as_deref(), Some("primary"));
    assert_eq!(list[0].used_by, 0);
}

#[tokio::test]
async fn duplicate_name_rejected_and_vault_rolled_back() {
    let Some(c) = ctx().await else {
        eprintln!("skip");
        return;
    };
    let ws = seed_workspace(&c.db).await;

    c.store
        .create(Some(ws), "dup", None, "first")
        .await
        .unwrap();

    let err = c
        .store
        .create(Some(ws), "dup", None, "second")
        .await
        .unwrap_err();
    match err {
        SecretsError::AlreadyExists(name) => assert_eq!(name, "dup"),
        other => panic!("expected AlreadyExists, got {other:?}"),
    }
    // Only the first named secret exists; the rolled-back attempt left no extra row.
    assert_eq!(c.store.list(Some(ws)).await.unwrap().len(), 1);
}

#[tokio::test]
async fn update_value_rotates_in_place() {
    let Some(c) = ctx().await else {
        eprintln!("skip");
        return;
    };
    let ws = seed_workspace(&c.db).await;
    let id = c
        .store
        .create(Some(ws), "rotate", None, "old")
        .await
        .unwrap();

    c.store.update_value(Some(ws), id, "new").await.unwrap();

    // Same named id, new plaintext.
    assert_eq!(
        c.store.resolve_plaintext(Some(ws), id).await.unwrap(),
        "new"
    );
}

#[tokio::test]
async fn delete_blocked_while_in_use_then_allowed() {
    let Some(c) = ctx().await else {
        eprintln!("skip");
        return;
    };
    let ws = seed_workspace(&c.db).await;
    let id = c.store.create(Some(ws), "shared", None, "v").await.unwrap();

    // Reference the secret from a storage connection.
    seed_storage(&c.db, id).await;

    let list = c.store.list(Some(ws)).await.unwrap();
    assert_eq!(list[0].used_by, 1);

    let err = c.store.delete(Some(ws), id).await.unwrap_err();
    assert!(matches!(err, SecretsError::InUse(1)), "got {err:?}");

    // Drop the reference, then delete succeeds and the vault row is gone.
    sqlx::query("DELETE FROM storage.storage_connections WHERE secret_ref = $1")
        .bind(id)
        .execute(&c.db.pool)
        .await
        .unwrap();

    // Capture the underlying vault id before deletion to confirm cascade cleanup.
    let vault_id: Uuid =
        sqlx::query_scalar("SELECT secret_ref FROM workspace_secrets WHERE id = $1")
            .bind(id)
            .fetch_one(&c.db.pool)
            .await
            .unwrap();

    c.store.delete(Some(ws), id).await.unwrap();
    assert!(c.store.get_meta(Some(ws), id).await.is_err());
    assert!(c.secrets.get(vault_id).await.is_err());
}

#[tokio::test]
async fn rename_rejects_duplicate_and_missing() {
    let Some(c) = ctx().await else {
        eprintln!("skip");
        return;
    };
    let ws = seed_workspace(&c.db).await;
    let a = c.store.create(Some(ws), "a", None, "v").await.unwrap();
    c.store.create(Some(ws), "b", None, "v").await.unwrap();

    // Renaming "a" to "b" collides.
    let err = c.store.rename(Some(ws), a, "b", None).await.unwrap_err();
    assert!(matches!(err, SecretsError::AlreadyExists(_)));

    // Renaming an id from another workspace is NotFound.
    let other_ws = seed_workspace(&c.db).await;
    let err = c
        .store
        .rename(Some(other_ws), a, "c", None)
        .await
        .unwrap_err();
    assert!(matches!(err, SecretsError::NotFound));

    // Valid rename succeeds.
    c.store
        .rename(Some(ws), a, "renamed", Some("desc"))
        .await
        .unwrap();
    let meta = c.store.get_meta(Some(ws), a).await.unwrap();
    assert_eq!(meta.name, "renamed");
    assert_eq!(meta.description.as_deref(), Some("desc"));
}

#[tokio::test]
async fn global_and_workspace_scopes_are_isolated() {
    let Some(c) = ctx().await else {
        eprintln!("skip");
        return;
    };
    let ws = seed_workspace(&c.db).await;
    // Unique names so parallel/repeat runs don't collide on the global partial
    // unique index (workspace_secrets_global_name_key on name WHERE ws_id IS NULL).
    let suffix = Uuid::new_v4().simple().to_string();
    let global_name = format!("global-{suffix}");
    let ws_name = format!("wsonly-{suffix}");

    // `None` scope = a global / control-plane secret (NULL workspace_id).
    let global_id = c
        .store
        .create(None, &global_name, Some("control-plane"), "gval")
        .await
        .unwrap();
    // `Some(ws)` scope = a workspace-scoped secret.
    let ws_id = c
        .store
        .create(Some(ws), &ws_name, None, "wval")
        .await
        .unwrap();

    // Both roundtrip within their own scope.
    assert_eq!(
        c.store.resolve_plaintext(None, global_id).await.unwrap(),
        "gval"
    );
    assert_eq!(
        c.store.resolve_plaintext(Some(ws), ws_id).await.unwrap(),
        "wval"
    );

    // Cross-scope resolution is NotFound (a plain `= $1` would never match NULL).
    assert!(matches!(
        c.store.get_meta(Some(ws), global_id).await.unwrap_err(),
        SecretsError::NotFound
    ));
    assert!(matches!(
        c.store.get_meta(None, ws_id).await.unwrap_err(),
        SecretsError::NotFound
    ));

    // list(None) sees the global secret but NOT the workspace secret.
    let global_list = c.store.list(None).await.unwrap();
    assert!(global_list.iter().any(|s| s.id == global_id));
    assert!(!global_list.iter().any(|s| s.id == ws_id));

    // list(Some(ws)) sees the workspace secret but NOT the global secret.
    let ws_list = c.store.list(Some(ws)).await.unwrap();
    assert!(ws_list.iter().any(|s| s.id == ws_id));
    assert!(!ws_list.iter().any(|s| s.id == global_id));

    // Clean up the global row (workspace rows go with the workspace).
    c.store.delete(None, global_id).await.unwrap();
}
