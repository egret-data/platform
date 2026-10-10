//! The streams W9a Task 12's whole-file tasks share (spec §9.3): a stored
//! file read as a `std::io::Read`, files written as they are produced, and
//! the two write caps, on one blocking thread per task run.
//!
//! # Why a blocking thread, and what it may hold
//!
//! The Ion reader is synchronous (`storage::ion::for_each_value`) and so is
//! the per-record render of `FilterItems` and `DeduplicateItems`, so a task's
//! body runs on a `spawn_blocking` thread ([`run_blocking`]) and drives the
//! store through the runtime's handle: [`Input`] is a `SyncIoBridge` over
//! `get_stream`, and each [`Upload`] is a `put_stream` task on the runtime
//! fed through a bounded channel. **Nothing holds a whole file**: an input is
//! read a buffer at a time, and an upload holds at most [`QUEUE`] chunks of
//! [`CHUNK`] bytes on top of what the store's own `put_stream` buffers (one
//! 8 MiB part on S3). `Reverse` alone reads its input whole, capped by
//! `workflow.storage.max_read_bytes` ([`Run::read_capped`]).
//!
//! # Errors (spec §11)
//!
//! [`Halt::Author`] is the author's: a failed task naming the URI or the cap.
//! [`Halt::Worker`] is egret's -- internal storage unreachable, before or in
//! the middle of a body -- and becomes `Err`, so the run is re-claimed with no
//! attempt spent. An object shorter or longer than its index row, or gone
//! behind its row, is the author's, as staging says (`storage::transfer`).
//!
//! # The caps
//!
//! Every byte an [`Upload`] is given is counted BEFORE it is sent: the bytes
//! that would cross `workflow.storage.max_file_bytes` (one file) or
//! `workflow.storage.max_task_bytes` (all of this run's files) are never sent,
//! and the upload is aborted ([`Upload`]'s `Drop`), so the store keeps no
//! object of a failed run's file -- at worst an unindexed one, an orphan the
//! pruner sweeps (spec §10).

use std::io::{self, BufRead, Read};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use egret_storage::StorageError;
use tokio::io::{AsyncRead, ReadBuf};
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio_util::io::SyncIoBridge;
use uuid::Uuid;

use crate::storage::ion::Stop;
use crate::storage::{uri, FileStore, StoredFile};

/// What an [`Upload`] sends the store at a time.
pub(super) const CHUNK: usize = 64 << 10;
/// How many chunks an [`Upload`] may have in flight before the producer
/// waits for the store.
pub(super) const QUEUE: usize = 4;

/// Why a whole-file task stopped short of its outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Halt {
    /// The author's: a failed task with this message.
    Author(String),
    /// Not the author's -- internal storage unreachable, a reader thread
    /// that could not start: `Err`, a worker failure (spec §11).
    Worker(String),
    /// The run's future was dropped (its timeout, a kill): nothing is
    /// reported, every upload is aborted.
    Cancelled,
}

pub(super) type Step<T> = std::result::Result<T, Halt>;

/// What every stream of one task run shares.
pub(super) struct Run {
    pub(super) handle: Handle,
    pub(super) fs: Arc<FileStore>,
    pub(super) workspace: Uuid,
    /// The run's own `…/tasks/{task}/{taskRunId}/`, under which it writes.
    pub(super) prefix: String,
    cancel: Arc<AtomicBool>,
    /// Bytes given to this run's uploads so far, against `max_task_bytes`.
    stored: AtomicU64,
    /// How long an aborted [`Upload`] waits for the store ([`ABORT_WAIT`]).
    abort_wait: std::time::Duration,
}

/// How long dropping an unfinished [`Upload`] waits for the store to take
/// the abort and end its `put_stream`; past it the upload task is cancelled
/// and the thread moves on, so a stalled store cannot hang it.
pub(super) const ABORT_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

impl Run {
    pub(super) fn new(fs: Arc<FileStore>, workspace: Uuid, prefix: String) -> Self {
        Self {
            handle: Handle::current(),
            fs,
            workspace,
            prefix,
            cancel: Arc::default(),
            stored: AtomicU64::new(0),
            abort_wait: ABORT_WAIT,
        }
    }

    /// The same run, waiting `wait` for an aborted upload (tests).
    #[cfg(test)]
    pub(super) fn with_abort_wait(mut self, wait: std::time::Duration) -> Self {
        self.abort_wait = wait;
        self
    }

