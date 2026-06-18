//! SQL Lab DTOs.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct SaveQuery {
    pub database_id: Option<Uuid>,
    pub name: String,
    pub sql: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Serialize)]
pub struct SavedQueryDto {
    pub id: String,
    pub database_id: Option<String>,
    pub name: String,
    pub sql: String,
    pub description: String,
    pub created_by: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// A single column in a DESCRIBE result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaColumn {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schema_node_serializes() {
        let c = SchemaColumn {
            name: "id".into(),
            ty: "bigint".into(),
        };
        assert_eq!(
            serde_json::to_value(&c).unwrap(),
            serde_json::json!({"name":"id","type":"bigint"})
        );
    }
}
