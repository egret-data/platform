//! The machine-IdP engine-token minter: signs a per-user `aud:"engine"` JWT
//! with egret's own key (no Keycloak, no stored refresh token needed), so any
//! authenticated user — including local/password logins — can mint an engine
//! token. `engine_claims` builds the pure claim set; `MachineIdpEngineMinter`
//! wires it to the DB-backed username/group lookups + the machine-IdP signer.

/// Build the `aud:"engine"` claim set. Pure (no crypto/DB) so the claim contract
/// is unit-testable. `preferred_username` MUST equal `UserRepo::ensure_username`
/// so StarRocks/Trino `principal_field=preferred_username` + Ranger match.
pub(crate) fn engine_claims(
    username: &str,
    groups: &[String],
    issuer: &str,
    iat_ts: i64,
    ttl_secs: i64,
) -> serde_json::Value {
    serde_json::json!({
        "iss": issuer.trim_end_matches('/'),
        "sub": username,
        "preferred_username": username,
        "groups": groups,
        "aud": "engine",
        "iat": iat_ts,
        "exp": iat_ts + ttl_secs,
    })
}

/// Signs `aud:"engine"` JWTs with egret's own machine-IdP key. Unlike the
/// Keycloak `EngineTokenBroker`, this needs no stored refresh token — it works
/// for any user (local/password logins included), since egret is the sole
/// issuer.
pub struct MachineIdpEngineMinter {
    keys: egret_machine_idp::PgSigningKeyStore,
    users: egret_auth::users::UserRepo,
    groups: egret_auth::groups::GroupRepo,
    issuer: String,
    ttl_secs: i64,
}

impl MachineIdpEngineMinter {
    pub fn new(
        keys: egret_machine_idp::PgSigningKeyStore,
        users: egret_auth::users::UserRepo,
        groups: egret_auth::groups::GroupRepo,
        issuer: String,
        ttl_secs: i64,
    ) -> Self {
        Self {
            keys,
            users,
            groups,
            issuer,
            ttl_secs,
        }
    }
}

#[async_trait::async_trait]
impl egret_analytics_api::engine_token::EngineTokenMinter for MachineIdpEngineMinter {
    async fn mint(
        &self,
        user_id: uuid::Uuid,
    ) -> Result<
        egret_analytics_api::engine_token::EngineToken,
        egret_analytics_api::engine_token::BrokerError,
    > {
        use egret_analytics_api::engine_token::BrokerError;
        use egret_machine_idp::SigningKeyStore as _;

        let uid = egret_core::UserId(user_id);
        // DB lookups return `egret_core::Error`; `?` converts to
        // `BrokerError::Other` (via `#[from]`) — an internal DB failure, not an
        // "idp" failure. Only the machine-IdP signing path below is `Idp`.
        let username = self.users.ensure_username(uid).await?;
        let groups = self.groups.groups_of_user(uid).await?;
        let km = self
            .keys
            .active()
            .await
            .map_err(|e| BrokerError::Idp(e.to_string()))?;

        let iat = chrono::Utc::now().timestamp();
        let claims = engine_claims(&username, &groups, &self.issuer, iat, self.ttl_secs);
        let jwt = egret_machine_idp::sign_claims(&km, &claims)
            .map_err(|e| BrokerError::Idp(e.to_string()))?;
        let expires_at = chrono::DateTime::from_timestamp(iat + self.ttl_secs, 0)
            .ok_or_else(|| BrokerError::Idp("exp out of range".into()))?;

        Ok(egret_analytics_api::engine_token::EngineToken {
            access_token: jwt,
            expires_at,
        })
    }
}

/// View governance B1 (spec R5): the engine JWT for `egret-lineage`, the
/// principal the lineage EXPLAIN runs as. Same signer, issuer, audience and
/// lifetime as a user's token, and no groups; the username is the constant, not
/// an argument, so this mints for that one principal only. Nothing is stored:
/// the credential is egret's signing key (`egret_query::execution::lineage`).
#[async_trait::async_trait]
impl egret_query::execution::lineage::LineageTokenMinter for MachineIdpEngineMinter {
    async fn mint_lineage_token(&self) -> Result<String, String> {
        use egret_machine_idp::SigningKeyStore as _;
        let km = self.keys.active().await.map_err(|e| e.to_string())?;
        let claims = lineage_claims(&self.issuer, chrono::Utc::now().timestamp(), self.ttl_secs);
        egret_machine_idp::sign_claims(&km, &claims).map_err(|e| e.to_string())
    }
}

/// The `egret-lineage` claim set (pure, for the test below).
pub(crate) fn lineage_claims(issuer: &str, iat_ts: i64, ttl_secs: i64) -> serde_json::Value {
    engine_claims(
        egret_core::engine_principals::LINEAGE_PRINCIPAL,
        &[],
        issuer,
        iat_ts,
        ttl_secs,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_claims_shape() {
        let c = engine_claims(
            "alice",
            &["data-team".into()],
            "https://egret.example/",
            1_000,
            900,
        );
        assert_eq!(c["iss"], "https://egret.example");
        assert_eq!(c["sub"], "alice");
        assert_eq!(c["preferred_username"], "alice");
        assert_eq!(c["aud"], "engine");
        assert_eq!(c["groups"], serde_json::json!(["data-team"]));
        assert_eq!(c["iat"], 1_000);
        assert_eq!(c["exp"], 1_900);
    }

    #[test]
    fn the_lineage_token_names_the_lineage_principal_and_no_group() {
        let c = lineage_claims("https://egret.example/", 1_000, 900);
        assert_eq!(c["sub"], "egret-lineage");
        assert_eq!(c["preferred_username"], "egret-lineage");
        assert_eq!(c["aud"], "engine");
        assert_eq!(c["groups"], serde_json::json!([]));
        assert_eq!(c["exp"], 1_900);
    }

    #[test]
    fn engine_claims_empty_groups_is_empty_array() {
        let c = engine_claims("bob", &[], "https://e/", 5, 10);
        assert_eq!(c["groups"], serde_json::json!([]));
    }

    #[test]
    fn engine_claims_issuer_without_trailing_slash_is_unchanged() {
        let c = engine_claims("x", &[], "https://e", 0, 1);
        assert_eq!(c["iss"], "https://e");
    }
}
