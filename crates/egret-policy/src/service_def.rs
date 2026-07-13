use crate::{PolicyStore, PolicyStoreError};
use serde_json::Value;

/// The official Trino Ranger service-def, embedded at compile time.
pub const TRINO_SERVICE_DEF_JSON: &str = include_str!("service-defs/trino.json");
/// Service-def name as declared in the embedded JSON.
pub const TRINO_DEF_NAME: &str = "trino";

/// The Ranger **tag** service-def (Access P3.2), embedded at compile time. It
/// lets tag-based masking/row-filter/access policies exist against classification
/// tags served by `download_tags`.
///
/// LIVE-ONLY: this JSON's EXACT field shape (resource matcher, accessTypes,
/// dataMaskDef/rowFilterDef, `implClass`) is validated against the real Ranger
/// tag plugin at runtime, NOT in CI. CI only asserts it parses + is seeded. If
/// the tag plugin rejects the def or the mask/row-filter surface, THIS FILE is
/// the tuning spot — it must stay valid Ranger tag service-def JSON.
pub const TAG_SERVICE_DEF_JSON: &str = include_str!("service-defs/tag.json");
/// Service-def name as declared in the embedded tag JSON.
pub const TAG_DEF_NAME: &str = "tag";

/// Parse the embedded Trino service-def.
pub fn trino_service_def() -> Result<Value, PolicyStoreError> {
    Ok(serde_json::from_str(TRINO_SERVICE_DEF_JSON)?)
}

/// Parse the embedded tag service-def.
pub fn tag_service_def() -> Result<Value, PolicyStoreError> {
    Ok(serde_json::from_str(TAG_SERVICE_DEF_JSON)?)
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

/// Idempotently seed the tag service-def into the store. Call at startup when
/// ranger is enabled (alongside `seed_trino`). LIVE-ONLY correctness — see
/// [`TAG_SERVICE_DEF_JSON`].
pub async fn seed_tag(store: &PolicyStore) -> Result<(), PolicyStoreError> {
    let def = tag_service_def()?;
    let id = def["id"]
        .as_i64()
        .ok_or_else(|| PolicyStoreError::BadRequest("tag def missing numeric id".into()))?;
    let name = def["name"].as_str().unwrap_or(TAG_DEF_NAME);
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

    #[test]
    fn embedded_tag_def_parses_with_id_and_mask_surface() {
        let def = tag_service_def().unwrap();
        assert_eq!(def["name"], "tag");
        // Stable id, sequenced after trino's (203).
        assert_eq!(def["id"].as_i64(), Some(204));
        // Single `tag` resource, the tag-policy target.
        let res = def["resources"].as_array().unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0]["name"], "tag");
        // Access types mirror trino's so tag access policies grant the same actions.
        let access: Vec<&str> = def["accessTypes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|a| a["name"].as_str())
            .collect();
        assert!(access.contains(&"select"));
        assert!(access.contains(&"all"));
        // Masking + row-filter surface present (tag-based masking is the point).
        assert!(def["dataMaskDef"]["maskTypes"].is_array());
        assert!(def["rowFilterDef"].is_object());
    }
}
