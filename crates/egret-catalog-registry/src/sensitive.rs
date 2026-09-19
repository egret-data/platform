//! Which property keys name secret material (spec SP1 §4.9), and which
//! key/value pairs are plaintext secrets that must not be stored in a catalog.

use std::sync::OnceLock;

/// Whether a property key names secret material.
pub fn is_sensitive_key(key: &str) -> bool {
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    PATTERN
        .get_or_init(|| {
            regex::Regex::new(
                r"(?i)(password|secret|token|credential|private[-_.]?key|access[-_.]?key)",
            )
            .expect("static sensitive-key pattern compiles")
        })
        .is_match(key)
}

/// A value that is a switch or a number, not a secret — so
/// `iceberg.rest-catalog.vended-credentials-enabled=true` and
/// `iceberg.rest-catalog.oauth2.token-refresh-enabled=false` stay plain.
fn is_flag_or_number(value: &str) -> bool {
    value.eq_ignore_ascii_case("true")
        || value.eq_ignore_ascii_case("false")
        || value.parse::<f64>().is_ok()
}

/// A non-empty, non-flag value under a sensitive key: it must live in a named
/// secret, never in `registry.catalogs`.
pub fn is_plaintext_secret(key: &str, value: &str) -> bool {
    is_sensitive_key(key) && !value.is_empty() && !is_flag_or_number(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_are_secrets_and_switches_are_not() {
        assert!(is_plaintext_secret("connection-password", "hunter2"));
        assert!(is_plaintext_secret("s3.aws-secret-key", "abc"));
        assert!(!is_plaintext_secret("connection-password", ""));
        assert!(!is_plaintext_secret(
            "iceberg.rest-catalog.vended-credentials-enabled",
            "true"
        ));
        assert!(!is_plaintext_secret(
            "iceberg.rest-catalog.oauth2.token-refresh-enabled",
            "FALSE"
        ));
        assert!(!is_plaintext_secret("connection-user", "reader"));
        assert!(is_sensitive_key("PRIVATE_KEY"));
    }
}
