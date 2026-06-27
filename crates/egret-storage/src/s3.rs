use crate::{Listing, ObjectMeta, ObjectStore, StorageConfig, StorageError};
use async_trait::async_trait;

#[derive(Clone)]
pub struct S3Store {
    client: aws_sdk_s3::Client,
    /// The bucket this store is scoped to (from `StorageConfig::bucket`).
    bucket: String,
}

impl S3Store {
    pub async fn new(cfg: &StorageConfig) -> Result<Self, StorageError> {
        let creds = aws_credential_types::Credentials::new(
            cfg.access_key.clone(),
            cfg.secret_key.clone(),
            None,
            None,
            "egret-storage",
        );
        let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(cfg.region.clone()))
            .credentials_provider(creds);
        if let Some(ep) = &cfg.endpoint {
            loader = loader.endpoint_url(ep.clone());
        }
        let shared = loader.load().await;
        let mut b = aws_sdk_s3::config::Builder::from(&shared);
        if cfg.path_style {
            b = b.force_path_style(true);
        }
        Ok(Self {
            client: aws_sdk_s3::Client::from_conf(b.build()),
            bucket: cfg.bucket.clone(),
        })
    }

    /// Split `s3://bucket/key` → (bucket, key).
    fn split(uri: &str) -> Result<(String, String), StorageError> {
        let rest = uri
            .strip_prefix("s3://")
            .ok_or_else(|| StorageError::S3(format!("not an s3 uri: {uri}")))?;
        let (bucket, key) = rest
            .split_once('/')
            .ok_or_else(|| StorageError::S3(format!("missing key: {uri}")))?;
        Ok((bucket.to_string(), key.to_string()))
    }
}

#[async_trait]
impl ObjectStore for S3Store {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError> {
        let (bucket, key) = Self::split(uri)?;
        self.client
            .put_object()
            .bucket(bucket)
            .key(key)
            .body(aws_sdk_s3::primitives::ByteStream::from(bytes))
            .send()
            .await
            .map_err(|e| StorageError::S3(e.to_string()))?;
        Ok(())
    }

    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
        let (bucket, key) = Self::split(uri)?;
        let out = self
            .client
            .get_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| {
                if let aws_sdk_s3::error::SdkError::ServiceError(se) = &e {
                    if se.err().is_no_such_key() {
                        return StorageError::NotFound(uri.to_string());
                    }
                }
                StorageError::S3(e.to_string())
            })?;
        let data = out
            .body
            .collect()
            .await
            .map_err(|e| StorageError::S3(e.to_string()))?;
        Ok(data.to_vec())
    }

    async fn delete(&self, uri: &str) -> Result<(), StorageError> {
        let (bucket, key) = Self::split(uri)?;
        self.client
            .delete_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| StorageError::S3(e.to_string()))?;
        Ok(())
    }

    async fn copy(&self, src: &str, dst: &str) -> Result<(), StorageError> {
        let (_src_bucket, src_key) = Self::split(src)?;
        let (_dst_bucket, dst_key) = Self::split(dst)?;
        // `copy_source` is `bucket/key`; we always copy within this store's
        // configured bucket. Keys are kept simple (no URL-encoding) — see the
        // trait doc.
        self.client
            .copy_object()
            .bucket(&self.bucket)
            .copy_source(format!("{}/{}", self.bucket, src_key))
            .key(dst_key)
            .send()
            .await
            .map_err(|e| StorageError::S3(e.to_string()))?;
        Ok(())
    }

    async fn list(&self, prefix: &str, delimiter: Option<&str>) -> Result<Listing, StorageError> {
        let mut folders: Vec<String> = Vec::new();
        let mut objects: Vec<ObjectMeta> = Vec::new();
        let mut continuation: Option<String> = None;

        loop {
            let mut req = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(prefix);

            if let Some(d) = delimiter {
                req = req.delimiter(d);
            }
            if let Some(ref token) = continuation {
                req = req.continuation_token(token);
            }

            let resp = req
                .send()
                .await
                .map_err(|e| StorageError::S3(e.to_string()))?;

            for cp in resp.common_prefixes() {
                if let Some(p) = cp.prefix() {
                    folders.push(p.to_string());
                }
            }

            for obj in resp.contents() {
                objects.push(ObjectMeta {
                    key: obj.key().unwrap_or_default().to_string(),
                    size: obj.size().unwrap_or(0),
                    last_modified: obj.last_modified().map(|t| t.to_string()),
                });
            }

            if resp.is_truncated().unwrap_or(false) {
                continuation = resp.next_continuation_token().map(str::to_string);
            } else {
                break;
            }
        }

        Ok(Listing { folders, objects })
    }
}
