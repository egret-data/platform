//! The render snapshot: every `kestra:///…` string a render can reach,
//! resolved before it starts (W9a spec §7.2, as the user ruled 2026-10-01).
//!
//! # What the snapshot holds, and the rule it implements
//!
//! [`Snapshot::take`] reads every `kestra:///` URI out of the flow **source**
//! and out of every **JSON value** of the render context (inputs, the outputs
//! of every terminated task, `execution`, `item`) and resolves all of them at
//! once through [`super::resolve_readable_many`] -- the one readability
//! decision -- on the caller's connection. Inside `settle` that is the
//! transaction that has just indexed the finished task's files, which a pool
//! would not see.
//!
//! That covers every URI a template can reach without building it from pieces.
//! **A URI assembled at render (`'kestra:///…' ~ execution.id ~ '/x'`) is not
//! looked up a second time**: it is an error naming the URI and saying why.
//! There is no database on the render thread to look it up with.
//!
//! # Reading
//!
//! `read()` blocks the render thread on the runtime handle captured by
//! [`Snapshot::take`]; the render thread is a plain OS thread
//! (`runners::env::render_str`), and the tokio worker that spawned it waits in
//! `join`. That needs the multi-thread runtime (global constraints).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, LazyLock, Mutex};

use egret_core::Result;
use regex::Regex;
use sqlx::PgConnection;
use tokio::io::AsyncReadExt;
use uuid::Uuid;

use super::{decode_utf8, readable_executions, resolve_readable_many, uri};
use super::{FileStore, IndexedFile, Unreadable};
use crate::runners::files::{FileMeta, FileScope, FileUnavailable, RenderFiles};

/// A `kestra:///` URI in text: the scheme, then the whole run of characters
/// that are not whitespace, a quote or a backslash (a backslash is never legal
/// in a URI, `uri::parse`, and is how a YAML double-quoted string escapes the
/// quote that ends one).
static URI_IN_TEXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"kestra:///[^\s'"\\]*"#).expect("a valid regex"));

/// The readable files a render may name, resolved before it. See the module doc.
pub struct Snapshot {
    files: BTreeMap<String, std::result::Result<IndexedFile, Unreadable>>,
    store: Arc<FileStore>,
    workspace: Uuid,
    handle: tokio::runtime::Handle,
    task_prefix: Option<String>,
    /// The first store outage a read met (spec §11); see
    /// [`RenderFiles::infrastructure_failure`].
    outage: Mutex<Option<String>>,
}

impl Snapshot {
    /// Every `kestra:///…` string in `texts` (regex, whole run of non-space, non-quote chars)
    /// resolved at once through resolve_readable_many on `conn`.
    ///
    /// `json` is walked: a string that starts with `kestra:///` is taken
    /// whole, and every other string, at any depth, is searched the same way.
    /// With no URI anywhere the database is not touched.
    pub async fn take(
        conn: &mut PgConnection,
        store: Arc<FileStore>,
        workspace: Uuid,
        execution: Uuid,
        texts: &[&str],
        json: &[&serde_json::Value],
        task_prefix: Option<String>,
    ) -> Result<Self> {
        let mut found = BTreeSet::new();
        for text in texts {
            collect(text, &mut found);
        }
        for value in json {
            walk(value, &mut found);
        }
        let files = if found.is_empty() {
            BTreeMap::new()
        } else {
            let readable = readable_executions(conn, workspace, execution).await?;
            let uris: Vec<String> = found.into_iter().collect();
            resolve_readable_many(conn, workspace, &readable, &uris).await?
        };
        Ok(Self {
            files,
            store,
            workspace,
            handle: tokio::runtime::Handle::current(),
            task_prefix,
            outage: Mutex::new(None),
        })
    }

    /// The indexed file, or why it is not readable here, naming the URI --
    /// what staging (T5) and the bridge (T9) call.
    pub fn resolve(&self, uri: &str) -> std::result::Result<&IndexedFile, String> {
        match self.files.get(uri) {
            Some(Ok(file)) => Ok(file),
            Some(Err(why)) => Err(why.to_string()),
            None => Err(not_in_snapshot(uri)),
        }
    }

    pub fn into_scope(self) -> FileScope {
        FileScope(Arc::new(self))
    }

