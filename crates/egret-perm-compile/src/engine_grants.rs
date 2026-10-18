//! Compile an engine grant (F11, spec §4) into the one Ranger access policy
//! it stands for, `perm-engine-{id}`, in its engine's profile.
//!
//! [`compile_engine_grant`] is pure, like `compile_grant`: the catalogue entry
//! gives the resource keys and the access type, the grant gives the values and
//! the (already expanded) grantees. The impersonation target is the one value
//! a grant does not carry as text: it is stored by user id (spec E21, amended)
//! and [`impersonation_target_ok`] renders the user's CURRENT username from it,
//! or says why the grant must be skipped this time (spec E11).

use std::collections::BTreeMap;

use egret_perm::{CompileError, Effect, EngineGrant, EngineType};
use egret_policy::admin::{AccessPolicyInput, ResourceLevel};
use egret_policy::COMPILER_ENGINE_GRANT_PREFIX;
use uuid::Uuid;

use crate::engine_capabilities::{capability, check_values, ValueKind};

/// The Ranger policy name of engine grant `id`: the one spelling the compiler
/// writes and `crate::attribution` reads back.
pub fn engine_grant_policy_name(id: Uuid) -> String {
    format!("{COMPILER_ENGINE_GRANT_PREFIX}{id}")
}

fn invalid(g: &EngineGrant, reason: impl Into<String>) -> CompileError {
    CompileError::InvalidEngineGrant {
        id: g.id,
        reason: reason.into(),
    }
}

/// The StarRocks superuser egret connects as for DDL and catalog management
/// (`egret_engine_mgmt::engines::starrocks_root_conn_opts`). Refused as an
/// impersonation target on every engine (security review C1): on StarRocks
/// impersonating it is root; on Trino no egret principal carries the name,
/// and refusing it there costs nothing.
pub(crate) const ROOT_TARGET: &str = "root";

/// Whether `username` may never be an impersonation target: a principal egret
/// itself runs as on an engine (spec E11, C6, and security review C1 for
/// `root`). Case-insensitive: both engines match these keys ignoring case.
pub(crate) fn is_reserved_target(username: &str) -> bool {
    egret_core::engine_principals::is_reserved_engine_username(&username.to_lowercase())
        || username.eq_ignore_ascii_case(egret_core::engine_principals::DELEGATOR_PRINCIPAL)
        || username.eq_ignore_ascii_case(ROOT_TARGET)
}

/// Compile `g` for `profile` on `engine`. `target` is the impersonation
/// target's current username ([`impersonation_target_ok`]); it is required for
/// an impersonation capability and refused for any other.
///
/// Refuses (`InvalidEngineGrant`) an unknown capability, one of another engine,
/// a value that breaks the catalogue's rules, a reserved target, and an
/// exclude; a grantee `User` ref fails as it does for a grant
/// (`UnusableUserRef`, `ReservedEnginePrincipal`).
pub fn compile_engine_grant(
    g: &EngineGrant,
    target: Option<&str>,
    profile: &str,
    engine: EngineType,
) -> Result<AccessPolicyInput, CompileError> {
    let cap = capability(&g.capability)
        .ok_or_else(|| invalid(g, format!("unknown capability {:?}", g.capability)))?;
    if cap.engine != engine {
        return Err(invalid(
            g,
            format!("{} is not a capability of a {engine:?} engine", cap.id),
        ));
    }
    if !g.principals.exclude.is_empty() {
        return Err(invalid(g, "an engine grant is include-only"));
    }

    // The values in key order, the target in its slot.
    let mut stored = g.values.iter();
    let mut values: Vec<String> = Vec::new();
    for kind in cap.value_kinds() {
        if kind == ValueKind::Username {
            let t = target.ok_or_else(|| invalid(g, "an impersonation grant needs its target"))?;
            values.push(t.to_string());
        } else {
            let v = stored
                .next()
                .ok_or_else(|| invalid(g, format!("{} is missing a value", cap.id)))?;
            values.push(v.clone());
        }
    }
    if stored.next().is_some() {
        return Err(invalid(g, format!("{} has too many values", cap.id)));
    }
    if target.is_some() && !cap.is_impersonation() {
        return Err(invalid(g, format!("{} takes no target user", cap.id)));
    }
    let values = check_values(cap, &values).map_err(|reason| invalid(g, reason))?;

    let mut values_iter = values.iter();
    let mut resources: BTreeMap<String, ResourceLevel> = BTreeMap::new();
    for (key, kind) in cap.resources {
        let value = match kind {
            Some(ValueKind::Username) => {
                let v = values_iter.next().expect("checked count");
                if is_reserved_target(v) {
                    return Err(invalid(
                        g,
                        format!("{v:?} is a principal egret runs as; it cannot be impersonated"),
                    ));
                }
                v.clone()
            }
            Some(_) => values_iter.next().expect("checked count").clone(),
            None => "*".to_string(),
        };
        resources.insert(
            (*key).to_string(),
            ResourceLevel {
                values: vec![value],
                is_excludes: false,
                is_recursive: false,
            },
        );
    }

    let item = crate::grants::expanded_principal_item(
        &g.principals.include,
        Effect::Allow,
        &[cap.access.to_string()],
    )?;

    Ok(AccessPolicyInput {
        name: engine_grant_policy_name(g.id),
        service: profile.to_string(),
        is_enabled: true,
        resources,
        items: vec![item],
        allow_exceptions: Vec::new(),
        deny_exceptions: Vec::new(),
        is_deny_all_else: false,
        description: String::new(),
        labels: Vec::new(),
        is_override: false,
        additional: Vec::new(),
    })
}

