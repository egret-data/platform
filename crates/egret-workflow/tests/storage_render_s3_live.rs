//! W9a Task 4 fix round 1, C1: `read()` on a real `S3Store` (RustFS), with as
//! many or more concurrent renders than tokio workers. W9a Task 7 adds two
//! cases for an uploaded FILE input ([`upload_input_on_s3`]).
//!
//! A render runs on its own thread and the worker that spawned it waits in
//! `join`; `read()` blocks that render thread on the runtime while S3's IO
//! needs the runtime's IO driver. Before the fix the join parked the worker
//! with its core, so once every worker was parked nothing drove the driver and
//! the runtime wedged for good (the reviewer measured 1/1, 2/2 and 4/4 hung).
//! `MemStore` cannot show it -- it needs no driver -- which is why this file
//! exists.
//!
//! # A hang must FAIL, not hang
//!
//! Each case runs on its own thread with its own runtime, and [`main`] waits
//! on a channel with a deadline. A runtime that wedges is reported FAILED and
//! the process exits; `tokio::time::timeout` could not do this, because the
//! timer is driven by the same driver that stopped.
//!
//! # Gated, and honest about it
//!
//! Needs `EGRET_TEST_S3_ENDPOINT` (+ `_KEY`, `_SECRET`, `_BUCKET`). Without it
//! every case is reported **ignored**, never passed (`harness = false`, as
//! `container_docker_live`). Objects go under a throwaway
//! `w9a-t4-test/<uuid>/` (Task 7: `w9a-t7-test/<uuid>/`) prefix and are deleted ONE BY ONE by key at the end;
//! nothing here deletes by prefix.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use egret_storage::{S3Store, StorageConfig};
use egret_workflow::storage::scope::Snapshot;
use egret_workflow::storage::{Caps, FileStore, IndexedFile};
use egret_workflow::{render_str, RenderContext};
use uuid::Uuid;

const DEADLINE: Duration = Duration::from_secs(60);

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

const WS: Uuid = Uuid::from_u128(0x5eed);

fn uri_of(name: &str) -> String {
    format!(
        "kestra:///w9a/t4/f/executions/{}/tasks/w/{}/{name}",
        Uuid::from_u128(1),
        Uuid::from_u128(2)
    )
}

/// `renders` concurrent `read()`s of one object on `workers` tokio workers,
/// each render issued from a spawned task on a worker (as `settle` is).
fn concurrent_reads(cfg: StorageConfig, workers: usize, renders: usize) -> Result<(), String> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async move {
        let bucket = cfg.bucket.clone();
        let store = S3Store::new(&cfg).await.map_err(|e| e.to_string())?;
        let fs = Arc::new(FileStore {
            store: Arc::new(store),
            bucket,
            prefix: format!("w9a-t4-test/{}/", Uuid::new_v4()),
            caps: Caps::default(),
            retention_days: 30,
        });
        let uri = uri_of("s3.txt");
        let object = fs.object_uri(WS, &uri)?;
        fs.store
            .put(&object, b"from rustfs".to_vec())
            .await
            .map_err(|e| e.to_string())?;
        let files = BTreeMap::from([(
            uri.clone(),
            Ok(IndexedFile {
                uri: uri.clone(),
                execution_id: Uuid::from_u128(1),
                task_run_id: Some(Uuid::from_u128(2)),
                size: 10,
            }),
        )]);
        let mut tasks = Vec::new();
        for _ in 0..renders {
            let (fs, files, uri) = (fs.clone(), files.clone(), uri.clone());
            tasks.push(tokio::spawn(async move {
                let scope = Snapshot::from_parts(files, fs, WS, None).into_scope();
                let cx = RenderContext::new().with_files(scope);
                render_str(&format!("{{{{ read('{uri}') }}}}"), &cx).map_err(|e| e.to_string())
            }));
        }
        let mut answers = Vec::new();
        for t in tasks {
            answers.push(t.await.map_err(|e| e.to_string())?);
        }
        // One by one, by key: never a prefix delete on a shared bucket.
        fs.store.delete(&object).await.map_err(|e| e.to_string())?;
        for a in answers {
            if a.as_deref() != Ok("from rustfs") {
                return Err(format!("a render answered {a:?}"));
            }
        }
        Ok(())
    })
}