    /// For tests: a snapshot over a given resolution, no database.
    #[cfg(any(test, feature = "test-util"))]
    pub fn from_parts(
        files: BTreeMap<String, std::result::Result<IndexedFile, Unreadable>>,
        store: Arc<FileStore>,
        workspace: Uuid,
        task_prefix: Option<String>,
    ) -> Self {
        Self {
            files,
            store,
            workspace,
            handle: tokio::runtime::Handle::current(),
            task_prefix,
            outage: Mutex::new(None),
        }
    }

    /// Record an outage (the first one wins) and hand back its message.
    fn outage(&self, message: String) -> String {
        let mut held = self.outage.lock().unwrap_or_else(|p| p.into_inner());
        held.get_or_insert_with(|| message.clone());
        message
    }
}

/// The answer for a URI the snapshot does not hold.
///
/// A URI that does not parse gets [`Unreadable::Malformed`]'s own text from
/// the same `uri::parse` that `resolve_readable_many` runs first -- it can
/// only refine a refusal, never make anything readable. Everything else was
/// assembled at render, which the user's ruling refuses.
fn not_in_snapshot(uri: &str) -> String {
    match uri::parse(uri) {
        Err(reason) => Unreadable::Malformed {
            uri: uri.to_string(),
            reason,
        }
        .to_string(),
        Ok(_) => format!(
            "{uri} was not looked up: a file function reads a kestra:// URI only when it appears \
             whole in the flow source or in a value of the render context (an input, a task's \
             outputs, the execution, the loop item); a URI assembled while rendering, by `~` or a \
             filter, is not looked up"
        ),
    }
}

impl RenderFiles for Snapshot {
    fn lookup(&self, uri: &str) -> std::result::Result<FileMeta, FileUnavailable> {
        match self.files.get(uri) {
            Some(Ok(file)) => Ok(FileMeta { size: file.size }),
            Some(Err(missing @ Unreadable::Missing { .. })) => {
                Err(FileUnavailable::Missing(missing.to_string()))
            }
            Some(Err(why)) => Err(FileUnavailable::Refused(why.to_string())),
            None => Err(FileUnavailable::Refused(not_in_snapshot(uri))),
        }
    }

    fn read_utf8(&self, uri: &str) -> std::result::Result<String, String> {
        let file = self.resolve(uri)?;
        let object = self.store.object_uri(self.workspace, uri)?;
        let cap = self.store.caps.max_read_bytes;
        let over_cap = || {
            format!(
                "{uri} is larger than workflow.storage.max_read_bytes ({cap} bytes): read() \
                 returns the whole file as a string, so it reads only files up to that size"
            )
        };
        // The index knows the size: refuse without a download (fix round 1,
        // M7). The `cap + 1` guard below stays for an object larger than its row.
        if file.size > cap {
            return Err(over_cap());
        }
        // `block_on` panics on a thread that is inside a runtime; the render
        // thread never is, and a caller that rendered on a tokio worker gets
        // an error rather than a panic.
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(format!(
                "{uri} cannot be read on this thread: read() must render on egret's render \
                 thread, not on a runtime worker"
            ));
        }
        // C1: on a current-thread runtime the one thread that drives the IO is
        // the one waiting for this render, so a store read would never finish
        // (it does on MemStore, which proves nothing about S3).
        if self.handle.runtime_flavor() != tokio::runtime::RuntimeFlavor::MultiThread {
            return Err(format!(
                "{uri} cannot be read: read() needs the multi-thread runtime, and this render \
                 runs on a current-thread one, whose only thread is waiting for it"
            ));
        }
        let bytes = self.handle.block_on(async {
            let stream = match self.store.store.get_stream(&object).await {
                Ok(stream) => stream,
                // The index has a row and the object is gone: a prune race or
                // an orphaned row. The author's error, naming the URI.
                Err(egret_storage::StorageError::NotFound(_)) => {
                    return Err(format!(
                        "{uri} is indexed but its object is missing from storage (it was \
                         pruned, or never finished uploading)"
                    ));
                }
                // Anything else is the store being unreachable: egret's outage.
                Err(e) => {
                    return Err(self.outage(format!(
                        "{uri} could not be read: internal storage is unavailable: {e}"
                    )))
                }
            };
            let mut bytes = Vec::new();
            // At most one byte past the cap: that byte is how a file over it is
            // told from one exactly at it, without reading the rest.
            stream
                .take(cap.saturating_add(1))
                .read_to_end(&mut bytes)
                .await
                // A mid-body error from `get_stream` is a bare io::Error that
                // does not name the URI (Task 2), and it is an outage too.
                .map_err(|e| {
                    self.outage(format!(
                        "{uri} could not be read: internal storage is unavailable: {e}"
                    ))
                })?;
            Ok::<_, String>(bytes)
        })?;
        if bytes.len() as u64 > cap {
            return Err(over_cap());
        }
        decode_utf8(uri, bytes).map_err(|e| e.to_string())
    }

    fn infrastructure_failure(&self) -> Option<String> {
        self.outage
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn task_prefix(&self) -> Option<&str> {
        self.task_prefix.as_deref()
    }
}