/// The checks an impersonation target must pass at compile (spec E11), read on
/// `conn`: `Ok(Ok(username))` with the user's CURRENT username, or
/// `Ok(Err(reason))` when the grant must be skipped this time — the user has no
/// username (or is gone), the name is reserved, or the user is now an engine
/// admin of `service_id` (named in an enabled `egret-admin - …` policy).
/// Skipped, not failed: a user changed elsewhere must not fail every recompile
/// of the profile.
pub(crate) async fn impersonation_target_ok(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    target_user_id: Uuid,
) -> Result<Result<String, String>, CompileError> {
    let username: Option<String> =
        sqlx::query_scalar("SELECT username::text FROM users WHERE id = $1")
            .bind(target_user_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| CompileError::Db(e.to_string()))?
            .flatten();
    let Some(username) = username else {
        return Ok(Err(format!(
            "no user carries the impersonation target {target_user_id} as a username any more"
        )));
    };
    if is_reserved_target(&username) {
        return Ok(Err(format!(
            "the impersonation target {username:?} is reserved: egret runs as it on the engine"
        )));
    }
    if let Some(held) = near_root_held_by(conn, service_id, target_user_id, &username).await? {
        return Ok(Err(format!(
            "the impersonation target {username:?} holds {held}: impersonating them would hand \
             it to the grantee, making them an engine admin"
        )));
    }
    Ok(Ok(username))
}

/// Whether a resource-key value is a pattern that names more than one user:
/// a Ranger wildcard. `{USER}` is not one — it is the requester themself.
fn is_pattern(value: &str) -> bool {
    value.contains('*') || value.contains('?')
}

