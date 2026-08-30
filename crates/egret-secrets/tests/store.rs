//! DB-gated SecretStore tests. Self-skip without DATABASE_URL ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAIL.
use egret_metastore::Db;
use egret_secrets::{Crypto, SecretStore};

fn crypto() -> Crypto {
    Crypto::new(&[7u8; 32]).unwrap()
}

async fn store() -> Option<SecretStore> {
    let url = egret_test_db::url().await?;
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

// P1b: a basic-auth secret has no `value` key; the single-value `get` shim must
// fall back to `password` so legacy `resolve_plaintext` readers keep working.
#[tokio::test]
async fn get_falls_back_to_password_for_basic_auth() {
    let Some(s) = store().await else {
        eprintln!("skip");
        return;
    };
    let data = std::collections::BTreeMap::from([
        ("username".to_string(), "ak".to_string()),
        ("password".to_string(), "sk".to_string()),
    ]);
    let id = s
        .put_typed("cfg", egret_secrets::secret_type::BASIC_AUTH, &data)
        .await
        .unwrap();
    assert_eq!(s.get(id).await.unwrap(), "sk");
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

#[tokio::test]
async fn typed_roundtrips_and_validates() {
    let Some(store) = store().await else {
        eprintln!("skip");
        return;
    };
    use std::collections::BTreeMap;

    // Opaque multi-key round-trip.
    let mut opaque = BTreeMap::new();
    opaque.insert("a".into(), "1".into());
    opaque.insert("b".into(), "2".into());
    let id = store
        .put_typed("cfg", egret_secrets::secret_type::OPAQUE, &opaque)
        .await
        .unwrap();
    let got = store.get_typed(id).await.unwrap();
    assert_eq!(got.r#type, "Opaque");
    assert_eq!(got.data.get("a").unwrap(), "1");
    assert_eq!(store.get_key(id, "b").await.unwrap(), "2");

    // TLS round-trip.
    let mut tls = BTreeMap::new();
    tls.insert("tls.crt".into(), "CERT".into());
    tls.insert("tls.key".into(), "KEY".into());
    let tid = store
        .put_typed("cfg", egret_secrets::secret_type::TLS, &tls)
        .await
        .unwrap();
    assert_eq!(
        store.get_typed(tid).await.unwrap().r#type,
        "kubernetes.io/tls"
    );

    // Validation: TLS missing tls.key → MissingKey; unknown type → InvalidType.
    let mut bad = BTreeMap::new();
    bad.insert("tls.crt".into(), "CERT".into());
    assert!(store
        .put_typed("cfg", egret_secrets::secret_type::TLS, &bad)
        .await
        .is_err());
    assert!(store.put_typed("cfg", "not/a-type", &opaque).await.is_err());

    // Backward-compat shim: put/get still round-trip a single value as Opaque{value}.
    let sid = store.put("legacy", "hunter2").await.unwrap();
    assert_eq!(store.get(sid).await.unwrap(), "hunter2");
    let shimmed = store.get_typed(sid).await.unwrap();
    assert_eq!(shimmed.r#type, "Opaque");
    assert_eq!(shimmed.data.get("value").unwrap(), "hunter2");
    store.update(sid, "hunter3").await.unwrap();
    assert_eq!(store.get(sid).await.unwrap(), "hunter3");
}
