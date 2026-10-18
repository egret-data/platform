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
    if let Some(problem) = impersonation_closure_problem(conn, service_id, &username).await? {
        return Ok(Err(format!(
            "the impersonation target {username:?} {problem}: impersonating them would hand \
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

/// The platform role whose holders egret seeds as engine admins
/// (`egret_engine_mgmt`'s `admin_usernames` → `seed_admin_all_access`).
pub(crate) const PLATFORM_ADMIN_ROLE: &str = "platform_admin";

/// How far [`impersonation_closure_problem`] walks before it refuses. A chain
/// longer than this is refused rather than followed: refusing is the safe side.
const MAX_IMPERSONATION_WALK: usize = 256;

/// The enabled access policies of a profile, and the engine it is for.
struct ProfilePolicies {
    engine: EngineType,
    /// `(name, body)`, every enabled `policyType` 0 policy, compiled ones too.
    policies: Vec<(String, serde_json::Value)>,
}

async fn profile_policies(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<Option<ProfilePolicies>, CompileError> {
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
    let policies: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT name, policy_jsonb FROM policy.authz_policies \
          WHERE service_id = $1 AND enabled ORDER BY name, id",
    )
    .bind(service_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;
    let policies = policies
        .into_iter()
        .filter(|(_, b)| b["policyType"].as_i64().unwrap_or(0) == 0)
        .collect();
    Ok(Some(ProfilePolicies { engine, policies }))
}

/// Who a username is to Ranger: its groups (lower-cased, `public` included;
/// Ranger matches group names ignoring case) and the egret roles its user holds
/// (an engine grant's `role` principal compiles to the holders).
async fn identity_of(
    conn: &mut sqlx::PgConnection,
    username: &str,
) -> Result<(Vec<String>, Vec<String>), CompileError> {
    let db = |e: sqlx::Error| CompileError::Db(e.to_string());
    let mut groups: Vec<String> = sqlx::query_scalar(
        "SELECT lower(g.name) FROM users u JOIN user_groups ug ON ug.user_id = u.id \
           JOIN groups g ON g.id = ug.group_id WHERE u.username = $1::citext",
    )
    .bind(username)
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;
    groups.push(egret_perm::PUBLIC_GROUP.to_string());
    let roles: Vec<String> = sqlx::query_scalar(
        "SELECT r.name FROM users u JOIN access_grants ag ON ag.principal_id = u.id \
           JOIN roles r ON r.id = ag.role_id \
          WHERE u.username = $1::citext AND ag.principal_type = 'user' \
            AND ag.effect = 'allow' AND (ag.expires_at IS NULL OR ag.expires_at > now())",
    )
    .bind(username)
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;
    Ok((groups, roles))
}

fn json_strs(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether an allow item names `username`: by username (any case), as
/// `{USER}`, or through one of `groups` (lower-cased) or `public`.
fn item_names(item: &serde_json::Value, username: &str, groups: &[String]) -> bool {
    json_strs(&item["users"])
        .iter()
        .any(|u| u.eq_ignore_ascii_case(username) || u == "{USER}")
        || json_strs(&item["groups"])
            .iter()
            .any(|g| groups.contains(&g.to_lowercase()))
}

/// The resource maps of a policy: `resources` and each `additionalResources`.
fn resource_maps(body: &serde_json::Value) -> Vec<&serde_json::Map<String, serde_json::Value>> {
    let mut maps = Vec::new();
    if let Some(m) = body["resources"].as_object() {
        maps.push(m);
    }
    for extra in body["additionalResources"].as_array().into_iter().flatten() {
        if let Some(m) = extra.as_object() {
            maps.push(m);
        }
    }
    maps
}

/// The allowed access types of an item.
fn item_accesses(item: &serde_json::Value) -> Vec<String> {
    item["accesses"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|a| a["isAllowed"].as_bool() != Some(false))
        .filter_map(|a| a["type"].as_str().map(str::to_string))
        .collect()
}

/// The first not-grantable (near-root) key/access pair `username` holds on the
/// profile (security review I1), described as `` `key access` through policy
/// "name" ``, or `None`.
///
/// **A scan of the profile's policies, not an evaluation.** Evaluating with
/// `egret-ranger-eval` would be the sharper instrument, but every delegating
/// Trino profile carries the self-impersonation baseline (`trinouser =
/// {USER}`), which the evaluator refuses (spec C9): every Trino target would
/// come back Undecidable and so be refused, which removes the capability. So
/// this reads every enabled access policy that is not a compiled engine grant
/// (`perm-engine-`; operator, infra and other compiler policies all count) and
/// finds an allow item naming the user ([`item_names`]) on a resource map
/// holding a not-grantable key with that access (Trino's `all` counts as every
/// access). For `trinouser`/`user` only a wildcard value or an exclude counts:
/// one named user is a hop ([`impersonation_hops`]), and `{USER}` is the
/// requester themself. Deny items and exceptions are ignored, so it errs toward
/// refusing.
fn near_root_in(p: &ProfilePolicies, username: &str, groups: &[String]) -> Option<String> {
    use crate::engine_capabilities::NOT_GRANTABLE;
    let rows: Vec<(&str, Option<&str>)> = NOT_GRANTABLE
        .iter()
        .filter(|n| n.engine == p.engine && !n.everyone_holds)
        .map(|n| match n.what.split_once(' ') {
            Some((k, a)) => (k, Some(a)),
            None => (n.what, None),
        })
        .collect();
    for (name, body) in &p.policies {
        if name.starts_with(COMPILER_ENGINE_GRANT_PREFIX) {
            continue;
        }
        let maps = resource_maps(body);
        for item in body["policyItems"].as_array().into_iter().flatten() {
            if !item_names(item, username, groups) {
                continue;
            }
            let accesses = item_accesses(item);
            let every_access = p.engine == EngineType::Trino && accesses.iter().any(|a| a == "all");
            for map in &maps {
                for (key, access) in &rows {
                    let Some(level) = map.get(*key) else {
                        continue;
                    };
                    if matches!(*key, "trinouser" | "user") {
                        let excludes = level["isExcludes"].as_bool() == Some(true);
                        if !excludes && !json_strs(&level["values"]).iter().any(|v| is_pattern(v)) {
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
                        return Some(format!("`{pair}` through policy {name:?}"));
                    }
                }
            }
        }
    }
    None
}

/// Every concrete user `username` can impersonate through a policy of the
/// profile (hand-written, infra AND compiled engine grants), lower-cased.
/// `{USER}` is the requester themself, a no-op hop, and a wildcard is not a
/// hop but a near-root pair ([`near_root_in`]).
fn impersonation_hops(p: &ProfilePolicies, username: &str, groups: &[String]) -> Vec<String> {
    let key = match p.engine {
        EngineType::Trino => "trinouser",
        EngineType::StarRocks => "user",
    };
    let mut out = Vec::new();
    for (_, body) in &p.policies {
        let maps = resource_maps(body);
        for item in body["policyItems"].as_array().into_iter().flatten() {
            if !item_names(item, username, groups) {
                continue;
            }
            let accesses = item_accesses(item);
            if !accesses
                .iter()
                .any(|a| a == "impersonate" || (p.engine == EngineType::Trino && a == "all"))
            {
                continue;
            }
            for map in &maps {
                let Some(level) = map.get(key) else { continue };
                for v in json_strs(&level["values"]) {
                    if v != "{USER}" && !is_pattern(&v) {
                        out.push(v.to_lowercase());
                    }
                }
            }
        }
    }
    out
}

/// The targets of the profile's engine grants that `username` is a grantee
/// of: the current model, so a grant just written (or not yet compiled) is a
/// hop too.
async fn engine_grant_hops(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    username: &str,
    groups: &[String],
    roles: &[String],
) -> Result<Vec<String>, CompileError> {
    sqlx::query_scalar(
        "SELECT DISTINCT lower(u.username::text) FROM perm.engine_grants g \
           JOIN engine_mgmt.engines e ON e.id = g.engine_id \
           JOIN users u ON u.id = g.target_user_id \
           JOIN perm.engine_grant_principals p ON p.engine_grant_id = g.id \
          WHERE e.authz_service_id = $1 AND u.username IS NOT NULL \
            AND ((p.kind = 'user' AND lower(p.ref) = lower($2)) \
              OR (p.kind = 'group' AND lower(p.ref) = ANY($3)) \
              OR (p.kind = 'role' AND p.ref = ANY($4)))",
    )
    .bind(service_id)
    .bind(username)
    .bind(groups)
    .bind(roles)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| CompileError::Db(e.to_string()))
}

/// Why impersonating `username` on profile `service_id` would hand out more
/// than one ordinary identity, or `None` (security review I1 and I-a).
///
/// Walks the transitive closure of concrete impersonation hops from
/// `username` — through every enabled access policy of the profile and every
/// engine grant of the profile's engines — and refuses when any user reached
/// (the target included) is reserved (`root`, egret's own principals) or holds
/// a near-root pair ([`near_root_in`]). Bounded by a visited set and by
/// [`MAX_IMPERSONATION_WALK`]; a walk that hits the bound is refused. Applied
/// identically on both engines: on StarRocks `EXECUTE AS` chains; on Trino the
/// chain semantics are not measured, so each hop is treated as the current
/// identity too.
pub(crate) async fn impersonation_closure_problem(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    username: &str,
) -> Result<Option<String>, CompileError> {
    let Some(profile) = profile_policies(conn, service_id).await? else {
        return Ok(None);
    };
    let start = username.to_lowercase();
    let mut visited: std::collections::BTreeMap<String, Option<String>> =
        std::collections::BTreeMap::new();
    visited.insert(start.clone(), None);
    let mut queue = std::collections::VecDeque::from([start.clone()]);
    let path_to = |visited: &std::collections::BTreeMap<String, Option<String>>, u: &str| {
        let mut chain = vec![u.to_string()];
        let mut at = u.to_string();
        while let Some(Some(prev)) = visited.get(&at) {
            chain.push(prev.clone());
            at = prev.clone();
        }
        chain.reverse();
        chain.join(" -> ")
    };
    while let Some(user) = queue.pop_front() {
        let via = if user == start {
            String::new()
        } else {
            format!(
                "can become {:?} (through {}), which ",
                user,
                path_to(&visited, &user)
            )
        };
        if user != start && is_reserved_target(&user) {
            return Ok(Some(format!(
                "{via}is reserved: egret runs as it on the engine"
            )));
        }
        let (groups, roles) = identity_of(conn, &user).await?;
        // egret seeds every platform admin as an engine admin (`egret-admin -
        // …`, `seed_admin_all_access`) at each engine apply, so a platform admin
        // is near-root whether or not that seed has run on this profile yet.
        if roles.iter().any(|r| r == PLATFORM_ADMIN_ROLE) {
            return Ok(Some(format!(
                "{via}is a platform admin, whom egret seeds as an engine admin"
            )));
        }
        if let Some(held) = near_root_in(&profile, &user, &groups) {
            return Ok(Some(format!("{via}holds {held}")));
        }
        let mut next = impersonation_hops(&profile, &user, &groups);
        next.extend(engine_grant_hops(conn, service_id, &user, &groups, &roles).await?);
        for n in next {
            if visited.contains_key(&n) {
                continue;
            }
            if visited.len() >= MAX_IMPERSONATION_WALK {
                return Ok(Some(format!(
                    "starts an impersonation chain longer than {MAX_IMPERSONATION_WALK} users, \
                     which egret refuses rather than follows"
                )));
            }
            visited.insert(n.clone(), Some(user.clone()));
            queue.push_back(n);
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