    /// `Err(Cancelled)` once the run's future is gone.
    pub(super) fn live(&self) -> Step<()> {
        if self.cancel.load(Ordering::SeqCst) {
            Err(Halt::Cancelled)
        } else {
            Ok(())
        }
    }

    /// The stored file `file` (whose index row says `size` bytes), as a
    /// stream. Its faults go to the returned [`Faults`], which the caller
    /// asks first when a read fails.
    pub(super) fn open(&self, file: &str, size: u64) -> Step<(Input, Faults)> {
        self.live()?;
        let object = self
            .fs
            .object_uri(self.workspace, file)
            .map_err(Halt::Author)?;
        let stream = match self.handle.block_on(self.fs.store.get_stream(&object)) {
            Ok(stream) => stream,
            // A row and no object: a prune race or an orphaned row, the
            // author's, as `read()` and staging say.
            Err(StorageError::NotFound(_)) => {
                return Err(Halt::Author(format!(
                    "{file} is indexed but its object is missing from storage (it was pruned, \
                     or never finished uploading)"
                )))
            }
            Err(e) => return Err(Halt::Worker(unreadable(file, &e))),
        };
        let faults = Faults::default();
        Ok((
            Input {
                inner: SyncIoBridge::new_with_handle(stream, self.handle.clone()),
                file: file.to_string(),
                size,
                read: 0,
                faults: faults.clone(),
                cancel: self.cancel.clone(),
            },
            faults,
        ))
    }

    /// The whole of `file`, for a task that cannot stream it: refused from
    /// the index row's size, before any download, when it is over
    /// `workflow.storage.max_read_bytes` -- `whole` finishes the message
    /// ("…: {whole}, so it reads only files up to that size", `read()`'s).
    pub(super) fn read_capped(&self, file: &str, size: u64, whole: &str) -> Step<Vec<u8>> {
        let cap = self.fs.caps.max_read_bytes;
        if size > cap {
            return Err(Halt::Author(format!(
                "{file} is larger than workflow.storage.max_read_bytes ({cap} bytes): {whole}, \
                 so it reads only files up to that size"
            )));
        }
        let (mut input, faults) = self.open(file, size)?;
        let mut bytes = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
        match input.read_to_end(&mut bytes) {
            Ok(_) => Ok(bytes),
            Err(e) => Err(faults.or_worker(file, &e)),
        }
    }

    /// A new file of this run, `name` under its prefix, written as it is
    /// produced.
    pub(super) fn create(&self, name: &str) -> Step<Upload<'_>> {
        self.live()?;
        let file = uri::verbatim_file_uri(&self.prefix, name).map_err(Halt::Author)?;
        let object = self
            .fs
            .object_uri(self.workspace, &file)
            .map_err(Halt::Author)?;
        let (tx, rx) = mpsc::channel(QUEUE);
        let store = self.fs.store.clone();
        let join = self.handle.spawn(async move {
            let mut body = ChannelBody {
                rx,
                chunk: Vec::new(),
                at: 0,
            };
            store.put_stream(&object, &mut body, None).await
        });
        Ok(Upload {
            run: self,
            file,
            tx: Some(tx),
            pending: Vec::new(),
            written: 0,
            join: Some(join),
        })
    }
}

fn unreadable(file: &str, e: &dyn std::fmt::Display) -> String {
    format!("{file} could not be read: internal storage is unavailable: {e}")
}

fn unstorable(file: &str, e: &dyn std::fmt::Display) -> String {
    format!("{file} could not be stored: internal storage is unavailable: {e}")
}

/// Run `body` on a blocking thread with `run`, and wait for it. When this
/// future is dropped -- the task's timeout, a kill -- the run is marked
/// cancelled: every stream answers [`Halt::Cancelled`] at its next read or
/// write, and every upload is aborted.
pub(super) async fn run_blocking<T: Send + 'static>(
    run: Run,
    body: impl FnOnce(&Run) -> Step<T> + Send + 'static,
) -> Step<T> {
    struct CancelOnDrop(Arc<AtomicBool>);
    impl Drop for CancelOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let _cancel = CancelOnDrop(run.cancel.clone());
    match tokio::task::spawn_blocking(move || body(&run)).await {
        Ok(step) => step,
        Err(e) => Err(Halt::Worker(format!("the task's thread failed: {e}"))),
    }
}

/// The first fault an [`Input`] met, which a reader that got an `io::Error`
/// asks for before deciding whose error it is.
#[derive(Clone, Default)]
pub(super) struct Faults(Arc<Mutex<Option<Halt>>>);

