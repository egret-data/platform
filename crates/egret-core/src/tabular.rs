//! Unified in-memory tabular query result + column metadata.
//!
//! A single shared representation of "columns + rows" for the query path,
//! so the engine and BI crates describe results with one type instead of
//! several parallel structs. The serialized shape is deliberately identical
//! to the historical query-engine `{Column, QueryResult}` structs so callers
//! that persist or forward these values see no wire change.

/// One column's name + engine-reported type name.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, utoipa::ToSchema)]
pub struct ColumnMeta {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

/// A fully-collected, in-memory query result (bounded by the caller's row limit).
///
/// `Serialize`/`Deserialize` are for the E4b contract crate + BI forwarding this
/// result directly across the seam boundary; today the store persists `columns`
/// and `rows` individually, so the whole-struct codecs are not yet exercised.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct TabularResult {
    pub columns: Vec<ColumnMeta>,
    /// Row-major cell values; each inner vec is one row aligned to `columns`.
    #[schema(value_type = Vec<Vec<Object>>)]
    pub rows: Vec<Vec<serde_json::Value>>,
    pub row_count: u64,
    pub bytes_scanned: Option<u64>,
    pub truncated: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn column_serializes_type_key() {
        let c = ColumnMeta {
            name: "n".into(),
            ty: "bigint".into(),
        };
        assert_eq!(
            serde_json::to_value(&c).unwrap(),
            json!({"name":"n","type":"bigint"})
        );
    }
}
