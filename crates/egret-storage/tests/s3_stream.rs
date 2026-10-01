//! Streaming against a real S3 (MinIO). Gated on EGRET_TEST_S3_ENDPOINT, plus
//! EGRET_TEST_S3_KEY / _SECRET / _BUCKET (the `minioadmin` default is refused by the dev MinIO).
//! Each test says so loudly when it skips, so a skip is never mistaken for a pass.
use egret_storage::{ObjectStore, S3Store, StorageConfig, StorageError, PART_BYTES};
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};

const MIB: u64 = 1024 * 1024;

/// Serialises the tests in this file: `ru_maxrss` is process-wide, so a concurrent
/// test's buffers would be charged to the memory test.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

macro_rules! store_or_skip {
    () => {{
        let Some(c) = cfg() else {
            eprintln!("SKIPPED (EGRET_TEST_S3_ENDPOINT unset) -- this test did NOT run");
            return;
        };
        let s = S3Store::new(&c).await.unwrap();
        (c, s)
    }};
}

fn prefix() -> String {
    format!("w9a-t2-test/{}/", uuid::Uuid::new_v4())
}

fn pattern(i: u64) -> u8 {
    (i % 251) as u8
}

/// A generated body: `total` bytes of the counter pattern, optionally failing at `fail_at`.
struct Gen {
    pos: u64,
    total: u64,
    fail_at: Option<u64>,
}

impl AsyncRead for Gen {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.fail_at.is_some_and(|f| self.pos >= f) {
            return Poll::Ready(Err(std::io::Error::other("generator failed")));
        }
        let mut limit = self.total;
        if let Some(f) = self.fail_at {
            limit = limit.min(f);
        }
        let n = (limit - self.pos).min(buf.remaining() as u64) as usize;
        let start = self.pos;
        let dst = buf.initialize_unfilled_to(n);
        for (j, b) in dst.iter_mut().enumerate() {
            *b = pattern(start + j as u64);
        }
        buf.advance(n);
        self.pos += n as u64;
        Poll::Ready(Ok(()))
    }
}

/// Reads the object back and checks it against the pattern chunk by chunk.
async fn assert_reads_back(store: &S3Store, uri: &str, total: u64) {
    let mut r = store.get_stream(uri).await.unwrap();
    let mut chunk = vec![0u8; MIB as usize];
    let mut off = 0u64;
    loop {
        let n = r.read(&mut chunk).await.unwrap();
        if n == 0 {
            break;
        }
        for (j, b) in chunk[..n].iter().enumerate() {
            assert_eq!(
                *b,
                pattern(off + j as u64),
                "byte {} differs",
                off + j as u64
            );
        }
        off += n as u64;
    }
    assert_eq!(off, total);
}

