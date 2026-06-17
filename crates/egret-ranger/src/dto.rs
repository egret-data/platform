//! Ranger wire DTOs. Only the ServicePolicies envelope is typed; policies and the
//! service-def pass through as `serde_json::Value` (stored verbatim as jsonb).
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The download payload the Trino Ranger plugin consumes (`@JsonInclude(NON_EMPTY)`, camelCase).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServicePolicies {
    pub service_name: String,
    pub service_id: i64,
    pub policy_version: i64,
    /// epoch millis
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_update_time: Option<i64>,
    pub service_def: Value,
    pub policies: Vec<Value>,
    #[serde(default = "default_audit_mode")]
    pub audit_mode: String,
}

fn default_audit_mode() -> String {
    "DEFAULT".to_string()
}

impl ServicePolicies {
    pub fn new(
        service_name: String,
        service_id: i64,
        policy_version: i64,
        service_def: Value,
        policies: Vec<Value>,
        updated_ms: Option<i64>,
    ) -> Self {
        Self {
            service_name,
            service_id,
            policy_version,
            policy_update_time: updated_ms,
            service_def,
            policies,
            audit_mode: default_audit_mode(),
        }
    }
}

/// Empty, well-formed tag payload so a polling plugin doesn't error (tag policies deferred).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceTags {
    pub op: String,
    pub service_name: String,
    pub tag_version: i64,
    pub tags: serde_json::Map<String, Value>,
    pub service_resources: Vec<Value>,
}

impl ServiceTags {
    pub fn empty(service_name: String) -> Self {
        Self {
            op: "add".into(),
            service_name,
            tag_version: 1,
            tags: serde_json::Map::new(),
            service_resources: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_policies_serializes_camel_case() {
        let sp = ServicePolicies::new(
            "trino_prod".into(),
            1,
            5,
            serde_json::json!({"name":"trino"}),
            vec![],
            Some(0),
        );
        let j = serde_json::to_value(&sp).unwrap();
        assert_eq!(j["serviceName"], "trino_prod");
        assert_eq!(j["policyVersion"], 5);
        assert_eq!(j["serviceDef"]["name"], "trino");
        assert_eq!(j["auditMode"], "DEFAULT");
    }

    #[test]
    fn trino_servicedef_fixture_is_valid_json_with_resources() {
        let raw = include_str!("../tests/fixtures/trino-servicedef.json");
        let def: Value = serde_json::from_str(raw).expect("trino def parses");
        assert_eq!(def["name"], "trino");
        let resources = def["resources"].as_array().unwrap();
        let names: Vec<&str> = resources
            .iter()
            .filter_map(|r| r["name"].as_str())
            .collect();
        for r in ["catalog", "schema", "table", "column"] {
            assert!(names.contains(&r), "missing {r}");
        }
    }
}
