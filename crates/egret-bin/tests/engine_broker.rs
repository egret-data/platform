use egret_auth::oidc_refresh::OidcRefreshRepo;
use egret_bin::engine_broker::{EngineTokenBroker, OidcTokenRefresher, RefreshedToken};
use egret_metastore::Db;
use egret_notebook::engine_token::EngineTokenMinter;
use egret_secrets::{Crypto, SecretStore};
use uuid::Uuid;

struct FakeRefresher;
#[async_trait::async_trait]
impl OidcTokenRefresher for FakeRefresher {
    async fn refresh(&self, refresh_token: &str) -> Result<RefreshedToken, String> {
        assert_eq!(refresh_token, "stored-rt");
        Ok(RefreshedToken {
            access_token: "new.access".into(),
            refresh_token: Some("rotated-rt".into()),
            expires_in_secs: 300,
        })
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn mint_uses_stored_token_and_rotates(pool: sqlx::PgPool) {
    let db = Db { pool };
    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, external_subject, email, name) \
         VALUES ('oidc','s','u@x','U') RETURNING id",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();

    let crypto = Crypto::new(&[7u8; 32]).unwrap();
    let secrets = SecretStore::new(db.clone(), Some(crypto));
    let repo = OidcRefreshRepo::new(db.clone());
    let sid = secrets.put("oidc_refresh", "stored-rt").await.unwrap();
    repo.set(user_id, sid).await.unwrap();

    let broker = EngineTokenBroker::new(secrets.clone(), repo.clone(), FakeRefresher);
    let tok = broker.mint(user_id).await.unwrap();
    assert_eq!(tok.access_token, "new.access");
    // rotated refresh token persisted back into the same secret row
    assert_eq!(secrets.get(sid).await.unwrap(), "rotated-rt");
}

/// A refresher that must never be reached: the broker should short-circuit when
/// the user has no stored refresh token.
struct PanicRefresher;
#[async_trait::async_trait]
impl OidcTokenRefresher for PanicRefresher {
    async fn refresh(&self, _refresh_token: &str) -> Result<RefreshedToken, String> {
        panic!("IdP refresh must not be called when there is no stored refresh token");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn mint_without_stored_token_errors_before_idp(pool: sqlx::PgPool) {
    let db = Db { pool };
    // User exists, but NO oidc_refresh_tokens row.
    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, external_subject, email, name) \
         VALUES ('oidc','s2','u2@x','U2') RETURNING id",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();

    let crypto = Crypto::new(&[7u8; 32]).unwrap();
    let secrets = SecretStore::new(db.clone(), Some(crypto));
    let repo = OidcRefreshRepo::new(db.clone());

    let broker = EngineTokenBroker::new(secrets, repo, PanicRefresher);
    let err = broker.mint(user_id).await.unwrap_err();
    assert!(matches!(
        err,
        egret_notebook::engine_token::BrokerError::NoRefreshToken
    ));
}
