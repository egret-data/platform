use egret_auth::oidc_refresh::OidcRefreshRepo;
use egret_metastore::Db;
use uuid::Uuid;

#[sqlx::test(migrations = "../../migrations")]
async fn set_then_get_roundtrips(pool: sqlx::PgPool) {
    let db = Db { pool };
    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (provider, external_subject, email, name) \
         VALUES ('oidc','sub-1','u@x','U') RETURNING id",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let secret_id: Uuid = sqlx::query_scalar(
        "INSERT INTO secrets (kind, nonce, ciphertext, key_id) \
         VALUES ('oidc_refresh', '\\x00', '\\x00', 'k1') RETURNING id",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();

    let repo = OidcRefreshRepo::new(db);
    assert!(repo.get(user_id).await.unwrap().is_none());
    repo.set(user_id, secret_id).await.unwrap();
    assert_eq!(repo.get(user_id).await.unwrap(), Some(secret_id));
}
