use crate::CatalogError;
use egret_iceberg_ext::catalog::CreateTableRequest;
use iceberg::spec::{
    FormatVersion, SortOrder, TableMetadata, TableMetadataBuilder, UnboundPartitionSpec,
};
use uuid::Uuid;

/// Build the initial TableMetadata for a CREATE TABLE request (format v2, assigned uuid + location).
pub fn create_table_metadata(
    req: &CreateTableRequest,
    table_uuid: Uuid,
    location: String,
) -> Result<TableMetadata, CatalogError> {
    let partition_spec = req
        .partition_spec
        .clone()
        .unwrap_or_else(|| UnboundPartitionSpec::builder().build());
    let sort_order = req
        .write_order
        .clone()
        .unwrap_or_else(SortOrder::unsorted_order);
    // Iceberg engines (e.g. Trino) include reserved properties like `format-version` in the
    // create request. The TableMetadataBuilder rejects them ("Table properties should not
    // contain reserved properties"); they control build behavior and are not persisted, so
    // strip them here. We always build format v2 (Trino's default).
    let mut props = req.properties.clone().unwrap_or_default();
    props.retain(|k, _| !iceberg::spec::TableProperties::RESERVED_PROPERTIES.contains(&k.as_str()));
    let built = TableMetadataBuilder::new(
        req.schema.clone(),
        partition_spec,
        sort_order,
        location,
        FormatVersion::V2,
        props,
    )
    .map_err(|e| CatalogError::BadRequest(e.to_string()))?
    .assign_uuid(table_uuid)
    .build()
    .map_err(|e| CatalogError::BadRequest(e.to_string()))?;
    Ok(built.metadata)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_iceberg_ext::catalog::CreateTableRequest;

    #[test]
    fn strips_reserved_properties_like_format_version() {
        // Trino sends `format-version` (a reserved property) in create requests; the iceberg
        // TableMetadataBuilder rejects reserved props. We must strip them and still succeed.
        let req: CreateTableRequest = serde_json::from_value(serde_json::json!({
            "name": "t",
            "schema": {"type":"struct","schema-id":0,
                "fields":[{"id":1,"name":"id","required":false,"type":"long"}]},
            "properties": {"format-version":"2","owner":"egret"}
        }))
        .unwrap();
        let md = create_table_metadata(&req, uuid::Uuid::nil(), "s3://wh/t".into()).unwrap();
        assert!(!md.properties().contains_key("format-version"));
        assert_eq!(
            md.properties().get("owner").map(String::as_str),
            Some("egret")
        );
    }
}
