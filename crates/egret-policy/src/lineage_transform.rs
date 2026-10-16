//! View governance R5 (fix round 1, user ruling 2026-10-14): what the
//! `egret-lineage` principal may do is decided **at download time, per plugin**.
//!
//! # Why not in the stored policies
//!
//! Stored policies (the compiler's output and hand-written ones alike) say
//! nothing about `egret-lineage`. Two reasons from the Task 7 review:
//!
//! - **I1.** A hand-written mask or row filter applies to the principal through
//!   its `{public}` item, and one pruned input of a join gives a lineage that is
//!   short but not empty, so the view is under-tagged. Only a transform that
//!   sees every served policy, hand-written included, can put the lineage item
//!   first in all of them.
//! - **I4.** A profile can be bound to an EXTERNAL engine, whose own
//!   authenticator decides who is called `egret-lineage`. egret controls that
//!   name only on a Trino engine it provisions, in JWT delegation mode, where
//!   egret signs every token. So only such an engine's plugin is served the
//!   transform; every other plugin gets the stored policies unchanged, and a
//!   lineage EXPLAIN there fails, which is a refusal (fail-closed).
//!
//! # What the transform does
//!
//! On the resource policies and on the embedded tag policies (`tagPolicies`;
//! Trino polls tags under its resource service):
//!
//! 1. **Masks and row filters.** A leading item for `egret-lineage`, `MASK_NONE`
//!    or an empty filter, in EVERY data-mask and row-filter policy. Ranger's
//!    first matching item wins.
//! 2. **Denies.** `egret-lineage` joins the `denyExceptions` of EVERY deny,
//!    hand-written included, for the read accesses that deny withholds
//!    (`select`, `use`) and nothing else. Hand-written denies are included by
//!    choice: excepting them makes every served deny behave alike, and the
//!    alternative (EXPLAIN errors) would only be a refusal.
//! 3. **Allows.** In every allow policy, an `allowExceptions` item for
//!    `egret-lineage` on the non-read accesses that policy grants (review I2: a
//!    `{public}` write allow would otherwise reach the principal). This also
//!    covers tag-phase allows, which no resource deny can reach.
//! 4. **Its own policies, appended.** `use`+`select` on every catalog the
//!    profile's engines attach (`catalog/*/*/*`); `alter` on the system
//!    session property `query_max_run_time` ([`LINEAGE_SYSTEM_PROPERTY`],
//!    Task 12, live finding I2); and, per resource hierarchy of the
//!    service-def, an override-priority DENY of every access type except what
//!    EXPLAIN needs ([`kept_accesses`]), narrowed further on `trinouser`
//!    (itself only) and `systemproperty` (`query_max_run_time`).
//!
//! **Who is served it is decided per POLLER** (fix round 2, N1): the
//! `pluginId`'s hostname must be a coordinator or worker pod of an eligible
//! engine bound to the polled profile ([`lineage_catalogs_for_poll_via`]). A
//! bind, unbind or delegation-mode change bumps the profile's version
//! (migration 0187), so no cached transform outlives a change of eligibility.
//!
//! The transform is a pure function of the served envelope and the catalog
//! list, so one policy version gives the same bytes to every eligible plugin.
//! Task 6's ledger records `policy_version`, which the transform does not
//! change.
//!
//! **One function, two callers.** `download_policies` and `explain`
//! (`egret_perm_compile::routes`, "evaluate what the engine actually
//! downloads") both call [`apply_lineage_transform`] after
//! [`lineage_catalogs_for_poll_via`] says the poller is eligible.

use std::collections::BTreeSet;

use egret_core::engine_principals::LINEAGE_PRINCIPAL;
use egret_policy_api::ServicePolicies;
use egret_ranger_eval::servicedef::{hierarchy_chain, is_hierarchy_leaf, parse_resources};
use serde_json::{json, Value};

use crate::PolicyStoreError;

/// The read accesses the principal holds on data.
pub const LINEAGE_READ_ACCESSES: [&str; 2] = ["select", "use"];