fn max_rss_bytes() -> u64 {
    let mut u: libc::rusage = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) }, 0);
    let raw = u.ru_maxrss as u64;
    if cfg!(target_os = "macos") {
        raw
    } else {
        raw * 1024
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stream_larger_than_one_part_is_uploaded_in_parts_and_reads_back_byte_for_byte() {
    let _g = SERIAL.lock().await;
    let (c, store) = store_or_skip!();
    let uri = format!("s3://{}/{}big.bin", c.bucket, prefix());
    let total = 20 * MIB + 3;
    let mut body = Gen {
        pos: 0,
        total,
        fail_at: None,
    };
    let n = store
        .put_stream(&uri, &mut body, Some(total))
        .await
        .unwrap();
    assert_eq!(n, total);
    assert_eq!(store.head(&uri).await.unwrap(), Some(total));
    assert_reads_back(&store, &uri, total).await;
    store.delete(&uri).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn resident_memory_does_not_grow_with_the_file() {
    let _g = SERIAL.lock().await;
    let (c, store) = store_or_skip!();
    let uri = format!("s3://{}/{}huge.bin", c.bucket, prefix());
    let total = 512 * MIB;

    // Warm-up: pay the fixed costs (allocator arenas, connection, runtime threads) first, so
    // the measurement below is growth WITH THE FILE, not the process's first-use cost.
    let warm_uri = format!("s3://{}/{}warm.bin", c.bucket, prefix());
    let mut warm = Gen {
        pos: 0,
        total: 40 * MIB,
        fail_at: None,
    };
    store.put_stream(&warm_uri, &mut warm, None).await.unwrap();
    assert_reads_back(&store, &warm_uri, 40 * MIB).await;
    store.delete(&warm_uri).await.unwrap();

    let baseline = max_rss_bytes();
    let before = baseline;
    let mut body = Gen {
        pos: 0,
        total,
        fail_at: None,
    };
    assert_eq!(
        store
            .put_stream(&uri, &mut body, Some(total))
            .await
            .unwrap(),
        total
    );
    assert_reads_back(&store, &uri, total).await;
    let streamed = max_rss_bytes().saturating_sub(before);
    store.delete(&uri).await.unwrap();
    assert!(
        streamed < 128 * MIB,
        "streaming 512 MiB grew RSS by {} MiB",
        streamed / MIB
    );

    // Control: the Vec path (the whole file resident) must be visible to this instrument.
    let ctl_uri = format!("s3://{}/{}vec.bin", c.bucket, prefix());
    let v = vec![1u8; (512 * MIB) as usize];
    store.put(&ctl_uri, v).await.unwrap();
    // Against the pre-streaming baseline: RSS is a high-water mark, so growth is only
    // visible above whatever the streaming phase already reached.
    let grown = max_rss_bytes().saturating_sub(baseline);
    store.delete(&ctl_uri).await.unwrap();
    assert!(
        grown >= 256 * MIB,
        "control grew only {} MiB; instrument is blind",
        grown / MIB
    );
    eprintln!(
        "streamed growth {} MiB, control growth {} MiB",
        streamed / MIB,
        grown / MIB
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_body_aborts_the_multipart_upload() {
    let _g = SERIAL.lock().await;
    let (c, store) = store_or_skip!();
    let p = prefix();
    let key = format!("{p}fail.bin");
    let uri = format!("s3://{}/{key}", c.bucket);
    let mut body = Gen {
        pos: 0,
        total: 30 * MIB,
        fail_at: Some(9 * MIB),
    };
    let r = store.put_stream(&uri, &mut body, None).await;
    assert!(r.is_err(), "a failing body must fail the upload");

    let creds =
        aws_credential_types::Credentials::new(&c.access_key, &c.secret_key, None, None, "t");
    let shared = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new("us-east-1"))
        .credentials_provider(creds)
        .endpoint_url(c.endpoint.clone().unwrap())
        .load()
        .await;
    let raw = aws_sdk_s3::Client::from_conf(
        aws_sdk_s3::config::Builder::from(&shared)
            .force_path_style(true)
            .build(),
    );
    // Control: an upload that is NOT aborted is visible to this listing.
    let ctl_key = format!("{p}control.bin");
    let ctl = raw
        .create_multipart_upload()
        .bucket(&c.bucket)
        .key(&ctl_key)
        .send()
        .await
        .unwrap();
    // MinIO lists open uploads only for an exact-key prefix (a directory prefix returns
    // none even with the upload open), so both listings below use the exact key.
    let seen = raw
        .list_multipart_uploads()
        .bucket(&c.bucket)
        .prefix(&ctl_key)
        .send()
        .await
        .unwrap();
    assert_eq!(
        seen.uploads().len(),
        1,
        "instrument is blind to open uploads: {seen:?}"
    );
    raw.abort_multipart_upload()
        .bucket(&c.bucket)
        .key(&ctl_key)
        .upload_id(ctl.upload_id().unwrap())
        .send()
        .await
        .unwrap();

    let mut body = Gen {
        pos: 0,
        total: 30 * MIB,
        fail_at: Some(9 * MIB),
    };
    let r = store.put_stream(&uri, &mut body, None).await;
    assert!(r.is_err(), "a failing body must fail the upload");
    let ups = raw
        .list_multipart_uploads()
        .bucket(&c.bucket)
        .prefix(&key)
        .send()
        .await
        .unwrap();
    assert!(
        ups.uploads().is_empty(),
        "leftover multipart uploads: {:?}",
        ups.uploads()
    );
    assert_eq!(store.head(&uri).await.unwrap(), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_prefix_deletes_more_than_one_list_page() {
    let _g = SERIAL.lock().await;
    let (c, store) = store_or_skip!();
    let p = prefix();
    let mut set = tokio::task::JoinSet::new();
    for i in 0..1001 {
        let (s, uri) = (store.clone(), format!("s3://{}/{p}d/{i}", c.bucket));
        set.spawn(async move { s.put(&uri, vec![0]).await });
        if set.len() >= 32 {
            set.join_next().await.unwrap().unwrap().unwrap();
        }
    }
    while let Some(r) = set.join_next().await {
        r.unwrap().unwrap();
    }
    // A sibling prefix that must survive.
    let keep = format!("s3://{}/{p}d2/keep", c.bucket);
    store.put(&keep, vec![1]).await.unwrap();

    let n = store
        .delete_prefix(&format!("s3://{}/{p}d/", c.bucket))
        .await
        .unwrap();
    assert_eq!(n, 1001);
    let left = store.list(&format!("{p}d/"), None).await.unwrap();
    assert!(left.objects.is_empty());
    assert_eq!(store.head(&keep).await.unwrap(), Some(1));
    store.delete(&keep).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_small_body_is_one_put() {
    let _g = SERIAL.lock().await;
    let (c, store) = store_or_skip!();
    let uri = format!("s3://{}/{}small", c.bucket, prefix());
    let mut body: &[u8] = b"abc";
    assert_eq!(store.put_stream(&uri, &mut body, None).await.unwrap(), 3);
    assert_eq!(store.head(&uri).await.unwrap(), Some(3));
    assert_eq!(store.get(&uri).await.unwrap(), b"abc");
    store.delete(&uri).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn head_of_an_absent_object_is_none() {
    let _g = SERIAL.lock().await;
    let (c, store) = store_or_skip!();
    let uri = format!("s3://{}/{}absent", c.bucket, prefix());
    assert_eq!(store.head(&uri).await.unwrap(), None);
    assert!(matches!(
        store.get_stream(&uri).await,
        Err(StorageError::NotFound(_))
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_credential_error_names_the_uri_and_not_the_secret() {
    let _g = SERIAL.lock().await;
    let Some(mut c) = cfg() else {
        eprintln!("SKIPPED (EGRET_TEST_S3_ENDPOINT unset) -- this test did NOT run");
        return;
    };
    c.secret_key = "w9a-secret-probe".into();
    let store = S3Store::new(&c).await.unwrap();
    let uri = format!("s3://{}/{}x", c.bucket, prefix());
    let msg = match store.get_stream(&uri).await {
        Ok(_) => panic!("a refused credential must be an error"),
        Err(e) => e.to_string(),
    };
    assert!(msg.contains(&uri), "error must name the uri: {msg}");
    assert!(
        !msg.contains("w9a-secret-probe"),
        "error leaks the secret: {msg}"
    );
    assert!(
        !format!("{c:?}").contains("w9a-secret-probe"),
        "config Debug leaks the secret"
    );
}

#[test]
fn part_size_is_eight_mib() {
    assert_eq!(PART_BYTES, 8 * 1024 * 1024);
}

/// Needs no network: a non-`s3://` uri is refused by S3Store itself, so the answer is
/// not the trait default's `Unsupported`.
#[tokio::test]
async fn s3_store_overrides_all_four_streaming_methods() {
    let c = StorageConfig {
        endpoint: Some("http://127.0.0.1:1".into()),
        region: "us-east-1".into(),
        bucket: "b".into(),
        access_key: "a".into(),
        secret_key: "s".into(),
        path_style: true,
    };
    let s = S3Store::new(&c).await.unwrap();
    let bad = "not-an-s3-uri";
    let not_default = |r: Option<StorageError>| !matches!(r, Some(StorageError::Unsupported(_)));
    assert!(not_default(
        s.put_stream(bad, &mut &b""[..], None).await.err()
    ));
    assert!(not_default(s.get_stream(bad).await.err()));
    assert!(not_default(s.head(bad).await.err()));
    assert!(not_default(s.delete_prefix(bad).await.err()));
}
