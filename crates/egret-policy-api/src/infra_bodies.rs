//! Infrastructure policy bodies for Ranger services, in one place: the
//! delegator's impersonate grant, the per-user baseline Trino 483's Ranger
//! plugin checks on every session (measured live in DG Task 6b, live result
//! §4.8), and the names that identify them. Both the Ranger renderer for the
//! IR's infrastructure statements and engine apply read them from here.

use serde_json::{json, Value};

use egret_core::engine_principals::{DELEGATOR_PRINCIPAL, QUERY_LOG_LOADER_PRINCIPAL};

pub use crate::infra_names::{
    EXECUTE_QUERY_POLICY_NAME, IMPERSONATE_POLICY_NAME, QUERY_LOG_DATABASE,
    QUERY_LOG_LOADER_POLICY_NAME, QUERY_LOG_TABLE, QUERY_TIMEOUT_POLICY_NAME,
    SELF_IMPERSONATE_POLICY_NAME,
};

/// The one system session property egret's connector sets on each statement.
pub const QUERY_TIMEOUT_PROPERTY: &str = "query_max_run_time";
/// Ranger's `RangerPolicyEngine.USER_CURRENT`: the requesting user.
const USER_MACRO: &str = "{USER}";

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
        json!([DELEGATOR_PRINCIPAL]),
        "impersonate",
    )
}

/// Every user may become THEMSELF: `trinouser = {USER}`, `impersonate`, users
/// `{USER}`. Trino 483 checks `checkCanSetUser` as an `impersonate` on
/// `trinouser=<user>` for every session, principal == user included; without
/// this a query fails `Principal u cannot become user u`.
///
/// Self only, as measured — not as a claim about Ranger's matcher. On the
/// real 2.5.0 and 2.8.0 engines `tests/ranger`'s `EgretUserTokenWildcardTest`
/// finds that a requester whose name is a wildcard (`*`, `a*`) is denied
/// becoming anyone else; those rows do not establish HOW `{USER}` is matched.
/// On Trino 483 (live verification §4.16, step 4a,
/// `docs/en/plans/2026-10-13-view-governance-live-result.md`) A→B is denied
/// (`Principal vcplain cannot become user vcownerc`) and A→A succeeds. The
/// match is case-insensitive (`trinouser` is `ignoreCase`);
/// `users.username` is `citext UNIQUE`, so no two egret users differ only by
/// case.
pub fn self_impersonate_policy(service: &str) -> Value {
    policy(
        service,
        SELF_IMPERSONATE_POLICY_NAME,
        json!({ "trinouser": { "values": [USER_MACRO], "isExcludes": false, "isRecursive": false } }),
        json!([USER_MACRO]),
        "impersonate",
    )
}

/// Every user may execute queries: `queryid = *`, `execute`, users `{USER}`.
/// Without it: `Access Denied: Cannot execute query`.
pub fn execute_query_policy(service: &str) -> Value {
    policy(
        service,
        EXECUTE_QUERY_POLICY_NAME,
        json!({ "queryid": { "values": ["*"], "isExcludes": false, "isRecursive": false } }),
        json!([USER_MACRO]),
        "execute",
    )
}

