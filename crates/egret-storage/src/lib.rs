//! Object-store abstraction (S3/MinIO) and S3 SigV4 remote signing for the catalog.
mod config;
mod error;
mod location;
mod s3;
pub mod signer;

pub use config::StorageConfig;
pub use error::StorageError;
pub use location::{table_base_location, table_metadata_location};
pub use s3::S3Store;

use async_trait::async_trait;

/// Minimal object-store surface the catalog needs (write/read/delete metadata files).
#[async_trait]
pub trait ObjectStore: Send + Sync {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError>;
    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError>;
    async fn delete(&self, uri: &str) -> Result<(), StorageError>;
}
