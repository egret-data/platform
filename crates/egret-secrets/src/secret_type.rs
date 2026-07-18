//! Typed-secret registry: k8s-style types and the data keys each requires.
//! P0 ships Opaque + TLS + basic-auth; adding a type later is one arm here
//! (plus, in a later phase, a k8s-projection mapping) — no schema change.

use std::collections::BTreeMap;

use crate::error::SecretsError;

pub const OPAQUE: &str = "Opaque";
pub const TLS: &str = "kubernetes.io/tls";
pub const BASIC_AUTH: &str = "kubernetes.io/basic-auth";

/// Required keys for a registered type; `Opaque` allows any keys. Returns `None`
/// for an unregistered type.
fn required_keys(r#type: &str) -> Option<&'static [&'static str]> {
    match r#type {
        OPAQUE => Some(&[]),
        TLS => Some(&["tls.crt", "tls.key"]),
        BASIC_AUTH => Some(&["username", "password"]),
        _ => None,
    }
}

/// Validate `data` against `type`: the type must be registered, and each of its
/// required keys must be present and non-empty.
pub fn validate(r#type: &str, data: &BTreeMap<String, String>) -> Result<(), SecretsError> {
    let required = required_keys(r#type).ok_or_else(|| SecretsError::InvalidType(r#type.into()))?;
    for k in required {
        if data.get(*k).map(String::is_empty).unwrap_or(true) {
            return Err(SecretsError::MissingKey((*k).into()));
        }
    }
    Ok(())
}
