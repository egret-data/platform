//! W9a Task 8: the execution file API's download against RustFS (a real
//! `S3Store`): a 64 MiB object is streamed through
//! `GET …/executions/{id}/file`, and the route has pulled only a sliver of it
//! from S3 when the first chunk of the response arrives.
//!
//! # Gated, and honest about it
//!
//! Needs `EGRET_TEST_S3_ENDPOINT` (+ `_KEY`, `_SECRET`, `_BUCKET`) and
//! `DATABASE_URL`. Without the S3 variables every case is reported
//! **ignored**, never passed (`harness = false`, as `storage_render_s3_live`).
//! The object goes under a throwaway `w9a-t8-test/<uuid>/` prefix and is
//! deleted by its exact key at the end; nothing here deletes by prefix.

mod common;

use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use egret_storage::{ObjectStore, S3Store, StorageConfig, StorageError};
use egret_workflow::storage::{self, index_task_files, uri, Caps, FileStore, StoredFile};
use tokio::io::{AsyncBufRead, AsyncRead, ReadBuf};
use uuid::Uuid;

const DEADLINE: Duration = Duration::from_secs(300);
const SIZE: u64 = 64 << 20;

fn cfg() -> Option<StorageConfig> {
    let endpoint = std::env::var("EGRET_TEST_S3_ENDPOINT").ok()?;
    Some(StorageConfig {
        endpoint: Some(endpoint),
        region: "us-east-1".into(),
        bucket: std::env::var("EGRET_TEST_S3_BUCKET").unwrap_or_else(|_| "warehouse".into()),
        access_key: std::env::var("EGRET_TEST_S3_KEY").unwrap_or_else(|_| "rustfsadmin".into()),
        secret_key: std::env::var("EGRET_TEST_S3_SECRET").unwrap_or_else(|_| "rustfsadmin".into()),
        path_style: true,
    })
}

fn pattern(i: u64) -> u8 {
    (i % 251) as u8
}

/// `SIZE` bytes of [`pattern`], made as they are read (the upload's body).
struct Generated(u64);

impl AsyncRead for Generated {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let n = (SIZE - self.0).min(buf.remaining() as u64);
        let at = self.0;
        let chunk: Vec<u8> = (at..at + n).map(pattern).collect();
        buf.put_slice(&chunk);
        self.0 += n;
        Poll::Ready(Ok(()))
    }
}

/// The S3 stream, counting the bytes the route pulls from it.
struct Counted {
    inner: Box<dyn AsyncBufRead + Send + Unpin>,
    count: Arc<AtomicU64>,
}

impl AsyncRead for Counted {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let r = Pin::new(&mut self.inner).poll_read(cx, buf);
        let n = buf.filled().len() - before;
        self.count.fetch_add(n as u64, Ordering::SeqCst);
        r
    }
}

impl AsyncBufRead for Counted {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<&[u8]>> {
        Pin::new(&mut self.get_mut().inner).poll_fill_buf(cx)
    }
    fn consume(mut self: Pin<&mut Self>, amt: usize) {
        self.count.fetch_add(amt as u64, Ordering::SeqCst);
        Pin::new(&mut self.inner).consume(amt);
    }
}

struct CountingS3 {
    s3: S3Store,
    pulled: Arc<AtomicU64>,
}

#[async_trait::async_trait]
impl ObjectStore for CountingS3 {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError> {
        self.s3.put(uri, bytes).await
    }
    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
        self.s3.get(uri).await
    }
    async fn delete(&self, uri: &str) -> Result<(), StorageError> {
        self.s3.delete(uri).await
    }
    async fn copy(&self, src: &str, dst: &str) -> Result<(), StorageError> {
        self.s3.copy(src, dst).await
    }
    async fn list(
        &self,
        prefix: &str,
        delimiter: Option<&str>,
    ) -> Result<egret_storage::Listing, StorageError> {
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
        Ok(Box::new(Counted {
            inner: self.s3.get_stream(uri).await?,
            count: self.pulled.clone(),
        }))
    }
    async fn head(&self, uri: &str) -> Result<Option<u64>, StorageError> {
        self.s3.head(uri).await
    }
}

