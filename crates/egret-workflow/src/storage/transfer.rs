//! Files INTO a task container, as a **streamed** POSIX ustar archive (W9a
//! spec §5): `docker cp -` reads it from its stdin, and on Kubernetes the
//! `files-in` helper's `tar -x` reads it from the exec's. And files OUT, the
//! other way (W9a Task 6): [`TarReader`] reads `docker cp c:<path> -` or the
//! `files-out` helper's `tar -c` entry by entry, as it arrives, for
//! [`collect`](super::collect) to stream into the store.
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
//! # The archive
//!
//! Every parent directory first, then every file in name order -- the bytes
//! W5a's in-memory packer (`worker::k8s::transfer::pack`, deleted in W9a
//! Task 6) wrote for the same contents; Task 5 proved the two byte-identical
//! before the packer went. Since Task 6 the archive is checked by reading it
//! back with [`TarReader`] (`write_tar_reads_back_entry_by_entry`).

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
use crate::worker::commands::{staged_name_problem, StoredInput};

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
    /// An empty directory the command needs to exist before it starts --
    /// `outputDirectory: true`'s [`OUTPUT_DIR`](crate::worker::commands::OUTPUT_DIR)
    /// (W9a Task 6), as Kestra creates its output directory.
    Directory,
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
///
/// Every entry carries ONE modification time, this call's `now` (fix round
/// 1, I1): W5a's `docker cp` of a staging directory carried real times, and
/// Kestra's tar does; a 1970 mtime breaks readers such as Python's `zipfile`,
/// which refuses times before 1980.
pub async fn write_tar(
    out: &mut (dyn AsyncWrite + Send + Unpin),
    files: &BTreeMap<String, TarSource<'_>>,
    store: Option<&FileStore>,
) -> Result<(), TarError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    write_tar_at(out, files, store, now).await
}

/// [`write_tar`] with every entry's mtime `mtime` (seconds since the epoch).
pub(crate) async fn write_tar_at(
    out: &mut (dyn AsyncWrite + Send + Unpin),
    files: &BTreeMap<String, TarSource<'_>>,
    store: Option<&FileStore>,
    mtime: u64,
) -> Result<(), TarError> {
    let dirs = plan(files).map_err(TarError::Author)?;
    write_entries(out, &dirs, files, store, mtime).await?;
    out.write_all(&ZEROS).await.map_err(TarError::Write)?;
    out.write_all(&ZEROS).await.map_err(TarError::Write)?;
    out.flush().await.map_err(TarError::Write)
}

/// The directories, then the sources, of an archive, without its end marker.
async fn write_entries(
    out: &mut (dyn AsyncWrite + Send + Unpin),
    dirs: &BTreeSet<String>,
    files: &BTreeMap<String, TarSource<'_>>,
    store: Option<&FileStore>,
    mtime: u64,
) -> Result<(), TarError> {
    for d in dirs {
        let own = files
            .get(d.trim_end_matches('/'))
            .is_some_and(|s| matches!(s, TarSource::Directory));
        let mode = if own { OUTPUT_DIR_MODE } else { 0o755 };
        let h = header(d, 0, b'5', mode, mtime).map_err(TarError::Author)?;
        out.write_all(&h).await.map_err(TarError::Write)?;
    }
    for (name, source) in files {
        match source {
            TarSource::Directory => {}
            TarSource::Contents(contents) => {
                let bytes = contents.as_bytes();
                let h = header(name, bytes.len() as u64, b'0', 0o644, mtime)
                    .map_err(TarError::Author)?;
                out.write_all(&h).await.map_err(TarError::Write)?;
                out.write_all(bytes).await.map_err(TarError::Write)?;
                pad(out, bytes.len() as u64).await?;
            }
            TarSource::Stored(stored) => copy_stored(out, name, stored, store, mtime).await?,
        }
    }
    Ok(())
}

