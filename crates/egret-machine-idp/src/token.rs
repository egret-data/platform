//! JWT signing + (test/verify) validation for the mini machine-IdP.

use crate::error::IdpError;
use crate::keys::KeyMaterial;
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};

/// Sign `claims` (an arbitrary JSON object) as an RS256 JWT with `km`'s key,
/// stamping the `kid` header.
pub fn sign_claims(km: &KeyMaterial, claims: &serde_json::Value) -> Result<String, IdpError> {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(km.kid.clone());
    let key = EncodingKey::from_rsa_pem(km.private_pem.as_bytes())
        .map_err(|e| IdpError::Crypto(e.to_string()))?;
    encode(&header, claims, &key).map_err(|e| IdpError::Crypto(e.to_string()))
}

/// Verify `token`'s signature against a public JWK (`n`/`e`) and return its
/// claims. Audience + expiry validation are disabled — this is a
/// signature/round-trip check (Polaris/lakekeeper enforce aud/exp themselves).
pub fn verify_with_jwk(
    token: &str,
    jwk: &serde_json::Value,
) -> Result<serde_json::Value, IdpError> {
    let n = jwk["n"]
        .as_str()
        .ok_or_else(|| IdpError::Crypto("jwk missing n".into()))?;
    let e = jwk["e"]
        .as_str()
        .ok_or_else(|| IdpError::Crypto("jwk missing e".into()))?;
    let key =
        DecodingKey::from_rsa_components(n, e).map_err(|e| IdpError::Crypto(e.to_string()))?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.validate_aud = false;
    validation.validate_exp = false;
    validation.required_spec_claims.clear();
    let data = decode::<serde_json::Value>(token, &key, &validation)
        .map_err(|e| IdpError::Crypto(e.to_string()))?;
    Ok(data.claims)
}

/// Verify `token` against any of `jwks` (by `kid`), require `iss == issuer` and an
/// unexpired `exp`. Audience is NOT validated here; the caller checks it.
///
/// Every way the token can fail — malformed, no `kid`, a `kid` not in `jwks`, a
/// bad signature, another issuer, expired, a missing `iss`/`exp`/`sub` — is
/// [`IdpError::InvalidGrant`]: the token is the grant, and it is not good. No
/// detail is carried, so nothing about the presented token reaches a log line.
pub fn verify_issued(
    token: &str,
    jwks: &[serde_json::Value],
    issuer: &str,
) -> Result<serde_json::Value, IdpError> {
    let header = jsonwebtoken::decode_header(token).map_err(|_| IdpError::InvalidGrant)?;
    let kid = header.kid.ok_or(IdpError::InvalidGrant)?;
    let jwk = jwks
        .iter()
        .find(|k| k["kid"].as_str() == Some(kid.as_str()))
        .ok_or(IdpError::InvalidGrant)?;
    let (Some(n), Some(e)) = (jwk["n"].as_str(), jwk["e"].as_str()) else {
        return Err(IdpError::InvalidGrant);
    };
    let key = DecodingKey::from_rsa_components(n, e).map_err(|_| IdpError::InvalidGrant)?;
    // RS256 only — `Validation::new` pins the algorithm, so a token whose header
    // names another one is refused rather than verified under it.
    let mut validation = Validation::new(Algorithm::RS256);
    validation.validate_aud = false;
    validation.validate_exp = true;
    // No grace: this IdP issued the token on its own clock, and the exchange is
    // the path a client takes BEFORE its token runs out.
    validation.leeway = 0;
    validation.set_issuer(&[issuer]);
    validation.set_required_spec_claims(&["exp", "iss", "sub"]);
    let data = decode::<serde_json::Value>(token, &key, &validation)
        .map_err(|_| IdpError::InvalidGrant)?;
    Ok(data.claims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::generate_rsa_key;

    #[test]
    fn generate_key_signs_and_verifies() {
        let km = generate_rsa_key("kid-1").unwrap();
        assert_eq!(km.public_jwk["kty"], "RSA");
        assert_eq!(km.public_jwk["kid"], "kid-1");
        let tok = sign_claims(
            &km,
            &serde_json::json!({"sub":"c","aud":"catalog","exp":9_999_999_999u64}),
        )
        .unwrap();
        let claims = verify_with_jwk(&tok, &km.public_jwk).unwrap();
        assert_eq!(claims["sub"], "c");
        assert_eq!(claims["aud"], "catalog");
    }
}
