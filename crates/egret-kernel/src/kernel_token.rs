//! Kernel callback bearer token: random plaintext + SHA-256 hash for at-rest storage.

use std::time::Duration;

use sha2::{Digest, Sha256};

/// Token lifetime backstop (the row delete on stop/reap is the primary revocation).
pub const TOKEN_TTL: Duration = Duration::from_secs(12 * 3600);

/// Mint a fresh token: returns `(plaintext, sha256_hex_hash)`.
pub fn mint() -> (String, String) {
    let raw: [u8; 32] = rand::random();
    let token = format!("egk_{}", hex::encode(raw));
    let hash = hash_token(&token);
    (token, hash)
}

/// SHA-256 hex of a token (for lookup + at-rest comparison).
pub fn hash_token(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hex::encode(h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_is_unique_and_hash_is_stable() {
        let (t1, h1) = mint();
        let (t2, _h2) = mint();
        assert_ne!(t1, t2, "tokens must be unique");
        assert!(t1.starts_with("egk_"));
        assert_eq!(hash_token(&t1), h1, "hash must be deterministic");
        assert_ne!(hash_token(&t1), hash_token(&t2));
        assert_eq!(h1.len(), 64, "sha256 hex is 64 chars");
    }
}