/// The one system session property egret's client sets on every statement
/// (`egret_query`'s Trino connector). The lineage EXPLAIN is refused without
/// `alter` on it (live result §2.6 I2). The transform allows it, and only it,
/// so the served lineage set does not depend on the stored set: since DG
/// Task 7 the `egret-query-timeout` seed is on every delegating Trino engine
/// (JWT included), but this download is built to stand on its own.
pub const LINEAGE_SYSTEM_PROPERTY: &str = "query_max_run_time";

/// Name prefix of the policies the transform appends. They exist only in a
/// download, never in `policy.authz_policies`.
pub const LINEAGE_POLICY_PREFIX: &str = "egret-lineage-";

/// Ids of the appended policies: far above any `bigserial` the store issues,
/// and deterministic, so the same envelope always serves the same bytes.
const LINEAGE_POLICY_ID_BASE: i64 = 9_100_000_000_000_000_000;

/// What EXPLAIN needs on a leaf resource of Trino's service-def, never denied.
///
/// - the catalog chain: `use` on the catalog, `select` on every referenced
///   column (live result §1 Q3, the measured minimum);
/// - `trinouser`: `impersonate`, which Trino 483 checks for the session user
///   on every query (the Q3 baseline policies);
/// - `queryid`: `execute`, likewise;
/// - `systemproperty`: `alter`, because egret's client sets the system session
///   property `query_max_run_time` on every statement.
///
/// Everything else on every hierarchy is denied.
pub fn kept_accesses(leaf: &str) -> &'static [&'static str] {
    match leaf {
        // The data chain, at whatever level a hierarchy stops (Trino's
        // service-def lets a policy stop at the catalog, schema or table).
        "catalog" | "schema" | "table" | "column" => &["select", "use"],
        "trinouser" => &["impersonate"],
        "queryid" => &["execute"],
        "systemproperty" => &["alter"],
        _ => &[],
    }
}

