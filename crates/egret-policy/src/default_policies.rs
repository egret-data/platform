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

use crate::service_def::{starrocks_service_def, trino_service_def};
use crate::{PolicyStore, PolicyStoreError};
use serde_json::{json, Map, Value};

struct ResourceDef {
    name: String,
    parent: String,
    is_valid_leaf: bool,
    access_type_restrictions: Vec<String>,
}

fn parse_resources(def: &Value) -> Vec<ResourceDef> {
    def["resources"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let name = r["name"].as_str()?.to_string();
            Some(ResourceDef {
                name,
                parent: r["parent"].as_str().unwrap_or("").to_string(),
                is_valid_leaf: r["isValidLeaf"].as_bool().unwrap_or(false),
                access_type_restrictions: r["accessTypeRestrictions"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
            })
        })
        .collect()
}

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

/// A resource is a hierarchy termination point if it's explicitly marked
/// `isValidLeaf`, or if it's a true tree leaf (nothing declares it as parent).
fn is_hierarchy_leaf(res: &ResourceDef, all: &[ResourceDef]) -> bool {
    res.is_valid_leaf || !all.iter().any(|r| r.parent == res.name)
}

/// Root-to-leaf resource-name chain for one hierarchy termination point.
fn hierarchy_chain<'a>(leaf: &'a ResourceDef, all: &'a [ResourceDef]) -> Vec<&'a ResourceDef> {
    let mut chain = vec![leaf];
    let mut cur = leaf;
    while !cur.parent.is_empty() {
        match all.iter().find(|r| r.name == cur.parent) {
            Some(p) => {
                chain.push(p);
                cur = p;
            }
            None => break,
        }
    }
    chain.reverse();
    chain
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
        if !self.list_policies(service_id).await?.is_empty() {
            return Ok(0);
        }

        let def = trino_service_def()?;
        let policies = build_default_policies(
            &def,
            service_name,
            "trino",
            "all",
            &[],
            &[admin_principal.to_string()],
        );

        let mut created = 0usize;
        for (name, body) in &policies {
            self.create_policy(service_id, name, body).await?;
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
        let policies =
            build_default_policies(&def, service_name, service_type, "egret-admin", users, &[]);

        let mut count = 0usize;
        for (name, body) in &policies {
            self.upsert_policy(service_id, name, body).await?;
            count += 1;
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
