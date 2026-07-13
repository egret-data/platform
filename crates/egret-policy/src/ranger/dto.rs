//! Ranger wire DTOs. The `ServicePolicies` / `ServiceTags` download envelopes
//! now live in the `egret-policy-api` contract crate (E2a); re-exported here so
//! the router/handlers are unchanged. Only the envelope is typed; policies and
//! the service-def pass through as `serde_json::Value` (stored verbatim as jsonb).
pub use egret_policy_api::{ServicePolicies, ServiceTags, TagAssignment};

#[cfg(test)]
mod tests {
    use serde_json::Value;

    #[test]
    fn trino_servicedef_fixture_is_valid_json_with_resources() {
        let raw = include_str!("../../tests/fixtures/trino-servicedef.json");
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