/// A kept access that is kept only on some VALUES of its resource (fix round
/// 2, N2), as `(access, the values it stays allowed on)`. A second
/// override-priority deny withholds `access` on every other value, through
/// `isExcludes: true`.
///
/// - `trinouser`: `impersonate` of itself only. With an egret JWT the session
///   user is the principal, so EXPLAIN never needs another user; a `{public}`
///   impersonate allow must not let the credential act as anyone else.
/// - `systemproperty`: `alter` of `query_max_run_time` only, the one system
///   session property egret's client sends.
///
/// `queryid` keeps `execute` on every value: its value is the id of the query
/// being run, which egret cannot know before the engine assigns it.
fn narrowed(leaf: &str) -> Option<(&'static str, &'static [&'static str])> {
    match leaf {
        "trinouser" => Some(("impersonate", &[LINEAGE_PRINCIPAL])),
        "systemproperty" => Some(("alter", &[LINEAGE_SYSTEM_PROPERTY])),
        _ => None,
    }
}

/// An access type as written in an item, without a tag service's
/// `<serviceType>:` prefix.
fn bare(access: &str) -> &str {
    access.rsplit(':').next().unwrap_or(access)
}

fn item_accesses(item: &Value) -> Vec<String> {
    item.get("accesses")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|a| a.get("type").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

fn accesses_json(accesses: &[String]) -> Value {
    Value::Array(
        accesses
            .iter()
            .map(|a| json!({"type": a, "isAllowed": true}))
            .collect(),
    )
}

fn lineage_item(accesses: &[String]) -> Value {
    json!({
        "users": [LINEAGE_PRINCIPAL],
        "groups": [],
        "roles": [],
        "conditions": [],
        "delegateAdmin": false,
        "accesses": accesses_json(accesses),
    })
}

fn int_field(body: &Value, field: &str) -> i64 {
    body.get(field).and_then(Value::as_i64).unwrap_or(0)
}

/// The accesses a policy keeps for the principal: read on the catalog chain
/// (and on a tag policy), plus [`kept_accesses`] of each special resource it
/// names.
fn kept_for_policy(body: &Value) -> BTreeSet<&'static str> {
    let mut kept: BTreeSet<&'static str> = LINEAGE_READ_ACCESSES.into_iter().collect();
    if let Some(res) = body.get("resources").and_then(Value::as_object) {
        for key in res.keys() {
            kept.extend(kept_accesses(key).iter().copied());
        }
    }
    kept
}

fn push_item(body: &mut Value, bucket: &str, item: Value) {
    match body.get_mut(bucket).and_then(Value::as_array_mut) {
        Some(items) => items.push(item),
        None => body[bucket] = Value::Array(vec![item]),
    }
}

/// Rewrite one served policy (resource or tag) for the principal.
fn transform_policy(body: &mut Value) {
    if !body.is_object() {
        return;
    }
    match int_field(body, "policyType") {
        1 => {
            let item = json!({
                "users": [LINEAGE_PRINCIPAL], "groups": [], "roles": [], "conditions": [],
                "delegateAdmin": false,
                "accesses": [{"type": "select", "isAllowed": true}],
                "dataMaskInfo": {"dataMaskType": "MASK_NONE"},
            });
            match body
                .get_mut("dataMaskPolicyItems")
                .and_then(Value::as_array_mut)
            {
                Some(items) => items.insert(0, item),
                None => body["dataMaskPolicyItems"] = Value::Array(vec![item]),
            }
        }
        2 => {
            let item = json!({
                "users": [LINEAGE_PRINCIPAL], "groups": [], "roles": [], "conditions": [],
                "delegateAdmin": false,
                "accesses": [{"type": "select", "isAllowed": true}],
                "rowFilterInfo": {"filterExpr": ""},
            });
            match body
                .get_mut("rowFilterPolicyItems")
                .and_then(Value::as_array_mut)
            {
                Some(items) => items.insert(0, item),
                None => body["rowFilterPolicyItems"] = Value::Array(vec![item]),
            }
        }
        0 => {
            let kept = kept_for_policy(body);
            let collect = |bucket: &str| -> Vec<String> {
                let mut seen = BTreeSet::new();
                let mut out = Vec::new();
                for item in body
                    .get(bucket)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    for a in item_accesses(item) {
                        if seen.insert(a.clone()) {
                            out.push(a);
                        }
                    }
                }
                out
            };
            // 2. The read part of every deny is lifted for the principal.
            let denied_reads: Vec<String> = collect("denyPolicyItems")
                .into_iter()
                .filter(|a| LINEAGE_READ_ACCESSES.contains(&bare(a)))
                .collect();
            // 3. Every non-kept access an allow grants is withheld from it.
            let allowed_others: Vec<String> = collect("policyItems")
                .into_iter()
                .filter(|a| !kept.contains(bare(a)))
                .collect();
            if !denied_reads.is_empty() {
                push_item(body, "denyExceptions", lineage_item(&denied_reads));
            }
            if !allowed_others.is_empty() {
                push_item(body, "allowExceptions", lineage_item(&allowed_others));
            }
        }
        _ => {}
    }
}

fn level(value: &str) -> Value {
    json!({"values": [value], "isExcludes": false, "isRecursive": false})
}

fn appended_policy(
    id: i64,
    name: String,
    service: &str,
    resources: serde_json::Map<String, Value>,
    allow: Option<Vec<String>>,
    deny: Option<Vec<String>>,
) -> Value {
    json!({
        "id": id,
        "version": 1,
        "name": name,
        "service": service,
        "serviceType": "trino",
        "policyType": 0,
        // The deny is an override so no inherited allow at a higher band can
        // outrank it; at equal priority Ranger evaluates a deny first.
        "policyPriority": if deny.is_some() { 1 } else { 0 },
        "isEnabled": true,
        "isAuditEnabled": true,
        "isDenyAllElse": false,
        "description": "view governance R5: egret-lineage, added at download time",
        "resources": Value::Object(resources),
        "policyItems": allow.map(|a| vec![lineage_item(&a)]).unwrap_or_default(),
        "denyPolicyItems": deny.map(|a| vec![lineage_item(&a)]).unwrap_or_default(),
        "allowExceptions": [],
        "denyExceptions": [],
    })
}

