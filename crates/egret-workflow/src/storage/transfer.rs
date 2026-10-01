//! Files INTO a task container, as a **streamed** POSIX ustar archive (W9a
//! spec §5): `docker cp -` reads it from its stdin, and on Kubernetes the
//! `files-in` helper's `tar -x` reads it from the exec's.
//!
//! # Streamed, and what "streamed" is measured as
//!
//! A stored input (`inputFiles: {name: kestra://…}`) is copied from the
//! object store to `out` as it is read: at most [`CHUNK`] bytes of it are
//! handed to `out` per write, one 512-byte header or padding block besides,
//! and nothing else is held -- **no file is ever held whole in egret's
//! memory** (global constraints). The one buffer is the store's own read
//! buffer. `a_stored_input_is_copied_from_the_store_without_being_collected`
//! measures the bytes read from the store and not yet written to `out`.
//!
//! # The archive, byte for byte
//!
//! W5a's in-memory packer (`worker::k8s::transfer::pack`) wrote every parent
//! directory first, then every file in name order; this writes the same bytes
//! for the same contents (`write_tar_is_byte_identical_to_the_old_packer_for_contents`).
//! The header, octal and long-name code moved here from that module.
//!
//! # Whose error
//!
//! [`TarError`] says, because the runners must split it (spec §11):
//! [`TarError::Author`] fails the task (a name ustar cannot carry, two names
//! that collide, an object missing behind its index row or not the size the
//! index says); [`TarError::Store`] is the store being unreachable, a worker
//! error; [`TarError::Write`] is the receiving process going away, which the
//! runner classifies the way it classifies that process's own failure.

use std::collections::{BTreeMap, BTreeSet};

use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

use super::FileStore;
use crate::worker::commands::{relative_problem, StoredInput};

pub(crate) const BLOCK: usize = 512;

/// The most bytes of a stored object handed to `out` in one write.
pub(crate) const CHUNK: usize = 64 * 1024;

const ZEROS: [u8; BLOCK] = [0u8; BLOCK];

/// The largest size ustar's 11 octal digits hold (8 GiB - 1).
const MAX_USTAR_SIZE: u64 = 0o77_777_777_777;

