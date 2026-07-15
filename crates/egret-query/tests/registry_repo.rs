//! DB-gated DatabaseRepo tests. Self-skip without DATABASE_URL.
//!
//! Epic B (named per-workspace secrets): the repo no longer seals/rotates its own
//! secret — it BINDS an already-resolved `workspace_secrets` id supplied by the
//! caller. These cover that binding lifecycle: create binds a named secret id,
//! `open_secret` resolves it (workspace-scoped, deriving the workspace from the
//! row), update rebinds / keeps, delete drops only the reference (the shared
//! named secret survives). Cross-workspace ownership isolation lives in
//! `repo_workspace_scope.rs`.
use egret_metastore::Db;
use egret_query::registry::model::Engine;
use egret_query::DatabaseRepo;
use egret_secrets::{Crypto, SecretStore, WorkspaceSecretStore};
use uuid::Uuid;

fn input(name: &str) -> egret_query::registry::model::DatabaseInput {
    egret_query::registry::model::DatabaseInput {
        name: name.into(),
        engine: Engine::Trino,
        host: "localhost".into(),
        port: 8080,
        options: egret_query::registry::model::ConnOptions {
            user: Some("admin".into()),
            http_scheme: Some("http".into()),
            ..Default::default()
        },
        secret_id: None,
        new_secret: None,
        delegated: false,
    }
}

/// Returns the repo, its named-secret store, the raw Db handle, and a freshly
/// seeded workspace id to own every database created in the test.
async fn repo_with_db(with_key: bool) -> Option<(DatabaseRepo, WorkspaceSecretStore, Db, Uuid)> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let crypto = with_key.then(|| Crypto::new(&[3u8; 32]).unwrap());
    let secrets = SecretStore::new(db.clone(), crypto);
    let workspace_secrets = WorkspaceSecretStore::new(db.clone(), secrets.clone());
    let wid = seed_workspace(&db).await;
    Some((
        DatabaseRepo::new(db.clone(), secrets, workspace_secrets.clone()),
        workspace_secrets,
        db,
        wid,
    ))
}

async fn repo(with_key: bool) -> Option<(DatabaseRepo, WorkspaceSecretStore, Uuid)> {
    repo_with_db(with_key)
        .await
        .map(|(r, ws, _db, wid)| (r, ws, wid))
}

async fn seed_workspace(db: &Db) -> Uuid {
    let uid: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, name, provider, status, password_hash) \
         VALUES ($1,'t','local','active','x') RETURNING id",
    )
    .bind(format!("u_{}@t.local", Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("user");
    sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by) VALUES ($1,'t',$2) RETURNING id",
    )
    .bind(format!("w-{}", Uuid::new_v4().simple()))
    .bind(uid)
    .fetch_one(&db.pool)
    .await
    .expect("ws")
}

fn uniq(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
}