impl Faults {
    fn record(&self, halt: Halt) {
        let mut held = self.0.lock().unwrap_or_else(|p| p.into_inner());
        held.get_or_insert(halt);
    }

    pub(super) fn take(&self) -> Option<Halt> {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).take()
    }

    /// The recorded fault, or else `e` -- an error no [`Input`] recorded,
    /// which is the store's -- as an outage of `file`.
    pub(super) fn or_worker(&self, file: &str, e: &io::Error) -> Halt {
        self.take()
            .unwrap_or_else(|| Halt::Worker(unreadable(file, e)))
    }

    /// How the Ion reader's stop over an [`Input`] ends the read: the
    /// input's own fault first (a store error is not a bad file), then the
    /// reader's verdict on `file`.
    pub(super) fn after_ion(&self, file: &str, stop: Stop) -> Step<()> {
        if let Some(halt) = self.take() {
            return Err(halt);
        }
        match stop {
            Stop::End | Stop::Full => Ok(()),
            Stop::Failed(why) | Stop::TooDeep(why) => Err(Halt::Author(format!(
                "{file} is not an Ion file egret can read: {why}"
            ))),
            Stop::TooLarge(why) => Err(Halt::Author(format!("{file}: {why}"))),
            Stop::Crashed(why) => Err(Halt::Worker(format!("{file}: {why}"))),
        }
    }
}

/// A stored file as a `Read`, checked against its index row's size: an
/// object that holds fewer or more bytes than its row says is the author's
/// error, and any error from the store is an outage (each recorded in
/// [`Faults`]).
pub(super) struct Input {
    inner: SyncIoBridge<Box<dyn tokio::io::AsyncBufRead + Send + Unpin>>,
    file: String,
    size: u64,
    read: u64,
    faults: Faults,
    cancel: Arc<AtomicBool>,
}

impl Input {
    fn fail(&self, halt: Halt) -> io::Error {
        let text = match &halt {
            Halt::Author(m) | Halt::Worker(m) => m.clone(),
            Halt::Cancelled => "cancelled".to_string(),
        };
        self.faults.record(halt);
        io::Error::other(text)
    }
}

impl Read for Input {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::SeqCst) {
            return Err(self.fail(Halt::Cancelled));
        }
        match self.inner.read(buf) {
            Err(e) => Err(self.fail(Halt::Worker(unreadable(&self.file, &e)))),
            Ok(0) if self.read < self.size => Err(self.fail(Halt::Author(format!(
                "{} is indexed as {} bytes but its object in storage holds only {}",
                self.file, self.size, self.read
            )))),
            Ok(0) => Ok(0),
            Ok(n) => {
                self.read += n as u64;
                if self.read > self.size {
                    return Err(self.fail(Halt::Author(format!(
                        "{} is indexed as {} bytes but its object in storage holds more",
                        self.file, self.size
                    ))));
                }
                Ok(n)
            }
        }
    }
}

/// The body `put_stream` reads: the chunks an [`Upload`] sends, and an
/// `Err` one to abort.
struct ChannelBody {
    rx: mpsc::Receiver<io::Result<Vec<u8>>>,
    chunk: Vec<u8>,
    at: usize,
}

impl AsyncRead for ChannelBody {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            if this.at < this.chunk.len() {
                let n = (this.chunk.len() - this.at).min(out.remaining());
                out.put_slice(&this.chunk[this.at..this.at + n]);
                this.at += n;
                return Poll::Ready(Ok(()));
            }
            match this.rx.poll_recv(cx) {
                Poll::Pending => return Poll::Pending,
                // Every sender gone: the end of the body.
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Err(e)),
                Poll::Ready(Some(Ok(chunk))) => {
                    this.chunk = chunk;
                    this.at = 0;
                }
            }
        }
    }
}

/// One file of the run, being written. [`Upload::finish`] makes it a
/// [`StoredFile`] for the outcome; dropped unfinished, it is ABORTED -- the
/// store's `put_stream` gets an error, so nothing is stored under its name
/// (a multipart upload is aborted, a single PUT never sent).
pub(super) struct Upload<'r> {
    run: &'r Run,
    file: String,
    tx: Option<mpsc::Sender<io::Result<Vec<u8>>>>,
    pending: Vec<u8>,
    written: u64,
    join: Option<tokio::task::JoinHandle<std::result::Result<u64, StorageError>>>,
}

