//! Editor form ↔ Ranger access-policy JSON. The React editor posts an
//! `AccessPolicyInput`; `access_to_ranger_json` renders it to the camelCase
//! Ranger policy body stored verbatim in `authz_policies.policy_jsonb`.
//! P2 covers policyType 0 (access) only; masking/row-filter come in P3.
//!
//! Called by the create/update handlers in `policies/mod.rs`.
//!
//! NOTE: this serializer is lossy for policies NOT authored by egret's editor.
//! It always emits `delegateAdmin: false`, `isAllowed: true` per access, and
//! `serviceType: "trino"`. Editing a foreign/Ranger-native policy through the
//! UI therefore drops those fields. P2 is trino-only + egret-authored by
//! design; revisit if importing external Ranger policies becomes a
//! requirement.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ResourceLevel {
    #[serde(default)]
    pub values: Vec<String>,
    #[serde(default, rename = "isExcludes")]
    pub is_excludes: bool,
    #[serde(default, rename = "isRecursive")]
    pub is_recursive: bool,
}

#[derive(Debug, Deserialize)]
pub struct AccessItemInput {
    #[serde(default)]
    pub users: Vec<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub roles: Vec<String>,
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
    pub resources: std::collections::BTreeMap<String, ResourceLevel>,
    #[serde(default)]
    pub items: Vec<AccessItemInput>,
    #[serde(default, rename = "allowExceptions")]
    pub allow_exceptions: Vec<AccessItemInput>,
    #[serde(default, rename = "denyExceptions")]
    pub deny_exceptions: Vec<AccessItemInput>,
    #[serde(default, rename = "isDenyAllElse")]
    pub is_deny_all_else: bool,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default, rename = "isOverride")]
    pub is_override: bool,
    /// Extra independent resource hierarchies (Ranger additionalResources).
    #[serde(default)]
    pub additional: Vec<std::collections::BTreeMap<String, ResourceLevel>>,
}

fn resource_obj(l: &ResourceLevel) -> Value {
    json!({ "values": l.values, "isExcludes": l.is_excludes, "isRecursive": l.is_recursive })
}

/// Build a Ranger `resources` object from a generic map, dropping empty levels.
fn resources_object(map: &std::collections::BTreeMap<String, ResourceLevel>) -> Value {
    let mut m = Map::new();
    for (name, level) in map {
        if !level.values.is_empty() {
            m.insert(name.clone(), resource_obj(level));
        }
    }
    Value::Object(m)
}

fn item_obj(it: &AccessItemInput) -> Value {
    let accesses: Vec<Value> = it
        .accesses
        .iter()
        .map(|a| json!({ "type": a, "isAllowed": true }))
        .collect();
    json!({
        "users": it.users, "groups": it.groups, "roles": it.roles,
        "accesses": accesses, "delegateAdmin": false,
    })
}

/// Render an `AccessPolicyInput` into a Ranger policyType-0 JSON body.
pub fn access_to_ranger_json(input: &AccessPolicyInput) -> Value {
    let resources = resources_object(&input.resources);
    let additional_resources: Vec<Value> = input
        .additional
        .iter()
        .map(resources_object)
        .filter(|v| v.as_object().map(|o| !o.is_empty()).unwrap_or(false))
        .collect();
    let (allow, deny): (Vec<_>, Vec<_>) = input.items.iter().partition(|it| it.effect != "deny");
    let policy_items: Vec<Value> = allow.iter().map(|it| item_obj(it)).collect();
    let deny_all = input.is_deny_all_else;
    let deny_items: Vec<Value> = if deny_all {
        vec![]
    } else {
        deny.iter().map(|it| item_obj(it)).collect()
    };
    let deny_exc: Vec<Value> = if deny_all {
        vec![]
    } else {
        input.deny_exceptions.iter().map(item_obj).collect()
    };
    let allow_exc: Vec<Value> = input.allow_exceptions.iter().map(item_obj).collect();
    json!({
        "name": input.name,
        "service": input.service,
        "serviceType": "trino",
        "policyType": 0,
        "isEnabled": input.is_enabled,
        "description": input.description,
        "policyLabels": input.labels,
        "resources": resources,
        "additionalResources": additional_resources,
        "policyItems": policy_items,
        "denyPolicyItems": deny_items,
        "allowExceptions": allow_exc,
        "denyExceptions": deny_exc,
        "isDenyAllElse": deny_all,
        "policyPriority": if input.is_override { 1 } else { 0 },
    })
}

