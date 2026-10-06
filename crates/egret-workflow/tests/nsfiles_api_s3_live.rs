//! W9b Task 8: the namespace file API against MinIO (a real `S3Store`): a
//! 64 MiB file uploaded through `POST …/files` and downloaded through
//! `GET …/files`, streamed both ways.
//!
//! # Gated, and honest about it
//!
//! Needs `EGRET_TEST_S3_ENDPOINT` (+ `_KEY`, `_SECRET`, `_BUCKET`) and
//! `DATABASE_URL`. Without the S3 variables the case is reported
//! **ignored**, never passed (`harness = false`, as `nsfiles_s3_live`). Its
//! own binary: the route reads the process's one installed store.
//!
//! # Fenced
//!
//! The `FileStore`'s prefix is a throwaway `w9b-t8-test/<uuid>/`; the store
//! refuses (and records) every `delete` outside it and every `delete_prefix`
//! but of exactly it. The case ends with `delete_prefix` of that prefix,
//! also when it fails.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use egret_storage::{Listing, ObjectStore, S3Store, StorageConfig, StorageError};
use egret_workflow::storage::{self, Caps, FileStore};
use tokio::io::{AsyncBufRead, AsyncRead};
use uuid::Uuid;

const DEADLINE: Duration = Duration::from_secs(300);
const NS: &str = "w9b.t8s3";
const SIZE: usize = 64 << 20;

fn cfg() -> Option<StorageConfig> {
    let endpoint = std::env::var("EGRET_TEST_S3_ENDPOINT").ok()?;
    Some(StorageConfig {
        endpoint: Some(endpoint),
        region: "us-east-1".into(),
        bucket: std::env::var("EGRET_TEST_S3_BUCKET").unwrap_or_else(|_| "warehouse".into()),
        access_key: std::env::var("EGRET_TEST_S3_KEY").unwrap_or_else(|_| "minioadmin".into()),
        secret_key: std::env::var("EGRET_TEST_S3_SECRET").unwrap_or_else(|_| "minioadmin".into()),
        path_style: true,
    })
}

struct Fence {
    s3: S3Store,
    allowed: String,
    refused: Mutex<Vec<String>>,
}

impl Fence {
    fn refuse(&self, uri: &str) -> StorageError {
        self.refused.lock().unwrap().push(uri.to_string());
        StorageError::S3(format!("{uri}: outside the test's prefix, refused"))
    }
}

#[async_trait::async_trait]
impl ObjectStore for Fence {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError> {
        self.s3.put(uri, bytes).await
    }
    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
        self.s3.get(uri).await
    }
    async fn delete(&self, uri: &str) -> Result<(), StorageError> {
        if !(uri.starts_with(&self.allowed) && uri.len() > self.allowed.len()) {
            return Err(self.refuse(uri));
        }
        self.s3.delete(uri).await
    }
    async fn copy(&self, src: &str, dst: &str) -> Result<(), StorageError> {
        self.s3.copy(src, dst).await
    }
    async fn list(&self, prefix: &str, delimiter: Option<&str>) -> Result<Listing, StorageError> {
        self.s3.list(prefix, delimiter).await
    }
    async fn put_stream(
        &self,
        uri: &str,
        body: &mut (dyn AsyncRead + Send + Unpin),
        size_hint: Option<u64>,
    ) -> Result<u64, StorageError> {
        self.s3.put_stream(uri, body, size_hint).await
    }
    async fn get_stream(
        &self,
        uri: &str,
    ) -> Result<Box<dyn AsyncBufRead + Send + Unpin>, StorageError> {
        self.s3.get_stream(uri).await
    }
    async fn head(&self, uri: &str) -> Result<Option<u64>, StorageError> {
        self.s3.head(uri).await
    }
    async fn delete_prefix(&self, uri: &str) -> Result<u64, StorageError> {
        if uri != self.allowed {
            return Err(self.refuse(uri));
        }
        self.s3.delete_prefix(uri).await
    }
}

fn pattern(i: usize) -> u8 {
    (i % 251) as u8
}