/// What the four functions answer when this process has no `[storage]`.
struct Unconfigured;

fn unconfigured_message(uri: &str) -> String {
    format!(
        "{uri} cannot be read: egret has no internal storage configured (the `[storage]` \
         section of its configuration), so read, fileSize, fileExists and isFileEmpty are \
         unavailable"
    )
}

impl RenderFiles for Unconfigured {
    fn lookup(&self, uri: &str) -> std::result::Result<FileMeta, FileUnavailable> {
        Err(FileUnavailable::Refused(unconfigured_message(uri)))
    }
    fn read_utf8(&self, uri: &str) -> std::result::Result<String, String> {
        Err(unconfigured_message(uri))
    }
    fn task_prefix(&self) -> Option<&str> {
        None
    }
}

/// The scope a render inside a running execution gets: a [`Snapshot`] over
/// the installed store, or -- with [`super::current`] `None` -- a scope whose
/// every answer names `[storage]`. A wiring point (the module doc of
/// `storage`): `worker::claim` calls this and nothing else does.
pub async fn scope_for(
    conn: &mut PgConnection,
    store: Option<Arc<FileStore>>,
    workspace: Uuid,
    execution: Uuid,
    texts: &[&str],
    json: &[&serde_json::Value],
    task_prefix: Option<String>,
) -> Result<FileScope> {
    match store {
        None => Ok(FileScope(Arc::new(Unconfigured))),
        Some(store) => {
            Ok(
                Snapshot::take(conn, store, workspace, execution, texts, json, task_prefix)
                    .await?
                    .into_scope(),
            )
        }
    }
}

fn collect(text: &str, found: &mut BTreeSet<String>) {
    for m in URI_IN_TEXT.find_iter(text) {
        found.insert(m.as_str().to_string());
    }
}