/// The policies the transform appends for `catalogs` under `service`, built
/// from the served Trino service-def's own resource hierarchies.
fn appended_policies(service: &str, service_def: &Value, catalogs: &[String]) -> Vec<Value> {
    let mut out = Vec::new();
    let mut id = LINEAGE_POLICY_ID_BASE;
    let mut sorted: Vec<&String> = catalogs.iter().collect();
    sorted.sort();
    sorted.dedup();
    for catalog in sorted {
        let mut res = serde_json::Map::new();
        res.insert("catalog".into(), level(catalog));
        for l in ["schema", "table", "column"] {
            res.insert(l.into(), level("*"));
        }
        out.push(appended_policy(
            id,
            format!("{LINEAGE_POLICY_PREFIX}read-{catalog}"),
            service,
            res,
            Some(
                LINEAGE_READ_ACCESSES
                    .iter()
                    .map(|a| a.to_string())
                    .collect(),
            ),
            None,
        ));
        id += 1;
    }
    // The system property egret's client sets (I2). Allowed here; every other
    // value stays denied by the narrowed deny below.
    let mut prop = serde_json::Map::new();
    prop.insert("systemproperty".into(), level(LINEAGE_SYSTEM_PROPERTY));
    out.push(appended_policy(
        id,
        format!("{LINEAGE_POLICY_PREFIX}alter-{LINEAGE_SYSTEM_PROPERTY}"),
        service,
        prop,
        Some(vec!["alter".to_string()]),
        None,
    ));
    id += 1;

    let all: Vec<String> = service_def
        .get("accessTypes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|a| a.get("name").and_then(Value::as_str))
        .filter(|n| *n != "all")
        .map(str::to_string)
        .collect();
    let resources = parse_resources(service_def);
    for leaf in resources
        .iter()
        .filter(|r| is_hierarchy_leaf(r, &resources))
    {
        let chain = hierarchy_chain(leaf, &resources);
        let allowed: &[String] = if leaf.access_type_restrictions.is_empty() {
            &all
        } else {
            &leaf.access_type_restrictions
        };
        let keep = kept_accesses(&leaf.name);
        let denied: Vec<String> = allowed
            .iter()
            .filter(|a| !keep.contains(&a.as_str()))
            .cloned()
            .collect();
        let mut res = serde_json::Map::new();
        for r in &chain {
            res.insert(r.name.clone(), level("*"));
        }
        let names: Vec<&str> = chain.iter().map(|r| r.name.as_str()).collect();
        if let Some((access, only)) = narrowed(&leaf.name) {
            let mut narrow = res.clone();
            narrow.insert(
                leaf.name.clone(),
                json!({"values": only, "isExcludes": true, "isRecursive": false}),
            );
            out.push(appended_policy(
                id,
                format!("{LINEAGE_POLICY_PREFIX}deny-other-{}", names.join("-")),
                service,
                narrow,
                None,
                Some(vec![access.to_string()]),
            ));
            id += 1;
        }
        if denied.is_empty() {
            continue;
        }
        out.push(appended_policy(
            id,
            format!("{LINEAGE_POLICY_PREFIX}deny-{}", names.join("-")),
            service,
            res,
            None,
            Some(denied),
        ));
        id += 1;
    }
    out
}

/// Rewrite a served envelope for `egret-lineage` (module doc). `catalogs` is
/// every catalog the profile's engines attach.
///
/// Trino only: an envelope whose service-def is not Trino's is left unchanged.
pub fn apply_lineage_transform(sp: &mut ServicePolicies, catalogs: &[String]) {
    if sp.service_def.get("name").and_then(Value::as_str) != Some(crate::TRINO_DEF_NAME) {
        return;
    }
    for body in sp.policies.iter_mut() {
        transform_policy(body);
    }
    if let Some(tags) = sp.tag_policies.as_mut() {
        for body in tags.policies.iter_mut() {
            transform_policy(body);
        }
    }
    let appended = appended_policies(&sp.service_name, &sp.service_def, catalogs);
    sp.policies.extend(appended);
}

/// The `<hostname>` of a poll's `pluginId` (`<appId>@<hostname>-<service>`,
/// live result §1 Q4) when the id names `service`, the service it polls.
pub fn plugin_host<'a>(plugin_id: Option<&'a str>, service: &str) -> Option<&'a str> {
    let (app, rest) = plugin_id?.split_once('@')?;
    if app.is_empty() {
        return None;
    }
    let host = rest.strip_suffix(service)?.strip_suffix('-')?;
    (!host.is_empty()).then_some(host)
}

