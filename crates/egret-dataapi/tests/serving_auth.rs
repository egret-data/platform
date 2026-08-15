//! Data API P4 Task 2 — turning a bearer token into a principal.
//!
//! Every test here names a property [`egret_machine_idp::verify_with_jwk`] does
//! **not** give you. That function sets `validate_aud = false`,
//! `validate_exp = false` and clears `required_spec_claims`, so it proves the
//! signature and nothing else. The three checks in
//! [`egret_dataapi::serving::verify_serving_token`] are the only thing standing
//! between a served request and a token that was never meant for it.
//!
//! No database: `verify_serving_token` takes a [`SigningKeyStore`] and a string,
//! so this file asserts on every run rather than self-skipping.

use egret_core::UserId;
use egret_dataapi::serving::{verify_serving_token, ServingError};
use egret_machine_idp::{generate_rsa_key, sign_claims, IdpError, KeyMaterial, SigningKeyStore};
use std::sync::{Arc, OnceLock};
use uuid::Uuid;

/// RSA-2048 generation is expensive in a debug build, so the two keys this file
/// needs — the IdP's active key and a foreign one — are generated once for the
/// whole test binary.
fn keys() -> &'static (KeyMaterial, KeyMaterial) {
    static KEYS: OnceLock<(KeyMaterial, KeyMaterial)> = OnceLock::new();
    KEYS.get_or_init(|| {
        (
            generate_rsa_key("egret-active").expect("generate active key"),
            generate_rsa_key("not-egrets-key").expect("generate foreign key"),
        )
    })
}

fn active_key() -> &'static KeyMaterial {
    &keys().0
}

fn foreign_key() -> &'static KeyMaterial {
    &keys().1
}

/// A [`SigningKeyStore`] that publishes exactly the IdP's active key — the same
/// thing `PgSigningKeyStore` hands the serving path in production.
struct FakeKeys;

#[async_trait::async_trait]
impl SigningKeyStore for FakeKeys {
    async fn active(&self) -> Result<KeyMaterial, IdpError> {
        let km = active_key();
        Ok(KeyMaterial {
            kid: km.kid.clone(),
            private_pem: km.private_pem.clone(),
            public_jwk: km.public_jwk.clone(),
        })
    }
    async fn all_public_jwks(&self) -> Result<Vec<serde_json::Value>, IdpError> {
        Ok(vec![active_key().public_jwk.clone()])
    }
}

struct Fixture {
    keys: Arc<dyn SigningKeyStore>,
    ws: Uuid,
    user: UserId,
}

/// A timestamp safely in the past.
fn past() -> i64 {
    chrono::Utc::now().timestamp() - 3600
}

/// A timestamp safely in the future.
fn future() -> i64 {
    chrono::Utc::now().timestamp() + 600
}

async fn fixture() -> Fixture {
    Fixture {
        keys: Arc::new(FakeKeys),
        ws: Uuid::new_v4(),
        user: UserId::new(),
    }
}

impl Fixture {
    /// The exact claim shape the machine-IdP mints for a Data API client: `iss`,
    /// `sub`, `aud` and `exp` from the IdP itself, `ws` and `egret_user_id` as
    /// STRINGS from P2's `ClientStore::verify`.
    fn claims(&self, aud: &str, ws: Uuid, user: UserId) -> serde_json::Value {
        serde_json::json!({
            "iss": "https://egret.test",
            "sub": "egret-dataapi-00000000-0000-0000-0000-000000000001",
            "aud": aud,
            "exp": future(),
            "ws": ws.to_string(),
            "egret_user_id": user.0.to_string(),
        })
    }

    async fn mint_token(&self, aud: &str, ws: Uuid, user: UserId) -> String {
        sign_claims(active_key(), &self.claims(aud, ws, user)).expect("sign")
    }

    async fn mint_token_expiring_at(&self, aud: &str, ws: Uuid, user: UserId, exp: i64) -> String {
        let mut c = self.claims(aud, ws, user);
        c["exp"] = serde_json::json!(exp);
        sign_claims(active_key(), &c).expect("sign")
    }

    async fn mint_token_without_exp(&self, aud: &str, ws: Uuid, user: UserId) -> String {
        let mut c = self.claims(aud, ws, user);
        c.as_object_mut().expect("object").remove("exp");
        sign_claims(active_key(), &c).expect("sign")
    }