/// What one archive entry's bytes come from.
#[derive(Clone, Copy, Debug)]
pub enum TarSource<'a> {
    /// Inline contents (W5a's `inputFiles` form, a handler's `main.py`, the
    /// plugin host's `task.json`).
    Contents(&'a str),
    /// A file in internal storage, read from the store as it is written.
    Stored(&'a StoredInput),
}

/// Why an archive could not be written, split by whose problem it is.
#[derive(Debug, thiserror::Error)]
pub enum TarError {
    /// The author's: the task fails with this message.
    #[error("{0}")]
    Author(String),
    /// Internal storage is unreachable: egret's outage, a worker error.
    #[error("{0}")]
    Store(String),
    /// Writing to `out` failed -- the process reading the archive went away.
    #[error("the input files could not be written to the container: {0}")]
    Write(#[source] std::io::Error),
}

/// A ustar stream of every file (parents first), written as it is read: at
/// most one 512-byte block plus the store's read buffer in memory.
///
/// Every name is checked before the first byte is written, so an author's
/// error never leaves half an archive behind it.
pub async fn write_tar(
    out: &mut (dyn AsyncWrite + Send + Unpin),
    files: &BTreeMap<String, TarSource<'_>>,
    store: Option<&FileStore>,
) -> Result<(), TarError> {
    let dirs = plan(files).map_err(TarError::Author)?;
    for d in &dirs {
        let h = header(d, 0, b'5', 0o755).map_err(TarError::Author)?;
        out.write_all(&h).await.map_err(TarError::Write)?;
    }
    for (name, source) in files {
        match source {
            TarSource::Contents(contents) => {
                let bytes = contents.as_bytes();
                let h = header(name, bytes.len() as u64, b'0', 0o644).map_err(TarError::Author)?;
                out.write_all(&h).await.map_err(TarError::Write)?;
                out.write_all(bytes).await.map_err(TarError::Write)?;
                pad(out, bytes.len() as u64).await?;
            }
            TarSource::Stored(stored) => copy_stored(out, name, stored, store).await?,
        }
    }
    out.write_all(&ZEROS).await.map_err(TarError::Write)?;
    out.write_all(&ZEROS).await.map_err(TarError::Write)?;
    out.flush().await.map_err(TarError::Write)
}

/// One stored input: its header, then its object, chunk by chunk, checked
/// against the size its index row holds.
async fn copy_stored(
    out: &mut (dyn AsyncWrite + Send + Unpin),
    name: &str,
    stored: &StoredInput,
    store: Option<&FileStore>,
) -> Result<(), TarError> {
    let uri = &stored.uri;
    let Some(store) = store else {
        return Err(TarError::Author(format!(
            "{uri} cannot be staged as the input file {name:?}: egret has no internal storage \
             configured (the `[storage]` section of its configuration)"
        )));
    };
    // Opened before the header is written, so a missing object is reported
    // before any of this entry is.
    let mut reader = match store.store.get_stream(&stored.object_uri).await {
        Ok(reader) => reader,
        // Carry-in (Task 4): an index row whose object is gone is the author's
        // error, naming the URI.
        Err(egret_storage::StorageError::NotFound(_)) => {
            return Err(TarError::Author(format!(
                "{uri} is indexed but its object is missing from storage (it was pruned, or \
                 never finished uploading), so it cannot be staged as the input file {name:?}"
            )))
        }
        Err(e) => return Err(TarError::Store(unavailable(uri, &e))),
    };
    let h = header(name, stored.size, b'0', 0o644).map_err(TarError::Author)?;
    out.write_all(&h).await.map_err(TarError::Write)?;
    let mut left = stored.size;
    while left > 0 {
        // A mid-body error is a bare io::Error (Task 2): named here.
        let buf = reader
            .fill_buf()
            .await
            .map_err(|e| TarError::Store(unavailable(uri, &e)))?;
        if buf.is_empty() {
            return Err(TarError::Author(format!(
                "{uri} is indexed as {} bytes but its object in storage holds only {}, so it \
                 cannot be staged as the input file {name:?}",
                stored.size,
                stored.size - left
            )));
        }
        let n = buf
            .len()
            .min(CHUNK)
            .min(usize::try_from(left).unwrap_or(usize::MAX));
        out.write_all(&buf[..n]).await.map_err(TarError::Write)?;
        reader.consume(n);
        left -= n as u64;
    }
    let more = reader
        .fill_buf()
        .await
        .map_err(|e| TarError::Store(unavailable(uri, &e)))?;
    if !more.is_empty() {
        return Err(TarError::Author(format!(
            "{uri} is indexed as {} bytes but its object in storage holds more, so it cannot be \
             staged as the input file {name:?}",
            stored.size
        )));
    }
    pad(out, stored.size).await
}

fn unavailable(uri: &str, e: &dyn std::fmt::Display) -> String {
    format!("{uri} could not be staged: internal storage is unavailable: {e}")
}

/// Zeros to the next block boundary after `len` bytes of contents.
async fn pad(out: &mut (dyn AsyncWrite + Send + Unpin), len: u64) -> Result<(), TarError> {
    let rem = (len % BLOCK as u64) as usize;
    if rem != 0 {
        out.write_all(&ZEROS[..BLOCK - rem])
            .await
            .map_err(TarError::Write)?;
    }
    Ok(())
}

/// Every check on the names, and the parent directories to write first.
///
/// The names are the author's (`inputFiles` keys), so every problem is a
/// message: a name that escapes the working directory, one ustar cannot
/// carry, or one that is also the parent directory of another name
/// (`{"a": …, "a/b": …}`) -- W5a's `stage_input` refused that last one on the
/// host's filesystem; the archive has none, so it is refused here by name.
fn plan(files: &BTreeMap<String, TarSource<'_>>) -> Result<BTreeSet<String>, String> {
    let mut dirs: BTreeSet<String> = BTreeSet::new();
    for name in files.keys() {
        if let Some(problem) = relative_problem(name) {
            return Err(problem);
        }
        split_name(name)?;
        let mut acc = String::new();
        let parts: Vec<&str> = name.split('/').collect();
        for p in &parts[..parts.len().saturating_sub(1)] {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(p);
            dirs.insert(format!("{acc}/"));
        }
    }
    for name in files.keys() {
        if dirs.contains(&format!("{name}/")) {
            let child = files
                .keys()
                .find(|other| other.starts_with(&format!("{name}/")))
                .map(String::as_str)
                .unwrap_or_default();
            return Err(format!(
                "the input file {child:?} cannot be placed: a parent of it, {name:?}, is also \
                 declared as a file"
            ));
        }
    }
    Ok(dirs)
}

/// One ustar header block.
pub(crate) fn header(name: &str, size: u64, kind: u8, mode: u32) -> Result<[u8; BLOCK], String> {
    if size > MAX_USTAR_SIZE {
        return Err(format!(
            "the input file {name:?} is {size} bytes, over the {MAX_USTAR_SIZE} a ustar entry \
             can carry"
        ));
    }
    let mut h = [0u8; BLOCK];
    let (prefix, base) = split_name(name)?;
    h[..base.len()].copy_from_slice(base.as_bytes());
    octal(&mut h[100..108], mode as u64);
    octal(&mut h[108..116], 0);
    octal(&mut h[116..124], 0);
    octal(&mut h[124..136], size);
    octal(&mut h[136..148], 0);
    h[156] = kind;
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    h[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
    h[148..156].copy_from_slice(b"        ");
    let sum: u32 = h.iter().map(|b| *b as u32).sum();
    let s = format!("{sum:06o}\0 ");
    h[148..156].copy_from_slice(s.as_bytes());
    Ok(h)
}

/// ustar's name field is 100 bytes and its prefix 155, split at a `/`.
fn split_name(name: &str) -> Result<(&str, &str), String> {
    let too_long = || {
        format!(
            "the input file name {name:?} is too long to transfer: a name may be at most 255 \
             bytes, with at most 100 after its last `/`"
        )
    };
    if name.len() <= 100 {
        return Ok(("", name));
    }
    // The longest prefix that fits, so the base is as short as possible.
    let bytes = name.as_bytes();
    for i in (0..bytes.len()).rev() {
        if bytes[i] == b'/' && i <= 155 && name.len() - i - 1 <= 100 && i > 0 {
            return Ok((&name[..i], &name[i + 1..]));
        }
    }
    Err(too_long())
}

/// An octal field; the size field's 11 digits hold up to 8 GiB - 1, over the
/// 5 GiB `max_task_bytes` default.
fn octal(field: &mut [u8], v: u64) {
    let width = field.len() - 1;
    let s = format!("{v:0width$o}");
    field[..width].copy_from_slice(s.as_bytes());
    field[width] = 0;
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::task::{Context, Poll};

    use egret_storage::{Listing, MemStore, ObjectStore, StorageError};
    use tokio::io::{AsyncBufRead, AsyncRead, ReadBuf};

    use super::*;
    use crate::storage::Caps;

    const URI: &str = "kestra:///w9a/t5/f/executions/00000000-0000-0000-0000-000000000001/tasks/w/00000000-0000-0000-0000-000000000002/big.bin";

    /// A [`FileStore`] over `store`.
    fn file_store(store: Arc<dyn ObjectStore>) -> FileStore {
        FileStore {
            store,
            bucket: "egret-test".into(),
            prefix: "workflow/".into(),
            caps: Caps::default(),
            retention_days: 30,
        }
    }

    fn stored(fs: &FileStore, size: u64) -> StoredInput {
        StoredInput {
            uri: URI.to_string(),
            object_uri: fs.object_uri(uuid::Uuid::nil(), URI).unwrap(),
            size,
        }
    }

    async fn tar_of(
        files: &BTreeMap<String, TarSource<'_>>,
        store: Option<&FileStore>,
    ) -> Result<Vec<u8>, TarError> {
        let mut out = Vec::new();
        write_tar(&mut out, files, store).await?;
        Ok(out)
    }

    #[tokio::test]
    async fn write_tar_is_byte_identical_to_the_old_packer_for_contents() {
        let cases: Vec<BTreeMap<String, String>> = vec![
            BTreeMap::new(),
            BTreeMap::from([("a.txt".to_string(), "hello\n".to_string())]),
            BTreeMap::from([
                ("a.txt".to_string(), "hello\n".to_string()),
                ("dir/sub/b.txt".to_string(), "x".repeat(700)),
                ("dir/c".to_string(), String::new()),
                ("exact".to_string(), "y".repeat(512)),
                (
                    format!("{}/{}", "d".repeat(120), "f".repeat(90)),
                    "z".to_string(),
                ),
            ]),
        ];
        for files in cases {
            let old = crate::worker::k8s::transfer::pack(&files).expect("the old packer");
            let sources: BTreeMap<String, TarSource<'_>> = files
                .iter()
                .map(|(k, v)| (k.clone(), TarSource::Contents(v)))
                .collect();
            let new = tar_of(&sources, None).await.expect("write_tar");
            assert_eq!(new, old, "the archive for {:?}", files.keys());
        }
    }

    /// A stored input's entry is the entry its contents would have made.
    #[tokio::test]
    async fn a_stored_input_reads_back_as_the_same_entry_its_contents_make() {
        let fs = file_store(Arc::new(MemStore::new()));
        let body = "stored ".repeat(300);
        let input = stored(&fs, body.len() as u64);
        fs.store
            .put(&input.object_uri, body.clone().into_bytes())
            .await
            .unwrap();
        let from_store = tar_of(
            &BTreeMap::from([
                ("d/nested.txt".to_string(), TarSource::Stored(&input)),
                (
                    "literal.txt".to_string(),
                    TarSource::Contents("just contents"),
                ),
            ]),
            Some(&fs),
        )
        .await
        .expect("write_tar");
        let inline = tar_of(
            &BTreeMap::from([
                ("d/nested.txt".to_string(), TarSource::Contents(&body)),
                (
                    "literal.txt".to_string(),
                    TarSource::Contents("just contents"),
                ),
            ]),
            None,
        )
        .await
        .expect("write_tar");
        assert_eq!(from_store, inline);
    }

    /// The bytes read from the store and not yet written to `out`, at their
    /// highest: what `write_tar` held of the object at once.
    #[derive(Default)]
    struct Flight {
        read: AtomicU64,
        written: AtomicU64,
        peak: AtomicU64,
    }

    impl Flight {
        fn sample(&self) {
            let held = self
                .read
                .load(Ordering::SeqCst)
                .saturating_sub(self.written.load(Ordering::SeqCst));
            self.peak.fetch_max(held, Ordering::SeqCst);
        }
    }

    /// A [`MemStore`] whose streams count what is taken from them.
    struct Counting {
        inner: MemStore,
        flight: Arc<Flight>,
    }

    struct CountingReader {
        inner: Box<dyn AsyncBufRead + Send + Unpin>,
        flight: Arc<Flight>,
    }

    impl AsyncRead for CountingReader {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            let before = buf.filled().len();
            let r = Pin::new(&mut self.inner).poll_read(cx, buf);
            let n = (buf.filled().len() - before) as u64;
            self.flight.read.fetch_add(n, Ordering::SeqCst);
            self.flight.sample();
            r
        }
    }

    impl AsyncBufRead for CountingReader {
        fn poll_fill_buf(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<std::io::Result<&[u8]>> {
            Pin::new(&mut self.get_mut().inner).poll_fill_buf(cx)
        }
        fn consume(mut self: Pin<&mut Self>, amt: usize) {
            self.flight.read.fetch_add(amt as u64, Ordering::SeqCst);
            self.flight.sample();
            Pin::new(&mut self.inner).consume(amt)
        }
    }

    #[async_trait::async_trait]
    impl ObjectStore for Counting {
        async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError> {
            self.inner.put(uri, bytes).await
        }
        async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
            self.inner.get(uri).await
        }
        async fn delete(&self, uri: &str) -> Result<(), StorageError> {
            self.inner.delete(uri).await
        }
        async fn copy(&self, src: &str, dst: &str) -> Result<(), StorageError> {
            self.inner.copy(src, dst).await
        }
        async fn list(&self, prefix: &str, d: Option<&str>) -> Result<Listing, StorageError> {
            self.inner.list(prefix, d).await
        }
        async fn get_stream(
            &self,
            uri: &str,
        ) -> Result<Box<dyn AsyncBufRead + Send + Unpin>, StorageError> {
            Ok(Box::new(CountingReader {
                inner: self.inner.get_stream(uri).await?,
                flight: self.flight.clone(),
            }))
        }
    }

    /// An `AsyncWrite` that keeps nothing and counts what it is given.
    struct CountingSink(Arc<Flight>);

    impl AsyncWrite for CountingSink {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            // Sampled BEFORE counting this write: the bytes it carries were
            // held until now.
            self.0.sample();
            self.0.written.fetch_add(buf.len() as u64, Ordering::SeqCst);
            Poll::Ready(Ok(buf.len()))
        }
        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn a_stored_input_is_copied_from_the_store_without_being_collected() {
        const SIZE: usize = 20 << 20;
        let flight = Arc::new(Flight::default());
        let fs = file_store(Arc::new(Counting {
            inner: MemStore::new(),
            flight: flight.clone(),
        }));
        let input = stored(&fs, SIZE as u64);
        fs.store
            .put(&input.object_uri, vec![7u8; SIZE])
            .await
            .unwrap();
        let mut sink = CountingSink(flight.clone());
        write_tar(
            &mut sink,
            &BTreeMap::from([("big.bin".to_string(), TarSource::Stored(&input))]),
            Some(&fs),
        )
        .await
        .expect("write_tar");
        let (read, written, peak) = (
            flight.read.load(Ordering::SeqCst),
            flight.written.load(Ordering::SeqCst),
            flight.peak.load(Ordering::SeqCst),
        );
        // Control: the whole object went through, so a small peak is not a
        // small object.
        assert_eq!(read, SIZE as u64, "every byte of the object was read");
        assert_eq!(
            written,
            (BLOCK + SIZE + 2 * BLOCK) as u64,
            "header + body + end"
        );
        assert!(
            peak < 1 << 20,
            "write_tar held {peak} bytes of the object at once; the bound is 1 MiB"
        );
    }

    #[tokio::test]
    async fn a_stored_input_whose_object_is_shorter_than_its_size_is_an_error() {
        let fs = file_store(Arc::new(MemStore::new()));
        let input = stored(&fs, 20);
        fs.store
            .put(&input.object_uri, b"ten bytes!".to_vec())
            .await
            .unwrap();
        let files = BTreeMap::from([("in.bin".to_string(), TarSource::Stored(&input))]);
        let err = tar_of(&files, Some(&fs)).await.expect_err("short object");
        assert!(
            matches!(&err, TarError::Author(m) if m.contains(URI) && m.contains("20 bytes") && m.contains("only 10")),
            "{err:?}"
        );
        // Longer than its index row: also refused, never cut silently.
        let longer = stored(&fs, 5);
        let files = BTreeMap::from([("in.bin".to_string(), TarSource::Stored(&longer))]);
        let err = tar_of(&files, Some(&fs)).await.expect_err("long object");
        assert!(
            matches!(&err, TarError::Author(m) if m.contains(URI) && m.contains("holds more")),
            "{err:?}"
        );
        // Control: the right size writes.
        let exact = stored(&fs, 10);
        let files = BTreeMap::from([("in.bin".to_string(), TarSource::Stored(&exact))]);
        tar_of(&files, Some(&fs))
            .await
            .expect("the indexed size writes");
    }

    /// An object store whose every read fails as an unreachable S3 does.
    struct Down;

    #[async_trait::async_trait]
    impl ObjectStore for Down {
        async fn put(&self, _: &str, _: Vec<u8>) -> Result<(), StorageError> {
            Err(StorageError::S3("connection refused".into()))
        }
        async fn get(&self, _: &str) -> Result<Vec<u8>, StorageError> {
            Err(StorageError::S3("connection refused".into()))
        }
        async fn delete(&self, _: &str) -> Result<(), StorageError> {
            Err(StorageError::S3("connection refused".into()))
        }
        async fn copy(&self, _: &str, _: &str) -> Result<(), StorageError> {
            Err(StorageError::S3("connection refused".into()))
        }
        async fn list(&self, _: &str, _: Option<&str>) -> Result<Listing, StorageError> {
            Err(StorageError::S3("connection refused".into()))
        }
        async fn get_stream(
            &self,
            _: &str,
        ) -> Result<Box<dyn AsyncBufRead + Send + Unpin>, StorageError> {
            Err(StorageError::S3("connection refused".into()))
        }
    }

    /// Spec §11 and the Task 4 carry-in: an object missing behind its index
    /// row is the author's error naming the URI; any other store error is an
    /// outage.
    #[tokio::test]
    async fn a_missing_object_is_the_authors_and_an_unreachable_store_is_an_outage() {
        let fs = file_store(Arc::new(MemStore::new()));
        let input = stored(&fs, 3);
        let files = BTreeMap::from([("in.bin".to_string(), TarSource::Stored(&input))]);
        let err = tar_of(&files, Some(&fs)).await.expect_err("no object");
        assert!(
            matches!(&err, TarError::Author(m) if m.contains(URI) && m.contains("missing")),
            "{err:?}"
        );
        let down = file_store(Arc::new(Down));
        let input = stored(&down, 3);
        let files = BTreeMap::from([("in.bin".to_string(), TarSource::Stored(&input))]);
        let err = tar_of(&files, Some(&down)).await.expect_err("down");
        assert!(
            matches!(&err, TarError::Store(m) if m.contains(URI) && m.contains("connection refused")),
            "{err:?}"
        );
        let err = tar_of(&files, None).await.expect_err("no [storage]");
        assert!(
            matches!(&err, TarError::Author(m) if m.contains("[storage]")),
            "{err:?}"
        );
    }

    /// W5a's `stage_input` refused `{"a": …, "a/b": …}` on the host's
    /// filesystem; the archive refuses it by name, before writing anything.
    #[tokio::test]
    async fn a_file_where_a_directory_must_go_is_the_authors_error_and_nothing_is_written() {
        let files = BTreeMap::from([
            ("a".to_string(), TarSource::Contents("x")),
            ("a/b".to_string(), TarSource::Contents("y")),
        ]);
        let mut out = Vec::new();
        let err = write_tar(&mut out, &files, None)
            .await
            .expect_err("collision");
        assert!(
            matches!(&err, TarError::Author(m) if m.contains("\"a/b\"") && m.contains("\"a\"")),
            "{err:?}"
        );
        assert!(
            out.is_empty(),
            "nothing is written before the names are checked"
        );
        for bad in ["../x", "/etc/passwd", ""] {
            let files = BTreeMap::from([(bad.to_string(), TarSource::Contents("y"))]);
            assert!(
                matches!(tar_of(&files, None).await, Err(TarError::Author(_))),
                "{bad:?}"
            );
        }
        let long = BTreeMap::from([("f".repeat(101), TarSource::Contents(""))]);
        assert!(
            matches!(tar_of(&long, None).await, Err(TarError::Author(m)) if m.contains("too long"))
        );
        assert!(header("x", MAX_USTAR_SIZE + 1, b'0', 0o644).is_err());
        assert!(header("x", MAX_USTAR_SIZE, b'0', 0o644).is_ok());
    }
}
