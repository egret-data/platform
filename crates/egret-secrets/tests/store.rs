//! DB-gated SecretStore tests. Self-skip without DATABASE_URL.
use egret_metastore::Db;
use egret_secrets::{Crypto, SecretStore};

fn crypto() -> Crypto {
    Crypto::new(&[7u8; 32]).unwrap()
}

async fn store() -> Option<SecretStore> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(SecretStore::new(db, Some(crypto())))
}

#[tokio::test]
async fn put_get_delete_roundtrip() {
    let Some(s) = store().await else {
        eprintln!("skip");
        return;
    };
    let id = s.put("db_password", "p@ss").await.unwrap();
    assert_eq!(s.get(id).await.unwrap(), "p@ss");
    s.delete(id).await.unwrap();
    assert!(s.get(id).await.is_err());
}

#[tokio::test]
async fn put_without_key_errors() {
    let Some(s) = store().await else {
        eprintln!("skip");
        return;
    };
    let no_key = SecretStore::new(s.db_handle(), None);
    let err = no_key.put("db_password", "x").await.unwrap_err();
    assert!(matches!(err, egret_secrets::SecretsError::NotConfigured));
}
