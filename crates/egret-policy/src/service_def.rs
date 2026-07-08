use crate::{PolicyStore, PolicyStoreError};
use serde_json::Value;

/// The official Trino Ranger service-def, embedded at compile time.
pub const TRINO_SERVICE_DEF_JSON: &str = include_str!("service-defs/trino.json");
/// Service-def name as declared in the embedded JSON.
pub const TRINO_DEF_NAME: &str = "trino";

/// Parse the embedded Trino service-def.
pub fn trino_service_def() -> Result<Value, PolicyStoreError> {
    Ok(serde_json::from_str(TRINO_SERVICE_DEF_JSON)?)
}

/// Idempotently seed the Trino service-def into the store. Call at startup when ranger is enabled.
pub async fn seed_trino(store: &PolicyStore) -> Result<(), PolicyStoreError> {
    let def = trino_service_def()?;
    let id = def["id"]
        .as_i64()
        .ok_or_else(|| PolicyStoreError::BadRequest("trino def missing numeric id".into()))?;
    let name = def["name"].as_str().unwrap_or(TRINO_DEF_NAME);
    store.upsert_service_def(id, name, &def).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn embedded_trino_def_parses_with_id_and_resources() {
        let def = trino_service_def().unwrap();
        assert_eq!(def["name"], "trino");
        assert!(def["id"].as_i64().is_some());
        let access: Vec<&str> = def["accessTypes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|a| a["name"].as_str())
            .collect();
        assert!(access.contains(&"select"));
        assert!(def["dataMaskDef"].is_object());
        assert!(def["rowFilterDef"].is_object());
    }
}
