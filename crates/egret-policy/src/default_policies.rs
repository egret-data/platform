//! Seed default "admin has all access" policies for a freshly-created Trino
//! service, mirroring Ranger's `ServiceDBStore.createDefaultPolicies`: one
//! default access policy per resource hierarchy, so a platform admin is never
//! locked out of a service the moment it's created.
//!
//! The hierarchies are computed from the embedded Trino service-def
//! (`service-defs/trino.json`), not hardcoded: a resource is a valid
//! hierarchy termination point when it is marked `isValidLeaf: true` OR when
//! no other resource declares it as `parent` (true tree leaves, e.g. `column`,
//! `trinouser`, `function`, ... never carry `isValidLeaf` themselves — nothing
//! is nested under them). Walking each termination point's `parent` chain up
//! to the root yields the resource-name path for that hierarchy's `resources`
//! map. This produces all 13 hierarchies in the def (the catalog/schema/
//! table/column chain plus the 9 standalone/attached admin resources).

use crate::infra_names::{
    ADMIN_ALL_ACCESS_NAME_PREFIX, DEFAULT_ALL_ACCESS_NAME_PREFIX, DEFAULT_ALL_ACCESS_SERVICE_TYPE,
};
use crate::service_def::{starrocks_service_def, trino_service_def};
use crate::{PolicyStore, PolicyStoreError};
// The hierarchy walker lives in `egret-ranger-eval` (SP4d Task 4), which
// needs the same walk to reproduce Ranger's resource matching. There is
// exactly one: a second walker that disagreed with this seeder would hand
// the evaluator a different hierarchy from the one the seeded policies were
// built against, and the disagreement would only ever surface as a wrong
// access decision. The dependency runs seeder -> evaluator, never back:
// `egret-ranger-eval` is pure (no sqlx, no tokio, no network, no
// filesystem) and knows nothing about this crate.
use egret_ranger_eval::servicedef::{hierarchy_chain, is_hierarchy_leaf, parse_resources};
use serde_json::{json, Map, Value};

