//! Policy/Ranger wire DTOs. The Ranger download envelopes (`ServicePolicies`,
//! `ServiceTags`) are the engine-facing contract; policies and the service-def
//! pass through as `serde_json::Value` (stored verbatim as jsonb). `PolicyService`
//! is the `PolicyPort` boundary projection of a policy profile.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;

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

/// One catalog resource → tag pairing feeding the Ranger tag download.
///
/// A catalog resource is identified by `catalog.schema.table[.column]`; an empty
/// `column` means the tag is table-level. `tag_id` is the tag's stable identity
/// (used only to de-duplicate distinct tags — the numeric wire tag-ids Ranger
/// keys on are assigned by [`ServiceTags::from_assignments`]); `tag_name` is the
/// classification name the tag-def (and tag-based policies) reference.
#[derive(Debug, Clone)]
pub struct TagAssignment {
    pub catalog: String,
    pub schema: String,
    pub table: String,
    pub column: String,
    pub tag_id: String,
    pub tag_name: String,
}

impl TagAssignment {
    pub fn new(
        catalog: impl Into<String>,
        schema: impl Into<String>,
        table: impl Into<String>,
        column: impl Into<String>,
        tag_id: impl Into<String>,
        tag_name: impl Into<String>,
    ) -> Self {
        Self {
            catalog: catalog.into(),
            schema: schema.into(),
            table: table.into(),
            column: column.into(),
            tag_id: tag_id.into(),
            tag_name: tag_name.into(),
        }
    }
}

/// The tag download payload the Trino Ranger plugin consumes (camelCase). Built
/// empty (deferred) via [`ServiceTags::empty`] or from real catalog-tag
/// assignments via [`ServiceTags::from_assignments`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceTags {
    pub op: String,
    pub service_name: String,
    pub tag_version: i64,
    /// Keyed by numeric tag-id (Long-as-string per Ranger wire format) → tag-def instance.
    pub tags: serde_json::Map<String, Value>,
    /// Each entry: `{ id, serviceResource: { resourceElements: { catalog, schema, table, column } } }`.
    pub service_resources: Vec<Value>,
    /// `{ "<resourceId>": [<tagId>, ...] }` — links each resource to its tags.
    pub resource_to_tag_ids: serde_json::Map<String, Value>,
}

impl ServiceTags {
    pub fn empty(service_name: String) -> Self {
        Self {
            op: "add".into(),
            service_name,
            tag_version: 1,
            tags: serde_json::Map::new(),
            service_resources: vec![],
            resource_to_tag_ids: serde_json::Map::new(),
        }
    }

