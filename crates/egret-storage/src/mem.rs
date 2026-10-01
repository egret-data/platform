//! In-memory `ObjectStore` for tests (feature `test-util`).
use crate::{Listing, ObjectMeta, ObjectStore, StorageError};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::io::AsyncReadExt;

/// Objects keyed by their full `s3://bucket/key` URI.
#[derive(Default, Clone)]
pub struct MemStore {
    objects: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
}

impl MemStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn keys(&self) -> Vec<String> {
        self.objects.lock().unwrap().keys().cloned().collect()
    }
}

#[async_trait]
impl ObjectStore for MemStore {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError> {
        self.objects.lock().unwrap().insert(uri.to_string(), bytes);
        Ok(())
    }

    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
        self.objects
            .lock()
            .unwrap()
            .get(uri)
            .cloned()
            .ok_or_else(|| StorageError::NotFound(uri.to_string()))
    }

    async fn delete(&self, uri: &str) -> Result<(), StorageError> {
        self.objects.lock().unwrap().remove(uri);
        Ok(())
    }

    async fn copy(&self, src: &str, dst: &str) -> Result<(), StorageError> {
        let bytes = self.get(src).await?;
        self.put(dst, bytes).await
    }

    /// Flat listing over keys that contain `prefix` as a URI substring after the
    /// bucket; folder-style listing is not modelled.
    async fn list(&self, prefix: &str, _delimiter: Option<&str>) -> Result<Listing, StorageError> {
        let objects = self
            .objects
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(uri, v)| {
                let key = uri.strip_prefix("s3://")?.split_once('/')?.1;
                key.starts_with(prefix).then(|| ObjectMeta {
                    key: key.to_string(),
                    size: v.len() as i64,
                    last_modified: None,
                })
            })
            .collect();
        Ok(Listing {
            folders: Vec::new(),
            objects,
        })
    }

    async fn put_stream(
        &self,
        uri: &str,
        body: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
        _size_hint: Option<u64>,
    ) -> Result<u64, StorageError> {
        let mut buf = Vec::new();
        body.read_to_end(&mut buf)
            .await
            .map_err(|e| StorageError::S3(format!("{uri}: {e}")))?;
        let n = buf.len() as u64;
        self.put(uri, buf).await?;
        Ok(n)
    }

    async fn get_stream(
        &self,
        uri: &str,
    ) -> Result<Box<dyn tokio::io::AsyncBufRead + Send + Unpin>, StorageError> {
        let bytes = self.get(uri).await?;
        Ok(Box::new(std::io::Cursor::new(bytes)))
    }

    async fn head(&self, uri: &str) -> Result<Option<u64>, StorageError> {
        Ok(self
            .objects
            .lock()
            .unwrap()
            .get(uri)
            .map(|v| v.len() as u64))
    }

    async fn delete_prefix(&self, uri: &str) -> Result<u64, StorageError> {
        let mut m = self.objects.lock().unwrap();
        let before = m.len();
        m.retain(|k, _| !k.starts_with(uri));
        Ok((before - m.len()) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mem_store_round_trips_a_stream_and_reports_its_size() {
        let s = MemStore::new();
        let mut body: &[u8] = b"hello stream";
        let n = s.put_stream("s3://b/k", &mut body, None).await.unwrap();
        assert_eq!(n, 12);
        assert_eq!(s.head("s3://b/k").await.unwrap(), Some(12));
        let mut r = s.get_stream("s3://b/k").await.unwrap();
        let mut out = Vec::new();
        r.read_to_end(&mut out).await.unwrap();
        assert_eq!(out, b"hello stream");
    }

    #[tokio::test]
    async fn mem_store_delete_prefix_removes_only_that_prefix() {
        let s = MemStore::new();
        for k in [
            "s3://b/a/b/1",
            "s3://b/a/b/2",
            "s3://b/a/b2/1",
            "s3://b/a/c",
        ] {
            s.put(k, vec![1]).await.unwrap();
        }
        assert_eq!(s.delete_prefix("s3://b/a/b/").await.unwrap(), 2);
        assert_eq!(s.keys(), vec!["s3://b/a/b2/1", "s3://b/a/c"]);
    }

    #[tokio::test]
    async fn head_of_an_absent_object_is_none_not_an_error() {
        assert_eq!(MemStore::new().head("s3://b/nope").await.unwrap(), None);
    }

    #[tokio::test]
    async fn mem_store_overrides_all_four_streaming_methods() {
        let s = MemStore::new();
        let r = s.put_stream("s3://b/x", &mut &b"x"[..], None).await;
        assert!(!matches!(r, Err(StorageError::Unsupported(_))));
        assert!(!matches!(
            s.get_stream("s3://b/x").await,
            Err(StorageError::Unsupported(_))
        ));
        assert!(!matches!(
            s.head("s3://b/x").await,
            Err(StorageError::Unsupported(_))
        ));
        assert!(!matches!(
            s.delete_prefix("s3://b/x").await,
            Err(StorageError::Unsupported(_))
        ));
    }
}
