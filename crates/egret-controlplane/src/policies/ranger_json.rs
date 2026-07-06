//! Editor form ↔ Ranger access-policy JSON. The React editor posts an
//! `AccessPolicyInput`; `access_to_ranger_json` renders it to the camelCase
//! Ranger policy body stored verbatim in `authz_policies.policy_jsonb`.
//! P2 covers policyType 0 (access) only; masking/row-filter come in P3.
//!
//! Only unit-tested for now — the create/update endpoints that call
//! `access_to_ranger_json` land in P2 Task 2, so `cargo clippy` would
//! otherwise flag this module as dead code until then.
#![allow(dead_code)]

use serde::Deserialize;
use serde_json::{json, Map, Value};

#[derive(Debug, Default, Deserialize)]
pub struct ResourceLevel {
    #[serde(default)]
    pub values: Vec<String>,
    #[serde(default, rename = "isExcludes")]
    pub is_excludes: bool,
    #[serde(default, rename = "isRecursive")]
    pub is_recursive: bool,
}

#[derive(Debug, Default, Deserialize)]
pub struct EditorResources {
    #[serde(default)]
    pub catalog: ResourceLevel,
    #[serde(default)]
    pub schema: ResourceLevel,
    #[serde(default)]
    pub table: ResourceLevel,
    #[serde(default)]
    pub column: ResourceLevel,
}

#[derive(Debug, Deserialize)]
pub struct AccessItemInput {
    #[serde(default)]
    pub users: Vec<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub accesses: Vec<String>,
    #[serde(default = "default_effect")]
    pub effect: String,
}

fn default_effect() -> String {
    "allow".to_string()
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct AccessPolicyInput {
    pub name: String,
    pub service: String,
    #[serde(default = "default_true", rename = "isEnabled")]
    pub is_enabled: bool,
    #[serde(default)]
    pub resources: EditorResources,
    #[serde(default)]
    pub items: Vec<AccessItemInput>,
}

fn resource_obj(l: &ResourceLevel) -> Value {
    json!({ "values": l.values, "isExcludes": l.is_excludes, "isRecursive": l.is_recursive })
}

fn item_obj(it: &AccessItemInput) -> Value {
    let accesses: Vec<Value> = it
        .accesses
        .iter()
        .map(|a| json!({ "type": a, "isAllowed": true }))
        .collect();
    json!({
        "users": it.users, "groups": it.groups, "roles": [],
        "accesses": accesses, "delegateAdmin": false,
    })
}

/// Render an `AccessPolicyInput` into a Ranger policyType-0 JSON body.
pub fn access_to_ranger_json(input: &AccessPolicyInput) -> Value {
    let mut resources = Map::new();
    for (key, level) in [
        ("catalog", &input.resources.catalog),
        ("schema", &input.resources.schema),
        ("table", &input.resources.table),
        ("column", &input.resources.column),
    ] {
        if !level.values.is_empty() {
            resources.insert(key.to_string(), resource_obj(level));
        }
    }
    let (allow, deny): (Vec<_>, Vec<_>) = input.items.iter().partition(|it| it.effect != "deny");
    let policy_items: Vec<Value> = allow.iter().map(|it| item_obj(it)).collect();
    let deny_items: Vec<Value> = deny.iter().map(|it| item_obj(it)).collect();
    json!({
        "name": input.name,
        "service": input.service,
        "serviceType": "trino",
        "policyType": 0,
        "isEnabled": input.is_enabled,
        "resources": Value::Object(resources),
        "policyItems": policy_items,
        "denyPolicyItems": deny_items,
        "allowExceptions": [],
        "denyExceptions": [],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summarize_policy;
    use serde_json::json;

    fn sample() -> AccessPolicyInput {
        serde_json::from_value(json!({
            "name": "allow-analysts-select",
            "service": "trino",
            "isEnabled": true,
            "resources": {
                "catalog": {"values": ["iceberg"]},
                "schema":  {"values": ["sales"]},
                "table":   {"values": ["orders"]},
                "column":  {"values": ["*"]}
            },
            "items": [
                {"users": ["alice"], "groups": ["analysts"], "accesses": ["select"], "effect": "allow"},
                {"groups": ["contractors"], "accesses": ["select"], "effect": "deny"}
            ]
        })).unwrap()
    }

    #[test]
    fn serializes_access_policy_to_ranger_json() {
        let body = access_to_ranger_json(&sample());
        assert_eq!(body["policyType"], 0);
        assert_eq!(body["name"], "allow-analysts-select");
        assert_eq!(body["isEnabled"], true);
        assert_eq!(body["resources"]["catalog"]["values"][0], "iceberg");
        assert_eq!(body["resources"]["column"]["values"][0], "*");
        assert_eq!(body["resources"]["catalog"]["isExcludes"], false);
        assert_eq!(body["policyItems"].as_array().unwrap().len(), 1);
        assert_eq!(body["denyPolicyItems"].as_array().unwrap().len(), 1);
        assert_eq!(body["policyItems"][0]["users"][0], "alice");
        assert_eq!(body["policyItems"][0]["accesses"][0]["type"], "select");
        assert_eq!(body["policyItems"][0]["accesses"][0]["isAllowed"], true);
    }

    #[test]
    fn round_trips_through_summary() {
        let body = access_to_ranger_json(&sample());
        let s = summarize_policy(7, "trino", true, &body);
        assert_eq!(s.policy_type, "access");
        assert_eq!(s.resource_path, "iceberg.sales.orders.*");
        assert!(s.principals.contains(&"alice".to_string()));
        assert!(s.principals.contains(&"analysts".to_string()));
        assert_eq!(s.permissions, vec!["select".to_string()]);
    }

    #[test]
    fn omits_empty_resource_levels() {
        let mut inp = sample();
        inp.resources.column.values.clear();
        let body = access_to_ranger_json(&inp);
        assert!(body["resources"].get("column").is_none());
        assert!(body["resources"].get("catalog").is_some());
    }
}
