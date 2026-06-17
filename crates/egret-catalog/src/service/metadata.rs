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
    let props = req.properties.clone().unwrap_or_default();
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
