//! Claim builder for the machine-IdP engine token: the `aud:"engine"` claim set
//! for a per-user JWT minted with egret's own key (no Keycloak, no refresh
//! token). The signing minter struct lands in a follow-up task; this file holds
//! only the pure `engine_claims` builder so the claim contract is unit-testable.

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
