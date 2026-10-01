//! Object-store abstraction (S3/MinIO) and S3 SigV4 remote signing for the catalog,
//! plus the global storage-connection registry (`storage.storage_connections`, E3a).
mod config;
mod error;
mod location;
#[cfg(feature = "test-util")]
mod mem;
mod registry;
mod s3;
pub mod signer;

pub use config::StorageConfig;
pub use error::StorageError;
pub use location::{table_base_location, table_metadata_location};
#[cfg(feature = "test-util")]
pub use mem::MemStore;
pub use registry::{StorageConn, StorageConnInput, StorageRegistry, StorageRegistryError};
pub use s3::S3Store;

/// Multipart part size, and the size under which `put_stream` is a single PUT.
pub const PART_BYTES: usize = 8 * 1024 * 1024;

use async_trait::async_trait;

/// A folder-style listing result.
#[derive(Debug, Clone, PartialEq)]
pub struct Listing {
    /// Key prefixes that share the delimiter (i.e. "virtual folders").
    pub folders: Vec<String>,
    /// Objects at this level.
    pub objects: Vec<ObjectMeta>,
}

/// Metadata for a single object returned by `list`.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectMeta {
    pub key: String,
    pub size: i64,
    /// ISO-8601 / RFC-3339 representation of the last-modified timestamp.
    pub last_modified: Option<String>,
}

/// Minimal object-store surface the catalog needs (write/read/delete metadata files).
#[async_trait]
pub trait ObjectStore: Send + Sync {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError>;
    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError>;
    async fn delete(&self, uri: &str) -> Result<(), StorageError>;
    /// Server-side copy `src` → `dst` (both `s3://bucket/key` URIs).
    ///
    /// Used by the file manager's move/rename (copy-then-delete). Keys are
    /// expected to be simple (the file-browser confines them under a connection
    /// root via `safe_key`); special characters in a key would need URL-encoding
    /// of the S3 copy-source and are not handled here.
    async fn copy(&self, src: &str, dst: &str) -> Result<(), StorageError>;
    /// List objects under `prefix` within the store's configured bucket.
    ///
    /// `prefix` is a plain key prefix (NOT an `s3://` URI).
    /// When `delimiter` is `Some("/")` the result is folder-style: objects at this
    /// level appear in `objects`, common prefixes (virtual folders) in `folders`.
    /// When `delimiter` is `None` the listing is flat: all objects recursively.
    async fn list(&self, prefix: &str, delimiter: Option<&str>) -> Result<Listing, StorageError>;

    /// A single PUT when the body is under PART_BYTES, else a multipart upload in
    /// PART_BYTES parts, aborted on any error. Returns the bytes written.
    async fn put_stream(
        &self,
        uri: &str,
        body: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
        size_hint: Option<u64>,
    ) -> Result<u64, StorageError> {
        let _ = (uri, body, size_hint);
        Err(StorageError::Unsupported("put_stream"))
    }

    async fn get_stream(
        &self,
        uri: &str,
    ) -> Result<Box<dyn tokio::io::AsyncBufRead + Send + Unpin>, StorageError> {
        let _ = uri;
        Err(StorageError::Unsupported("get_stream"))
    }

    /// The object's size, `None` when absent.
    async fn head(&self, uri: &str) -> Result<Option<u64>, StorageError> {
        let _ = uri;
        Err(StorageError::Unsupported("head"))
    }

    /// Deletes every object whose key starts with `uri`'s key. Returns how many.
    async fn delete_prefix(&self, uri: &str) -> Result<u64, StorageError> {
        let _ = uri;
        Err(StorageError::Unsupported("delete_prefix"))
    }
}
