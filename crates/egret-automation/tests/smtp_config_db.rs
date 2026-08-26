//! SMTP config persistence, against a real database.
//!
//! The interesting behaviour is entirely about the password: that it is never
//! stored in the table, that an omitted one is kept rather than erased, and
//! that a replaced one does not leave its predecessor lying in the vault.

use egret_automation::deliver::smtp_config::{SmtpConfigBody, SmtpConfigRepo, TlsMode};
use egret_metastore::Db;
use uuid::Uuid;

async fn seed_ws(db: &Db) -> Uuid {
    let user: Uuid =
        sqlx::query_scalar("INSERT INTO users (email, provider) VALUES ($1,'local') RETURNING id")
            .bind(format!("s_{}@t.local", Uuid::new_v4().simple()))
            .fetch_one(&db.pool)
            .await
            .unwrap();
    sqlx::query_scalar(
        "INSERT INTO workspaces (slug,name,created_by) VALUES ($1,'t',$2) RETURNING id",
    )
    .bind(format!("w-{}", Uuid::new_v4().simple()))
    .bind(user)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

fn body(password: Option<&str>) -> SmtpConfigBody {
    SmtpConfigBody {
        host: "smtp.example.com".into(),
        port: 587,
        username: password.map(|_| "mailer".to_string()),
        from_address: "alerts@example.com".into(),
        from_name: Some("Egret".into()),
        tls_mode: TlsMode::Starttls,
        password: password.map(str::to_string),
    }
}

/// A store WITH a master key. `SecretStore::new(db, None)` is the shape most
/// fixtures in this repo use, but it cannot encrypt — every write fails with
/// "master_key is not configured", which is a genuine and useful refusal.
fn repo(db: &Db) -> SmtpConfigRepo {
    let crypto = egret_secrets::Crypto::new(&[7u8; 32]).expect("32-byte test key");
    SmtpConfigRepo::new(
        db.clone(),
        egret_secrets::SecretStore::new(db.clone(), Some(crypto)),
    )
}

/// Every ciphertext row in the vault, so orphans are visible.
async fn vault_rows(db: &Db) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM secrets WHERE kind = 'automation_smtp_password'")
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_password_is_never_written_into_the_config_table(pool: sqlx::PgPool) {
    // The whole reason the column is an id. Asserted by reading the table back
    // as text and searching it, rather than by trusting the schema.
    let db = Db { pool };
    let ws = seed_ws(&db).await;
    repo(&db)
        .upsert(ws, &body(Some("hunter2")), None)
        .await
        .unwrap();

    let row_text: String = sqlx::query_scalar(
        "SELECT c::text FROM automation.smtp_config c WHERE c.workspace_id = $1",
    )
    .bind(ws)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(
        !row_text.contains("hunter2"),
        "the plaintext password must not appear anywhere in the row: {row_text}"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_password_round_trips_through_the_vault(pool: sqlx::PgPool) {
    // The other half: it is not stored, but it IS retrievable at send time —
    // otherwise "never stored" would be satisfied by losing it.
    let db = Db { pool };
    let ws = seed_ws(&db).await;
    let r = repo(&db);
    let cfg = r.upsert(ws, &body(Some("hunter2")), None).await.unwrap();

    assert_eq!(
        r.resolve_password(&cfg).await.unwrap().as_deref(),
        Some("hunter2")
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn saving_without_a_password_keeps_the_stored_one(pool: sqlx::PgPool) {
    // The editor cannot render the stored password, so it sends none. If that
    // meant "delete", every unrelated edit would silently break sending.
    let db = Db { pool };
    let ws = seed_ws(&db).await;
    let r = repo(&db);
    r.upsert(ws, &body(Some("hunter2")), None).await.unwrap();

    let mut edit = body(None);
    edit.username = Some("mailer".into()); // unchanged auth, new host
    edit.host = "smtp2.example.com".into();
    let after = r.upsert(ws, &edit, None).await.unwrap();

    assert_eq!(after.host, "smtp2.example.com");
    assert!(after.password_secret_id.is_some(), "password still present");
    assert_eq!(
        r.resolve_password(&after).await.unwrap().as_deref(),
        Some("hunter2"),
        "and it is still the same password"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_empty_password_removes_it_along_with_the_username(pool: sqlx::PgPool) {
    let db = Db { pool };
    let ws = seed_ws(&db).await;
    let r = repo(&db);
    r.upsert(ws, &body(Some("hunter2")), None).await.unwrap();

    let mut clear = body(None);
    clear.password = Some(String::new());
    clear.username = None; // the CHECK requires both or neither
    let after = r.upsert(ws, &clear, None).await.unwrap();

    assert_eq!(after.password_secret_id, None);
    assert_eq!(r.resolve_password(&after).await.unwrap(), None);
    assert_eq!(vault_rows(&db).await, 0, "the vault row is removed too");
}

#[sqlx::test(migrations = "../../migrations")]
async fn replacing_the_password_does_not_orphan_the_old_ciphertext(pool: sqlx::PgPool) {
    // A superseded credential left in the vault is a credential nobody is
    // watching and nobody will rotate.
    let db = Db { pool };
    let ws = seed_ws(&db).await;
    let r = repo(&db);
    let first = r.upsert(ws, &body(Some("old")), None).await.unwrap();
    let second = r.upsert(ws, &body(Some("new")), None).await.unwrap();

    assert_ne!(first.password_secret_id, second.password_secret_id);
    assert_eq!(
        r.resolve_password(&second).await.unwrap().as_deref(),
        Some("new")
    );
    assert_eq!(
        vault_rows(&db).await,
        1,
        "exactly one ciphertext row should remain"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn deleting_the_config_takes_the_password_with_it(pool: sqlx::PgPool) {
    let db = Db { pool };
    let ws = seed_ws(&db).await;
    let r = repo(&db);
    r.upsert(ws, &body(Some("hunter2")), None).await.unwrap();

    assert!(r.delete(ws).await.unwrap());
    assert!(r.get(ws).await.unwrap().is_none());
    assert_eq!(vault_rows(&db).await, 0, "no credential is left behind");
}

#[sqlx::test(migrations = "../../migrations")]
async fn config_is_per_workspace(pool: sqlx::PgPool) {
    // Two tenants must not share a relay identity.
    let db = Db { pool };
    let a = seed_ws(&db).await;
    let b = seed_ws(&db).await;
    let r = repo(&db);

    let mut for_b = body(Some("b-pass"));
    for_b.from_address = "b@example.com".into();
    r.upsert(a, &body(Some("a-pass")), None).await.unwrap();
    r.upsert(b, &for_b, None).await.unwrap();

    let got_a = r.get(a).await.unwrap().unwrap();
    let got_b = r.get(b).await.unwrap().unwrap();
    assert_eq!(got_a.from_address, "alerts@example.com");
    assert_eq!(got_b.from_address, "b@example.com");
    assert_eq!(
        r.resolve_password(&got_a).await.unwrap().as_deref(),
        Some("a-pass")
    );
    assert_eq!(
        r.resolve_password(&got_b).await.unwrap().as_deref(),
        Some("b-pass")
    );
}

/// `#[sqlx::test]` resolves `DATABASE_URL` through dotenvy, so an unexported
/// variable does not skip these tests — it silently points them at the database
/// `.env` names, which is the DEV one. See
/// `egret_test_db::assert_database_url_is_exported`.
#[test]
fn database_url_is_exported_rather_than_read_from_dotenv() {
    egret_test_db::assert_database_url_is_exported();
}