/// A restore of an archive (spec §5.4): `overlay` is written first, then every
/// regular file of `base` that `keep` accepts and `overlay` does not name, so
/// the overlay wins a collision. A cache is restored this way before the
/// block's own files are written over it. The directories of `base` are not
/// carried: a file's parents are made by the extraction that reads it.
///
/// A `base` that breaks off or is not a ustar stream is the store's (`Store`),
/// since egret wrote it; the names are the author's, as in [`write_tar`].
pub async fn write_tar_over(
    out: &mut (dyn AsyncWrite + Send + Unpin),
    base: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
    keep: &(dyn Fn(&str) -> bool + Sync),
    overlay: &BTreeMap<String, TarSource<'_>>,
    store: Option<&FileStore>,
) -> Result<(), TarError> {
    use tokio::io::AsyncReadExt;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let dirs = plan(overlay).map_err(TarError::Author)?;
    write_entries(out, &dirs, overlay, store, now).await?;
    let mut reader = TarReader::new(base);
    while let Some(entry) = reader
        .next_entry()
        .await
        .map_err(|e| TarError::Store(format!("the archive could not be read: {e}")))?
    {
        if entry.kind != EntryKind::File || overlay.contains_key(&entry.name) || !keep(&entry.name)
        {
            continue;
        }
        let h = header(&entry.name, entry.size, b'0', entry.mode, now).map_err(TarError::Author)?;
        out.write_all(&h).await.map_err(TarError::Write)?;
        let mut body = reader.body();
        let mut left = entry.size;
        let mut buf = vec![0u8; CHUNK];
        while left > 0 {
            let want = buf.len().min(usize::try_from(left).unwrap_or(usize::MAX));
            let n = body
                .read(&mut buf[..want])
                .await
                .map_err(|e| TarError::Store(format!("{}: {e}", entry.name)))?;
            if n == 0 {
                return Err(TarError::Store(format!(
                    "the archive ends inside {:?}, which declares {} bytes",
                    entry.name, entry.size
                )));
            }
            out.write_all(&buf[..n]).await.map_err(TarError::Write)?;
            left -= n as u64;
        }
        pad(out, entry.size).await?;
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
    mtime: u64,
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
    let h = header(name, stored.size, b'0', 0o644, mtime).map_err(TarError::Author)?;
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
    for (name, source) in files {
        if let Some(problem) = staged_name_problem(name) {
            return Err(problem);
        }
        split_name(name)?;
        let mut acc = String::new();
        let parts: Vec<&str> = name.split('/').collect();
        let own = usize::from(matches!(source, TarSource::Directory));
        for p in &parts[..(parts.len() - 1 + own)] {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(p);
            dirs.insert(format!("{acc}/"));
        }
    }
    for (name, source) in files {
        if matches!(source, TarSource::Directory) {
            continue;
        }
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

/// The mode of a [`TarSource::Directory`] -- the output directory a task
/// writes into: world-writable (W9a Task 9). `docker cp` makes every entry
/// root-owned, and the plugin host image runs as `USER 65532`
/// (`deploy/docker/plugin-host.Dockerfile`), whose `EgretStorage` writes
/// `.egret/out/`; at `0o755` every plugin write failed in the live test. The
/// directory exists only inside the task's own container. Its parents keep
/// `0o755`.
pub(crate) const OUTPUT_DIR_MODE: u32 = 0o777;

/// One ustar header block.
pub(crate) fn header(
    name: &str,
    size: u64,
    kind: u8,
    mode: u32,
    mtime: u64,
) -> Result<[u8; BLOCK], String> {
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
    octal(&mut h[136..148], mtime.min(MAX_USTAR_SIZE));
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

// ---------------------------------------------------------------------------
// Files OUT: an archive read as a stream (W9a Task 6)
// ---------------------------------------------------------------------------

/// What one archive entry is. Only [`EntryKind::File`] carries contents a
/// caller takes; every other kind is refused or skipped by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// A regular file (`0`, NUL, or `7`).
    File,
    Directory,
    /// A symbolic link and its target. Its target's bytes are never read.
    Symlink(String),
    /// A hard link and the entry it repeats.
    HardLink(String),
    /// A device, a FIFO, anything else: the type flag.
    Other(char),
}

/// One entry's header, as [`TarReader::next_entry`] read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TarEntry {
    /// The full name: a GNU `L` or PAX `path` record when there is one, else
    /// the ustar prefix and name.
    pub name: String,
    pub kind: EntryKind,
    /// The bytes of contents that follow the header (a PAX `size` wins).
    pub size: u64,
    /// The permission bits of the header's mode field (`0o7777`), so a copy
    /// keeps the execute bit a script needs (spec §5.2).
    pub mode: u32,
}

/// Why an archive could not be read. Both are the transfer failing -- the
/// process writing it went away or wrote something that is not ustar --
/// and the runner decides whose that is from the process's own exit.
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("the archive could not be read: {0}")]
    Io(#[source] std::io::Error),
    #[error("the archive is corrupt: {0}")]
    Corrupt(String),
}

/// The largest GNU long-name or PAX record egret reads into memory. A path
/// is at most 4 KiB on Linux; anything near this is not a header.
const MAX_META_BYTES: u64 = 64 * 1024;

/// A ustar stream read entry by entry -- `docker cp c:<path> -` (Docker) and
/// the `files-out` helper's `tar -c` (Kubernetes) -- with only one 512-byte
/// header (or a bounded long-name record) in memory at a time. An entry's
/// contents are read through [`TarReader::body`] as they arrive and never
/// collected; whatever a caller leaves unread is skipped by the next
/// [`TarReader::next_entry`].
///
/// Reads ustar, the GNU `L`/`K` long names and PAX `x` records (`path`,
/// `linkpath`, `size`) -- what busybox, GNU tar and Docker's Go
/// `archive/tar` write.
pub struct TarReader<'a> {
    inner: &'a mut (dyn tokio::io::AsyncRead + Send + Unpin),
    /// Contents of the current entry not yet read.
    left: u64,
    /// Padding after the current entry's contents.
    pad: u64,
    ended: bool,
}