    /// Build a Ranger `ServiceTags` from catalog-tag assignments. Distinct tags
    /// (by `tag_id`) and distinct resources (by `catalog.schema.table.column`)
    /// are each assigned a stable 1-based numeric wire id in first-seen order;
    /// `tags` is keyed by tag-id, `serviceResources` carries the resource
    /// elements, and `resourceToTagIds` links resource-id → the tag-ids on it.
    /// An empty `column`/`schema`/`table` level is omitted from `resourceElements`.
    pub fn from_assignments(
        service_name: String,
        tag_version: i64,
        assignments: Vec<TagAssignment>,
    ) -> Self {
        // Distinct tags in first-seen order → 1-based numeric wire id.
        let mut tag_num: HashMap<String, i64> = HashMap::new();
        let mut tag_name_of: HashMap<i64, String> = HashMap::new();
        // Distinct resources in first-seen order → 1-based numeric wire id.
        type ResKey = (String, String, String, String);
        let mut res_num: HashMap<ResKey, i64> = HashMap::new();
        let mut res_order: Vec<ResKey> = Vec::new();
        // resource wire-id → its tag wire-ids (de-duplicated, in first-seen order).
        let mut res_tags: HashMap<i64, Vec<i64>> = HashMap::new();

        for a in &assignments {
            let next_tag = (tag_num.len() as i64) + 1;
            let tnum = *tag_num.entry(a.tag_id.clone()).or_insert(next_tag);
            tag_name_of
                .entry(tnum)
                .or_insert_with(|| a.tag_name.clone());

            let key: ResKey = (
                a.catalog.clone(),
                a.schema.clone(),
                a.table.clone(),
                a.column.clone(),
            );
            let next_res = (res_num.len() as i64) + 1;
            let rnum = *res_num.entry(key.clone()).or_insert_with(|| {
                res_order.push(key);
                next_res
            });

            let tids = res_tags.entry(rnum).or_default();
            if !tids.contains(&tnum) {
                tids.push(tnum);
            }
        }

        // tags map: numeric-id (as string) → tag-def instance.
        let mut tags = serde_json::Map::new();
        for (tnum, name) in tag_name_of.iter() {
            tags.insert(tnum.to_string(), json!({ "type": name, "owner": 0 }));
        }

        // serviceResources + resourceToTagIds, in resource first-seen order.
        let mut service_resources = Vec::with_capacity(res_order.len());
        let mut resource_to_tag_ids = serde_json::Map::new();
        for (i, (catalog, schema, table, column)) in res_order.iter().enumerate() {
            let rnum = (i as i64) + 1;
            let mut elements = serde_json::Map::new();
            elements.insert("catalog".into(), json!({ "values": [catalog] }));
            if !schema.is_empty() {
                elements.insert("schema".into(), json!({ "values": [schema] }));
            }
            if !table.is_empty() {
                elements.insert("table".into(), json!({ "values": [table] }));
            }
            if !column.is_empty() {
                elements.insert("column".into(), json!({ "values": [column] }));
            }
            service_resources.push(json!({
                "id": rnum,
                "serviceResource": { "resourceElements": elements },
            }));
            if let Some(tids) = res_tags.get(&rnum) {
                resource_to_tag_ids.insert(rnum.to_string(), json!(tids));
            }
        }

        Self {
            op: "add".into(),
            service_name,
            tag_version,
            tags,
            service_resources,
            resource_to_tag_ids,
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
    fn service_tags_from_assignments_builds_ranger_shape() {
        // One table-level tag (PII) and one column-level pair (PII + EMAIL on the
        // same column) — exercises distinct-tag numbering, distinct-resource
        // numbering, the '' column → table-level omission, and multi-tag linkage.
        let assignments = vec![
            TagAssignment::new("wh1", "sales", "orders", "", "tag-uuid-a", "PII"),
            TagAssignment::new("wh1", "sales", "orders", "email", "tag-uuid-a", "PII"),
            TagAssignment::new("wh1", "sales", "orders", "email", "tag-uuid-b", "EMAIL"),
        ];
        let t = ServiceTags::from_assignments("trino_prod".into(), 5, assignments);
        let j = serde_json::to_value(&t).unwrap();

        assert_eq!(j["op"], "add");
        assert_eq!(j["serviceName"], "trino_prod");
        assert_eq!(j["tagVersion"], 5);

        // Two distinct tags, keyed by 1-based numeric wire id (first-seen order).
        let tags = j["tags"].as_object().unwrap();
        assert_eq!(tags.len(), 2, "two distinct tags, got {tags:?}");
        assert_eq!(j["tags"]["1"]["type"], "PII");
        assert_eq!(j["tags"]["1"]["owner"], 0);
        assert_eq!(j["tags"]["2"]["type"], "EMAIL");

        // Two distinct resources: table-level then column-level.
        let srs = j["serviceResources"].as_array().unwrap();
        assert_eq!(srs.len(), 2, "two distinct resources, got {srs:?}");
        let els0 = &srs[0]["serviceResource"]["resourceElements"];
        assert_eq!(srs[0]["id"], 1);
        assert_eq!(els0["catalog"]["values"][0], "wh1");
        assert_eq!(els0["schema"]["values"][0], "sales");
        assert_eq!(els0["table"]["values"][0], "orders");
        assert!(
            els0.get("column").is_none(),
            "table-level ('' column) must omit the column element, got {els0}"
        );
        let els1 = &srs[1]["serviceResource"]["resourceElements"];
        assert_eq!(srs[1]["id"], 2);
        assert_eq!(els1["column"]["values"][0], "email");

        // resourceToTagIds linkage: table-level resource carries PII only; the
        // column-level resource carries both PII and EMAIL.
        assert_eq!(j["resourceToTagIds"]["1"], json!([1]));
        assert_eq!(j["resourceToTagIds"]["2"], json!([1, 2]));
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