#[derive(Debug, Deserialize, Serialize)]
pub struct MaskItemInput {
    #[serde(default)]
    pub users: Vec<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default, rename = "maskType")]
    pub mask_type: String,
    #[serde(default, rename = "valueExpr", skip_serializing_if = "Option::is_none")]
    pub value_expr: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct MaskingPolicyInput {
    #[serde(default = "kind_masking")]
    pub kind: String,
    pub name: String,
    pub service: String,
    #[serde(default = "default_true", rename = "isEnabled")]
    pub is_enabled: bool,
    #[serde(default)]
    pub resources: std::collections::BTreeMap<String, ResourceLevel>,
    #[serde(default)]
    pub items: Vec<MaskItemInput>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default, rename = "isOverride")]
    pub is_override: bool,
}

fn kind_masking() -> String {
    "masking".to_string()
}

fn mask_item_obj(it: &MaskItemInput) -> Value {
    let mut info = Map::new();
    info.insert("dataMaskType".into(), Value::from(it.mask_type.clone()));
    if it.mask_type == "CUSTOM" {
        if let Some(expr) = &it.value_expr {
            info.insert("valueExpr".into(), Value::from(expr.clone()));
        }
    }
    json!({
        "users": it.users, "groups": it.groups, "roles": [],
        "accesses": [{ "type": "select", "isAllowed": true }],
        "dataMaskInfo": Value::Object(info),
    })
}

/// Render a `MaskingPolicyInput` into a Ranger policyType-1 (masking) body.
pub fn masking_to_ranger_json(input: &MaskingPolicyInput) -> Value {
    json!({
        "name": input.name, "service": input.service, "serviceType": "trino",
        "policyType": 1, "isEnabled": input.is_enabled,
        "description": input.description, "policyLabels": input.labels,
        "resources": resources_object(&input.resources),
        "dataMaskPolicyItems": input.items.iter().map(mask_item_obj).collect::<Vec<_>>(),
        "policyItems": [], "denyPolicyItems": [],
        "policyPriority": if input.is_override { 1 } else { 0 },
    })
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RowFilterItemInput {
    #[serde(default)]
    pub users: Vec<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default, rename = "filterExpr")]
    pub filter_expr: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RowFilterPolicyInput {
    #[serde(default = "kind_rowfilter")]
    pub kind: String,
    pub name: String,
    pub service: String,
    #[serde(default = "default_true", rename = "isEnabled")]
    pub is_enabled: bool,
    #[serde(default)]
    pub resources: std::collections::BTreeMap<String, ResourceLevel>,
    #[serde(default)]
    pub items: Vec<RowFilterItemInput>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default, rename = "isOverride")]
    pub is_override: bool,
}

fn kind_rowfilter() -> String {
    "rowfilter".to_string()
}

fn rowfilter_item_obj(it: &RowFilterItemInput) -> Value {
    json!({
        "users": it.users, "groups": it.groups, "roles": [],
        "accesses": [{ "type": "select", "isAllowed": true }],
        "rowFilterInfo": { "filterExpr": it.filter_expr },
    })
}

/// Render a `RowFilterPolicyInput` into a Ranger policyType-2 (row-filter) body.
pub fn rowfilter_to_ranger_json(input: &RowFilterPolicyInput) -> Value {
    json!({
        "name": input.name, "service": input.service, "serviceType": "trino",
        "policyType": 2, "isEnabled": input.is_enabled,
        "description": input.description, "policyLabels": input.labels,
        "resources": resources_object(&input.resources),
        "rowFilterPolicyItems": input.items.iter().map(rowfilter_item_obj).collect::<Vec<_>>(),
        "policyItems": [], "denyPolicyItems": [],
        "policyPriority": if input.is_override { 1 } else { 0 },
    })
}

/// A serialized policy plus the metadata the handlers need to store it.
pub struct RangerPolicy {
    pub name: String,
    pub service: String,
    pub is_enabled: bool,
    pub body: Value,
}