/// Upload 64 MiB through the API, then download it through the API: every
/// byte comes back, in order, from MinIO.
fn a_download_of_64_mib_streams_from_minio() -> Result<(), String> {
    let cfg = cfg().ok_or("no S3")?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async move {
        let Some(db) = common::db().await else {
            return Err("DATABASE_URL is required".to_string());
        };
        let throwaway = format!("w9b-t8-test/{}/", Uuid::new_v4());
        let bucket = cfg.bucket.clone();
        let fence = Arc::new(Fence {
            s3: S3Store::new(&cfg).await.map_err(|e| e.to_string())?,
            allowed: format!("s3://{bucket}/{throwaway}"),
            refused: Mutex::new(Vec::new()),
        });
        storage::install(Arc::new(FileStore {
            store: fence.clone(),
            bucket,
            prefix: throwaway.clone(),
            caps: Caps::default(),
            retention_days: 30,
        }))
        .map_err(|_| "a store was already installed".to_string())?;
        let (ws, user) = common::seed_workspace(&db).await;
        common::grant(&db, ws, user, &["workflow:manage", "workflow:read"]).await;
        let app = common::app(&db).await;
        let case = async {
            let boundary = "w9b-t8-s3";
            let mut body = format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"fileContent\"; \
                 filename=\"big.bin\"\r\n\r\n"
            )
            .into_bytes();
            body.extend((0..SIZE).map(pattern));
            body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
            let uri = format!("/api/ws/{ws}/workflows/namespaces/{NS}/files?path=/big/big.bin");
            let res = common::send_raw(
                &app,
                "POST",
                &uri,
                user,
                &format!("multipart/form-data; boundary={boundary}"),
                body,
            )
            .await;
            if res.status() != 200 {
                return Err(format!("upload: {}", res.status()));
            }
            let res = common::get(&app, &uri, user).await;
            if res.status() != 200 {
                return Err(format!("download: {}", res.status()));
            }
            if res.headers()["content-type"] != "application/octet-stream" {
                return Err(format!("{:?}", res.headers()));
            }
            // Chunk by chunk: the body is a stream, never collected here.
            let mut stream = res.into_body().into_data_stream();
            let (mut at, mut chunks) = (0usize, 0usize);
            while let Some(chunk) = futures_util::StreamExt::next(&mut stream).await {
                let chunk = chunk.map_err(|e| e.to_string())?;
                for (i, b) in chunk.iter().enumerate() {
                    if *b != pattern(at + i) {
                        return Err(format!("byte {} differs", at + i));
                    }
                }
                at += chunk.len();
                chunks += 1;
            }
            if at != SIZE {
                return Err(format!("{at} bytes downloaded, {SIZE} uploaded"));
            }
            println!("    {at} bytes in {chunks} chunks");
            if chunks < 2 {
                return Err("one chunk: the download was not streamed".into());
            }
            Ok(())
        };
        let result = case.await;
        let removed = fence
            .delete_prefix(&fence.allowed)
            .await
            .map_err(|e| e.to_string())?;
        println!("    delete_prefix {}: {removed} object(s)", fence.allowed);
        let refused = fence.refused.lock().unwrap().clone();
        result?;
        if !refused.is_empty() {
            return Err(format!("deletes outside {throwaway}: {refused:?}"));
        }
        Ok(())
    })
}

type Case = fn() -> Result<(), String>;

fn cases() -> Vec<(&'static str, Case)> {
    vec![(
        "a_download_of_64_mib_streams_from_minio",
        a_download_of_64_mib_streams_from_minio,
    )]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let filter = args.iter().find(|a| !a.starts_with('-')).cloned();
    let all = cases();
    let selected: Vec<_> = all
        .iter()
        .filter(|(n, _)| filter.as_ref().is_none_or(|f| n.contains(f.as_str())))
        .collect();
    let filtered_out = all.len() - selected.len();
    let started = std::time::Instant::now();
    println!("\nrunning {} tests", selected.len());
    let (mut passed, mut ignored, mut failed) = (0, 0, Vec::new());
    for (name, case) in &selected {
        if cfg().is_none() {
            println!("test {name} ... ignored, EGRET_TEST_S3_ENDPOINT unset");
            ignored += 1;
            continue;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let case = *case;
        std::thread::spawn(move || {
            let _ = tx.send(case());
        });
        match rx.recv_timeout(DEADLINE) {
            Ok(Ok(())) => {
                println!("test {name} ... ok");
                passed += 1;
            }
            Ok(Err(why)) => {
                println!("test {name} ... FAILED\n    {why}");
                failed.push(name.to_string());
            }
            Err(_) => {
                println!("test {name} ... FAILED\n    no answer in {DEADLINE:?}");
                failed.push(name.to_string());
            }
        }
    }
    let verdict = if failed.is_empty() { "ok" } else { "FAILED" };
    println!(
        "\ntest result: {verdict}. {passed} passed; {} failed; {ignored} ignored; 0 measured; \
         {filtered_out} filtered out; finished in {:.2}s\n",
        failed.len(),
        started.elapsed().as_secs_f64()
    );
    std::process::exit(if failed.is_empty() { 0 } else { 101 });
}