/// Whether `host` is the hostname of a pod egret's provisioner runs for the
/// Trino engine `engine`. The provisioner renders two Deployments,
/// `{engine}-coordinator` and `{engine}-worker`
/// (`egret_provisioner::trino`), and a Deployment's pod is named
/// `<deployment>-<pod-template-hash>-<suffix>`, which Kubernetes uses as the
/// pod's hostname. So the host must be exactly that: the prefix and then two
/// lowercase alphanumeric segments, nothing more. An engine whose name happens
/// to extend another's cannot match it, because the remainder would have more
/// than two segments.
pub fn host_is_engine_pod(host: &str, engine: &str) -> bool {
    let segment = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    };
    ["coordinator", "worker"].iter().any(|role| {
        host.strip_prefix(engine)
            .and_then(|r| r.strip_prefix('-'))
            .and_then(|r| r.strip_prefix(role))
            .and_then(|r| r.strip_prefix('-'))
            .and_then(|r| r.split_once('-'))
            .is_some_and(|(hash, suffix)| segment(hash) && segment(suffix))
    })
}

/// The `pluginId` a coordinator pod of `engine` sends when polling `service`,
/// with a placeholder pod suffix. `explain` decides eligibility through it, so
/// explain and the download cannot disagree (fix round 2, N4).
pub fn representative_plugin_id(engine: &str, service: &str) -> String {
    format!("trino@{engine}-coordinator-explain-0-{service}")
}

