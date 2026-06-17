//! Opaque session tokens. The raw token lives only in the cookie; the DB stores its hash.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};

/// Generate a fresh 256-bit url-safe token.
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Hash a token for storage/lookup (sha-256, hex).
pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_unique() {
        assert_ne!(generate_token(), generate_token());
    }

    #[test]
    fn hash_is_deterministic_and_hex() {
        let t = generate_token();
        assert_eq!(hash_token(&t), hash_token(&t));
        assert_eq!(hash_token(&t).len(), 64); // sha-256 hex
    }
}
