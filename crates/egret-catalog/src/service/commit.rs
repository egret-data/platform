use crate::CatalogError;
use iceberg::spec::{TableMetadata, TableMetadataBuilder};
use iceberg::{TableRequirement, TableUpdate};

/// Validate requirements against current metadata, then apply updates → new metadata.
/// `metadata_location` is None for a staged/just-created table (affects assert-create).
pub fn apply_commit(
    current: TableMetadata,
    metadata_location: Option<&str>,
    requirements: &[TableRequirement],
    updates: Vec<TableUpdate>,
) -> Result<TableMetadata, CatalogError> {
    // 1. Requirements (optimistic concurrency). Pass Some(&metadata) only when the table
    //    already exists (has a metadata_location); None signals a staged/new table.
    for r in requirements {
        r.check(metadata_location.map(|_| &current))
            .map_err(|e| CatalogError::Conflict(e.to_string()))?;
    }
    // 2. Apply updates in order via the iceberg builder.
    let mut builder = TableMetadataBuilder::new_from_metadata(
        current,
        metadata_location.map(str::to_string),
    );
    for u in updates {
        builder = TableUpdate::apply(u, builder)
            .map_err(|e| CatalogError::BadRequest(e.to_string()))?;
    }
    let built = builder
        .build()
        .map_err(|e| CatalogError::Conflict(e.to_string()))?;
    Ok(built.metadata)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::test_support::minimal_table_metadata;
    use std::collections::HashMap;

    #[test]
    fn apply_set_properties_update() {
        let md = minimal_table_metadata("s3://wh/ns/t-x", uuid::Uuid::nil());
        let updates = vec![TableUpdate::SetProperties {
            updates: HashMap::from([("owner".to_string(), "egret".to_string())]),
        }];
        let new_md =
            apply_commit(md, Some("s3://wh/ns/t-x/metadata/00000-x.json"), &[], updates)
                .unwrap();
        assert_eq!(
            new_md.properties().get("owner").map(String::as_str),
            Some("egret")
        );
    }
}
