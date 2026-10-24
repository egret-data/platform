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
    is_admin_all_access_policy_name, ADMIN_ALL_ACCESS_NAME_PREFIX, DEFAULT_ALL_ACCESS_NAME_PREFIX,
    DEFAULT_ALL_ACCESS_SERVICE_TYPE,
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
    /// given `users` on any Ranger service (Trino or StarRocks): engine apply's
    /// call, [`sync_admin_all_access_in_tx`] in its own transaction. Unlike
    /// [`seed_default_trino_policies`], this has NO empty-service guard, so it
    /// is safe to call repeatedly. An empty `users` rewrites the existing
    /// bodies to name nobody and creates none (before the username-takeover
    /// fix it wrote nothing, so a set that shrank to nothing never cleared a
    /// stale body). The bodies carry `service_name` and are built from
    /// `service_type`'s def, as apply passes them. Returns how many
    /// `egret-admin` policies the service has afterwards.
    pub async fn seed_admin_all_access(
        &self,
        service_id: i64,
        service_name: &str,
        service_type: &str,
        users: &[String],
    ) -> Result<usize, PolicyStoreError> {
        let mut tx = self.pool().begin().await?;
        let sync =
            write_admin_all_access_in_tx(&mut tx, service_id, service_name, service_type, users)
                .await?;
        tx.commit().await?;
        Ok(sync.present)
    }
}

/// What [`sync_admin_all_access_in_tx`] did to one service's `egret-admin`
/// policies.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdminPolicySync {
    /// Whether any body changed (and so `policy_version` moved, once).
    pub changed: bool,
    /// How many `egret-admin` policies the service has afterwards.
    pub present: usize,
    /// Names the bodies named before and no longer name, sorted.
    pub removed: Vec<String>,
    /// Names they name now and did not before, sorted.
    pub added: Vec<String>,
}

/// Every service holding at least one `egret-admin - …` infra policy: the
/// engines egret seeded, whose admin set [`sync_admin_all_access_in_tx`]
/// keeps current. Ascending.
pub async fn admin_policy_services_via(
    conn: &mut sqlx::PgConnection,
) -> Result<Vec<i64>, PolicyStoreError> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT service_id, name FROM policy.authz_policies \
          WHERE managed_by = 'infra' AND starts_with(name, $1) ORDER BY service_id",
    )
    .bind(ADMIN_ALL_ACCESS_NAME_PREFIX)
    .fetch_all(&mut *conn)
    .await?;
    let mut ids: Vec<i64> = rows
        .into_iter()
        .filter(|(_, name)| is_admin_all_access_policy_name(name))
        .map(|(id, _)| id)
        .collect();
    ids.dedup();
    Ok(ids)
}

/// Make `service_id`'s `egret-admin - …` policies name exactly `users`, on
/// the caller's transaction (username-takeover spec 2026-10-23 §4.2, D2).
///
/// `users` is the engine-admin set's usernames, sorted (the caller mints
/// them). One body per resource hierarchy of the service's def, upserted only
/// where it differs (`IS DISTINCT FROM`), and `policy_version` bumped once
/// when any did, so an unchanged set writes nothing and a plugin re-downloads
/// nothing. An empty `users` rewrites the existing bodies to `users: []`, an
/// item that matches nobody, and creates none. The caller holds the service's
/// profile lock (every recompile does).
pub async fn sync_admin_all_access_in_tx(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    users: &[String],
) -> Result<AdminPolicySync, PolicyStoreError> {
    let (service_name, service_type) = service_name_and_type(conn, service_id).await?;
    write_admin_all_access_in_tx(conn, service_id, &service_name, &service_type, users).await
}