/// Dispatch an editor form (`{kind, ...}`) to the right serializer. Missing
/// `kind` defaults to "access" so P2 clients stay wire-compatible.
pub fn editor_to_ranger_json(v: &Value) -> Result<RangerPolicy, String> {
    let kind = v.get("kind").and_then(Value::as_str).unwrap_or("access");
    match kind {
        "access" => {
            let inp: AccessPolicyInput =
                serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
            Ok(RangerPolicy {
                name: inp.name.clone(),
                service: inp.service.clone(),
                is_enabled: inp.is_enabled,
                body: access_to_ranger_json(&inp),
            })
        }
        "masking" => {
            let inp: MaskingPolicyInput =
                serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
            Ok(RangerPolicy {
                name: inp.name.clone(),
                service: inp.service.clone(),
                is_enabled: inp.is_enabled,
                body: masking_to_ranger_json(&inp),
            })
        }
        "rowfilter" => {
            let inp: RowFilterPolicyInput =
                serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
            Ok(RangerPolicy {
                name: inp.name.clone(),
                service: inp.service.clone(),
                is_enabled: inp.is_enabled,
                body: rowfilter_to_ranger_json(&inp),
            })
        }
        other => Err(format!("unknown policy kind: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::super::summarize_policy;
    use super::*;
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
        inp.resources.get_mut("column").unwrap().values.clear();
        let body = access_to_ranger_json(&inp);
        assert!(body["resources"].get("column").is_none());
        assert!(body["resources"].get("catalog").is_some());
    }

    fn masking_sample() -> MaskingPolicyInput {
        serde_json::from_value(json!({
            "kind": "masking", "name": "mask-email", "service": "trino", "isEnabled": true,
            "resources": {"catalog":{"values":["iceberg"]},"schema":{"values":["sales"]},
                          "table":{"values":["users"]},"column":{"values":["email"]}},
            "items": [{"users":["alice"],"groups":["pii"],"maskType":"MASK_HASH"}]
        }))
        .unwrap()
    }

    #[test]
    fn serializes_masking_policy() {
        let body = masking_to_ranger_json(&masking_sample());
        assert_eq!(body["policyType"], 1);
        assert_eq!(body["resources"]["column"]["values"][0], "email");
        let items = body["dataMaskPolicyItems"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["dataMaskInfo"]["dataMaskType"], "MASK_HASH");
        assert_eq!(items[0]["accesses"][0]["type"], "select");
        assert_eq!(items[0]["users"][0], "alice");
        let s = summarize_policy(1, "trino", true, &body);
        assert_eq!(s.policy_type, "masking");
        assert_eq!(s.resource_path, "iceberg.sales.users.email");
    }

    #[test]
    fn serializes_masking_custom_value_expr() {
        let mut inp = masking_sample();
        inp.items[0].mask_type = "CUSTOM".into();
        inp.items[0].value_expr = Some("substr({col},1,2)".into());
        let body = masking_to_ranger_json(&inp);
        assert_eq!(
            body["dataMaskPolicyItems"][0]["dataMaskInfo"]["valueExpr"],
            "substr({col},1,2)"
        );
    }

    fn rowfilter_sample() -> RowFilterPolicyInput {
        serde_json::from_value(json!({
            "kind": "rowfilter", "name": "us-only", "service": "trino", "isEnabled": true,
            "resources": {"catalog":{"values":["iceberg"]},"schema":{"values":["sales"]},"table":{"values":["orders"]}},
            "items": [{"groups":["analysts"],"filterExpr":"country = 'US'"}]
        }))
        .unwrap()
    }

    #[test]
    fn serializes_rowfilter_policy() {
        let body = rowfilter_to_ranger_json(&rowfilter_sample());
        assert_eq!(body["policyType"], 2);
        assert!(body["resources"].get("column").is_none());
        let items = body["rowFilterPolicyItems"].as_array().unwrap();
        assert_eq!(items[0]["rowFilterInfo"]["filterExpr"], "country = 'US'");
        assert_eq!(items[0]["groups"][0], "analysts");
        let s = summarize_policy(2, "trino", true, &body);
        assert_eq!(s.policy_type, "rowfilter");
        assert_eq!(s.resource_path, "iceberg.sales.orders");
    }

    #[test]
    fn access_serializes_exceptions_and_deny_all_else() {
        let inp: AccessPolicyInput = serde_json::from_value(json!({
            "kind":"access","name":"p","service":"trino","isEnabled":true,
            "description":"analysts read","labels":["pii","sales"],
            "resources":{"catalog":{"values":["iceberg"],"isExcludes":false,"isRecursive":false},
                         "schema":{"values":["sales"]},"table":{"values":["a","b"]},"column":{"values":["*"]}},
            "items":[{"users":["alice"],"accesses":["select"],"effect":"allow"}],
            "allowExceptions":[{"groups":["contractors"],"accesses":["select"]}],
            "isDenyAllElse":true,
            "denyExceptions":[{"users":["x"],"accesses":["select"]}]
        })).unwrap();
        let b = access_to_ranger_json(&inp);
        assert_eq!(b["policyType"], 0);
        assert_eq!(b["description"], "analysts read");
        assert_eq!(b["policyLabels"][0], "pii");
        assert_eq!(
            b["resources"]["table"]["values"].as_array().unwrap().len(),
            2
        );
        assert_eq!(b["allowExceptions"][0]["groups"][0], "contractors");
        assert_eq!(b["isDenyAllElse"], true);
        assert!(b["denyPolicyItems"].as_array().unwrap().is_empty());
        assert!(b["denyExceptions"].as_array().unwrap().is_empty());
    }

    #[test]
    fn summary_unions_deny_and_exception_principals() {
        let body = json!({
            "name":"deny-only","policyType":0,
            "resources":{"catalog":{"values":["iceberg"]}},
            "policyItems":[],
            "denyPolicyItems":[{"groups":["contractors"],"accesses":[{"type":"select"}]}],
            "allowExceptions":[{"users":["alice"],"accesses":[{"type":"insert"}]}],
            "description":"d","policyLabels":["l1"]
        });
        let s = summarize_policy(1, "trino", true, &body);
        assert!(s.principals.contains(&"contractors".to_string()));
        assert!(s.principals.contains(&"alice".to_string()));
        assert!(s.permissions.contains(&"select".to_string()));
        assert!(s.permissions.contains(&"insert".to_string()));
        assert_eq!(s.description, "d");
        assert_eq!(s.labels, vec!["l1".to_string()]);
    }

    #[test]
    fn serializes_override_priority() {
        let mut inp = sample();
        inp.is_override = true;
        let body = access_to_ranger_json(&inp);
        assert_eq!(body["policyPriority"], 1);

        let default_body = access_to_ranger_json(&sample());
        assert_eq!(default_body["policyPriority"], 0);
    }

    #[test]
    fn serializes_impersonation_as_generic_resource() {
        let inp: AccessPolicyInput = serde_json::from_value(json!({
            "name": "allow-alice-impersonate-bob",
            "service": "trino",
            "isEnabled": true,
            "resources": {"trinouser": {"values": ["bob"]}},
            "items": [
                {"users": ["alice"], "accesses": ["impersonate"], "effect": "allow"}
            ]
        }))
        .unwrap();
        let body = access_to_ranger_json(&inp);
        assert_eq!(body["policyType"], 0);
        assert_eq!(body["resources"]["trinouser"]["values"][0], "bob");
        assert!(body["resources"].get("catalog").is_none());
        assert_eq!(body["policyItems"][0]["accesses"][0]["type"], "impersonate");
    }

    #[test]
    fn serializes_arbitrary_non_table_resource_type() {
        // Any resource-type name works generically — no special-casing needed.
        let inp: AccessPolicyInput = serde_json::from_value(json!({
            "name": "allow-role-admin",
            "service": "trino",
            "isEnabled": true,
            "resources": {"role": {"values": ["*"]}},
            "items": [
                {"users": ["alice"], "accesses": ["alter"], "effect": "allow"}
            ]
        }))
        .unwrap();
        let body = access_to_ranger_json(&inp);
        assert_eq!(body["resources"]["role"]["values"][0], "*");
        assert!(body["resources"].get("catalog").is_none());
        assert!(body["resources"].get("schema").is_none());
        assert!(body["resources"].get("table").is_none());
        assert!(body["resources"].get("column").is_none());
    }

    #[test]
    fn table_resources_still_serialize_when_no_other_type_present() {
        let body = access_to_ranger_json(&sample());
        assert!(body["resources"].get("catalog").is_some());
        assert!(body["resources"].get("trinouser").is_none());
    }

    #[test]
    fn serializes_additional_resources_and_roles() {
        let inp: AccessPolicyInput = serde_json::from_value(json!({
            "kind":"access","name":"p","service":"trino","isEnabled":true,
            "resources":{"catalog":{"values":["iceberg"]},"schema":{"values":["sales"]}},
            "additional":[{"catalog":{"values":["hive"]},"schema":{"values":["reports"]},"table":{"values":["daily"]}}],
            "items":[{"users":["alice"],"groups":["g1"],"roles":["analyst_role"],"accesses":["select"],"effect":"allow"}]
        })).unwrap();
        let b = access_to_ranger_json(&inp);
        assert_eq!(b["resources"]["catalog"]["values"][0], "iceberg");
        let add = b["additionalResources"].as_array().unwrap();
        assert_eq!(add.len(), 1);
        assert_eq!(add[0]["catalog"]["values"][0], "hive");
        assert_eq!(add[0]["table"]["values"][0], "daily");
        assert_eq!(b["policyItems"][0]["roles"][0], "analyst_role");
    }

    #[test]
    fn dispatcher_routes_by_kind() {
        let acc = editor_to_ranger_json(&json!({"kind":"access","name":"a","service":"trino",
            "resources":{"catalog":{"values":["c"]}},"items":[]}))
        .unwrap();
        assert_eq!(acc.body["policyType"], 0);
        assert_eq!(acc.name, "a");
        let m = editor_to_ranger_json(&serde_json::to_value(masking_sample()).unwrap()).unwrap();
        assert_eq!(m.body["policyType"], 1);
        let d =
            editor_to_ranger_json(&json!({"name":"x","service":"trino","resources":{},"items":[]}))
                .unwrap();
        assert_eq!(d.body["policyType"], 0);
    }
}
