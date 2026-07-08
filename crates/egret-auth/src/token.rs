//! Opaque session tokens. The raw token lives only in the cookie; the DB stores
//! its hash. Thin wrappers over the shared [`egret_core::token`] primitives so
//! the minting format and hashing algorithm are defined once workspace-wide.

pub use egret_core::token::hash_token;

/// Generate a fresh 256-bit url-safe token.
pub fn generate_token() -> String {
    egret_core::token::random_token_b64()
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