/// The first not-grantable (near-root) key/access pair the user holds on
/// profile `service_id` (security review I1), described as `` `key access`
/// through policy "name" ``, or `None`.
///
/// **A scan of the profile's policies, not an evaluation.** Evaluating the
/// target against the served artifact with `egret-ranger-eval` would be the
/// sharper instrument, but every delegating Trino profile carries the
/// self-impersonation baseline (`trinouser = {USER}`), which the evaluator
/// refuses (spec C9): every Trino target would come back Undecidable and so be
/// refused, which removes the capability. So this reads every ENABLED access
/// policy of the profile that is not a compiled engine grant (`perm-engine-`;
/// operator, infra and other compiler policies all count) and refuses when an
/// allow item names the user — by username (any case), as `{USER}`, through a
/// group they belong to, or through `public` — on a resource map holding a
/// not-grantable key with that access (Trino's `all` counts as every access).
/// For `trinouser`/`user` only a wildcard value counts: one named user is the
/// grantable capability, and `{USER}` is the requester themself. Deny items and
/// exceptions are ignored, so it errs toward refusing.
pub(crate) async fn near_root_held_by(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    user_id: Uuid,
    username: &str,
) -> Result<Option<String>, CompileError> {
    use crate::engine_capabilities::NOT_GRANTABLE;
    let db = |e: sqlx::Error| CompileError::Db(e.to_string());
    let service_type: Option<String> =
        sqlx::query_scalar("SELECT type FROM policy.authz_services WHERE id = $1")
            .bind(service_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(db)?;
    let engine = match service_type.as_deref() {
        Some("trino") => EngineType::Trino,
        Some("starrocks") => EngineType::StarRocks,
        _ => return Ok(None),
    };
    let mut groups: Vec<String> = sqlx::query_scalar(
        "SELECT g.name FROM user_groups ug JOIN groups g ON g.id = ug.group_id \
          WHERE ug.user_id = $1",
    )
    .bind(user_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;
    groups.push(egret_perm::PUBLIC_GROUP.to_string());
    let policies: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT name, policy_jsonb FROM policy.authz_policies \
          WHERE service_id = $1 AND enabled AND name NOT LIKE 'perm-engine-%' \
          ORDER BY name, id",
    )
    .bind(service_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;
    let rows: Vec<(&str, Option<&str>)> = NOT_GRANTABLE
        .iter()
        .filter(|n| n.engine == engine && !n.everyone_holds)
        .map(|n| match n.what.split_once(' ') {
            Some((k, a)) => (k, Some(a)),
            None => (n.what, None),
        })
        .collect();
    let strs = |v: &serde_json::Value| -> Vec<String> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    for (name, body) in policies {
        if body["policyType"].as_i64().unwrap_or(0) != 0 {
            continue;
        }
        let mut maps: Vec<&serde_json::Map<String, serde_json::Value>> = Vec::new();
        if let Some(m) = body["resources"].as_object() {
            maps.push(m);
        }
        for extra in body["additionalResources"].as_array().into_iter().flatten() {
            if let Some(m) = extra.as_object() {
                maps.push(m);
            }
        }
        for item in body["policyItems"].as_array().into_iter().flatten() {
            let names_user = strs(&item["users"])
                .iter()
                .any(|u| u.eq_ignore_ascii_case(username) || u == "{USER}");
            let names_group = strs(&item["groups"]).iter().any(|g| groups.contains(g));
            if !names_user && !names_group {
                continue;
            }
            let accesses: Vec<String> = item["accesses"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|a| a["isAllowed"].as_bool() != Some(false))
                .filter_map(|a| a["type"].as_str().map(str::to_string))
                .collect();
            let every_access = engine == EngineType::Trino && accesses.iter().any(|a| a == "all");
            for map in &maps {
                for (key, access) in &rows {
                    let Some(level) = map.get(*key) else {
                        continue;
                    };
                    if matches!(*key, "trinouser" | "user") {
                        let excludes = level["isExcludes"].as_bool() == Some(true);
                        if !excludes && !strs(&level["values"]).iter().any(|v| is_pattern(v)) {
                            continue;
                        }
                    }
                    let held = match access {
                        None => !accesses.is_empty(),
                        Some(a) => every_access || accesses.iter().any(|x| x == a),
                    };
                    if held {
                        let pair = match access {
                            Some(a) => format!("{key} {a}"),
                            None => (*key).to_string(),
                        };
                        return Ok(Some(format!("`{pair}` through policy {name:?}")));
                    }
                }
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine_capabilities::CAPABILITIES;
    use crate::rules::stamp_service_type;
    use egret_perm::{Principal, PrincipalKind, PrincipalSet};
    use egret_policy::admin::access_to_ranger_json;
    use serde_json::{json, Value};

    fn grant(capability: &str, values: &[&str], users: &[&str]) -> EngineGrant {
        EngineGrant {
            id: Uuid::parse_str("0f0e0d0c-0b0a-4908-8706-050403020100").unwrap(),
            engine_id: Uuid::nil(),
            capability: capability.into(),
            values: values.iter().map(|v| v.to_string()).collect(),
            target_user_id: None,
            principals: PrincipalSet {
                include: users
                    .iter()
                    .map(|u| Principal {
                        kind: PrincipalKind::User,
                        r#ref: u.to_string(),
                    })
                    .collect(),
                exclude: vec![],
            },
        }
    }

    /// `(values, target)` that satisfy each capability.
    fn sample(id: &str) -> (Vec<&'static str>, Option<&'static str>) {
        match id {
            "trino.impersonate_user" | "starrocks.impersonate_user" => (vec![], Some("bob")),
            "trino.set_system_session_property" => (vec!["query_max_memory"], None),
            "trino.set_catalog_session_property" => (vec!["ice", "max_splits"], None),
            "trino.execute_procedure" => (vec!["ice", "system", "rollback_to_snapshot"], None),
            "trino.execute_function" => (vec!["ice", "fns", "my_fn"], None),
            "starrocks.use_global_function" => (vec!["my_udf(INT,VARCHAR)"], None),
            "starrocks.manage_blacklist" => (vec![], None),
            other => panic!("no sample for {other}"),
        }
    }

    fn body(g: &EngineGrant, target: Option<&str>, engine: EngineType) -> Value {
        let input = compile_engine_grant(g, target, "prof", engine).expect("compiles");
        stamp_service_type(access_to_ranger_json(&input), engine)
    }

    #[test]
    fn every_capability_compiles_to_one_allow_item_on_its_catalogue_keys() {
        for cap in CAPABILITIES {
            let (values, target) = sample(cap.id);
            let g = grant(cap.id, &values, &["alice", "carol"]);
            let b = body(&g, target, cap.engine);

            assert_eq!(
                b["name"],
                json!(format!("perm-engine-{}", g.id)),
                "{}",
                cap.id
            );
            let mut stored = values.iter();
            let mut want = serde_json::Map::new();
            for (key, kind) in cap.resources {
                let v = match kind {
                    None => "*".to_string(),
                    Some(ValueKind::Username) => target.unwrap().to_string(),
                    Some(_) => stored.next().unwrap().to_string(),
                };
                want.insert(
                    (*key).to_string(),
                    json!({"values": [v], "isExcludes": false, "isRecursive": false}),
                );
            }
            assert_eq!(b["resources"], Value::Object(want), "{}", cap.id);
            assert_eq!(
                b["policyItems"],
                json!([{
                    "users": ["alice", "carol"], "groups": [], "roles": [],
                    "accesses": [{"type": cap.access, "isAllowed": true}],
                    "delegateAdmin": false,
                }]),
                "{}",
                cap.id
            );
            assert_eq!(b["denyPolicyItems"], json!([]), "{}", cap.id);
            assert_eq!(b["allowExceptions"], json!([]), "{}", cap.id);
            assert_eq!(b["denyExceptions"], json!([]), "{}", cap.id);
            let service_type = match cap.engine {
                EngineType::Trino => "trino",
                EngineType::StarRocks => "starrocks",
            };
            assert_eq!(b["serviceType"], json!(service_type), "{}", cap.id);
        }
    }

    #[test]
    fn a_capability_of_another_engine_is_refused() {
        let g = grant("starrocks.use_global_function", &["f(INT)"], &["alice"]);
        assert!(matches!(
            compile_engine_grant(&g, None, "prof", EngineType::Trino),
            Err(CompileError::InvalidEngineGrant { .. })
        ));
        let g = grant("trino.kill_any", &[], &["alice"]);
        assert!(matches!(
            compile_engine_grant(&g, None, "prof", EngineType::Trino),
            Err(CompileError::InvalidEngineGrant { .. })
        ));
    }

    #[test]
    fn grantee_refs_are_checked_as_a_grants_are() {
        let g = grant("trino.set_system_session_property", &["q"], &["a@b"]);
        assert!(matches!(
            compile_engine_grant(&g, None, "prof", EngineType::Trino),
            Err(CompileError::UnusableUserRef { .. })
        ));
        let g = grant(
            "trino.set_system_session_property",
            &["q"],
            &["egret-lineage"],
        );
        assert!(matches!(
            compile_engine_grant(&g, None, "prof", EngineType::Trino),
            Err(CompileError::ReservedEnginePrincipal { .. })
        ));
    }

    #[test]
    fn values_and_targets_that_break_the_rules_are_refused() {
        let sys = "trino.set_system_session_property";
        for bad in [
            &["*"][..],
            &["a?b"],
            &["{USER}"],
            &[""],
            &[" q"],
            &[],
            &["a", "b"],
        ] {
            let g = grant(sys, bad, &["alice"]);
            assert!(
                matches!(
                    compile_engine_grant(&g, None, "prof", EngineType::Trino),
                    Err(CompileError::InvalidEngineGrant { .. })
                ),
                "{bad:?}"
            );
        }
        let imp = grant("trino.impersonate_user", &[], &["alice"]);
        for (target, why) in [
            (None, "no target"),
            (Some("al*"), "a wildcard target"),
            (Some("egret-lineage"), "the lineage principal"),
            (Some("egret_delegator"), "the delegator"),
            (Some("root"), "root, which egret runs as on StarRocks"),
            (Some("ROOT"), "root in another case"),
        ] {
            assert!(
                matches!(
                    compile_engine_grant(&imp, target, "prof", EngineType::Trino),
                    Err(CompileError::InvalidEngineGrant { .. })
                ),
                "{why}"
            );
        }
        // A target on a capability that has none.
        let g = grant(sys, &["q"], &["alice"]);
        assert!(matches!(
            compile_engine_grant(&g, Some("bob"), "prof", EngineType::Trino),
            Err(CompileError::InvalidEngineGrant { .. })
        ));
        // An exclude.
        let mut g = grant(sys, &["q"], &["alice"]);
        g.principals.exclude.push(Principal {
            kind: PrincipalKind::User,
            r#ref: "bob".into(),
        });
        assert!(matches!(
            compile_engine_grant(&g, None, "prof", EngineType::Trino),
            Err(CompileError::InvalidEngineGrant { .. })
        ));
    }

    #[test]
    fn the_target_is_rendered_lower_case_as_the_engines_compare_it() {
        let g = grant("trino.impersonate_user", &[], &["alice"]);
        let b = body(&g, Some("Bob"), EngineType::Trino);
        assert_eq!(b["resources"]["trinouser"]["values"], json!(["bob"]));
    }
}
