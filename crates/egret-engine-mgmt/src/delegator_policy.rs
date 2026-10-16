//! Ranger seed policies for delegated Trino engines, stored as real,
//! auditable infrastructure policies (`managed_by = 'infra'`).
//!
//! - **Delegator** (impersonate engines only): `egret_delegator` may
//!   impersonate any user — [`delegator_impersonate_policy`].
//! - **Per-user baseline** (every delegating Trino engine, JWT and
//!   impersonate): the three per-user checks Trino 483's Ranger plugin makes on
//!   every session, measured live in DG Task 6b (live result §4.8) —
//!   [`user_baseline_policies`]. Without them every delegated query fails
//!   before it reads a row.

use serde_json::{json, Value};

/// The Trino service user egret authenticates as for impersonate-mode delegation.
pub const DELEGATOR_USERNAME: &str = "egret_delegator";
// Declared in `egret_policy_api::infra_names` so the guard, the backfill and this
// producer read one list. Re-exported rather than moved outright: `engines.rs`
// refers to them as `dp::IMPERSONATE_POLICY_NAME` / `dp::QUERY_TIMEOUT_POLICY_NAME`
// and those call sites are correct as they stand.
pub use egret_policy_api::infra_names::{
    EXECUTE_QUERY_POLICY_NAME, IMPERSONATE_POLICY_NAME, QUERY_TIMEOUT_POLICY_NAME,
    SELF_IMPERSONATE_POLICY_NAME,
};
const QUERY_TIMEOUT_PROPERTY: &str = "query_max_run_time";
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
        json!([DELEGATOR_USERNAME]),
        "impersonate",
    )
}

/// Every user may become THEMSELF: `trinouser = {USER}`, `impersonate`, users
/// `{USER}`. Trino 483 checks `checkCanSetUser` as an `impersonate` on
/// `trinouser=<user>` for every session, principal == user included; without
/// this a query fails `Principal u cannot become user u`.
///
/// Self only: Ranger expands `{USER}` in the resource value to the requesting
/// user and matches the result as a literal, not a pattern — measured on the
/// real 2.5.0 and 2.8.0 engines by `tests/ranger`'s
/// `EgretUserTokenWildcardTest` (a user named `*` or `a*` cannot become
/// anyone else). The match is case-insensitive (`trinouser` is `ignoreCase`);
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

    /// `trinouser = {USER}` lets a user become only THEMSELF.
    ///
    /// Three sources, because none is enough alone:
    ///
    /// 1. **The real engine** decides it: `tests/ranger`'s
    ///    `EgretUserTokenWildcardTest` runs this exact body through
    ///    `RangerPolicyEngineImpl` 2.5.0 and 2.8.0. `alice → alice` is allowed;
    ///    `alice → bob`, `* → bob`, `a* → alice2`, `?lice → alice` are not.
    ///    The expanded value is matched as a LITERAL, never as a pattern.
    /// 2. **egret-ranger-eval on the stored body** declines: it reads a resource
    ///    value as a literal and refuses one carrying a token
    ///    (`ResourceValueExpressionMarker`), so for a `trinouser` request it
    ///    answers neither allow nor deny. Asserted below so a change to that is
    ///    seen, and so nobody reads the evaluator as having proved (1).
    /// 3. **A substitution model** (expand `{USER}`, then evaluate) agrees with
    ///    (1) for usernames without wildcard characters only. For `*` it would
    ///    WRONGLY allow everything, because the expanded literal becomes a
    ///    policy value and the evaluator matches policy values as patterns —
    ///    which is exactly what (1) shows Ranger does not do. So the model is
    ///    run on plain names only; egret's usernames are `[a-z0-9_-]`
    ///    (`egret_auth::users::slugify_localpart`), so those are the names that
    ///    exist.
    #[test]
    fn a_user_cannot_impersonate_another_user_through_the_self_policy() {
        use egret_ranger_eval::eval::{evaluate, AccessRequest, Decision, EngineConfig};
        use egret_ranger_eval::model::ServicePolicySet;
        use egret_ranger_eval::resource::RequestResource;
        use egret_ranger_eval::servicedef::ServiceDef;

        let def_json = egret_policy::trino_service_def().unwrap();
        let def = ServiceDef::parse(&def_json);
        let ask = |body: serde_json::Value, user: &str, target: &str| {
            let mut body = body;
            body["id"] = json!(1);
            let set: ServicePolicySet = serde_json::from_value(json!({
                "serviceName": "svc",
                "policies": [body],
                "serviceDef": def_json,
            }))
            .unwrap();
            evaluate(
                &set,
                &AccessRequest {
                    user: user.into(),
                    groups: Default::default(),
                    roles: Default::default(),
                    access_type: "impersonate".into(),
                    resource: RequestResource::new().with("trinouser", target),
                    owner: None,
                    resolved_tags: None,
                },
                &def,
                &EngineConfig::without_super_users(),
            )
        };

        // (2) the stored body: never Allowed, for any of the asked pairs.
        for (user, target) in [
            ("alice", "alice"),
            ("alice", "bob"),
            ("*", "bob"),
            ("a*", "alice2"),
            ("?lice", "alice"),
        ] {
            let d = ask(self_impersonate_policy("svc"), user, target);
            assert!(
                matches!(d, Decision::Undecidable { .. }),
                "the evaluator must decline, not decide, {user} as {target}: {d:?}"
            );
        }

        // (3) the substitution model, plain usernames only.
        let expanded_for = |user: &str| {
            let mut body = self_impersonate_policy("svc");
            body["resources"]["trinouser"]["values"] = json!([user]);
            body
        };
        let d = ask(expanded_for("alice"), "alice", "alice");
        assert!(
            matches!(d, Decision::Allowed { .. }),
            "alice as alice: {d:?}"
        );
        // `trinouser` is `ignoreCase` in the def, so `ALICE` is `alice` here;
        // `users.username` is `citext UNIQUE`, so two egret users cannot differ
        // only by case.
        let d = ask(expanded_for("alice"), "alice", "ALICE");
        assert!(
            matches!(d, Decision::Allowed { .. }),
            "alice as ALICE: {d:?}"
        );
        for (user, target) in [
            ("alice", "bob"),
            ("bob", "alice"),
            ("alice", "alice2"),
            ("alice2", "alice"),
        ] {
            let d = ask(expanded_for(user), user, target);
            assert!(
                matches!(d, Decision::Denied { .. }),
                "{user} must not become {target}: {d:?}"
            );
        }
    }
}