impl<'a> TarReader<'a> {
    pub fn new(inner: &'a mut (dyn tokio::io::AsyncRead + Send + Unpin)) -> Self {
        Self {
            inner,
            left: 0,
            pad: 0,
            ended: false,
        }
    }

    /// The next entry's header; `None` at the end-of-archive block or at a
    /// clean end of the stream between entries.
    pub async fn next_entry(&mut self) -> Result<Option<TarEntry>, ReadError> {
        if self.ended {
            return Ok(None);
        }
        let skip = self.left + self.pad;
        self.left = 0;
        self.pad = 0;
        discard(self.inner, skip).await?;
        let mut long_name: Option<String> = None;
        let mut long_link: Option<String> = None;
        let mut pax_size: Option<u64> = None;
        loop {
            let mut h = [0u8; BLOCK];
            if !read_block(self.inner, &mut h).await? {
                self.ended = true;
                return Ok(None);
            }
            if h.iter().all(|b| *b == 0) {
                self.ended = true;
                return Ok(None);
            }
            check_sum(&h)?;
            let size = parse_size(&h[124..136])?;
            let mode = (parse_size(&h[100..108])? as u32) & 0o7777;
            let flag = h[156];
            if matches!(flag, b'L' | b'K' | b'x' | b'g') {
                if size > MAX_META_BYTES {
                    return Err(ReadError::Corrupt(format!(
                        "a {:?} record of {size} bytes",
                        flag as char
                    )));
                }
                let mut data = vec![0u8; size as usize];
                tokio::io::AsyncReadExt::read_exact(&mut *self.inner, &mut data)
                    .await
                    .map_err(ReadError::Io)?;
                discard(self.inner, pad_of(size)).await?;
                match flag {
                    b'L' => long_name = Some(cstr(&data)),
                    b'K' => long_link = Some(cstr(&data)),
                    b'x' => {
                        for (k, v) in pax_records(&data) {
                            match k.as_str() {
                                "path" => long_name = Some(v),
                                "linkpath" => long_link = Some(v),
                                "size" => {
                                    pax_size = Some(v.parse().map_err(|_| {
                                        ReadError::Corrupt(format!("PAX size {v:?}"))
                                    })?)
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
                continue;
            }
            let name = long_name.take().unwrap_or_else(|| name_of(&h));
            let size = pax_size.take().unwrap_or(size);
            let link = long_link.take().unwrap_or_else(|| cstr(&h[157..257]));
            let kind = match flag {
                b'0' | 0 | b'7' => EntryKind::File,
                b'5' => EntryKind::Directory,
                b'2' => EntryKind::Symlink(link),
                b'1' => EntryKind::HardLink(link),
                other => EntryKind::Other(other as char),
            };
            self.left = size;
            self.pad = pad_of(size);
            return Ok(Some(TarEntry {
                name,
                kind,
                size,
                mode,
            }));
        }
    }

    /// The current entry's contents, read as they arrive: at most its size,
    /// and a stream that ends sooner is an `UnexpectedEof` error.
    pub fn body(&mut self) -> EntryBody<'_, 'a> {
        EntryBody { reader: self }
    }
}

/// [`TarReader::body`].
pub struct EntryBody<'r, 'a> {
    reader: &'r mut TarReader<'a>,
}

impl tokio::io::AsyncRead for EntryBody<'_, '_> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        use std::task::Poll;
        let r = &mut *self.get_mut().reader;
        if r.left == 0 || buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let max = usize::try_from(r.left)
            .unwrap_or(usize::MAX)
            .min(buf.remaining());
        let mut sub = buf.take(max);
        match std::pin::Pin::new(&mut *r.inner).poll_read(cx, &mut sub) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Ready(Ok(())) => {}
        }
        let n = sub.filled().len();
        if n == 0 {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("the archive ended {} bytes short of the entry", r.left),
            )));
        }
        // SAFETY: `sub` is a view of `buf`'s unfilled part, and `n` of its
        // bytes were just initialised by the read.
        unsafe { buf.assume_init(n) };
        buf.advance(n);
        r.left -= n as u64;
        Poll::Ready(Ok(()))
    }
}

