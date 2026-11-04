//! Ranger seed policies for delegated Trino engines are built in
//! `egret_policy_api::infra_bodies`; this module keeps the names and the
//! delegator principal that `engines.rs` and `assignment.rs` refer to.

/// The Trino service user egret authenticates as for impersonate-mode delegation.
/// Declared in `egret_core` so `egret-perm-compile` can refuse it as an
/// impersonation target (engine grants spec, C6).
pub use egret_core::engine_principals::DELEGATOR_PRINCIPAL as DELEGATOR_USERNAME;
pub use egret_policy_api::infra_names::{
    EXECUTE_QUERY_POLICY_NAME, IMPERSONATE_POLICY_NAME, QUERY_TIMEOUT_POLICY_NAME,
    SELF_IMPERSONATE_POLICY_NAME,
};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// F11 (spec C6): one name, declared in `egret_core::engine_principals`
    /// so `egret-perm-compile` can refuse it as an impersonation target.
    #[test]
    fn the_delegator_username_is_the_core_principal() {
        assert_eq!(
            DELEGATOR_USERNAME,
            egret_core::engine_principals::DELEGATOR_PRINCIPAL
        );
    }

    // Kept here, not in `egret-policy-api`: it needs `egret-policy` (the def) and
    // `egret-ranger-eval`, which only this crate has as dev-dependencies.
    /// `trinouser = {USER}` lets a user become only THEMSELF.
    ///
    /// Three sources, because none is enough alone:
    ///
    /// 1. **The real engine** decides it: `tests/ranger`'s
    ///    `EgretUserTokenWildcardTest` runs this exact body through
    ///    `RangerPolicyEngineImpl` 2.5.0 and 2.8.0. `alice → alice` is allowed;
    ///    `alice → bob`, `* → bob`, `a* → alice2`, `?lice → alice` are not.
    ///    Those rows establish the outcome — wildcard-named requesters are
    ///    denied — not the mechanism that produces it.
    /// 2. **egret-ranger-eval on the stored body** declines: it reads a resource
    ///    value as a literal and refuses one carrying a token
    ///    (`ResourceValueExpressionMarker`), so for a `trinouser` request it
    ///    answers neither allow nor deny. Asserted below so a change to that is
    ///    seen, and so nobody reads the evaluator as having proved (1).
    /// 3. **A substitution model** (expand `{USER}`, then evaluate) agrees with
    ///    (1) for usernames without wildcard characters only. For `*` it would
    ///    WRONGLY allow everything, because the expanded literal becomes a
    ///    policy value and the evaluator matches policy values as patterns —
    ///    and (1) shows Ranger denies that case. So the model is
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
            let d = ask(
                egret_policy_api::infra_bodies::self_impersonate_policy("svc"),
                user,
                target,
            );
            assert!(
                matches!(d, Decision::Undecidable { .. }),
                "the evaluator must decline, not decide, {user} as {target}: {d:?}"
            );
        }

        // (3) the substitution model, plain usernames only.
        let expanded_for = |user: &str| {
            let mut body = egret_policy_api::infra_bodies::self_impersonate_policy("svc");
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