impl Upload<'_> {
    /// Append `bytes`, refusing -- before a byte of them is sent -- what
    /// would cross either cap.
    pub(super) fn write(&mut self, bytes: &[u8]) -> Step<()> {
        self.run.live()?;
        let n = bytes.len() as u64;
        let caps = self.run.fs.caps;
        if self.written + n > caps.max_file_bytes {
            return Err(Halt::Author(format!(
                "{} would be over the {} bytes one file may be (workflow.storage.max_file_bytes)",
                self.file, caps.max_file_bytes
            )));
        }
        let stored = self.run.stored.load(Ordering::SeqCst);
        if stored + n > caps.max_task_bytes {
            return Err(Halt::Author(format!(
                "{} would take this task run over the {} bytes one task run may store \
                 (workflow.storage.max_task_bytes)",
                self.file, caps.max_task_bytes
            )));
        }
        self.run.stored.fetch_add(n, Ordering::SeqCst);
        self.written += n;
        self.pending.extend_from_slice(bytes);
        while self.pending.len() >= CHUNK {
            let rest = self.pending.split_off(CHUNK);
            let chunk = std::mem::replace(&mut self.pending, rest);
            self.send(chunk)?;
        }
        Ok(())
    }

    fn send(&mut self, chunk: Vec<u8>) -> Step<()> {
        let Some(tx) = &self.tx else {
            return Err(Halt::Worker(format!(
                "{}: written after it ended",
                self.file
            )));
        };
        if tx.blocking_send(Ok(chunk)).is_ok() {
            return Ok(());
        }
        // The store stopped reading: its answer says why.
        self.tx = None;
        Err(
            match self.join.take().map(|j| self.run.handle.block_on(j)) {
                Some(Ok(Err(e))) => Halt::Worker(unstorable(&self.file, &e)),
                Some(Err(e)) => Halt::Worker(unstorable(&self.file, &e)),
                _ => Halt::Worker(unstorable(&self.file, &"the upload stopped reading")),
            },
        )
    }

    /// End the file and wait for the store: the file, with the size the
    /// store holds (`head`, spec §4.2), for the outcome commit to index.
    pub(super) fn finish(mut self) -> Step<StoredFile> {
        self.run.live()?;
        let last = std::mem::take(&mut self.pending);
        if !last.is_empty() {
            self.send(last)?;
        }
        // Dropping the sender ends the body.
        self.tx = None;
        let file = self.file.clone();
        let written = self.written;
        let uploaded = match self.join.take().map(|j| self.run.handle.block_on(j)) {
            Some(Ok(Ok(n))) => n,
            Some(Ok(Err(e))) => return Err(Halt::Worker(unstorable(&file, &e))),
            Some(Err(e)) => return Err(Halt::Worker(unstorable(&file, &e))),
            None => return Err(Halt::Worker(format!("{file}: finished twice"))),
        };
        let object = self
            .run
            .fs
            .object_uri(self.run.workspace, &file)
            .map_err(Halt::Author)?;
        match self.run.handle.block_on(self.run.fs.store.head(&object)) {
            Ok(Some(held)) if held == written && uploaded == written => Ok(StoredFile {
                uri: file,
                size: written,
            }),
            Ok(Some(held)) => Err(Halt::Worker(format!(
                "{file} holds {held} bytes in internal storage after {written} were written \
                 ({uploaded} uploaded)"
            ))),
            Ok(None) => Err(Halt::Worker(format!(
                "{file} was written ({uploaded} bytes) but internal storage does not have it"
            ))),
            Err(e) => Err(Halt::Worker(format!(
                "{file} could not be checked: internal storage is unavailable: {e}"
            ))),
        }
    }
}

impl Drop for Upload<'_> {
    fn drop(&mut self) {
        let Some(tx) = self.tx.take() else {
            return;
        };
        // An `Err` in the body: `put_stream` fails, and stores nothing. Each
        // wait is bounded by `abort_wait`: a stalled store -- a full queue it
        // never reads, a `put_stream` that never ends -- must not hold this
        // thread for ever. Past it the upload task is cancelled, and a
        // cancelled `put_stream` aborts its own multipart upload as it drops
        // (`egret_storage::s3`'s `AbortOnDrop`), so the store keeps no object.
        let wait = self.run.abort_wait;
        let join = self.join.take();
        self.run.handle.block_on(async move {
            let aborted = Err(io::Error::other("aborted by egret"));
            let _ = tokio::time::timeout(wait, tx.send(aborted)).await;
            drop(tx);
            if let Some(mut join) = join {
                if tokio::time::timeout(wait, &mut join).await.is_err() {
                    join.abort();
                }
            }
        });
    }
}

