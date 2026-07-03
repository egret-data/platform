//! RSA signing-key generation + JWK export for the mini machine-IdP.

use crate::error::IdpError;
use base64::Engine as _;
use rsa::pkcs8::EncodePrivateKey as _;
use rsa::traits::PublicKeyParts as _;
use rsa::{RsaPrivateKey, RsaPublicKey};

/// A signing key: its `kid`, the PKCS#8 PEM private key (for signing), and the
/// public JWK (for JWKS / verification).
pub struct KeyMaterial {
    pub kid: String,
    pub private_pem: String,
    pub public_jwk: serde_json::Value,
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Generate a fresh RSA-2048 signing key + its public JWK (`kty=RSA`, `alg=RS256`).
pub fn generate_rsa_key(kid: &str) -> Result<KeyMaterial, IdpError> {
    let mut rng = rand::thread_rng();
    let priv_key =
        RsaPrivateKey::new(&mut rng, 2048).map_err(|e| IdpError::Crypto(e.to_string()))?;
    let private_pem = priv_key
        .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
        .map_err(|e| IdpError::Crypto(e.to_string()))?
        .to_string();
    let pub_key = RsaPublicKey::from(&priv_key);
    let n = b64url(&pub_key.n().to_bytes_be());
    let e = b64url(&pub_key.e().to_bytes_be());
    let public_jwk = serde_json::json!({
        "kty": "RSA",
        "use": "sig",
        "alg": "RS256",
        "kid": kid,
        "n": n,
        "e": e,
    });
    Ok(KeyMaterial {
        kid: kid.to_string(),
        private_pem,
        public_jwk,
    })
}