fn download_streams(cfg: StorageConfig) -> Result<(), String> {
    use http_body_util::BodyExt as _;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async move {
        let Some(db) = common::db().await else {
            return Err("DATABASE_URL is required".into());
        };
        let pulled = Arc::new(AtomicU64::new(0));
        let bucket = cfg.bucket.clone();
        let s3 = S3Store::new(&cfg).await.map_err(|e| e.to_string())?;
        storage::install(Arc::new(FileStore {
            store: Arc::new(CountingS3 {
                s3,
                pulled: pulled.clone(),
            }),
            bucket,
            prefix: format!("w9a-t8-test/{}/", Uuid::new_v4()),
            caps: Caps::default(),
            retention_days: 30,
        }))
        .map_err(|_| "a store was already installed".to_string())?;
        let fs = storage::current().unwrap();
        let (ws, user) = common::seed_workspace(&db).await;
        common::grant(&db, ws, user, &["workflow:read"]).await;
        let app = common::app(&db).await;
        let execution: Uuid = sqlx::query_scalar(
            "INSERT INTO workflow.executions (workspace_id, namespace, flow_key, source, state, \
               history) VALUES ($1, 'w9a.t8', 'S3', 'id: f\ntasks: []\n', 'SUCCESS', \
               '[\"SUCCESS\"]'::jsonb) RETURNING id",
        )
        .bind(ws)
        .fetch_one(&db.pool)
        .await
        .map_err(|e| e.to_string())?;
        let run: Uuid = sqlx::query_scalar(
            "INSERT INTO workflow.task_runs (execution_id, run_ix, task_ix, task_id, type_id, \
               state, history) VALUES ($1, 0, 0, 'Write_It', \
               'io.kestra.plugin.core.storage.Write', 'SUCCESS', '[\"SUCCESS\"]'::jsonb) \
             RETURNING id",
        )
        .bind(execution)
        .fetch_one(&db.pool)
        .await
        .map_err(|e| e.to_string())?;
        let file = format!(
            "{}big.bin",
            uri::task_prefix("w9a.t8", "S3", execution, "Write_It", run)?
        );
        let object = fs.object_uri(ws, &file)?;
        let mut conn = db.pool.acquire().await.map_err(|e| e.to_string())?;
        index_task_files(
            &mut conn,
            ws,
            run,
            &[StoredFile {
                uri: file.clone(),
                size: SIZE,
            }],
        )
        .await
        .map_err(|e| e.to_string())?;
        drop(conn);
        let result = async {
            let written = fs
                .store
                .put_stream(&object, &mut Generated(0), Some(SIZE))
                .await
                .map_err(|e| e.to_string())?;
            if written != SIZE {
                return Err(format!("wrote {written} bytes"));
            }
            let path = format!(
                "/api/ws/{ws}/workflows/executions/{execution}/file?path={}",
                url::form_urlencoded::byte_serialize(file.as_bytes()).collect::<String>()
            );
            let res = common::get(&app, &path, user).await;
            if res.status() != 200 {
                return Err(format!("status {}", res.status()));
            }
            let mut body = res.into_body();
            let mut total = 0u64;
            let mut at_first = None;
            let mut in_order = true;
            while let Some(frame) = body.frame().await {
                let data = frame
                    .map_err(|e| e.to_string())?
                    .into_data()
                    .map_err(|_| "a non-data frame".to_string())?;
                at_first.get_or_insert(pulled.load(Ordering::SeqCst));
                in_order &= data
                    .iter()
                    .enumerate()
                    .all(|(i, b)| *b == pattern(total + i as u64));
                total += data.len() as u64;
            }
            let at_first = at_first.unwrap_or(0);
            println!("    pulled from S3 at the first frame: {at_first} of {SIZE}");
            if total != SIZE || !in_order {
                return Err(format!("{total} bytes, in order: {in_order}"));
            }
            if at_first >= SIZE / 8 {
                return Err(format!(
                    "{at_first} bytes pulled from S3 before the first frame: not streamed"
                ));
            }
            Ok(())
        }
        .await;
        // One by one, by key: never a prefix delete on a shared bucket.
        let deleted = fs.store.delete(&object).await.map_err(|e| e.to_string());
        let gone = fs.store.head(&object).await.map_err(|e| e.to_string())?;
        result?;
        deleted?;
        if gone.is_some() {
            return Err(format!("{object} is still there"));
        }
        Ok(())
    })
}

type Case = fn(StorageConfig) -> Result<(), String>;

fn cases() -> Vec<(&'static str, Case)> {
    vec![(
        "a_download_larger_than_memory_budget_streams_from_rustfs",
        download_streams,
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
    let cfg = cfg();
    for (name, case) in &selected {
        let Some(cfg) = cfg.clone() else {
            println!("test {name} ... ignored, EGRET_TEST_S3_ENDPOINT unset");
            ignored += 1;
            continue;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let case = *case;
        std::thread::spawn(move || {
            let _ = tx.send(case(cfg));
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
