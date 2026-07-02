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
pub fn verify_with_jwk(token: &str, jwk: &serde_json::Value) -> Result<serde_json::Value, IdpError> {
    let n = jwk["n"]
        .as_str()
        .ok_or_else(|| IdpError::Crypto("jwk missing n".into()))?;
    let e = jwk["e"]
        .as_str()
        .ok_or_else(|| IdpError::Crypto("jwk missing e".into()))?;
    let key = DecodingKey::from_rsa_components(n, e).map_err(|e| IdpError::Crypto(e.to_string()))?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.validate_aud = false;
    validation.validate_exp = false;
    validation.required_spec_claims.clear();
    let data = decode::<serde_json::Value>(token, &key, &validation)
        .map_err(|e| IdpError::Crypto(e.to_string()))?;
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
