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
    // `secrets` no longer stores an inline value: the typed-secrets migration
    // (0088) dropped `nonce`/`ciphertext` in favor of the `secret_data` table.
    // This test only needs a valid `secrets` row as the FK target.
    let secret_id: Uuid = sqlx::query_scalar(
        "INSERT INTO secrets (kind, key_id) \
         VALUES ('oidc_refresh', 'k1') RETURNING id",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();

    let repo = OidcRefreshRepo::new(db);
    assert!(repo.get(user_id).await.unwrap().is_none());
    repo.set(user_id, secret_id).await.unwrap();
    assert_eq!(repo.get(user_id).await.unwrap(), Some(secret_id));
}