/// The per-user baseline, `(name, body)`, in the order Trino checks it on a
/// session: set user, execute query, system session property.
pub fn user_baseline_policies(service: &str) -> [(&'static str, Value); 3] {
    [
        (
            SELF_IMPERSONATE_POLICY_NAME,
            self_impersonate_policy(service),
        ),
        (EXECUTE_QUERY_POLICY_NAME, execute_query_policy(service)),
        (QUERY_TIMEOUT_POLICY_NAME, query_timeout_policy(service)),
    ]
}

/// Every user may `alter` the `query_max_run_time` system property on their own
/// session (egret's connector sets it on each statement). Without it:
/// `Cannot set system session property query_max_run_time`. Not an
/// impersonation grant — no cross-user identity or data access.
pub fn query_timeout_policy(service: &str) -> Value {
    policy(
        service,
        QUERY_TIMEOUT_POLICY_NAME,
        json!({ "systemproperty": { "values": [QUERY_TIMEOUT_PROPERTY], "isExcludes": false, "isRecursive": false } }),
        json!([USER_MACRO]),
        "alter",
    )
}

/// The StarRocks AuditLoader's allow: `egret_query_log` may `insert` into
/// `default_catalog.starrocks_audit_db__.starrocks_audit_tbl__`, and nothing
/// else. Resources and item have the shape of the fork's `tests/e2e/policies.py`
/// policy `audit-loader`, which the fork's e2e measured against the real
/// plugin: no `column` level, since `insert` is a table-leaf access type in
/// the StarRocks def (the column leaf allows only `select`).
pub fn query_log_loader_policy(service: &str) -> Value {
    let one = |v: &str| json!({ "values": [v], "isExcludes": false, "isRecursive": false });
    json!({
        "name": QUERY_LOG_LOADER_POLICY_NAME,
        "service": service,
        "serviceType": "starrocks",
        "policyType": 0,
        "isEnabled": true,
        "resources": {
            "catalog": one("default_catalog"),
            "database": one(QUERY_LOG_DATABASE),
            "table": one(QUERY_LOG_TABLE)
        },
        "policyItems": [{
            "users": [QUERY_LOG_LOADER_PRINCIPAL],
            "groups": [],
            "roles": [],
            "accesses": [{ "type": "insert", "isAllowed": true }],
            "delegateAdmin": false
        }],
        "denyPolicyItems": [],
        "allowExceptions": [],
        "denyExceptions": [],
        "isDenyAllElse": false,
        "policyPriority": 0
    })
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
        assert_eq!(SELF_IMPERSONATE_POLICY_NAME, "egret-user-impersonate-self");
        assert_eq!(EXECUTE_QUERY_POLICY_NAME, "egret-user-execute-query");
    }

    /// The one shape every baseline policy shares: an allow for `{USER}`
    /// alone, one access, no groups, no roles, no deny or exception buckets,
    /// and exactly one resource level that is not excluded.
    fn assert_user_only_allow(p: &serde_json::Value, resource: &str, value: &str, access: &str) {
        assert_eq!(p["serviceType"], "trino");
        assert_eq!(p["policyType"], 0);
        assert_eq!(p["isEnabled"], true);
        assert_eq!(
            p["resources"],
            json!({ resource: { "values": [value], "isExcludes": false, "isRecursive": false } }),
            "exactly one resource level, not excluded"
        );
        assert_eq!(
            p["policyItems"],
            json!([{
                "users": ["{USER}"],
                "groups": [],
                "roles": [],
                "accesses": [{ "type": access, "isAllowed": true }],
                "delegateAdmin": false
            }]),
            "one item: {{USER}} only, one access"
        );
        for bucket in ["denyPolicyItems", "allowExceptions", "denyExceptions"] {
            assert_eq!(p[bucket], json!([]), "{bucket}");
        }
        assert_eq!(p["isDenyAllElse"], false);
    }

    #[test]
    fn self_impersonate_policy_names_the_session_user_as_the_target() {
        let p = self_impersonate_policy("rgr2");
        assert_eq!(p["name"], SELF_IMPERSONATE_POLICY_NAME);
        assert_eq!(p["service"], "rgr2");
        assert_user_only_allow(&p, "trinouser", "{USER}", "impersonate");
    }

    #[test]
    fn execute_query_policy_lets_each_user_run_queries() {
        let p = execute_query_policy("rgr2");
        assert_eq!(p["name"], EXECUTE_QUERY_POLICY_NAME);
        assert_user_only_allow(&p, "queryid", "*", "execute");
    }

    #[test]
    fn query_timeout_policy_has_the_baseline_shape() {
        let p = query_timeout_policy("rgr2");
        assert_eq!(p["name"], QUERY_TIMEOUT_POLICY_NAME);
        assert_user_only_allow(&p, "systemproperty", "query_max_run_time", "alter");
    }

    /// The three, in the order Trino 483 checks them on a session (live
    /// result §4.8): set user, execute query, system session property.
    #[test]
    fn the_user_baseline_is_the_measured_three_in_check_order() {
        let set = user_baseline_policies("rgr2");
        let names: Vec<&str> = set.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            [
                SELF_IMPERSONATE_POLICY_NAME,
                EXECUTE_QUERY_POLICY_NAME,
                QUERY_TIMEOUT_POLICY_NAME
            ]
        );
        for (name, body) in &set {
            assert_eq!(
                body["name"], *name,
                "the body carries the name it is stored under"
            );
            assert_eq!(body["service"], "rgr2");
        }
        // The delegator's grant is NOT part of the per-user baseline.
        assert!(!names.contains(&IMPERSONATE_POLICY_NAME));
    }
}