/// W9a Task 7: an uploaded FILE input on a real `S3Store`, through the
/// multipart path (over `PART_BYTES`): `under_cap = true` stores it whole and
/// `head` agrees; `false` is one byte over the cap, refused naming it, and
/// the aborted upload leaves no object. Under a throwaway
/// `w9a-t7-test/<uuid>/` prefix, deleted by key.
fn upload_input_on_s3(cfg: StorageConfig, under_cap: bool) -> Result<(), String> {
    use egret_workflow::storage::inputs::{upload_input, UploadError};
    use tokio::io::AsyncReadExt as _;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async move {
        let bucket = cfg.bucket.clone();
        let store = S3Store::new(&cfg).await.map_err(|e| e.to_string())?;
        let cap = egret_storage::PART_BYTES as u64 + 1024 * 1024;
        let fs = FileStore {
            store: Arc::new(store),
            bucket,
            prefix: format!("w9a-t7-test/{}/", Uuid::new_v4()),
            caps: Caps {
                max_file_bytes: cap,
                ..Caps::default()
            },
            retention_days: 30,
        };
        let input: egret_workflow::models::Input =
            serde_norway::from_str("id: f\ntype: FILE\n").map_err(|e| e.to_string())?;
        let size = if under_cap { cap } else { cap + 1 };
        let execution = Uuid::new_v4();
        let mut body = tokio::io::repeat(b'x').take(size);
        let got = upload_input(
            &fs, WS, "w9a.t7", "s3", execution, &input, "big.bin", &mut body,
        )
        .await;
        let uri =
            egret_workflow::storage::uri::input_uri("w9a.t7", "s3", execution, "f", "big.bin")?;
        let object = fs.object_uri(WS, &uri)?;
        let head = fs.store.head(&object).await.map_err(|e| e.to_string())?;
        if head.is_some() {
            // One by one, by key: never a prefix delete on a shared bucket.
            fs.store.delete(&object).await.map_err(|e| e.to_string())?;
        }
        match (under_cap, got) {
            (true, Ok(f)) if f.size == cap && head == Some(cap) && f.uri == uri => Ok(()),
            (false, Err(UploadError::TooLarge(m)))
                if m.contains(&cap.to_string()) && head.is_none() =>
            {
                Ok(())
            }
            (_, other) => Err(format!("under_cap={under_cap}: {other:?}, head {head:?}")),
        }
    })
}

type Case = fn(StorageConfig) -> Result<(), String>;

fn cases() -> Vec<(&'static str, Case)> {
    vec![
        ("one_worker_one_read_does_not_wedge", |c| {
            concurrent_reads(c, 1, 1)
        }),
        ("one_worker_four_concurrent_reads_do_not_wedge", |c| {
            concurrent_reads(c, 1, 4)
        }),
        ("two_workers_four_concurrent_reads_do_not_wedge", |c| {
            concurrent_reads(c, 2, 4)
        }),
        ("four_workers_eight_concurrent_reads_do_not_wedge", |c| {
            concurrent_reads(c, 4, 8)
        }),
        (
            "an_uploaded_file_input_is_stored_by_multipart_upload",
            |c| upload_input_on_s3(c, true),
        ),
        (
            "an_upload_over_the_cap_is_refused_and_leaves_no_object",
            |c| upload_input_on_s3(c, false),
        ),
    ]
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
                println!(
                    "test {name} ... FAILED\n    the runtime wedged: no answer in {DEADLINE:?}"
                );
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
    // `exit`, not return: a wedged case's thread would keep the process alive.
    std::process::exit(if failed.is_empty() { 0 } else { 101 });
}