/// Text rows as Java's `BufferedReader.readLine` reads them (`Split`'s
/// `TextSplitStrategy`): a row ends at `\n`, `\r` or `\r\n`, the terminator
/// is not part of it, and a last row with no terminator is still a row. A
/// row is held whole, so one longer than `workflow.storage.max_read_bytes`
/// is refused.
pub(super) struct Lines<R> {
    inner: R,
    file: String,
    faults: Faults,
    cap: u64,
    /// The last row ended at a `\r`: a `\n` right after it is that
    /// terminator's.
    skip_lf: bool,
}

impl<R: BufRead> Lines<R> {
    pub(super) fn new(inner: R, file: &str, faults: Faults, cap: u64) -> Self {
        Self {
            inner,
            file: file.to_string(),
            faults,
            cap,
            skip_lf: false,
        }
    }

    pub(super) fn next_row(&mut self) -> Step<Option<Vec<u8>>> {
        let mut row = Vec::new();
        let mut any = false;
        loop {
            let buf = match self.inner.fill_buf() {
                Ok(buf) => buf,
                Err(e) => return Err(self.faults.or_worker(&self.file, &e)),
            };
            if buf.is_empty() {
                return Ok(any.then_some(row));
            }
            let start = usize::from(std::mem::take(&mut self.skip_lf) && buf[0] == b'\n');
            let found = buf[start..].iter().position(|&b| b == b'\n' || b == b'\r');
            let (taken, ended) = match found {
                Some(i) => {
                    row.extend_from_slice(&buf[start..start + i]);
                    self.skip_lf = buf[start + i] == b'\r';
                    (start + i + 1, true)
                }
                None => {
                    row.extend_from_slice(&buf[start..]);
                    (buf.len(), false)
                }
            };
            self.inner.consume(taken);
            any |= ended || taken > start;
            if row.len() as u64 > self.cap {
                return Err(Halt::Author(format!(
                    "a row of {} is longer than workflow.storage.max_read_bytes ({} bytes): a \
                     row is held whole, so rows are read only up to that size",
                    self.file, self.cap
                )));
            }
            if ended {
                return Ok(Some(row));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(text: &[u8], chunk: usize) -> Vec<String> {
        // A reader that hands over `chunk` bytes at a time, so a `\r\n`
        // falls across two fills.
        struct Trickle<'a>(&'a [u8], usize);
        impl Read for Trickle<'_> {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let n = self.0.len().min(self.1).min(buf.len());
                buf[..n].copy_from_slice(&self.0[..n]);
                self.0 = &self.0[n..];
                Ok(n)
            }
        }
        let mut lines = Lines::new(
            io::BufReader::with_capacity(chunk, Trickle(text, chunk)),
            "kestra:///x",
            Faults::default(),
            1 << 20,
        );
        let mut out = Vec::new();
        while let Some(row) = lines.next_row().unwrap() {
            out.push(String::from_utf8(row).unwrap());
        }
        out
    }

    /// `BufferedReader.readLine`'s rows, at every fill size.
    #[test]
    fn rows_end_where_javas_readline_ends_them() {
        for chunk in [1, 2, 3, 64] {
            assert_eq!(rows(b"l1\nl2\n", chunk), ["l1", "l2"], "{chunk}");
            assert_eq!(rows(b"a\r\nb\rc\n\nd", chunk), ["a", "b", "c", "", "d"]);
            assert_eq!(rows(b"\n", chunk), [""]);
            assert_eq!(rows(b"", chunk), Vec::<String>::new());
            assert_eq!(rows(b"a\r", chunk), ["a"]);
            assert_eq!(rows(b"a\r\r\n", chunk), ["a", ""]);
        }
    }

    #[test]
    fn a_row_over_the_cap_is_refused_naming_it() {
        let mut lines = Lines::new(&b"abcdef\n"[..], "kestra:///x", Faults::default(), 5);
        let Err(Halt::Author(m)) = lines.next_row() else {
            panic!("refused")
        };
        assert!(m.contains("max_read_bytes (5 bytes)"), "{m}");
        let mut lines = Lines::new(&b"abcde\n"[..], "kestra:///x", Faults::default(), 5);
        assert_eq!(
            lines.next_row().unwrap(),
            Some(b"abcde".to_vec()),
            "at the cap"
        );
    }
}
