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
use std::sync::{Arc, LazyLock};

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
}

impl Snapshot {
    /// Every `kestra:///…` string in `texts` (regex, whole run of non-space, non-quote chars)
    /// resolved at once through resolve_readable_many on `conn`.
    ///
    /// `json` is walked: every string in it, at any depth, is searched the same
    /// way. With no URI anywhere the database is not touched.
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

    /// For unit tests: a snapshot over a given resolution, no database.
    #[cfg(test)]
    pub(crate) fn from_parts(
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
        }
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
        self.resolve(uri)?;
        let object = self.store.object_uri(self.workspace, uri)?;
        let cap = self.store.caps.max_read_bytes;
        // `block_on` panics on a thread that is inside a runtime; the render
        // thread never is, and a caller that rendered on a tokio worker gets
        // an error rather than a panic.
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(format!(
                "{uri} cannot be read on this thread: read() must render on egret's render \
                 thread, not on a runtime worker"
            ));
        }
        let bytes = self.handle.block_on(async {
            let stream = self
                .store
                .store
                .get_stream(&object)
                .await
                .map_err(|e| format!("{uri} could not be read from storage: {e}"))?;
            let mut bytes = Vec::new();
            // At most one byte past the cap: that byte is how a file over it is
            // told from one exactly at it, without reading the rest.
            stream
                .take(cap.saturating_add(1))
                .read_to_end(&mut bytes)
                .await
                // A mid-body error from `get_stream` is a bare io::Error that
                // does not name the URI (Task 2).
                .map_err(|e| format!("{uri} could not be read from storage: {e}"))?;
            Ok::<_, String>(bytes)
        })?;
        if bytes.len() as u64 > cap {
            return Err(format!(
                "{uri} is larger than workflow.storage.max_read_bytes ({cap} bytes): read() \
                 returns the whole file as a string, so it reads only files up to that size"
            ));
        }
        decode_utf8(uri, bytes).map_err(|e| e.to_string())
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

    /// An object store whose body fails after a few bytes, as `S3Store`'s
    /// can mid-download: a bare io::Error that does not name anything.
    struct Breaks;

    #[async_trait::async_trait]
    impl egret_storage::ObjectStore for Breaks {
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
            _: &str,
        ) -> std::result::Result<
            Box<dyn tokio::io::AsyncBufRead + Send + Unpin>,
            egret_storage::StorageError,
        > {
            let broken = tokio::io::BufReader::new(HalfThenReset { sent: false });
            Ok(Box::new(broken))
        }
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

    #[tokio::test(flavor = "multi_thread")]
    async fn a_store_error_mid_body_names_the_uri() {
        let fs = Arc::new(FileStore {
            store: Arc::new(Breaks),
            bucket: "egret-test".into(),
            prefix: "workflow/".into(),
            caps: Caps::default(),
            retention_days: 30,
        });
        let uri = uri_of("x.txt");
        let file = IndexedFile {
            uri: uri.clone(),
            execution_id: Uuid::from_u128(1),
            task_run_id: Some(Uuid::from_u128(2)),
            size: 9,
        };
        let snap = Snapshot::from_parts(BTreeMap::from([(uri.clone(), Ok(file))]), fs, WS, None);
        let err = render(&format!("{{{{ read('{uri}') }}}}"), snap).unwrap_err();
        assert!(
            err.contains(&uri) && err.contains("connection reset"),
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