/// Seed a storage connection + warehouse (owned by `workspace_id`) + engine,
/// returning the engine id — the FK target that marks a `query.databases` row
/// provisioned. Uses raw SQL against the shared schemas so this crate needn't
/// depend on the catalog/engine seams' stores.
async fn seed_engine(db: &Db, workspace_id: Uuid) -> Uuid {
    let sc_id: Uuid = sqlx::query_scalar(
        "INSERT INTO storage.storage_connections \
           (name, provider, region, bucket, access_key, path_style) \
         VALUES ($1, 's3', 'us-east-1', 'warehouse', 'egret', true) RETURNING id",
    )
    .bind(uniq("store"))
    .fetch_one(&db.pool)
    .await
    .expect("seed storage connection");

    let wh_id: Uuid = sqlx::query_scalar(
        "INSERT INTO catalog.warehouses (name, workspace_id, storage_conn_id) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(uniq("wh"))
    .bind(workspace_id)
    .bind(sc_id)
    .fetch_one(&db.pool)
    .await
    .expect("seed warehouse");

    sqlx::query_scalar(
        "INSERT INTO engine_mgmt.engines (name, type, namespace, warehouse_id, spec) \
         VALUES ($1, 'trino', 'egret-engines', $2, '{}'::jsonb) RETURNING id",
    )
    .bind(uniq("eng"))
    .bind(wh_id)
    .fetch_one(&db.pool)
    .await
    .expect("seed engine")
}

#[tokio::test]
async fn create_provisioned_sets_engine_id_and_delegated() {
    let Some((r, _ws, db, wid)) = repo_with_db(true).await else {
        eprintln!("skip");
        return;
    };
    let engine_id = seed_engine(&db, wid).await;

    let created = r
        .create_provisioned(
            wid,
            engine_id,
            &uniq("trino-prov"),
            "trino",
            "trino-prod-coordinator.engines.svc",
            8080,
            &serde_json::json!({ "catalog": "sale" }),
            None,
            None,
        )
        .await
        .unwrap();
    assert!(created.delegated, "provisioned rows are delegated");
    assert!(!created.has_secret, "provisioned rows bind no secret");
    assert_eq!(created.engine_id, Some(engine_id));

    // engine_id survives a round-trip through get + list_for_workspace.
    let fetched = r.get(created.id).await.unwrap();
    assert_eq!(fetched.engine_id, Some(engine_id));
    assert!(fetched.delegated);
    assert!(r
        .list_for_workspace(wid)
        .await
        .unwrap()
        .iter()
        .any(|d| d.id == created.id && d.engine_id == Some(engine_id)));

    // delete_by_engine removes it and is idempotent.
    r.delete_by_engine(wid, engine_id).await.unwrap();
    assert!(matches!(
        r.get(created.id).await.unwrap_err(),
        egret_query::DatabaseError::NotFound(_)
    ));
    r.delete_by_engine(wid, engine_id).await.unwrap();
}

#[tokio::test]
async fn register_list_get_delete() {
    let Some((r, ws, wid)) = repo(true).await else {
        eprintln!("skip");
        return;
    };
    let name = uniq("db");
    let secret_id = ws
        .create(Some(wid), &uniq("sec"), None, "pw")
        .await
        .expect("create named secret");
    let db = r
        .create_in_workspace(wid, &input(&name), Some(secret_id), None)
        .await
        .unwrap();
    assert!(db.has_secret);
    let got = r.get_in_workspace(wid, db.id).await.unwrap();
    assert_eq!(got.name, name);
    assert!(got.has_secret);
    assert!(r
        .list_for_workspace(wid)
        .await
        .unwrap()
        .iter()
        .any(|d| d.id == db.id));
    r.delete_in_workspace(wid, db.id).await.unwrap();
    assert!(matches!(
        r.get_in_workspace(wid, db.id).await.unwrap_err(),
        egret_query::DatabaseError::NotFound(_)
    ));
}

#[tokio::test]
async fn delegated_flag_roundtrips() {
    let Some((r, _ws, wid)) = repo(true).await else {
        eprintln!("skip");
        return;
    };
    // Default (omitted in the helper) persists as false.
    let default_db = r
        .create_in_workspace(wid, &input(&uniq("db")), None, None)
        .await
        .unwrap();
    assert!(
        !r.get(default_db.id).await.unwrap().delegated,
        "default delegated must be false"
    );

    // Explicit delegated=true round-trips through create → get.
    let mut inp = input(&uniq("db"));
    inp.delegated = true;
    let del_db = r.create_in_workspace(wid, &inp, None, None).await.unwrap();
    assert!(del_db.delegated, "create must return delegated=true");
    assert!(
        r.get(del_db.id).await.unwrap().delegated,
        "get must read delegated=true"
    );

    // Update can flip it back to false.
    let mut off = input("");
    off.name = del_db.name.clone();
    off.delegated = false;
    let updated = r
        .update_in_workspace(wid, del_db.id, &off, None, None)
        .await
        .unwrap();
    assert!(!updated.delegated, "update must persist delegated=false");

    r.delete_in_workspace(wid, default_db.id).await.unwrap();
    r.delete_in_workspace(wid, del_db.id).await.unwrap();
}

#[tokio::test]
async fn duplicate_name_conflicts() {
    let Some((r, _ws, wid)) = repo(true).await else {
        eprintln!("skip");
        return;
    };
    let name = uniq("db");
    let a = r
        .create_in_workspace(wid, &input(&name), None, None)
        .await
        .unwrap();
    let err = r
        .create_in_workspace(wid, &input(&name), None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, egret_query::DatabaseError::AlreadyExists(_)));
    r.delete_in_workspace(wid, a.id).await.unwrap();
}