async fn service_name_and_type(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<(String, String), PolicyStoreError> {
    sqlx::query_as("SELECT name, type FROM policy.authz_services WHERE id = $1")
        .bind(service_id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| PolicyStoreError::NotFound(format!("service id {service_id}")))
}

/// [`sync_admin_all_access_in_tx`] with the bodies' service name and def type
/// given (engine apply passes its own). The name guard reads the row's type.
async fn write_admin_all_access_in_tx(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    service_name: &str,
    service_type: &str,
    users: &[String],
) -> Result<AdminPolicySync, PolicyStoreError> {
    let (_, row_type) = service_name_and_type(conn, service_id).await?;
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

    let before: Vec<(String, Value)> = sqlx::query_as(
        "SELECT name, policy_jsonb FROM policy.authz_policies \
          WHERE service_id = $1 AND managed_by = 'infra' AND starts_with(name, $2)",
    )
    .bind(service_id)
    .bind(ADMIN_ALL_ACCESS_NAME_PREFIX)
    .fetch_all(&mut *conn)
    .await?;
    let named_before: std::collections::BTreeSet<String> = before
        .iter()
        .filter(|(name, _)| is_admin_all_access_policy_name(name))
        .flat_map(|(_, body)| {
            body["policyItems"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|item| item["users"].as_array().into_iter().flatten())
                .filter_map(|u| u.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .collect();

    let mut changed = false;
    let mut present = 0usize;
    for (name, body) in &policies {
        crate::store::refuse_reserved_name(name)?;
        crate::store::refuse_non_infrastructure_name(&row_type, name)?;
        let exists = before.iter().any(|(n, _)| n == name);
        if users.is_empty() && !exists {
            continue;
        }
        present += 1;
        let wrote: Option<i64> = sqlx::query_scalar(
            "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, managed_by) \
             VALUES ($1, $2, $3, 'infra') \
             ON CONFLICT (service_id, name) DO UPDATE \
               SET policy_jsonb = EXCLUDED.policy_jsonb, \
                   managed_by = 'infra', \
                   version = policy.authz_policies.version + 1, \
                   updated_at = now() \
             WHERE policy.authz_policies.policy_jsonb IS DISTINCT FROM EXCLUDED.policy_jsonb \
                OR policy.authz_policies.managed_by <> 'infra' \
             RETURNING id",
        )
        .bind(service_id)
        .bind(name)
        .bind(body)
        .fetch_optional(&mut *conn)
        .await?;
        changed |= wrote.is_some();
    }
    // An `egret-admin - …` row the def no longer builds (an older def's
    // hierarchy, or one seeded from another engine type's def) would keep its
    // old names, ALL access included, forever: it is rewritten to name
    // `users` as well, its resources untouched.
    for (name, body) in &before {
        if !is_admin_all_access_policy_name(name) || policies.iter().any(|(n, _)| n == name) {
            continue;
        }
        present += 1;
        let mut rewritten = body.clone();
        if let Some(items) = rewritten["policyItems"].as_array_mut() {
            for item in items {
                item["users"] = Value::from(users.to_vec());
            }
        }
        let wrote: Option<i64> = sqlx::query_scalar(
            "UPDATE policy.authz_policies \
                SET policy_jsonb = $3, version = version + 1, updated_at = now() \
              WHERE service_id = $1 AND name = $2 AND policy_jsonb IS DISTINCT FROM $3 \
             RETURNING id",
        )
        .bind(service_id)
        .bind(name)
        .bind(&rewritten)
        .fetch_optional(&mut *conn)
        .await?;
        changed |= wrote.is_some();
    }
    if changed {
        sqlx::query(
            "UPDATE policy.authz_services SET policy_version = policy_version + 1, \
             updated_at = now() WHERE id = $1",
        )
        .bind(service_id)
        .execute(&mut *conn)
        .await?;
    }
    let now: std::collections::BTreeSet<String> = users.iter().cloned().collect();
    Ok(AdminPolicySync {
        changed,
        present,
        removed: named_before.difference(&now).cloned().collect(),
        added: now.difference(&named_before).cloned().collect(),
    })
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
