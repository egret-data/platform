//! DB-gated DatabaseRepo tests. Self-skip without DATABASE_URL.
//!
//! Covers the workspace-scoped repo methods' secret lifecycle (ADR-19): create
//! seals, update rotates / preserves, delete removes the backing secret row,
//! and a secret without a configured master key errors. Cross-workspace
//! ownership isolation lives in `repo_workspace_scope.rs`.
use egret_database::{DatabaseRepo, Engine};
use egret_metastore::Db;
use egret_secrets::{Crypto, SecretStore};

fn input(name: &str, secret: Option<&str>) -> egret_database::model::DatabaseInput {
    egret_database::model::DatabaseInput {
        name: name.into(),
        engine: Engine::Trino,
        host: "localhost".into(),
        port: 8080,
        options: egret_database::model::ConnOptions {
            user: Some("admin".into()),
            http_scheme: Some("http".into()),
            ..Default::default()
        },
        secret: secret.map(|s| s.into()),
    }
}

/// Returns the repo, the raw Db handle, and a freshly-seeded workspace id to own
/// every database created in the test.
async fn repo_with_db(with_key: bool) -> Option<(DatabaseRepo, Db, uuid::Uuid)> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let crypto = with_key.then(|| Crypto::new(&[3u8; 32]).unwrap());
    let wid = seed_workspace(&db).await;
    Some((
        DatabaseRepo::new(db.clone(), SecretStore::new(db.clone(), crypto)),
        db,
        wid,
    ))
}

async fn repo(with_key: bool) -> Option<(DatabaseRepo, uuid::Uuid)> {
    repo_with_db(with_key).await.map(|(r, _db, wid)| (r, wid))
}

async fn seed_workspace(db: &Db) -> uuid::Uuid {
    let uid: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, name, provider, status, password_hash) \
         VALUES ($1,'t','local','active','x') RETURNING id",
    )
    .bind(format!("u_{}@t.local", uuid::Uuid::new_v4().simple()))
    .fetch_one(&db.pool)
    .await
    .expect("user");
    sqlx::query_scalar(
        "INSERT INTO workspaces (slug, name, created_by) VALUES ($1,'t',$2) RETURNING id",
    )
    .bind(format!("w-{}", uuid::Uuid::new_v4().simple()))
    .bind(uid)
    .fetch_one(&db.pool)
    .await
    .expect("ws")
}

fn uniq(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}

#[tokio::test]
async fn register_list_get_delete() {
    let Some((r, wid)) = repo(true).await else {
        eprintln!("skip");
        return;
    };
    let name = uniq("db");
    let db = r
        .create_in_workspace(wid, &input(&name, Some("pw")), None)
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
        egret_database::DatabaseError::NotFound(_)
    ));
}

#[tokio::test]
async fn duplicate_name_conflicts() {
    let Some((r, wid)) = repo(true).await else {
        eprintln!("skip");
        return;
    };
    let name = uniq("db");
    let a = r
        .create_in_workspace(wid, &input(&name, None), None)
        .await
        .unwrap();
    let err = r
        .create_in_workspace(wid, &input(&name, None), None)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        egret_database::DatabaseError::AlreadyExists(_)
    ));
    r.delete_in_workspace(wid, a.id).await.unwrap();
}

#[tokio::test]
async fn update_rotates_secret() {
    let Some((r, wid)) = repo(true).await else {
        eprintln!("skip");
        return;
    };
    let name = uniq("db");
    let db = r
        .create_in_workspace(wid, &input(&name, Some("old")), None)
        .await
        .unwrap();
    let updated = r
        .update_in_workspace(wid, db.id, &input(&name, Some("new")), None)
        .await
        .unwrap();
    assert!(updated.has_secret);
    assert_eq!(r.open_secret(db.id).await.unwrap(), Some("new".to_string()));
    r.delete_in_workspace(wid, db.id).await.unwrap();
}

#[tokio::test]
async fn register_with_secret_without_key_errors() {
    let Some((r, wid)) = repo(false).await else {
        eprintln!("skip");
        return;
    };
    let err = r
        .create_in_workspace(wid, &input(&uniq("db"), Some("pw")), None)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        egret_database::DatabaseError::SecretsNotConfigured
    ));
}

#[tokio::test]
async fn update_without_secret_preserves_old() {
    let Some((r, wid)) = repo(true).await else {
        eprintln!("skip");
        return;
    };
    let name = uniq("db");
    let db = r
        .create_in_workspace(wid, &input(&name, Some("keepme")), None)
        .await
        .unwrap();
    let mut inp = input(&name, None); // no secret supplied
    inp.host = "newhost".into();
    let updated = r.update_in_workspace(wid, db.id, &inp, None).await.unwrap();
    assert_eq!(updated.host, "newhost");
    assert!(
        updated.has_secret,
        "secret must be preserved across a no-secret update"
    );
    assert_eq!(
        r.open_secret(db.id).await.unwrap(),
        Some("keepme".to_string())
    );
    r.delete_in_workspace(wid, db.id).await.unwrap();
}

#[tokio::test]
async fn delete_removes_secret_row() {
    let Some((r, db, wid)) = repo_with_db(true).await else {
        eprintln!("skip");
        return;
    };
    let database = r
        .create_in_workspace(wid, &input(&uniq("db"), Some("pw")), None)
        .await
        .unwrap();
    let secret_id: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT secret_id FROM databases WHERE id = $1")
            .bind(database.id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let secret_id = secret_id.expect("secret_id should be set");
    r.delete_in_workspace(wid, database.id).await.unwrap();
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM secrets WHERE id = $1")
        .bind(secret_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(remaining, 0, "delete must remove the backing secret row");
}