#[tokio::test]
async fn update_rebinds_secret() {
    let Some((r, ws, wid)) = repo(true).await else {
        eprintln!("skip");
        return;
    };
    let name = uniq("db");
    let old = ws
        .create(Some(wid), &uniq("sec"), None, "old")
        .await
        .expect("old secret");
    let db = r
        .create_in_workspace(wid, &input(&name), Some(old), None)
        .await
        .unwrap();
    assert_eq!(r.open_secret(db.id).await.unwrap(), Some("old".to_string()));

    // Rebind to a freshly-rotated named secret.
    let new = ws
        .create(Some(wid), &uniq("sec"), None, "new")
        .await
        .expect("new secret");
    let updated = r
        .update_in_workspace(wid, db.id, &input(&name), Some(new), None)
        .await
        .unwrap();
    assert!(updated.has_secret);
    assert_eq!(r.open_secret(db.id).await.unwrap(), Some("new".to_string()));
    r.delete_in_workspace(wid, db.id).await.unwrap();
}

#[tokio::test]
async fn open_secret_without_key_errors() {
    let Some((_r, ws, wid)) = repo(false).await else {
        eprintln!("skip");
        return;
    };
    // With no master key, creating the named secret itself fails (NotConfigured).
    let err = ws
        .create(Some(wid), &uniq("sec"), None, "pw")
        .await
        .unwrap_err();
    assert!(matches!(err, egret_secrets::SecretsError::NotConfigured));
}

#[tokio::test]
async fn update_without_secret_preserves_old() {
    let Some((r, ws, wid)) = repo(true).await else {
        eprintln!("skip");
        return;
    };
    let name = uniq("db");
    let secret_id = ws
        .create(Some(wid), &uniq("sec"), None, "keepme")
        .await
        .expect("named secret");
    let db = r
        .create_in_workspace(wid, &input(&name), Some(secret_id), None)
        .await
        .unwrap();
    // No secret supplied → the route would pass the existing id; emulate that by
    // passing the same `secret_id` (keep-current).
    let mut inp = input(&name);
    inp.host = "newhost".into();
    let updated = r
        .update_in_workspace(wid, db.id, &inp, Some(secret_id), None)
        .await
        .unwrap();
    assert_eq!(updated.host, "newhost");
    assert!(
        updated.has_secret,
        "secret must be preserved across a keep-current update"
    );
    assert_eq!(
        r.open_secret(db.id).await.unwrap(),
        Some("keepme".to_string())
    );
    r.delete_in_workspace(wid, db.id).await.unwrap();
}

#[tokio::test]
async fn delete_leaves_named_secret() {
    let Some((r, ws, db, wid)) = repo_with_db(true).await else {
        eprintln!("skip");
        return;
    };
    let secret_id = ws
        .create(Some(wid), &uniq("sec"), None, "pw")
        .await
        .expect("named secret");
    let database = r
        .create_in_workspace(wid, &input(&uniq("db")), Some(secret_id), None)
        .await
        .unwrap();
    r.delete_in_workspace(wid, database.id).await.unwrap();
    // Epic B: deleting a connection drops only the reference; the shared named
    // secret (and its backing vault row) survives, with used_by back to 0.
    let meta = ws
        .get_meta(Some(wid), secret_id)
        .await
        .expect("secret survives");
    assert_eq!(meta.used_by, 0, "reference dropped");
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM workspace_secrets WHERE id = $1")
        .bind(secret_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(
        remaining, 1,
        "named secret row must remain after conn delete"
    );
}