/// The engines bound to `service_id` when the profile as a whole may be served
/// the transform: at least one is bound, and EVERY bound engine is a Trino
/// engine egret provisioned, in JWT delegation mode (review I4). Sorted by
/// name. `None` otherwise.
pub async fn eligible_bound_engines_via(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<Option<Vec<String>>, PolicyStoreError> {
    let rows: Vec<(String, bool)> = sqlx::query_as(
        // `coalesce`: a spec with no `delegationMode` makes the comparison
        // NULL, which is "not eligible", not a decode error.
        "SELECT name, coalesce(type = 'trino' AND origin = 'provisioned' \
                       AND spec->'trinoConfig'->>'delegationMode' = 'jwt', false) \
           FROM engine_mgmt.engines WHERE authz_service_id = $1 ORDER BY name",
    )
    .bind(service_id)
    .fetch_all(&mut *conn)
    .await?;
    if rows.is_empty() || rows.iter().any(|(_, ok)| !ok) {
        return Ok(None);
    }
    Ok(Some(rows.into_iter().map(|(n, _)| n).collect()))
}

/// `Some(catalogs)` when the POLLER is served the transform (fix round 2, N1):
///
/// 1. its `pluginId` names `service_name`, the polled service;
/// 2. the profile is eligible as a whole ([`eligible_bound_engines_via`]);
/// 3. its hostname is a pod of one of those bound engines
///    ([`host_is_engine_pod`]).
///
/// Anything else, an unknown host, an unbound or ineligible engine's host, a
/// missing or malformed id, is `None`: the stored policies, untransformed.
/// `catalogs` are every catalog the profile's engines attach.
pub async fn lineage_catalogs_for_poll_via(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    service_name: &str,
    plugin_id: Option<&str>,
) -> Result<Option<Vec<String>>, PolicyStoreError> {
    let Some(host) = plugin_host(plugin_id, service_name) else {
        return Ok(None);
    };
    let Some(engines) = eligible_bound_engines_via(conn, service_id).await? else {
        return Ok(None);
    };
    if !engines.iter().any(|e| host_is_engine_pod(host, e)) {
        return Ok(None);
    }
    let mut names: Vec<String> = crate::catalogs_for_service_via(conn, service_id)
        .await?
        .into_iter()
        .map(|c| c.name)
        .collect();
    names.sort();
    Ok(Some(names))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plugin_id_names_its_service_only_in_the_measured_shape() {
        assert_eq!(
            plugin_host(Some("trino@e00d4f5be6a4-vg-trino"), "vg-trino"),
            Some("e00d4f5be6a4")
        );
        assert_eq!(plugin_host(Some("trino@pod-1-a-b"), "a-b"), Some("pod-1"));
        for bad in [
            None,
            Some(""),
            Some("trino@e00d4f5be6a4-vg-trino"),
            Some("@host-svc"),
            Some("trino@-svc"),
            Some("trino-host-svc"),
        ] {
            assert_eq!(plugin_host(bad, "svc"), None, "{bad:?}");
        }
    }

    #[test]
    fn only_the_provisioners_pod_names_are_an_engines_hosts() {
        for ok in [
            "lake-coordinator-7d9f8c6b5-x2k4p",
            "lake-worker-5c4b8d9f7-ab12c",
        ] {
            assert!(host_is_engine_pod(ok, "lake"), "{ok}");
        }
        for bad in [
            "lake",
            "lake-coordinator",
            "lake-coordinator-7d9f8c6b5",
            "lake-coordinator-7d9f8c6b5-x2k4p-extra",
            "lake-gateway-7d9f8c6b5-x2k4p",
            "lake2-coordinator-7d9f8c6b5-x2k4p",
            "lake-coordinator-7D9F-x2k4p",
            "e00d4f5be6a4",
        ] {
            assert!(!host_is_engine_pod(bad, "lake"), "{bad}");
        }
        // An engine whose name extends another's is not the other's pod.
        assert!(!host_is_engine_pod(
            "lake-coordinator-abc-coordinator-h-s",
            "lake"
        ));
        assert!(host_is_engine_pod(
            "lake-coordinator-abc-coordinator-h-s",
            "lake-coordinator-abc"
        ));
        let rep = representative_plugin_id("lake", "svc-1");
        assert!(host_is_engine_pod(
            plugin_host(Some(&rep), "svc-1").unwrap(),
            "lake"
        ));
    }

    fn envelope(policies: Vec<Value>) -> ServicePolicies {
        ServicePolicies::new(
            "p".into(),
            1,
            7,
            crate::trino_service_def().unwrap(),
            policies,
            None,
        )
    }

    #[test]
    fn the_transform_is_deterministic_and_leaves_starrocks_alone() {
        let mask = json!({"id": 3, "name": "m", "policyType": 1, "dataMaskPolicyItems": [
            {"users": [], "groups": ["public"], "accesses": [{"type": "select"}],
             "dataMaskInfo": {"dataMaskType": "MASK_NULL"}}]});
        let mut a = envelope(vec![mask.clone()]);
        let mut b = envelope(vec![mask.clone()]);
        apply_lineage_transform(&mut a, &["ice".into(), "hive".into()]);
        apply_lineage_transform(&mut b, &["hive".into(), "ice".into()]);
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
        assert_eq!(
            a.policies[0]["dataMaskPolicyItems"][0]["users"][0],
            LINEAGE_PRINCIPAL
        );
        assert_eq!(a.policy_version, 7, "the served version is unchanged");

        let mut sr = ServicePolicies::new(
            "s".into(),
            2,
            1,
            crate::starrocks_service_def().unwrap(),
            vec![mask.clone()],
            None,
        );
        apply_lineage_transform(&mut sr, &["ice".into()]);
        assert_eq!(sr.policies, vec![mask]);
    }

    /// Review I2, each defence on its own: with `{public}` allowed every
    /// access on the catalog, the principal is refused a write by the appended
    /// deny alone AND by the allow exceptions alone, at table and column level.
    #[test]
    fn each_write_defence_holds_on_its_own() {
        use egret_ranger_eval::eval::{
            evaluate, AccessRequest, Decision, EngineConfig, EngineType, PluginVersion,
        };
        use egret_ranger_eval::model::ServicePolicySet;
        use egret_ranger_eval::resource::RequestResource;
        use egret_ranger_eval::servicedef::ServiceDef;

        let lvl = |v: &str| json!({"values": [v], "isExcludes": false, "isRecursive": false});
        let all: Vec<Value> = [
            "select", "insert", "delete", "create", "drop", "alter", "use", "show",
        ]
        .iter()
        .map(|a| json!({"type": a, "isAllowed": true}))
        .collect();
        let public_all = json!({
            "id": 1, "version": 1, "name": "public all", "service": "p", "policyType": 0,
            "isEnabled": true, "policyPriority": 0,
            "resources": {"catalog": lvl("ice"), "schema": lvl("*"), "table": lvl("*"), "column": lvl("*")},
            "policyItems": [{"users": [], "groups": ["public"], "roles": [],
                "accesses": all}],
            "denyPolicyItems": [], "allowExceptions": [], "denyExceptions": []
        });
        let mut sp = envelope(vec![public_all]);
        apply_lineage_transform(&mut sp, &["ice".into()]);
        let without = |drop_deny: bool, drop_exceptions: bool| {
            let mut v = serde_json::to_value(&sp).unwrap();
            let ps = v["policies"].as_array_mut().unwrap();
            if drop_deny {
                ps.retain(|p| {
                    !p["name"]
                        .as_str()
                        .unwrap()
                        .starts_with(&format!("{LINEAGE_POLICY_PREFIX}deny"))
                });
            }
            if drop_exceptions {
                for p in ps.iter_mut() {
                    p["allowExceptions"] = json!([]);
                }
            }
            serde_json::from_value::<ServicePolicySet>(v).unwrap()
        };
        let def = ServiceDef::parse(&crate::trino_service_def().unwrap());
        let ask = |set: &ServicePolicySet, user: &str, access: &str, column: bool| {
            let mut r = RequestResource::new()
                .with("catalog", "ice")
                .with("schema", "s")
                .with("table", "t");
            if column {
                r = r.with("column", "c");
            }
            evaluate(
                set,
                &AccessRequest {
                    user: user.into(),
                    groups: Default::default(),
                    roles: Default::default(),
                    access_type: access.into(),
                    resource: r,
                    owner: None,
                    resolved_tags: None,
                },
                &def,
                &EngineConfig {
                    engine: Some(EngineType::Trino),
                    plugin_version: Some(PluginVersion::Ranger2_8_0),
                    ..EngineConfig::without_super_users()
                },
            )
        };
        for (drop_deny, drop_exceptions) in [(false, true), (true, false)] {
            let set = without(drop_deny, drop_exceptions);
            for column in [false, true] {
                assert!(matches!(
                    ask(&set, "alice", "insert", column),
                    Decision::Allowed { .. }
                ));
                for w in ["insert", "delete", "create", "drop", "alter", "show"] {
                    let d = ask(&set, LINEAGE_PRINCIPAL, w, column);
                    assert!(
                        !matches!(d, Decision::Allowed { .. }),
                        "deny dropped {drop_deny}, exceptions dropped {drop_exceptions}: `{w}` \
                         (column {column}): {d:?}"
                    );
                }
                assert!(matches!(
                    ask(&set, LINEAGE_PRINCIPAL, "select", column),
                    Decision::Allowed { .. }
                ));
            }
        }
    }

    /// Beyond `select` and `use`, the one allow is `alter` on the single
    /// system property egret's client sets (I2).
    #[test]
    fn no_appended_policy_allows_more_than_select_and_use() {
        let mut sp = envelope(Vec::new());
        apply_lineage_transform(&mut sp, &["ice".into()]);
        let property = json!({"systemproperty": level(LINEAGE_SYSTEM_PROPERTY)});
        for p in &sp.policies {
            for item in p["policyItems"].as_array().unwrap() {
                for a in item_accesses(item) {
                    let read = LINEAGE_READ_ACCESSES.contains(&a.as_str());
                    let the_property = a == "alter" && p["resources"] == property;
                    assert!(read || the_property, "{p}");
                }
            }
            for item in p["denyPolicyItems"].as_array().unwrap() {
                for a in item_accesses(item) {
                    assert!(!LINEAGE_READ_ACCESSES.contains(&a.as_str()), "{p}");
                }
            }
        }
    }
}
