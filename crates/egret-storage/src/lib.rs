//! Object-store abstraction (S3/RustFS) and S3 SigV4 remote signing for the catalog,
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

/// The most one [`ObjectStore::copy`] moves: 5 GiB, S3's limit for a single
/// CopyObject (a larger object needs UploadPartCopy).
pub const MAX_SINGLE_COPY_BYTES: u64 = 5 * 1024 * 1024 * 1024;

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
    /// Server-side copy `src` → `dst` (both `s3://bucket/key` URIs, within
    /// this store's bucket), replacing `dst`. Any key is accepted: the S3 copy
    /// source is percent-encoded. One copy moves at most
    /// [`MAX_SINGLE_COPY_BYTES`] (S3's CopyObject limit; larger objects need a
    /// multipart copy, which this method does not do): a caller that copies
    /// what a client uploaded caps the upload at that.
    ///
    /// Used by the file manager's move/rename (copy-then-delete) and by W9b's
    /// namespace-file write (staging object → revision key).
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

/// `delete_prefix` removes a whole subtree, so it only accepts a directory-style prefix:
/// a non-empty key ending in `/` (`a/b` would also take `a/bc`; an empty key is the bucket).
pub(crate) fn check_delete_prefix(uri: &str) -> Result<(), StorageError> {
    let key = uri
        .strip_prefix("s3://")
        .and_then(|rest| rest.split_once('/'))
        .map(|(_, key)| key);
    match key {
        Some(k) if !k.is_empty() && k.ends_with('/') => Ok(()),
        _ => Err(StorageError::S3(format!(
            "{uri}: delete_prefix needs a non-empty key ending in '/'"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::check_delete_prefix;

    #[test]
    fn delete_prefix_accepts_only_a_non_empty_key_ending_in_a_slash() {
        for bad in ["s3://b", "s3://b/", "s3://b/a/b", "a/b/", ""] {
            let e = check_delete_prefix(bad).unwrap_err().to_string();
            assert!(e.contains(bad), "{e}");
        }
        assert!(check_delete_prefix("s3://b/a/b/").is_ok());
    }
}
