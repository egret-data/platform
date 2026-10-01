use crate::{Listing, ObjectMeta, ObjectStore, StorageConfig, StorageError};
use async_trait::async_trait;
use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart, Delete, ObjectIdentifier};
use tokio::io::{AsyncRead, AsyncReadExt};

/// Reads until `buf` holds `PART_BYTES` or the body ends; returns whether the body ended.
async fn fill(
    body: &mut (dyn AsyncRead + Send + Unpin),
    buf: &mut Vec<u8>,
) -> std::io::Result<bool> {
    buf.clear();
    while buf.len() < crate::PART_BYTES {
        if body
            .take((crate::PART_BYTES - buf.len()) as u64)
            .read_buf(buf)
            .await?
            == 0
        {
            return Ok(true);
        }
    }
    Ok(false)
}

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

    /// Uploads `first` and the rest of `body` as parts of one multipart upload.
    /// The caller aborts the upload when this errors.
    async fn upload_parts(
        &self,
        uri: &str,
        bucket: &str,
        key: &str,
        upload_id: &str,
        body: &mut (dyn AsyncRead + Send + Unpin),
        buf: &mut Vec<u8>,
    ) -> Result<u64, StorageError> {
        let err = |e: &dyn std::fmt::Display| StorageError::S3(format!("{uri}: {e}"));
        let mut ended = false;
        let mut parts = Vec::new();
        let mut total = 0u64;
        loop {
            if !buf.is_empty() {
                let number = parts.len() as i32 + 1;
                total += buf.len() as u64;
                let out = self
                    .client
                    .upload_part()
                    .bucket(bucket)
                    .key(key)
                    .upload_id(upload_id)
                    .part_number(number)
                    .body(aws_sdk_s3::primitives::ByteStream::from(std::mem::replace(
                        buf,
                        Vec::with_capacity(crate::PART_BYTES),
                    )))
                    .send()
                    .await
                    .map_err(|e| err(&aws_sdk_s3::error::DisplayErrorContext(e)))?;
                parts.push(
                    CompletedPart::builder()
                        .part_number(number)
                        .set_e_tag(out.e_tag().map(str::to_string))
                        .build(),
                );
            }
            if ended {
                break;
            }
            ended = fill(body, buf).await.map_err(|e| err(&e))?;
        }
        self.client
            .complete_multipart_upload()
            .bucket(bucket)
            .key(key)
            .upload_id(upload_id)
            .multipart_upload(
                CompletedMultipartUpload::builder()
                    .set_parts(Some(parts))
                    .build(),
            )
            .send()
            .await
            .map_err(|e| err(&aws_sdk_s3::error::DisplayErrorContext(e)))?;
        Ok(total)
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

    async fn put_stream(
        &self,
        uri: &str,
        body: &mut (dyn AsyncRead + Send + Unpin),
        _size_hint: Option<u64>,
    ) -> Result<u64, StorageError> {
        let (bucket, key) = Self::split(uri)?;
        let err = |e: &dyn std::fmt::Display| StorageError::S3(format!("{uri}: {e}"));
        let mut buf = Vec::with_capacity(crate::PART_BYTES);
        let ended = fill(body, &mut buf).await.map_err(|e| err(&e))?;
        if ended {
            let n = buf.len() as u64;
            self.client
                .put_object()
                .bucket(bucket)
                .key(key)
                .body(aws_sdk_s3::primitives::ByteStream::from(buf))
                .send()
                .await
                .map_err(|e| err(&aws_sdk_s3::error::DisplayErrorContext(e)))?;
            return Ok(n);
        }
        let created = self
            .client
            .create_multipart_upload()
            .bucket(&bucket)
            .key(&key)
            .send()
            .await
            .map_err(|e| err(&aws_sdk_s3::error::DisplayErrorContext(e)))?;
        let upload_id = created
            .upload_id()
            .ok_or_else(|| err(&"no upload id returned"))?
            .to_string();
        match self
            .upload_parts(uri, &bucket, &key, &upload_id, body, &mut buf)
            .await
        {
            Ok(n) => Ok(n),
            Err(e) => {
                if let Err(abort) = self
                    .client
                    .abort_multipart_upload()
                    .bucket(&bucket)
                    .key(&key)
                    .upload_id(&upload_id)
                    .send()
                    .await
                {
                    tracing::warn!(%uri, error = %aws_sdk_s3::error::DisplayErrorContext(abort), "abort_multipart_upload failed");
                }
                Err(e)
            }
        }
    }

    async fn get_stream(
        &self,
        uri: &str,
    ) -> Result<Box<dyn tokio::io::AsyncBufRead + Send + Unpin>, StorageError> {
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
                StorageError::S3(format!(
                    "{uri}: {}",
                    aws_sdk_s3::error::DisplayErrorContext(e)
                ))
            })?;
        Ok(Box::new(out.body.into_async_read()))
    }

    async fn head(&self, uri: &str) -> Result<Option<u64>, StorageError> {
        let (bucket, key) = Self::split(uri)?;
        match self
            .client
            .head_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
        {
            Ok(out) => Ok(Some(out.content_length().unwrap_or(0).max(0) as u64)),
            Err(aws_sdk_s3::error::SdkError::ServiceError(se)) if se.err().is_not_found() => {
                Ok(None)
            }
            Err(e) => Err(StorageError::S3(format!(
                "{uri}: {}",
                aws_sdk_s3::error::DisplayErrorContext(e)
            ))),
        }
    }

    async fn delete_prefix(&self, uri: &str) -> Result<u64, StorageError> {
        let (bucket, prefix) = Self::split(uri)?;
        let err = |e: &dyn std::fmt::Display| StorageError::S3(format!("{uri}: {e}"));
        let mut deleted = 0u64;
        let mut continuation: Option<String> = None;
        loop {
            let mut req = self
                .client
                .list_objects_v2()
                .bucket(&bucket)
                .prefix(&prefix);
            if let Some(token) = &continuation {
                req = req.continuation_token(token);
            }
            let page = req
                .send()
                .await
                .map_err(|e| err(&aws_sdk_s3::error::DisplayErrorContext(e)))?;
            let ids = page
                .contents()
                .iter()
                .filter_map(|o| o.key())
                .map(|k| ObjectIdentifier::builder().key(k).build())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| err(&e))?;
            for batch in ids.chunks(1000) {
                let out = self
                    .client
                    .delete_objects()
                    .bucket(&bucket)
                    .delete(
                        Delete::builder()
                            .set_objects(Some(batch.to_vec()))
                            .quiet(true)
                            .build()
                            .map_err(|e| err(&e))?,
                    )
                    .send()
                    .await
                    .map_err(|e| err(&aws_sdk_s3::error::DisplayErrorContext(e)))?;
                if let Some(first) = out.errors().first() {
                    return Err(err(&format!(
                        "delete of {} failed: {}",
                        first.key().unwrap_or("?"),
                        first.message().unwrap_or("no message")
                    )));
                }
                deleted += batch.len() as u64;
            }
            match page.next_continuation_token() {
                Some(t) if page.is_truncated().unwrap_or(false) => {
                    continuation = Some(t.to_string())
                }
                _ => break,
            }
        }
        Ok(deleted)
    }
}