/// Every top-level accessType name in the def, excluding the `all` alias
/// (its `impliedGrants` already enumerate these individually — listing both
/// would just be redundant policyItems accesses). Used as the fallback grant
/// set for a hierarchy whose leaf resource has no `accessTypeRestrictions`.
fn all_access_types(def: &Value) -> Vec<String> {
    def["accessTypes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| a["name"].as_str())
        .filter(|n| *n != "all")
        .map(str::to_string)
        .collect()
}

/// Build one default all-access policy per resource hierarchy in `def`,
/// granting the given `users` and `groups`. Returns `(name, body)` pairs;
/// persistence is the caller's concern. The policy name is
/// `"{name_prefix} - {resource, chain, ...}"` and `serviceType` is
/// `service_type`, so this works for any Ranger service-def (Trino,
/// StarRocks, ...) and either a group-grant or a user-grant.
fn build_default_policies(
    def: &Value,
    service_name: &str,
    service_type: &str,
    name_prefix: &str,
    users: &[String],
    groups: &[String],
) -> Vec<(String, Value)> {
    let resources = parse_resources(def);
    let fallback_accesses = all_access_types(def);

    let mut policies = Vec::new();
    for res in &resources {
        if !is_hierarchy_leaf(res, &resources) {
            continue;
        }
        let chain = hierarchy_chain(res, &resources);
        let accesses: &[String] = if res.access_type_restrictions.is_empty() {
            &fallback_accesses
        } else {
            &res.access_type_restrictions
        };

        let mut resources_map = Map::new();
        for r in &chain {
            resources_map.insert(
                r.name.clone(),
                json!({ "values": ["*"], "isExcludes": false, "isRecursive": false }),
            );
        }
        let names: Vec<&str> = chain.iter().map(|r| r.name.as_str()).collect();
        let policy_name = format!("{} - {}", name_prefix, names.join(", "));

        let body = json!({
            "name": policy_name,
            "service": service_name,
            "serviceType": service_type,
            "policyType": 0,
            "isEnabled": true,
            "resources": Value::Object(resources_map),
            "policyItems": [{
                "groups": groups,
                "users": users,
                "roles": [],
                "accesses": accesses.iter().map(|a| json!({ "type": a, "isAllowed": true })).collect::<Vec<_>>(),
                "delegateAdmin": true,
            }],
            "denyPolicyItems": [],
            "allowExceptions": [],
            "denyExceptions": [],
            "isDenyAllElse": false,
        });

        policies.push((policy_name, body));
    }
    policies
}

impl PolicyStore {
    /// Idempotently seed one default admin-all-access policy per Trino
    /// resource hierarchy. If the service already has any policy (default or
    /// user-authored), this is a no-op that returns `0` — we never want to
    /// duplicate policies on a re-run. Returns the number of policies created.
    pub async fn seed_default_trino_policies(
        &self,
        service_id: i64,
        service_name: &str,
        admin_principal: &str,
    ) -> Result<usize, PolicyStoreError> {
        // Task 8: check BEFORE the "already has policies" no-op below — a
        // generated profile always has at least one policy once the compiler
        // has run, so without this ordering the guard would never fire here
        // and this call would just silently no-op instead of refusing.
        self.refuse_if_generated_pooled(service_id).await?;
        // SP4d-3 final review I-1: the set below is built from the Trino
        // service-def only. On any other type the real plugin rejects every row
        // on every refresh, so refuse here — the single enforcement point, so
        // no caller can seed a non-Trino profile. After the generated-profile
        // check, so that 409 still wins.
        let svc = self.get_service_by_id(service_id).await?;
        if svc.r#type != DEFAULT_ALL_ACCESS_SERVICE_TYPE {
            return Err(PolicyStoreError::BadRequest(format!(
                "seed-defaults seeds the Trino default policy set; profile {:?} is of type {:?}, \
                 whose engine would reject every one of those policies",
                svc.name, svc.r#type
            )));
        }
        if !self.list_policies(service_id).await?.is_empty() {
            return Ok(0);
        }

        let def = trino_service_def()?;
        let policies = build_default_policies(
            &def,
            service_name,
            "trino",
            DEFAULT_ALL_ACCESS_NAME_PREFIX,
            &[],
            &[admin_principal.to_string()],
        );

        let mut created = 0usize;
        for (name, body) in &policies {
            self.upsert_infra_policy(service_id, name, body).await?;
            created += 1;
        }
        Ok(created)
    }

    /// Seed an "admin has all access" policy per resource hierarchy for the
    /// given `users` on any Ranger service (Trino or StarRocks). Unlike
    /// [`seed_default_trino_policies`], this has NO empty-service guard and
    /// upserts each policy, so it is safe to call repeatedly (e.g. as the set
    /// of platform admins changes). Returns `Ok(0)` when `users` is empty.
    pub async fn seed_admin_all_access(
        &self,
        service_id: i64,
        service_name: &str,
        service_type: &str,
        users: &[String],
    ) -> Result<usize, PolicyStoreError> {
        if users.is_empty() {
            return Ok(0);
        }
        let def = if service_type == "starrocks" {
            starrocks_service_def()?
        } else {
            trino_service_def()?
        };
        let policies = build_default_policies(
            &def,
            service_name,
            service_type,
            ADMIN_ALL_ACCESS_NAME_PREFIX,
            users,
            &[],
        );

        let mut count = 0usize;
        for (name, body) in &policies {
            self.upsert_infra_policy(service_id, name, body).await?;
            count += 1;
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_ranger_eval::servicedef::ResourceDef;

    /// `infra_names` lives in a leaf crate that cannot import the def name, so
    /// its copy of "trino" is pinned to the real one here.
    #[test]
    fn the_default_set_is_infrastructure_on_the_trino_def_name() {
        assert_eq!(
            DEFAULT_ALL_ACCESS_SERVICE_TYPE,
            crate::service_def::TRINO_DEF_NAME
        );
    }

    #[test]
    fn computes_all_13_trino_hierarchies() {
        let def = trino_service_def().unwrap();
        let resources = parse_resources(&def);
        let leaves: Vec<&ResourceDef> = resources
            .iter()
            .filter(|r| is_hierarchy_leaf(r, &resources))
            .collect();
        assert_eq!(leaves.len(), 13, "expected one hierarchy per def resource");

        let names: Vec<String> = leaves
            .iter()
            .map(|l| {
                hierarchy_chain(l, &resources)
                    .iter()
                    .map(|r| r.name.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .collect();
        assert!(names.contains(&"catalog".to_string()));
        assert!(names.contains(&"catalog,schema".to_string()));
        assert!(names.contains(&"catalog,schema,table".to_string()));
        assert!(names.contains(&"catalog,schema,table,column".to_string()));
        assert!(names.contains(&"trinouser".to_string()));
        assert!(names.contains(&"catalog,sessionproperty".to_string()));
        assert!(names.contains(&"catalog,schema,procedure".to_string()));
        assert!(names.contains(&"catalog,schema,schemafunction".to_string()));
        assert!(names.contains(&"queryid".to_string()));
        assert!(names.contains(&"sysinfo".to_string()));
        assert!(names.contains(&"role".to_string()));
        assert!(names.contains(&"function".to_string()));
        assert!(names.contains(&"systemproperty".to_string()));
    }

    #[test]
    fn restricted_resource_uses_its_own_access_types() {
        let def = trino_service_def().unwrap();
        let resources = parse_resources(&def);
        let trinouser = resources.iter().find(|r| r.name == "trinouser").unwrap();
        assert_eq!(trinouser.access_type_restrictions, vec!["impersonate"]);
    }

    #[test]
    fn fallback_access_types_exclude_all_alias() {
        let def = trino_service_def().unwrap();
        let fallback = all_access_types(&def);
        assert!(!fallback.contains(&"all".to_string()));
        assert!(fallback.contains(&"select".to_string()));
        assert!(fallback.contains(&"impersonate".to_string()));
    }

    #[test]
    fn build_grants_users_with_service_type() {
        let def = trino_service_def().unwrap();
        let policies = build_default_policies(
            &def,
            "eng1",
            "trino",
            "egret-admin",
            &["alice".to_string()],
            &[],
        );
        assert_eq!(policies.len(), 13);
        let (name, body) = policies
            .iter()
            .find(|(n, _)| n == "egret-admin - catalog, schema, table")
            .unwrap();
        assert_eq!(name, "egret-admin - catalog, schema, table");
        assert_eq!(body["serviceType"], "trino");
        assert_eq!(body["policyItems"][0]["users"], json!(["alice"]));
        assert_eq!(body["policyItems"][0]["groups"], json!([]));
        let accesses = body["policyItems"][0]["accesses"].as_array().unwrap();
        assert!(accesses.iter().any(|a| a["type"] == "select"));
    }

    #[test]
    fn build_works_for_starrocks_service_type() {
        let def = crate::service_def::starrocks_service_def().unwrap();
        let policies = build_default_policies(
            &def,
            "sr1",
            "starrocks",
            "egret-admin",
            &["bob".to_string()],
            &[],
        );
        assert!(!policies.is_empty());
        let (_, body) = &policies[0];
        assert_eq!(body["serviceType"], "starrocks");
        assert_eq!(body["policyItems"][0]["users"], json!(["bob"]));
    }
}
