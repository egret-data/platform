//! Test-only helpers shared by integration tests.
use iceberg::spec::{
    FormatVersion, NestedField, PrimitiveType, Schema, SortOrder, TableMetadata,
    TableMetadataBuilder, Type, UnboundPartitionSpec,
};
use std::sync::Arc;
use uuid::Uuid;

/// Build a minimal single-column (id: long) table metadata for tests.
pub fn minimal_table_metadata(location: &str, table_uuid: Uuid) -> TableMetadata {
    let schema = Schema::builder()
        .with_fields(vec![Arc::new(NestedField::required(
            1,
            "id",
            Type::Primitive(PrimitiveType::Long),
        ))])
        .build()
        .unwrap();
    TableMetadataBuilder::new(
        schema,
        UnboundPartitionSpec::builder().build(),
        SortOrder::unsorted_order(),
        location.to_string(),
        FormatVersion::V2,
        Default::default(),
    )
    .unwrap()
    .assign_uuid(table_uuid)
    .build()
    .unwrap()
    .metadata
}
