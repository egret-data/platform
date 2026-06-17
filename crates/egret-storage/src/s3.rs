use crate::{ObjectStore, StorageConfig, StorageError};
use async_trait::async_trait;

#[derive(Clone)]
pub struct S3Store {
    client: aws_sdk_s3::Client,
}

impl S3Store {
    pub async fn new(cfg: &StorageConfig) -> Self {
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
        Self {
            client: aws_sdk_s3::Client::from_conf(b.build()),
        }
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
            .map_err(|e| StorageError::S3(e.to_string()))?;
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
}
