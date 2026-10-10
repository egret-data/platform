//! Ranger seed policies for impersonate-mode delegation. `egret_delegator` may
//! impersonate any user (the trusted service user); every user may set the
//! `query_max_run_time` system session property on their own session (the
//! connector sets it on each statement). Both are stored as real, auditable
//! policies via `PolicyStore::create_policy` at provision time.

use serde_json::{json, Value};

/// The Trino service user egret authenticates as for impersonate-mode delegation.
pub const DELEGATOR_USERNAME: &str = "egret_delegator";
// Declared in `egret_policy_api::infra_names` so the guard, the backfill and this
// producer read one list. Re-exported rather than moved outright: `engines.rs`
// refers to them as `dp::IMPERSONATE_POLICY_NAME` / `dp::QUERY_TIMEOUT_POLICY_NAME`
// and those call sites are correct as they stand.
pub use egret_policy_api::infra_names::{IMPERSONATE_POLICY_NAME, QUERY_TIMEOUT_POLICY_NAME};
const QUERY_TIMEOUT_PROPERTY: &str = "query_max_run_time";

fn policy(service: &str, name: &str, resources: Value, users: Value, access: &str) -> Value {
    json!({
        "name": name,
        "service": service,
        "serviceType": "trino",
        "policyType": 0,
        "isEnabled": true,
        "resources": resources,
        "policyItems": [{
            "users": users,
            "groups": [],
            "roles": [],
            "accesses": [{ "type": access, "isAllowed": true }],
            "delegateAdmin": false
        }],
        "denyPolicyItems": [],
        "allowExceptions": [],
        "denyExceptions": [],
        "isDenyAllElse": false,
        "policyPriority": 0
    })
}

/// `egret_delegator` may impersonate any user. Only the delegator holds this
/// grant; its password lives only in an egret secret, so no end-user can use it.
pub fn delegator_impersonate_policy(service: &str) -> Value {
    policy(
        service,
        IMPERSONATE_POLICY_NAME,
        json!({ "trinouser": { "values": ["*"], "isExcludes": false, "isRecursive": false } }),
        json!([DELEGATOR_USERNAME]),
        "impersonate",
    )
}

/// Every user may `alter` the `query_max_run_time` system property on their own
/// session (the effective query user is the impersonated end-user). Not an
/// impersonation grant — no cross-user identity or data access.
pub fn query_timeout_policy(service: &str) -> Value {
    policy(
        service,
        QUERY_TIMEOUT_POLICY_NAME,
        json!({ "systemproperty": { "values": [QUERY_TIMEOUT_PROPERTY], "isExcludes": false, "isRecursive": false } }),
        json!(["{USER}"]),
        "alter",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impersonate_policy_grants_delegator_impersonate_any_user() {
        let p = delegator_impersonate_policy("rgr2");
        assert_eq!(p["service"], "rgr2");
        assert_eq!(p["policyType"], 0);
        assert_eq!(p["resources"]["trinouser"]["values"][0], "*");
        let item = &p["policyItems"][0];
        assert_eq!(item["users"][0], "egret_delegator");
        assert_eq!(item["accesses"][0]["type"], "impersonate");
        assert_eq!(item["accesses"][0]["isAllowed"], true);
    }

    #[test]
    fn query_timeout_policy_lets_each_user_set_the_property() {
        let p = query_timeout_policy("rgr2");
        assert_eq!(
            p["resources"]["systemproperty"]["values"][0],
            "query_max_run_time"
        );
        let item = &p["policyItems"][0];
        assert_eq!(item["users"][0], "{USER}");
        assert_eq!(item["accesses"][0]["type"], "alter");
    }

    #[test]
    fn seed_names_are_stable() {
        assert_eq!(IMPERSONATE_POLICY_NAME, "egret-delegator-impersonate");
        assert_eq!(QUERY_TIMEOUT_POLICY_NAME, "egret-query-timeout");
    }
}