    async fn mint_with_foreign_key(&self, aud: &str, ws: Uuid, user: UserId) -> String {
        sign_claims(foreign_key(), &self.claims(aud, ws, user)).expect("sign")
    }

    /// Mint with one claim overridden or (on `None`) removed — for the strict
    /// parsing tests.
    async fn mint_with_claim(&self, claim: &str, value: Option<serde_json::Value>) -> String {
        let mut c = self.claims("data-api", self.ws, self.user);
        match value {
            Some(v) => c[claim] = v,
            None => {
                c.as_object_mut().expect("object").remove(claim);
            }
        }
        sign_claims(active_key(), &c).expect("sign")
    }
}

#[tokio::test]
async fn a_catalog_token_is_refused_by_the_data_api() {
    // Same signing key, different audience. verify_with_jwk sets
    // validate_aud = false, so the signature alone proves nothing about intent.
    let f = fixture().await;
    let catalog = f.mint_token("catalog", f.ws, f.user).await;
    assert!(matches!(
        verify_serving_token(&*f.keys, &catalog).await,
        Err(ServingError::Unauthorized)
    ));
}

#[tokio::test]
async fn an_expired_token_is_refused() {
    // verify_with_jwk ALSO sets validate_exp = false. Nothing but this check
    // stands between an expired credential and a served request.
    let f = fixture().await;
    let stale = f
        .mint_token_expiring_at("data-api", f.ws, f.user, past())
        .await;
    assert!(matches!(
        verify_serving_token(&*f.keys, &stale).await,
        Err(ServingError::Unauthorized)
    ));
}

#[tokio::test]
async fn a_token_with_no_exp_at_all_is_refused() {
    // required_spec_claims is cleared, so a token carrying no `exp` verifies.
    // Treat a missing exp as expired, never as "never expires".
    let f = fixture().await;
    let no_exp = f.mint_token_without_exp("data-api", f.ws, f.user).await;
    assert!(matches!(
        verify_serving_token(&*f.keys, &no_exp).await,
        Err(ServingError::Unauthorized)
    ));
}

#[tokio::test]
async fn a_valid_token_yields_its_principal_and_workspace() {
    let f = fixture().await;
    let t = f.mint_token("data-api", f.ws, f.user).await;
    let p = verify_serving_token(&*f.keys, &t).await.unwrap();
    assert_eq!(p.workspace_id, f.ws);
    assert_eq!(p.user_id, f.user);
    assert_eq!(
        p.client_id,
        "egret-dataapi-00000000-0000-0000-0000-000000000001"
    );
}

#[tokio::test]
async fn a_token_signed_by_another_key_is_refused() {
    let f = fixture().await;
    let foreign = f.mint_with_foreign_key("data-api", f.ws, f.user).await;
    assert!(matches!(
        verify_serving_token(&*f.keys, &foreign).await,
        Err(ServingError::Unauthorized)
    ));
}

/// Strict parsing: an absent or malformed identity claim is a refusal, never a
/// default. A `unwrap_or_default()` here would hand every such token
/// `Uuid::nil()` as its workspace — one shared, attacker-nameable tenant.
#[tokio::test]
async fn a_token_whose_identity_claims_are_missing_or_malformed_is_refused() {
    let f = fixture().await;
    for (claim, value) in [
        ("ws", None),
        ("ws", Some(serde_json::json!("not-a-uuid"))),
        // A UUID that arrived as something other than a string is not the shape
        // `ClientStore::verify` mints, so it is not a Data API token.
        ("ws", Some(serde_json::json!(7))),
        ("egret_user_id", None),
        ("egret_user_id", Some(serde_json::json!("not-a-uuid"))),
        ("sub", None),
        ("sub", Some(serde_json::json!(""))),
    ] {
        let t = f.mint_with_claim(claim, value.clone()).await;
        assert!(
            matches!(
                verify_serving_token(&*f.keys, &t).await,
                Err(ServingError::Unauthorized)
            ),
            "claim {claim} = {value:?} was accepted"
        );
    }
}

/// Not a JWT at all. The serving path receives whatever a caller put in the
/// Authorization header.
#[tokio::test]
async fn a_garbage_bearer_is_refused() {
    let f = fixture().await;
    for junk in ["", "not.a.jwt", "Bearer something", "a.b"] {
        assert!(matches!(
            verify_serving_token(&*f.keys, junk).await,
            Err(ServingError::Unauthorized)
        ));
    }
}