/// One whole header block; `false` at a clean end of the stream.
async fn read_block(
    r: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
    h: &mut [u8; BLOCK],
) -> Result<bool, ReadError> {
    use tokio::io::AsyncReadExt;
    let mut got = 0;
    while got < BLOCK {
        let n = r.read(&mut h[got..]).await.map_err(ReadError::Io)?;
        if n == 0 {
            if got == 0 {
                return Ok(false);
            }
            return Err(ReadError::Corrupt(
                "the archive ended inside a header".to_string(),
            ));
        }
        got += n;
    }
    Ok(true)
}

/// Read and drop exactly `n` bytes.
async fn discard(
    r: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
    n: u64,
) -> Result<(), ReadError> {
    use tokio::io::AsyncReadExt;
    if n == 0 {
        return Ok(());
    }
    let copied = tokio::io::copy(&mut (&mut *r).take(n), &mut tokio::io::sink())
        .await
        .map_err(ReadError::Io)?;
    if copied != n {
        return Err(ReadError::Corrupt(
            "the archive ended inside an entry".to_string(),
        ));
    }
    Ok(())
}

fn pad_of(size: u64) -> u64 {
    size.next_multiple_of(BLOCK as u64) - size
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|c| *c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

/// The ustar name; the prefix field is a prefix only under the POSIX magic
/// (`ustar\0`) -- old GNU headers (`ustar  \0`) keep other fields there.
fn name_of(h: &[u8]) -> String {
    let base = cstr(&h[0..100]);
    let prefix = if &h[257..263] == b"ustar\0" {
        cstr(&h[345..500])
    } else {
        String::new()
    };
    if prefix.is_empty() {
        base
    } else {
        format!("{prefix}/{base}")
    }
}

/// PAX records, `"<len> <key>=<value>\n"`, each `len` bytes long in all.
fn pax_records(data: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let Some(sp) = data[i..].iter().position(|b| *b == b' ') else {
            break;
        };
        let Some(len) = std::str::from_utf8(&data[i..i + sp])
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
        else {
            break;
        };
        if len <= sp + 1 || i + len > data.len() {
            break;
        }
        let rec = &data[i + sp + 1..i + len];
        let rec = rec.strip_suffix(b"\n").unwrap_or(rec);
        if let Some(eq) = rec.iter().position(|b| *b == b'=') {
            out.push((
                String::from_utf8_lossy(&rec[..eq]).into_owned(),
                String::from_utf8_lossy(&rec[eq + 1..]).into_owned(),
            ));
        }
        i += len;
    }
    out
}

fn parse_size(f: &[u8]) -> Result<u64, ReadError> {
    if f[0] & 0x80 != 0 {
        // GNU base-256: big-endian, the first byte's top bit is the flag.
        let mut v: u64 = (f[0] & 0x7f) as u64;
        for b in &f[1..] {
            v = v
                .checked_mul(256)
                .and_then(|v| v.checked_add(*b as u64))
                .ok_or_else(|| ReadError::Corrupt("an entry size overflows".to_string()))?;
        }
        return Ok(v);
    }
    let s = cstr(f);
    let s = s.trim();
    if s.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(s, 8)
        .map_err(|_| ReadError::Corrupt(format!("entry size {s:?} is not octal")))
}

