//! Engine delegation facts as pure functions of `(engine type, stored spec)`.
//!
//! These are the predicates the engine apply path uses to decide whether an
//! engine delegates per-user queries and which service user it authenticates
//! as. They live here, free of any engine-management dependency, so the
//! permission compiler can ask the same questions of a stored spec.
//! `egret_engine_mgmt::Engine`'s delegation methods delegate to this module.

use serde_json::Value;

/// `origin` of an engine registered by address rather than provisioned by egret.
pub const ORIGIN_EXTERNAL: &str = "external";

fn is_starrocks(engine_type: &str) -> bool {
    engine_type == "starrocks"
}

/// Whether the engine accepts a minted per-user engine JWT.
///
/// Trino: the spec's `trinoConfig.auth.jwt` is an object (mirrors
/// `resolve_auth`). StarRocks has no such block: jwt-capable means the stored
/// delegation mode is `jwt`.
pub fn supports_jwt_delegation(engine_type: &str, spec: &Value) -> bool {
    if is_starrocks(engine_type) {
        delegation_mode(engine_type, spec) == "jwt"
    } else {
        spec.get("trinoConfig")
            .and_then(|tc| tc.get("auth"))
            .and_then(|a| a.get("jwt"))
            .map(|j| j.is_object())
            .unwrap_or(false)
    }
}

/// Delegation mode from the stored spec.
///
/// Trino: `trinoConfig.delegationMode`, defaulting to `impersonate` when absent
/// (back-compat: pre-delegation Trino engines default to the impersonate path).
///
/// StarRocks: `starrocksConfig.delegation.mode`. CRITICAL: unlike Trino, this
/// does NOT default to `impersonate` when absent: a StarRocks spec has no
/// `trinoConfig`, so falling through to Trino's default would make every
/// un-configured StarRocks engine report itself delegation-capable. Absent
/// delegation reports `"none"`.
pub fn delegation_mode<'a>(engine_type: &str, spec: &'a Value) -> &'a str {
    if is_starrocks(engine_type) {
        spec.get("starrocksConfig")
            .and_then(|c| c.get("delegation"))
            .and_then(|d| d.get("mode"))
            .and_then(|v| v.as_str())
            .unwrap_or("none")
    } else {
        spec.get("trinoConfig")
            .and_then(|t| t.get("delegationMode"))
            .and_then(|v| v.as_str())
            .unwrap_or("impersonate")
    }
}

/// Whether the engine can back a delegated (per-user) connection at all.
/// Impersonate mode always can; jwt mode requires JWT auth enabled.
pub fn supports_delegation(engine_type: &str, spec: &Value) -> bool {
    delegation_mode(engine_type, spec) == "impersonate"
        || supports_jwt_delegation(engine_type, spec)
}

/// Whether the Trino spec carries an auth block: `trinoConfig.auth` is a JSON
/// object, however empty. This is exactly when the apply path's `resolve_auth`
/// returns `Some`, i.e. a `password.db` exists for the delegator to log into.
pub fn trino_auth_enabled(spec: &Value) -> bool {
    spec.get("trinoConfig")
        .and_then(|tc| tc.get("auth"))
        .is_some_and(Value::is_object)
}

/// Whether applying this engine provisions the `egret_delegator` service user
/// and its impersonation grant: a Trino engine in `impersonate` mode with
/// auth, or a StarRocks engine in `impersonate` mode, whose post-Ready
/// bootstrap creates the delegator (package D).
pub fn delegator_impersonates(engine_type: &str, spec: &Value) -> bool {
    let impersonate = delegation_mode(engine_type, spec) == "impersonate";
    match engine_type {
        "trino" => impersonate && trino_auth_enabled(spec),
        "starrocks" => impersonate,
        _ => false,
    }
}

/// Whether an engine contributes infrastructure policies to its bound profile:
/// provisioned by egret (not external) and delegating.
pub fn qualifies_for_infra(engine_type: &str, origin: &str, spec: &Value) -> bool {
    origin != ORIGIN_EXTERNAL && supports_delegation(engine_type, spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn trino_mode_defaults_to_impersonate_and_starrocks_to_none() {
        assert_eq!(delegation_mode("trino", &json!({})), "impersonate");
        assert_eq!(delegation_mode("starrocks", &json!({})), "none");
        assert!(!supports_delegation("starrocks", &json!({})));
    }

    #[test]
    fn auth_enabled_needs_an_object() {
        assert!(trino_auth_enabled(&json!({"trinoConfig": {"auth": {}}})));
        assert!(!trino_auth_enabled(&json!({"trinoConfig": {"auth": null}})));
        assert!(!trino_auth_enabled(&json!({"trinoConfig": {}})));
        assert!(!trino_auth_enabled(&json!({})));
    }

    #[test]
    fn delegator_needs_trino_impersonate_and_auth() {
        let auth = json!({"trinoConfig": {"auth": {"passwordUsers": []}}});
        assert!(delegator_impersonates("trino", &auth));
        assert!(!delegator_impersonates("trino", &json!({})));
        assert!(!delegator_impersonates("starrocks", &auth));
        let jwt = json!({"trinoConfig": {"delegationMode": "jwt", "auth": {"jwt": {}}}});
        assert!(!delegator_impersonates("trino", &jwt));
        // Package D: a StarRocks engine in impersonate mode creates the delegator
        // in its bootstrap, so it impersonates with it.
        assert!(delegator_impersonates(
            "starrocks",
            &json!({"starrocksConfig": {"delegation": {"mode": "impersonate"}}})
        ));
        assert!(!delegator_impersonates(
            "starrocks",
            &json!({"starrocksConfig": {"delegation": {"mode": "jwt"}}})
        ));
        assert!(!delegator_impersonates("starrocks", &json!({})));
    }

    #[test]
    fn external_engines_never_qualify() {
        assert!(qualifies_for_infra("trino", "provisioned", &json!({})));
        assert!(!qualifies_for_infra("trino", "external", &json!({})));
    }
}
