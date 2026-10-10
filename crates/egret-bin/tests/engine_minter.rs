//! MachineIdpEngineMinter mints an egret-signed engine JWT for ANY user —
//! including a local user with no refresh token (the regression this fixes:
//! the Keycloak-refresh broker cannot mint for local/password logins).

use egret_analytics_api::engine_token::EngineTokenMinter;
use egret_auth::groups::GroupRepo;
use egret_auth::users::UserRepo;
use egret_bin::engine_minter::MachineIdpEngineMinter;
use egret_machine_idp::{PgSigningKeyStore, SigningKeyStore as _};
use egret_metastore::Db;

#[sqlx::test(migrations = "../../migrations")]
async fn mints_egret_signed_token_for_local_user(pool: sqlx::PgPool) {
    let db = Db { pool };

    // Seed a LOCAL user (provider='local') — no `oidc_refresh_tokens` row
    // exists for this user, which is exactly the case that the Keycloak
    // refresh-grant broker cannot mint for.
    let users = UserRepo::new(db.clone());
    let user_id = users
        .insert_local("admin@egret.local", "Admin", "hash")
        .await
        .expect("insert local user");

    let groups = GroupRepo::new(db.clone());
    let keys = PgSigningKeyStore::new(db.clone());

    let minter = MachineIdpEngineMinter::new(
        keys.clone(),
        users.clone(),
        groups,
        "https://egret.example".to_string(),
        900,
    );

    let before = chrono::Utc::now();
    let tok = minter
        .mint(user_id.0)
        .await
        .expect("mint must succeed with NO refresh token");

    let km = keys.active().await.expect("active signing key");
    let claims = egret_machine_idp::verify_with_jwk(&tok.access_token, &km.public_jwk)
        .expect("verify egret-signed engine token");

    assert_eq!(claims["aud"], "engine");
    // `preferred_username` is the ensure_username slug derived from the email
    // local-part ("admin@egret.local" -> "admin").
    assert_eq!(claims["preferred_username"], "admin");
    assert_eq!(claims["sub"], "admin");
    assert_eq!(claims["iss"], "https://egret.example");
    assert_eq!(claims["groups"], serde_json::json!([]));
    assert!(tok.expires_at > before);
}

#[sqlx::test(migrations = "../../migrations")]
async fn minted_token_carries_user_groups_from_db(pool: sqlx::PgPool) {
    let db = Db { pool };

    let users = UserRepo::new(db.clone());
    let user_id = users
        .insert_local("dana@egret.local", "Dana", "hash")
        .await
        .expect("insert local user");

    // Create a local group and add the user to it, so the group flows through
    // the real `groups_of_user` DB path into the minted claim set.
    let groups = GroupRepo::new(db.clone());
    let group_id = groups
        .create_local("data-team")
        .await
        .expect("create local group");
    groups
        .add_member(group_id, user_id)
        .await
        .expect("add member");

    let keys = PgSigningKeyStore::new(db.clone());
    let minter = MachineIdpEngineMinter::new(
        keys.clone(),
        users,
        groups,
        "https://egret.example".to_string(),
        900,
    );

    let tok = minter.mint(user_id.0).await.expect("mint");

    let km = keys.active().await.expect("active signing key");
    let claims = egret_machine_idp::verify_with_jwk(&tok.access_token, &km.public_jwk)
        .expect("verify egret-signed engine token");

    assert_eq!(claims["groups"], serde_json::json!(["data-team"]));
    assert_eq!(claims["preferred_username"], "dana");
}

/// `#[sqlx::test]` resolves `DATABASE_URL` through dotenvy, so an unexported
/// variable does not skip these tests — it silently points them at the database
/// `.env` names, which is the DEV one. See
/// `egret_test_db::assert_database_url_is_exported`.
#[test]
fn database_url_is_exported_rather_than_read_from_dotenv() {
    egret_test_db::assert_database_url_is_exported();
}
