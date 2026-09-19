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

/// A boolean-like word, case-insensitive — always exempt, whatever the key
/// (a switch cannot be a secret, e.g. `vended-credentials-enabled=true`).
fn is_boolean_like(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "true" | "false" | "yes" | "no" | "on" | "off" | "enabled" | "disabled"
    )
}

/// Key suffixes (the segment after the last `.`/`-`/`_`) that name a
/// non-secret quantity, so a numeric value under one of them is not exempted
/// on digits alone — an all-digit password or PIN (`password=123456`,
/// `access-key=00012345`) must still be treated as a secret.
const NONSECRET_QUANTITY_SUFFIXES: &[&str] = &[
    "enabled", "interval", "ttl", "timeout", "size", "count", "seconds", "ms", "millis", "minutes",
    "retries", "port", "version",
];

/// Whether `key`'s final segment names a non-secret quantity (see
/// [`NONSECRET_QUANTITY_SUFFIXES`]).
fn key_names_nonsecret_quantity(key: &str) -> bool {
    let last_segment = key.rsplit(['.', '-', '_']).next().unwrap_or(key);
    NONSECRET_QUANTITY_SUFFIXES
        .iter()
        .any(|s| last_segment.eq_ignore_ascii_case(s))
}

/// A value that cannot be a secret regardless of the digits it contains:
/// a boolean-like word (`true`, `enabled`, …), or a number under a key whose
/// final segment clearly names a non-secret quantity (`…-ttl=60`,
/// `…-interval=300`). Every other numeric value — including an all-digit
/// password or access key — is NOT exempted here.
fn is_exempt_value(key: &str, value: &str) -> bool {
    is_boolean_like(value) || (value.parse::<f64>().is_ok() && key_names_nonsecret_quantity(key))
}

/// A non-empty, non-exempt value under a sensitive key: it must live in a
/// named secret, never in `registry.catalogs`. Digits alone do not exempt a
/// value — only a boolean-like word, or a number under a key that clearly
/// names a non-secret quantity (see [`is_exempt_value`]).
pub fn is_plaintext_secret(key: &str, value: &str) -> bool {
    is_sensitive_key(key) && !value.is_empty() && !is_exempt_value(key, value)
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

    #[test]
    fn an_all_digit_value_under_a_sensitive_key_is_still_a_secret() {
        assert!(is_plaintext_secret("password", "123456"));
        assert!(is_plaintext_secret("access-key", "00012345"));
    }

    #[test]
    fn a_quantity_suffix_exempts_a_numeric_value_under_a_sensitive_key() {
        assert!(!is_plaintext_secret(
            "iceberg.rest-catalog.vended-credentials-enabled",
            "true"
        ));
        assert!(!is_plaintext_secret("oauth2.token-refresh-interval", "300"));
        assert!(!is_plaintext_secret("credential-cache-ttl", "60"));
    }
}
