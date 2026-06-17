//! Iceberg REST Catalog request/response DTOs, mirroring lakekeeper's `iceberg-ext`.
pub mod catalog;
pub mod error;
pub mod sign;

pub use iceberg::{NamespaceIdent, TableIdent, TableRequirement, TableUpdate};
pub use iceberg::spec::{Schema, SortOrder, TableMetadata, TableMetadataRef, UnboundPartitionSpec};