fn check_sum(h: &[u8]) -> Result<(), ReadError> {
    let stored = cstr(&h[148..156]);
    let stored = stored.trim();
    let want = u32::from_str_radix(stored, 8)
        .map_err(|_| ReadError::Corrupt(format!("header checksum {stored:?} is not octal")))?;
    let got: u32 = h
        .iter()
        .enumerate()
        .map(|(i, b)| {
            if (148..156).contains(&i) {
                32
            } else {
                *b as u32
            }
        })
        .sum();
    if want != got {
        return Err(ReadError::Corrupt(format!(
            "header checksum {want} does not match {got}"
        )));
    }
    Ok(())
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

    /// Every entry `write_tar` writes reads back through [`TarReader`] as
    /// itself: parents first, names (a long one through the ustar prefix),
    /// kinds and contents.
    #[tokio::test]
    async fn write_tar_reads_back_entry_by_entry() {
        let long = format!("{}/{}", "d".repeat(120), "f".repeat(90));
        let files = BTreeMap::from([
            ("a.txt".to_string(), "hello\n".to_string()),
            ("dir/sub/b.txt".to_string(), "x".repeat(700)),
            ("dir/c".to_string(), String::new()),
            ("exact".to_string(), "y".repeat(512)),
            (long.clone(), "z".to_string()),
        ]);
        let mut sources: BTreeMap<String, TarSource<'_>> = files
            .iter()
            .map(|(k, v)| (k.clone(), TarSource::Contents(v)))
            .collect();
        sources.insert(".egret/out".to_string(), TarSource::Directory);
        let mut tar = Vec::new();
        write_tar_at(&mut tar, &sources, None, 1_700_000_000)
            .await
            .expect("write_tar");
        assert_eq!(tar.len() % BLOCK, 0);
        let mut src = &tar[..];
        let mut reader = TarReader::new(&mut src);
        let mut seen = Vec::new();
        while let Some(e) = reader.next_entry().await.expect("reads") {
            let mut body = Vec::new();
            tokio::io::AsyncReadExt::read_to_end(&mut reader.body(), &mut body)
                .await
                .expect("body");
            seen.push((e.name, e.kind, String::from_utf8(body).unwrap()));
        }
        let dirs: Vec<&str> = seen
            .iter()
            .filter(|(_, k, _)| *k == EntryKind::Directory)
            .map(|(n, _, _)| n.as_str())
            .collect();
        assert_eq!(
            dirs,
            [
                ".egret/",
                ".egret/out/",
                format!("{}/", "d".repeat(120)).as_str(),
                "dir/",
                "dir/sub/"
            ],
            "every parent, and the output directory itself, first"
        );
        let got: BTreeMap<String, String> = seen
            .into_iter()
            .filter(|(_, k, _)| *k == EntryKind::File)
            .map(|(n, _, b)| (n, b))
            .collect();
        assert_eq!(got, files);
        // The output directory is writable by a non-root task (W9a Task 9);
        // its parent and every other directory are not.
        let mode_of = |name: &str| -> u32 {
            let block = tar
                .chunks(BLOCK)
                .find(|b| b[..name.len()] == *name.as_bytes() && b[name.len()] == 0)
                .unwrap_or_else(|| panic!("no header for {name}"));
            let text = std::str::from_utf8(&block[100..107]).expect("octal");
            u32::from_str_radix(text.trim_matches(|c: char| c == '\0' || c == ' '), 8)
                .expect("a mode")
        };
        assert_eq!(mode_of(".egret/out/"), 0o777);
        assert_eq!(mode_of(".egret/"), 0o755);
        assert_eq!(mode_of("dir/sub/"), 0o755);
    }

    fn raw(name: &str, size: u64, kind: u8, link: &str) -> [u8; BLOCK] {
        let mut h = header(name, size, kind, 0o644, 0).unwrap();
        if !link.is_empty() {
            h[157..157 + link.len()].copy_from_slice(link.as_bytes());
        }
        h[148..156].copy_from_slice(b"        ");
        let sum: u32 = h.iter().map(|b| *b as u32).sum();
        h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        h
    }

    fn padded(out: &mut Vec<u8>, data: &[u8]) {
        out.extend_from_slice(data);
        out.resize(out.len().next_multiple_of(BLOCK), 0);
    }

    /// GNU `L`/`K` and PAX `x` records name the entry after them; a PAX
    /// `size` wins over the header's; a link's target is reported and its
    /// "contents" are none.
    #[tokio::test]
    async fn long_names_pax_records_and_links_are_read() {
        let mut tar = Vec::new();
        let name = "n".repeat(150);
        tar.extend_from_slice(&raw("././@LongLink", name.len() as u64 + 1, b'L', ""));
        padded(&mut tar, format!("{name}\0").as_bytes());
        tar.extend_from_slice(&raw("short", 3, b'0', ""));
        padded(&mut tar, b"abc");
        let pax = "30 path=données/中文 x.txt\n";
        assert_eq!(pax.len(), 30);
        tar.extend_from_slice(&raw("PaxHeaders/x", pax.len() as u64, b'x', ""));
        padded(&mut tar, pax.as_bytes());
        tar.extend_from_slice(&raw("ignored", 2, b'0', ""));
        padded(&mut tar, b"hi");
        tar.extend_from_slice(&raw("leaked.txt", 0, b'2', "/etc/passwd"));
        tar.extend_from_slice(&raw("hard", 0, b'1', "short"));
        tar.extend_from_slice(&raw("fifo", 0, b'6', ""));
        tar.extend_from_slice(&[0u8; 1024]);
        let mut src = &tar[..];
        let mut r = TarReader::new(&mut src);
        let e = r.next_entry().await.unwrap().unwrap();
        assert_eq!(
            (e.name.as_str(), &e.kind, e.size),
            (name.as_str(), &EntryKind::File, 3)
        );
        // The body is not read: the next entry skips it.
        let e = r.next_entry().await.unwrap().unwrap();
        assert_eq!(e.name, "données/中文 x.txt");
        let mut body = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut r.body(), &mut body)
            .await
            .unwrap();
        assert_eq!(body, b"hi");
        let e = r.next_entry().await.unwrap().unwrap();
        assert_eq!(e.kind, EntryKind::Symlink("/etc/passwd".into()));
        let e = r.next_entry().await.unwrap().unwrap();
        assert_eq!(e.kind, EntryKind::HardLink("short".into()));
        let e = r.next_entry().await.unwrap().unwrap();
        assert_eq!(e.kind, EntryKind::Other('6'));
        assert_eq!(r.next_entry().await.unwrap(), None);
        assert_eq!(r.next_entry().await.unwrap(), None, "and it stays ended");
    }

    #[tokio::test]
    async fn a_corrupt_or_cut_archive_is_an_error_and_an_empty_one_is_the_end() {
        let mut nothing: &[u8] = &[];
        assert_eq!(
            TarReader::new(&mut nothing).next_entry().await.unwrap(),
            None
        );
        let mut tar = Vec::new();
        tar.extend_from_slice(&raw("a", 10, b'0', ""));
        padded(&mut tar, b"0123456789");
        let mut bad = tar.clone();
        bad[0] ^= 1;
        let mut src = &bad[..];
        let err = TarReader::new(&mut src).next_entry().await.unwrap_err();
        assert!(err.to_string().contains("checksum"), "{err}");
        let mut src = &tar[..100];
        let err = TarReader::new(&mut src).next_entry().await.unwrap_err();
        assert!(err.to_string().contains("inside a header"), "{err}");
        let mut src = &tar[..BLOCK + 4];
        let mut r = TarReader::new(&mut src);
        r.next_entry().await.unwrap().unwrap();
        let mut body = Vec::new();
        let err = tokio::io::AsyncReadExt::read_to_end(&mut r.body(), &mut body)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
        assert_eq!(body, b"0123", "what arrived was handed on, nothing more");
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
        for bad in ["../x", "/etc/passwd", "", "x\0/../../etc/passwd"] {
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
        assert!(header("x", MAX_USTAR_SIZE + 1, b'0', 0o644, 0).is_err());
        assert!(header("x", MAX_USTAR_SIZE, b'0', 0o644, 0).is_ok());
    }

    /// Fix round 1, I1: every entry carries this archive's `now` -- one time
    /// for all of them -- never 1970, which Python's `zipfile` refuses.
    #[tokio::test]
    async fn every_entry_carries_one_real_modification_time() {
        let before = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let tar = tar_of(
            &BTreeMap::from([
                ("a.txt".to_string(), TarSource::Contents("x")),
                ("d/b.txt".to_string(), TarSource::Contents("y")),
            ]),
            None,
        )
        .await
        .expect("write_tar");
        let after = before + 60;
        let mtimes: Vec<u64> = tar
            .chunks(BLOCK)
            .filter(|h| &h[257..262] == b"ustar")
            .map(|h| {
                let field = std::str::from_utf8(&h[136..147]).unwrap();
                u64::from_str_radix(field, 8).unwrap()
            })
            .collect();
        assert_eq!(mtimes.len(), 3, "d/, a.txt, d/b.txt");
        // 1980-01-01, zipfile's floor.
        const ZIP_EPOCH: u64 = 315_532_800;
        for m in &mtimes {
            assert!(*m >= before && *m <= after && *m >= ZIP_EPOCH, "{mtimes:?}");
        }
        assert!(
            mtimes.windows(2).all(|w| w[0] == w[1]),
            "one time: {mtimes:?}"
        );
    }
}