fn walk(value: &serde_json::Value, found: &mut BTreeSet<String>) {
    match value {
        // A string that IS a URI is taken whole (fix round 1, I3): `uri::parse`
        // accepts `'` and spaces, which end the regex's run in source text.
        serde_json::Value::String(s) if s.starts_with(uri::SCHEME_PREFIX) => {
            found.insert(s.clone());
        }
        serde_json::Value::String(s) => collect(s, found),
        serde_json::Value::Array(items) => items.iter().for_each(|v| walk(v, found)),
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                collect(k, found);
                walk(v, found);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runners::{render_str, RenderContext};
    use crate::storage::Caps;

    const WS: Uuid = Uuid::from_u128(0x5eed);

    fn uri_of(name: &str) -> String {
        format!(
            "kestra:///w9a/t4/f/executions/{}/tasks/w/{}/{name}",
            Uuid::from_u128(1),
            Uuid::from_u128(2)
        )
    }

    fn store(max_read_bytes: u64) -> Arc<FileStore> {
        Arc::new(FileStore {
            store: Arc::new(egret_storage::MemStore::new()),
            bucket: "egret-test".into(),
            prefix: "workflow/".into(),
            caps: Caps {
                max_read_bytes,
                ..Caps::default()
            },
            retention_days: 30,
        })
    }

    /// A readable file holding `bytes`, written to `fs`'s store.
    async fn put(
        fs: &FileStore,
        name: &str,
        bytes: &[u8],
    ) -> (String, std::result::Result<IndexedFile, Unreadable>) {
        let uri = uri_of(name);
        fs.store
            .put(&fs.object_uri(WS, &uri).unwrap(), bytes.to_vec())
            .await
            .unwrap();
        let file = IndexedFile {
            uri: uri.clone(),
            execution_id: Uuid::from_u128(1),
            task_run_id: Some(Uuid::from_u128(2)),
            size: bytes.len() as u64,
        };
        (uri, Ok(file))
    }

    fn render(template: &str, snapshot: Snapshot) -> std::result::Result<String, String> {
        let cx = RenderContext::new().with_files(snapshot.into_scope());
        render_str(template, &cx).map_err(|e| e.to_string())
    }

    #[test]
    fn the_regex_takes_the_whole_uri_and_stops_at_a_quote_space_or_backslash() {
        let mut found = BTreeSet::new();
        collect(
            "a: \"{{ read('kestra:///a/b/c.txt') }}\"\nb: kestra:///x/y z\nc: \"{{ read(\\\"kestra:///q/r\\\") }}\"",
            &mut found,
        );
        walk(
            &serde_json::json!({"o": [{"uri": "kestra:///j/k%20l+m.txt"}], "n": 1}),
            &mut found,
        );
        assert_eq!(
            found.into_iter().collect::<Vec<_>>(),
            [
                "kestra:///a/b/c.txt",
                "kestra:///j/k%20l+m.txt",
                "kestra:///q/r",
                "kestra:///x/y"
            ]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn read_over_the_cap_is_an_error_naming_the_cap_and_at_the_cap_is_not() {
        let fs = store(4);
        let at = put(&fs, "at.txt", b"abcd").await;
        let over = put(&fs, "over.txt", b"abcde").await;
        let (at_uri, over_uri) = (at.0.clone(), over.0.clone());
        let files = BTreeMap::from([at, over]);
        let snap = || Snapshot::from_parts(files.clone(), fs.clone(), WS, None);

        assert_eq!(
            render(&format!("{{{{ read('{at_uri}') }}}}"), snap()),
            Ok("abcd".into())
        );
        let err = render(&format!("{{{{ read('{over_uri}') }}}}"), snap()).unwrap_err();
        assert!(
            err.contains(&over_uri) && err.contains("max_read_bytes (4 bytes)"),
            "{err}"
        );
        // fileSize of the same file is not capped: only read() holds the bytes.
        assert_eq!(
            render(&format!("{{{{ fileSize('{over_uri}') }}}}"), snap()),
            Ok("5".into())
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn read_of_non_utf8_is_an_error_naming_the_uri() {
        // `w9a-storage.json` `file_functions.not_utf8`: the bytes ff fe 00 81.
        let fs = store(1 << 20);
        let bad = put(&fs, "bin.dat", &[0xff, 0xfe, 0x00, 0x81]).await;
        let good = put(&fs, "ok.txt", "café".as_bytes()).await;
        let (bad_uri, good_uri) = (bad.0.clone(), good.0.clone());
        let files = BTreeMap::from([bad, good]);
        let snap = || Snapshot::from_parts(files.clone(), fs.clone(), WS, None);

        let err = render(&format!("{{{{ read('{bad_uri}') }}}}"), snap()).unwrap_err();
        assert!(
            err.contains(&bad_uri) && err.contains("is not UTF-8"),
            "{err}"
        );
        assert_eq!(
            render(&format!("{{{{ read('{good_uri}') }}}}"), snap()),
            Ok("café".into())
        );
        // The other three answer: NotUtf8 is a property of the bytes, not readability.
        for (call, want) in [
            ("fileSize", "4"),
            ("fileExists", "true"),
            ("isFileEmpty", "false"),
        ] {
            assert_eq!(
                render(&format!("{{{{ {call}('{bad_uri}') }}}}"), snap()),
                Ok(want.into()),
                "{call}"
            );
        }
    }

    /// An object store that fails the way a real one can: the body breaks
    /// after a few bytes (a bare io::Error naming nothing, as `S3Store`'s
    /// can mid-download), the store is unreachable, the object is gone, or
    /// (for the deadlock test) every read waits on the timer driver first.
    enum Faulty {
        MidBody,
        Outage,
        Gone,
        SlowThen(Vec<u8>),
    }

    #[async_trait::async_trait]
    impl egret_storage::ObjectStore for Faulty {
        async fn put(
            &self,
            _: &str,
            _: Vec<u8>,
        ) -> std::result::Result<(), egret_storage::StorageError> {
            unreachable!()
        }
        async fn get(&self, _: &str) -> std::result::Result<Vec<u8>, egret_storage::StorageError> {
            unreachable!()
        }
        async fn delete(&self, _: &str) -> std::result::Result<(), egret_storage::StorageError> {
            unreachable!()
        }
        async fn copy(
            &self,
            _: &str,
            _: &str,
        ) -> std::result::Result<(), egret_storage::StorageError> {
            unreachable!()
        }
        async fn list(
            &self,
            _: &str,
            _: Option<&str>,
        ) -> std::result::Result<egret_storage::Listing, egret_storage::StorageError> {
            unreachable!()
        }
        async fn get_stream(
            &self,
            uri: &str,
        ) -> std::result::Result<
            Box<dyn tokio::io::AsyncBufRead + Send + Unpin>,
            egret_storage::StorageError,
        > {
            match self {
                Self::MidBody => Ok(Box::new(tokio::io::BufReader::new(HalfThenReset {
                    sent: false,
                }))),
                Self::Outage => Err(egret_storage::StorageError::S3(format!(
                    "{uri}: dispatch failure: connection refused"
                ))),
                Self::Gone => Err(egret_storage::StorageError::NotFound(uri.to_string())),
                Self::SlowThen(bytes) => {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    Ok(Box::new(std::io::Cursor::new(bytes.clone())))
                }
            }
        }
    }

    fn faulty(mode: Faulty) -> Arc<FileStore> {
        Arc::new(FileStore {
            store: Arc::new(mode),
            bucket: "egret-test".into(),
            prefix: "workflow/".into(),
            caps: Caps::default(),
            retention_days: 30,
        })
    }

    fn indexed(
        uri: &str,
        size: u64,
    ) -> BTreeMap<String, std::result::Result<IndexedFile, Unreadable>> {
        BTreeMap::from([(
            uri.to_string(),
            Ok(IndexedFile {
                uri: uri.to_string(),
                execution_id: Uuid::from_u128(1),
                task_run_id: Some(Uuid::from_u128(2)),
                size,
            }),
        )])
    }

    /// Render `template` over `snapshot`, and say what the scope recorded.
    fn render_recording(
        template: &str,
        snapshot: Snapshot,
    ) -> (std::result::Result<String, String>, Option<String>) {
        let scope = snapshot.into_scope();
        let cx = RenderContext::new().with_files(scope.clone());
        let out = render_str(template, &cx).map_err(|e| e.to_string());
        (out, scope.0.infrastructure_failure())
    }

    /// Three bytes, then a bare `io::Error`.
    struct HalfThenReset {
        sent: bool,
    }

    impl tokio::io::AsyncRead for HalfThenReset {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            if self.sent {
                return std::task::Poll::Ready(Err(std::io::Error::other("connection reset")));
            }
            self.sent = true;
            buf.put_slice(b"abc");
            std::task::Poll::Ready(Ok(()))
        }
    }

    /// Spec §11: an unreachable store is egret's outage, not the author's.
    /// The render still fails naming the URI, and the scope RECORDS it, which
    /// is what `settle` and the worker turn into an `Err`.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_store_error_mid_body_names_the_uri_and_is_recorded_as_an_outage() {
        let uri = uri_of("x.txt");
        for mode in [Faulty::MidBody, Faulty::Outage] {
            let snap = Snapshot::from_parts(indexed(&uri, 9), faulty(mode), WS, None);
            let (out, failure) = render_recording(&format!("{{{{ read('{uri}') }}}}"), snap);
            let err = out.unwrap_err();
            assert!(err.contains(&uri), "{err}");
            let failure = failure.expect("an outage is recorded");
            assert!(failure.contains(&uri), "{failure}");
        }
    }

    /// An index row whose object is gone (a prune race, an orphaned row) is
    /// the author's error naming the URI, and NOT an outage.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_indexed_file_whose_object_is_gone_is_the_authors_error_not_an_outage() {
        let uri = uri_of("gone.txt");
        let snap = Snapshot::from_parts(indexed(&uri, 9), faulty(Faulty::Gone), WS, None);
        let (out, failure) = render_recording(&format!("{{{{ read('{uri}') }}}}"), snap);
        let err = out.unwrap_err();
        assert!(
            err.contains(&uri) && err.contains("missing from storage"),
            "{err}"
        );
        assert_eq!(failure, None);
    }

    /// M7: the index already knows the size, so a file over the cap is refused
    /// without a download -- the object here does not even exist, and the
    /// answer is still the cap.
    #[tokio::test(flavor = "multi_thread")]
    async fn read_refuses_from_the_indexed_size_before_downloading() {
        let uri = uri_of("big.txt");
        let snap = Snapshot::from_parts(indexed(&uri, 5), store(4), WS, None);
        let err = render(&format!("{{{{ read('{uri}') }}}}"), snap).unwrap_err();
        assert!(
            err.contains(&uri) && err.contains("max_read_bytes (4 bytes)"),
            "{err}"
        );
    }

    /// The `cap + 1` stream guard stays: an object larger than its index row
    /// says is refused too.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_stream_guard_refuses_an_object_larger_than_its_index_row() {
        let fs = store(4);
        let (uri, _) = put(&fs, "lies.txt", b"abcde").await;
        let snap = Snapshot::from_parts(indexed(&uri, 4), fs, WS, None);
        let err = render(&format!("{{{{ read('{uri}') }}}}"), snap).unwrap_err();
        assert!(
            err.contains(&uri) && err.contains("max_read_bytes (4 bytes)"),
            "{err}"
        );
    }

    /// I3: a JSON string that IS a URI is taken whole, `'` and spaces and all
    /// -- `uri::parse` accepts both, and a FILE input's URI is not encoded.
    #[test]
    fn a_json_string_that_is_a_whole_uri_is_taken_whole() {
        let mut found = BTreeSet::new();
        let quoted = uri_of("it's a.csv");
        walk(
            &serde_json::json!({"outputFiles": {"it's a.csv": quoted.clone()}, "note": "see kestra:///x/y z"}),
            &mut found,
        );
        assert!(found.contains(&quoted), "{found:?}");
        assert!(
            found.contains("kestra:///x/y"),
            "a URI inside prose still goes through the regex: {found:?}"
        );
    }

    /// C1: `read()` blocks the render thread on the runtime while a worker
    /// waits in `join`. With every worker parked there and nothing driving
    /// the timer/IO driver, the runtime wedged for good. One worker, four
    /// concurrent renders, each read waiting on the timer driver; a std-thread
    /// watchdog so a regression FAILS instead of hanging the suite.
    #[test]
    fn concurrent_reads_on_one_worker_do_not_wedge_the_runtime() {
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .unwrap();
            let out = rt.block_on(async {
                let uri = uri_of("slow.txt");
                let fs = faulty(Faulty::SlowThen(b"slow".to_vec()));
                let mut renders = Vec::new();
                for _ in 0..4 {
                    let (uri, fs) = (uri.clone(), fs.clone());
                    renders.push(tokio::spawn(async move {
                        let snap = Snapshot::from_parts(indexed(&uri, 4), fs, WS, None);
                        render(&format!("{{{{ read('{uri}') }}}}"), snap)
                    }));
                }
                let mut all = Vec::new();
                for r in renders {
                    all.push(r.await.unwrap());
                }
                all
            });
            let _ = done.send(out);
        });
        let out = finished
            .recv_timeout(std::time::Duration::from_secs(20))
            .expect("the runtime wedged: four renders on one worker did not finish in 20s");
        assert_eq!(out, vec![Ok("slow".to_string()); 4]);
    }

    /// A current-thread runtime has no other thread to drive the store's IO
    /// while its one thread waits for the render, so `read()` refuses there,
    /// saying why, rather than hanging on S3 (and passing on MemStore).
    #[tokio::test(flavor = "current_thread")]
    async fn read_on_a_current_thread_runtime_is_refused_saying_why() {
        let fs = store(1 << 20);
        let (uri, file) = put(&fs, "ct.txt", b"x").await;
        let snap = Snapshot::from_parts(BTreeMap::from([(uri.clone(), file)]), fs, WS, None);
        let err = render(&format!("{{{{ read('{uri}') }}}}"), snap).unwrap_err();
        assert!(
            err.contains(&uri) && err.contains("multi-thread runtime"),
            "{err}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_uri_the_snapshot_does_not_hold_is_refused_naming_it_and_saying_why() {
        let snap = || Snapshot::from_parts(BTreeMap::new(), store(1 << 20), WS, None);
        let assembled = uri_of("late.txt");
        for call in ["read", "fileSize", "fileExists", "isFileEmpty"] {
            let err = render(&format!("{{{{ {call}('{assembled}') }}}}"), snap()).unwrap_err();
            assert!(
                err.contains(&assembled) && err.contains("assembled while rendering"),
                "{call}: {err}"
            );
            let err =
                render(&format!("{{{{ {call}('s3://bucket/key.txt') }}}}"), snap()).unwrap_err();
            assert!(
                err.contains("s3://bucket/key.txt is not a storage URI egret can read"),
                "{call}: {err}"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn without_storage_the_four_refuse_naming_the_storage_section() {
        let cx = RenderContext::new().with_files(FileScope(Arc::new(Unconfigured)));
        for call in ["read", "fileSize", "fileExists", "isFileEmpty"] {
            let err = render_str(&format!("{{{{ {call}('{}') }}}}", uri_of("a")), &cx)
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("[storage]") && err.contains(&uri_of("a")),
                "{call}: {err}"
            );
        }
    }
}
