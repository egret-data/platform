//! Policy/Ranger wire DTOs. The Ranger download envelopes (`ServicePolicies`,
//! `ServiceTags`) are the engine-facing contract; policies and the service-def
//! pass through as `serde_json::Value` (stored verbatim as jsonb). `PolicyService`
//! is the `PolicyPort` boundary projection of a policy profile.
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A policy profile (an `authz_services` row) as seen across the seam boundary.
/// Minimal projection — cross-seam callers (engine-mgmt auto-bind) key off `id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyService {
    pub id: i64,
    pub name: String,
    pub r#type: String,
}

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
    pub tags: serde_json::Map<String, Value>, // keyed by tag-id (Long-as-string per Ranger wire format); empty this phase
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
        assert_eq!(j["policyUpdateTime"], 0);
    }

    #[test]
    fn policy_update_time_omitted_when_none() {
        let sp = ServicePolicies::new("s".into(), 1, 1, serde_json::json!({}), vec![], None);
        let j = serde_json::to_value(&sp).unwrap();
        assert!(
            j.get("policyUpdateTime").is_none(),
            "policyUpdateTime must be absent when None, got {j}"
        );
    }

    #[test]
    fn audit_mode_defaults_on_deserialize() {
        let json =
            r#"{"serviceName":"s","serviceId":1,"policyVersion":2,"serviceDef":{},"policies":[]}"#;
        let sp: ServicePolicies = serde_json::from_str(json).unwrap();
        assert_eq!(sp.audit_mode, "DEFAULT");
    }

    #[test]
    fn service_tags_empty_serializes_camel_case() {
        let t = ServiceTags::empty("trino_prod".into());
        let j = serde_json::to_value(&t).unwrap();
        assert_eq!(j["op"], "add");
        assert_eq!(j["serviceName"], "trino_prod");
        assert_eq!(j["tagVersion"], 1);
        assert!(j["tags"].is_object());
        assert!(j["serviceResources"].is_array());
    }

    #[test]
    fn policy_service_is_plain_json() {
        let s = PolicyService {
            id: 7,
            name: "trino_prod".into(),
            r#type: "trino".into(),
        };
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["id"], 7);
        assert_eq!(j["name"], "trino_prod");
        assert_eq!(j["type"], "trino");
    }
}
