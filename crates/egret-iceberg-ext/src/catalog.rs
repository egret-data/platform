use iceberg::spec::{Schema, SortOrder, TableMetadataRef, UnboundPartitionSpec};
use iceberg::{NamespaceIdent, TableIdent, TableRequirement, TableUpdate};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Default, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CatalogConfig {
    pub overrides: HashMap<String, String>,
    pub defaults: HashMap<String, String>,
    pub endpoints: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateNamespaceRequest {
    pub namespace: NamespaceIdent,
    pub properties: Option<HashMap<String, String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateNamespaceResponse {
    pub namespace: NamespaceIdent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<HashMap<String, String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct GetNamespaceResponse {
    pub namespace: NamespaceIdent,
    pub properties: Option<HashMap<String, String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ListNamespacesResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_page_token: Option<String>,
    pub namespaces: Vec<NamespaceIdent>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct UpdateNamespacePropertiesRequest {
    pub removals: Option<Vec<String>>,
    pub updates: Option<HashMap<String, String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct UpdateNamespacePropertiesResponse {
    pub updated: Vec<String>,
    pub removed: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missing: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CreateTableRequest {
    pub name: String,
    pub location: Option<String>,
    pub schema: Schema,
    pub partition_spec: Option<UnboundPartitionSpec>,
    pub write_order: Option<SortOrder>,
    pub stage_create: Option<bool>,
    pub properties: Option<HashMap<String, String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct LoadTableResult {
    pub metadata_location: Option<String>,
    pub metadata: TableMetadataRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<HashMap<String, String>>,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CommitTableRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identifier: Option<TableIdent>,
    pub requirements: Vec<TableRequirement>,
    pub updates: Vec<TableUpdate>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CommitTableResponse {
    pub metadata_location: String,
    pub metadata: TableMetadataRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<HashMap<String, String>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ListTablesResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_page_token: Option<String>,
    pub identifiers: Vec<TableIdent>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct RegisterTableRequest {
    pub name: String,
    pub metadata_location: String,
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct RenameTableRequest {
    pub source: TableIdent,
    pub destination: TableIdent,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_response_uses_kebab_case() {
        let c = CatalogConfig {
            overrides: std::collections::HashMap::from([("warehouse".into(), "s3://wh".into())]),
            defaults: Default::default(),
            endpoints: vec![],
        };
        let j = serde_json::to_value(&c).unwrap();
        assert_eq!(j["overrides"]["warehouse"], "s3://wh");
    }

    #[test]
    fn commit_request_deserializes_requirements_and_updates() {
        // Minimal commit: assert-create requirement + set-properties update.
        // serde tags verified against the pinned iceberg rev (bb70173):
        //   TableRequirement uses #[serde(tag = "type")], variant "assert-create"
        //   TableUpdate uses #[serde(tag = "action")], variant "set-properties"
        let body = serde_json::json!({
            "requirements": [{ "type": "assert-create" }],
            "updates": [{ "action": "set-properties", "updates": { "owner": "egret" } }]
        });
        let req: CommitTableRequest = serde_json::from_value(body).unwrap();
        assert_eq!(req.requirements.len(), 1);
        assert_eq!(req.updates.len(), 1);
    }
}
