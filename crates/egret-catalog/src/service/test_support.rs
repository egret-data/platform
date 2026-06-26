//! Test-only helpers shared by integration tests.
use iceberg::spec::{
    FormatVersion, NestedField, PrimitiveType, Schema, SortOrder, SqlViewRepresentation,
    TableMetadata, TableMetadataBuilder, Type, UnboundPartitionSpec, ViewFormatVersion,
    ViewMetadata, ViewMetadataBuilder, ViewRepresentation, ViewRepresentations, ViewVersion,
};
use iceberg::NamespaceIdent;
use std::collections::HashMap;
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

/// Build a minimal v1 view metadata for tests: one schema (id: long),
/// one SQL view-version (`SELECT 1`, dialect "trino"), current-version 1.
pub fn minimal_view_metadata(location: &str, view_uuid: Uuid) -> ViewMetadata {
    let schema = Schema::builder()
        .with_schema_id(0)
        .with_fields(vec![Arc::new(NestedField::required(
            1,
            "id",
            Type::Primitive(PrimitiveType::Long),
        ))])
        .build()
        .unwrap();

    let representations = ViewRepresentations::builder()
        .add_representation(ViewRepresentation::Sql(SqlViewRepresentation {
            sql: "SELECT 1".to_string(),
            dialect: "trino".to_string(),
        }))
        .build()
        .unwrap();

    let version = ViewVersion::builder()
        .with_version_id(1)
        .with_schema_id(0)
        .with_timestamp_ms(chrono::Utc::now().timestamp_millis())
        .with_summary(HashMap::new())
        .with_representations(representations)
        .with_default_catalog(None)
        .with_default_namespace(NamespaceIdent::new("default".to_string()))
        .build();

    ViewMetadataBuilder::new(
        location.to_string(),
        schema,
        version,
        ViewFormatVersion::V1,
        HashMap::new(),
    )
    .unwrap()
    .assign_uuid(view_uuid)
    .build()
    .unwrap()
    .metadata
}
