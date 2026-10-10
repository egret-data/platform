//! Files OUT of a task container, into internal storage (W9a spec §5, §6.2).
//!
//! A runner reads an archive of the container's files -- `docker cp c:<path> -`
//! on Docker, the `files-out` helper's `tar -c` on Kubernetes -- through
//! [`TarReader`], and hands every regular file's contents here as they
//! arrive. Each one is streamed to the object store ([`collect_one`]): **no
//! file is ever held whole in egret's memory**, and nothing touches the
//! worker's disk, so a link the author planted can never resolve against the
//! worker host (the hazard W5a's host staging directory had, and refused
//! component by component; a link entry has no contents here to follow).
//!
//! # Whose error
//!
//! [`CollectError`] says, because the runner must split it (spec §11):
//!
//! * [`CollectError::Author`] -- a cap crossed, a name no URI can carry, a
//!   directory, link or special file declared as an output: the task fails,
//!   naming the file;
//! * [`CollectError::Store`] -- the store refused or did not answer: egret's
//!   outage, a worker error (the run is re-claimed, no attempt is spent);
//! * [`CollectError::Source`] -- the archive itself broke off: the runner
//!   decides from the process that was writing it (a `tar -c` that died is
//!   the pod's; a dropped connection is egret's).
//!
//! # Nothing is indexed here
//!
//! A collected file is a [`StoredFile`] on the task's result. Its index row is
//! written by `apply_outcome`, in the transaction that commits the run's
//! outcome, behind the lease fence (spec §4.3); a file uploaded by a run that
//! never commits is an orphan the pruner sweeps (§10). Names are
//! deterministic per task run, so a re-claim overwrites its own uploads.

use std::collections::{BTreeMap, BTreeSet};

use super::transfer::{EntryKind, ReadError, TarReader};
use super::{uri, FileStore, StoredFile};
use crate::worker::commands::{relative_problem, CollectTo, Naming, WORKING_DIR};

/// Why a file could not be collected, split by whose problem it is.
#[derive(Debug, thiserror::Error)]
pub enum CollectError {
    /// The author's: the task fails with this message.
    #[error("{0}")]
    Author(String),
    /// Internal storage refused the write or did not answer: egret's outage.
    #[error("{0}")]
    Store(String),
    /// The archive broke off; the runner decides whose that is.
    #[error("{0}")]
    Source(String),
}

impl From<ReadError> for CollectError {
    fn from(e: ReadError) -> Self {
        CollectError::Source(e.to_string())
    }
}

/// The URI one collected file gets: [`Naming::KestraOutputName`] hashes the
/// file's absolute path in the pod, `{WORKING_DIR}/{rel}`, as Kestra's
/// `FilesService` does (`IdUtils.from(path) + "-" + filename`, spaces as `+`);
/// [`Naming::UnderPrefixVerbatim`] keeps `rel` as it is.
pub fn uri_for(to: &CollectTo, rel: &str) -> Result<String, String> {
    match to.naming {
        Naming::KestraOutputName => {
            uri::output_file_uri(&to.task_prefix, &format!("{WORKING_DIR}/{rel}"))
        }
        Naming::UnderPrefixVerbatim => uri::verbatim_file_uri(&to.task_prefix, rel),
    }
}

/// One file out of a container: stream to put_stream, count against both caps, head for the real size.
///
/// `declared_size` is the archive entry's size and `body` yields exactly that
/// many bytes or fails. Both caps are judged on it **before** a byte is
/// uploaded; `task_bytes_so_far` is the run's running total and grows by the
/// size `head` reports.
///
/// The brief's signature returned `Result<_, String>`; that cannot say whose
/// error it is, which the runner must know (spec §11), so this returns a
/// [`CollectError`].
pub async fn collect_one(
    fs: &FileStore,
    to: &CollectTo,
    rel: &str,
    body: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
    declared_size: u64,
    task_bytes_so_far: &mut u64,
) -> Result<StoredFile, CollectError> {
    let caps = fs.caps;
    if declared_size > caps.max_file_bytes {
        return Err(CollectError::Author(format!(
            "the output file {rel:?} is {declared_size} bytes, over the {} one file may be \
             (workflow.storage.max_file_bytes)",
            caps.max_file_bytes
        )));
    }
    let total = task_bytes_so_far.saturating_add(declared_size);
    if total > caps.max_task_bytes {
        return Err(CollectError::Author(format!(
            "the output files of this task come to {total} bytes with {rel:?}, over the {} \
             one task run may store (workflow.storage.max_task_bytes)",
            caps.max_task_bytes
        )));
    }
    let uri = uri_for(to, rel).map_err(CollectError::Author)?;
    let object = fs
        .object_uri(to.workspace, &uri)
        .map_err(CollectError::Author)?;
    let mut counted = Watched {
        inner: body,
        failed: None,
    };
    let put = fs
        .store
        .put_stream(&object, &mut counted, Some(declared_size))
        .await;
    // The ARCHIVE failing is not the store failing, though `put_stream`
    // reports both as its own error: the reader says which it was.
    if let Some(e) = counted.failed {
        return Err(CollectError::Source(format!(
            "the output file {rel:?} could not be read out of the container: {e}"
        )));
    }
    let written = put.map_err(|e| {
        CollectError::Store(format!(
            "{uri} could not be stored: internal storage is unavailable: {e}"
        ))
    })?;
    // `head` for the real size (spec §4.2): what the index row will hold.
    let size = match fs.store.head(&object).await {
        Ok(Some(size)) => size,
        Ok(None) => {
            return Err(CollectError::Store(format!(
                "{uri} was written ({written} bytes) but internal storage does not have it"
            )))
        }
        Err(e) => {
            return Err(CollectError::Store(format!(
                "{uri} could not be checked: internal storage is unavailable: {e}"
            )))
        }
    };
    if size != declared_size {
        return Err(CollectError::Store(format!(
            "{uri} holds {size} bytes in internal storage after {declared_size} were written"
        )));
    }
    *task_bytes_so_far += size;
    Ok(StoredFile { uri, size })
}

/// The archive body as `put_stream` reads it, remembering the first error
/// it gave -- so a broken archive is told apart from a failing store.
struct Watched<'a> {
    inner: &'a mut (dyn tokio::io::AsyncRead + Send + Unpin),
    failed: Option<String>,
}

impl tokio::io::AsyncRead for Watched<'_> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let r = std::pin::Pin::new(&mut *this.inner).poll_read(cx, buf);
        if let std::task::Poll::Ready(Err(e)) = &r {
            this.failed.get_or_insert_with(|| e.to_string());
        }
        r
    }
}

/// §5: every URI under `task_prefix` that appears anywhere in `outputs` was collected.
///
/// "Anywhere" is every string the render snapshot would find: a string that
/// is a whole URI, a URI inside text, and an object key. A URI under another
/// prefix -- an input the task passed through, another task's file -- is
/// not this check's business: the snapshot decides whether it is readable.
pub fn check_outputs_collected(
    outputs: &serde_json::Value,
    task_prefix: &str,
    collected: &[StoredFile],
) -> Result<(), String> {
    let mut found = BTreeSet::new();
    super::scope::walk(outputs, &mut found);
    for uri in found {
        if uri.starts_with(task_prefix) && !collected.iter().any(|f| f.uri == uri) {
            return Err(format!(
                "the outputs name {uri}, a file under this task run's storage prefix that was \
                 never collected: an output may not point at a file that does not exist"
            ));
        }
    }
    Ok(())
}

/// What one declared name of `outputFiles` turned out to be: the first entry
/// of the archive of that one path. `Ok(None)`: the archive holds nothing
/// (the file is not there -- IGNORED, as Kestra's `missing_declared_file`) or
/// a glob matched a directory, which a glob skips.
pub async fn collect_declared(
    fs: &FileStore,
    to: &CollectTo,
    rel: &str,
    from_glob: bool,
    reader: &mut TarReader<'_>,
    task_bytes_so_far: &mut u64,
) -> Result<Option<StoredFile>, CollectError> {
    let Some(entry) = reader.next_entry().await? else {
        return Ok(None);
    };
    match entry.kind {
        EntryKind::File => collect_one(
            fs,
            to,
            rel,
            &mut reader.body(),
            entry.size,
            task_bytes_so_far,
        )
        .await
        .map(Some),
        // A glob visits files only (Kestra's `MatcherFileVisitor`).
        EntryKind::Directory if from_glob => Ok(None),
        EntryKind::Directory => Err(CollectError::Author(format!(
            "`outputFiles` declares {rel:?}, which is a DIRECTORY; name the files inside it \
             instead, or write them to the output directory (`outputDirectory: true`)"
        ))),
        kind => Err(CollectError::Author(not_a_file(
            "`outputFiles` declares",
            rel,
            &kind,
        ))),
    }
}

/// The refusal for an entry that is neither a file nor a directory. A link
/// is refused rather than followed or stored empty (what Kestra's
/// `Files.copy` of a link entry makes): an empty output the author did not
/// write is a wrong answer that looks like a right one.
fn not_a_file(said: &str, rel: &str, kind: &EntryKind) -> String {
    match kind {
        EntryKind::Symlink(target) => format!(
            "{said} {rel:?}, which is a SYMLINK (to {target:?}). A symlink is refused rather \
             than followed or returned empty: name the file itself instead"
        ),
        EntryKind::HardLink(target) => format!(
            "{said} {rel:?}, which is a hard link (to {target:?}), not a regular file; name a \
             regular file instead"
        ),
        EntryKind::Other(flag) => format!(
            "{said} {rel:?}, which is not a regular file (tar entry type {flag:?}); name a \
             regular file instead"
        ),
        EntryKind::File | EntryKind::Directory => {
            format!("{said} {rel:?}, which is not a regular file")
        }
    }
}

/// The archive's next entry, counted (fix round 1, M1): EVERY entry of the
/// output directory -- the root, a directory, a link, a file -- counts
/// against [`MAX_OUTPUT_DIR_ENTRIES`], so an archive of endless empty
/// directories ends by name rather than keeping the worker reading headers
/// with no byte or name cap ever crossed.
///
/// [`MAX_OUTPUT_DIR_ENTRIES`]: crate::worker::commands::MAX_OUTPUT_DIR_ENTRIES
async fn counted_next(
    reader: &mut TarReader<'_>,
    entries: &mut usize,
) -> Result<Option<super::transfer::TarEntry>, CollectError> {
    use crate::worker::commands::{MAX_OUTPUT_DIR_ENTRIES, OUTPUT_DIR};
    let entry = reader.next_entry().await?;
    if entry.is_some() {
        *entries += 1;
        if *entries > MAX_OUTPUT_DIR_ENTRIES {
            return Err(CollectError::Author(format!(
                "`outputDirectory`: {OUTPUT_DIR} holds more than {MAX_OUTPUT_DIR_ENTRIES} \
                 entries (files, directories and links together); one task may produce at \
                 most {MAX_OUTPUT_DIR_ENTRIES}"
            )));
        }
    }
    Ok(entry)
}

/// Every regular file of an archive of the output directory, keyed by its
/// path relative to that directory (`deep/d.txt`), each stored verbatim
/// under the task prefix -- Kestra's measured keying and URI
/// (`w9a-storage.json` `egret_expected.output_directory`). The archive's
/// first entry is the directory itself; an empty archive means the command
/// removed it, and nothing is collected.
///
/// `names_left` bounds how many files one task may produce, shared with the
/// declared names ([`MAX_OUTPUT_NAMES`](crate::worker::commands::MAX_OUTPUT_NAMES)).
pub async fn collect_directory(
    fs: &FileStore,
    to: &CollectTo,
    reader: &mut TarReader<'_>,
    task_bytes_so_far: &mut u64,
    names_left: &mut usize,
    out: &mut BTreeMap<String, StoredFile>,
) -> Result<(), CollectError> {
    use crate::worker::commands::{MAX_OUTPUT_NAMES, OUTPUT_DIR};
    // Fix round 1, M1: EVERY entry counts -- the root, a directory, a link --
    // so an archive of endless empty directories ends here by name rather
    // than keeping the worker reading headers with no cap ever crossed.
    let mut entries = 0usize;
    let Some(root) = counted_next(reader, &mut entries).await? else {
        return Ok(());
    };
    if root.kind != EntryKind::Directory {
        return Err(CollectError::Author(format!(
            "`outputDirectory`: {OUTPUT_DIR} is not a directory -- the command replaced it \
             ({})",
            not_a_file("it is", &root.name, &root.kind)
        )));
    }
    let under = format!("{}/", root.name.trim_end_matches('/'));
    let verbatim = CollectTo {
        naming: Naming::UnderPrefixVerbatim,
        ..to.clone()
    };
    while let Some(entry) = counted_next(reader, &mut entries).await? {
        let Some(rel) = entry.name.strip_prefix(&under) else {
            return Err(CollectError::Source(format!(
                "the archive of {OUTPUT_DIR} holds {:?}, outside it",
                entry.name
            )));
        };
        let rel = rel.trim_end_matches('/').to_string();
        if rel.is_empty() {
            continue;
        }
        match entry.kind {
            EntryKind::Directory => continue,
            EntryKind::File => {
                if let Some(problem) = relative_problem(&rel) {
                    return Err(CollectError::Author(format!(
                        "`outputDirectory`: {problem}"
                    )));
                }
                if *names_left == 0 {
                    return Err(CollectError::Author(format!(
                        "`outputFiles` and `outputDirectory` together resolve to more than \
                         {MAX_OUTPUT_NAMES} files; one task may produce at most \
                         {MAX_OUTPUT_NAMES}"
                    )));
                }
                *names_left -= 1;
                // Fix round 1, I1: a declared output's URI is public
                // (`{prefix}{IdUtils.from(path)}-{name}`), and this file's is
                // `{prefix}{rel}`: the author can make them one. Refused
                // BEFORE the upload, which would overwrite the declared
                // file's object, and as the author's error -- two outputs on
                // one URI would otherwise fail every commit as egret's.
                let uri = uri_for(&verbatim, &rel).map_err(CollectError::Author)?;
                if let Some((other, _)) = out.iter().find(|(_, f)| f.uri == uri) {
                    return Err(CollectError::Author(format!(
                        "`outputDirectory` holds {rel:?}, whose URI {uri} is the declared \
                         output {other:?}'s: two outputs cannot share one file; rename the \
                         one in the output directory"
                    )));
                }
                let file = collect_one(
                    fs,
                    &verbatim,
                    &rel,
                    &mut reader.body(),
                    entry.size,
                    task_bytes_so_far,
                )
                .await?;
                out.insert(rel, file);
            }
            kind => {
                return Err(CollectError::Author(not_a_file(
                    "`outputDirectory` holds",
                    &rel,
                    &kind,
                )))
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use egret_storage::{Listing, MemStore, ObjectStore, StorageError};
    use uuid::Uuid;

    use super::*;
    use crate::storage::transfer::header;
    use crate::storage::Caps;

    const WS: Uuid = Uuid::from_u128(0x7e57);
    const EXEC: Uuid = Uuid::from_u128(0xe1);
    const RUN: Uuid = Uuid::from_u128(0x41);

    fn prefix() -> String {
        uri::task_prefix("w9a.t6", "f", EXEC, "t", RUN).unwrap()
    }

    fn to(naming: Naming) -> CollectTo {
        CollectTo {
            workspace: WS,
            task_prefix: prefix(),
            naming,
        }
    }

    fn mem(caps: Caps) -> FileStore {
        FileStore {
            store: Arc::new(MemStore::new()),
            bucket: "egret-test".into(),
            prefix: "workflow/".into(),
            caps,
            retention_days: 30,
        }
    }

    fn caps(file: u64, task: u64) -> Caps {
        Caps {
            max_file_bytes: file,
            max_task_bytes: task,
            max_read_bytes: 1 << 20,
            ..Caps::default()
        }
    }

    async fn read_back(fs: &FileStore, uri: &str) -> Vec<u8> {
        fs.store
            .get(&fs.object_uri(WS, uri).unwrap())
            .await
            .expect("the object is there")
    }

    /// One archive entry: a header, its contents, padding.
    fn entry(out: &mut Vec<u8>, name: &str, kind: u8, contents: &[u8], link: &str) {
        let mut h = header(name, contents.len() as u64, kind, 0o644, 1_700_000_000).unwrap();
        if !link.is_empty() {
            h[157..157 + link.len()].copy_from_slice(link.as_bytes());
            h[148..156].copy_from_slice(b"        ");
            let sum: u32 = h.iter().map(|b| *b as u32).sum();
            h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        }
        out.extend_from_slice(&h);
        out.extend_from_slice(contents);
        out.resize(out.len().next_multiple_of(512), 0);
    }

    fn end(out: &mut Vec<u8>) {
        out.extend_from_slice(&[0u8; 1024]);
    }

    #[tokio::test]
    async fn a_file_is_streamed_under_its_kestra_name_and_reads_back_byte_identical() {
        let fs = mem(Caps::default());
        let bytes: Vec<u8> = (0..70_000u32).map(|i| (i % 251) as u8).collect();
        let mut so_far = 0;
        let got = collect_one(
            &fs,
            &to(Naming::KestraOutputName),
            "sub/x.csv",
            &mut &bytes[..],
            bytes.len() as u64,
            &mut so_far,
        )
        .await
        .expect("collected");
        // `w9a-storage.json` `id_utils_from`: /egret/working-dir/sub/x.csv.
        assert_eq!(
            got,
            StoredFile {
                uri: format!("{}10ehIGUrkxwg9tBZYZJQ9K-x.csv", prefix()),
                size: 70_000
            }
        );
        assert_eq!(so_far, 70_000);
        assert_eq!(read_back(&fs, &got.uri).await, bytes);
        // A space becomes `+` (FilesService), the hash is of the real path.
        let got = collect_one(
            &fs,
            &to(Naming::KestraOutputName),
            "a b.txt",
            &mut &b"hello a b"[..],
            9,
            &mut so_far,
        )
        .await
        .unwrap();
        assert_eq!(
            got.uri,
            format!("{}3a3goS5hpDPIJpezcLagQ0-a+b.txt", prefix())
        );
        let got = collect_one(
            &fs,
            &to(Naming::UnderPrefixVerbatim),
            "deep/d.txt",
            &mut &b"deep"[..],
            4,
            &mut so_far,
        )
        .await
        .unwrap();
        assert_eq!(got.uri, format!("{}deep/d.txt", prefix()));
    }

    #[tokio::test]
    async fn a_file_over_max_file_bytes_fails_naming_the_cap() {
        let fs = mem(caps(10, 1000));
        let mut so_far = 0;
        let err = collect_one(
            &fs,
            &to(Naming::KestraOutputName),
            "big.txt",
            &mut &[b'x'; 11][..],
            11,
            &mut so_far,
        )
        .await
        .unwrap_err();
        let CollectError::Author(msg) = err else {
            panic!("a cap is the author's: {err:?}")
        };
        assert!(
            msg.contains("big.txt")
                && msg.contains("11 bytes")
                && msg.contains("workflow.storage.max_file_bytes")
                && msg.contains("10"),
            "{msg}"
        );
        assert!(
            fs.store.list("", None).await.unwrap().objects.is_empty(),
            "nothing was uploaded"
        );
        // At the cap is not over it.
        collect_one(
            &fs,
            &to(Naming::KestraOutputName),
            "big.txt",
            &mut &[b'x'; 10][..],
            10,
            &mut so_far,
        )
        .await
        .expect("exactly the cap");
    }

    #[tokio::test]
    async fn files_over_max_task_bytes_together_fail_naming_the_cap() {
        let fs = mem(caps(100, 150));
        let mut so_far = 0;
        for name in ["one.bin", "two.bin"] {
            collect_one(
                &fs,
                &to(Naming::KestraOutputName),
                name,
                &mut &[b'x'; 70][..],
                70,
                &mut so_far,
            )
            .await
            .unwrap_or_else(|e| panic!("{name} is under both caps: {e}"));
        }
        assert_eq!(so_far, 140);
        let err = collect_one(
            &fs,
            &to(Naming::KestraOutputName),
            "three.bin",
            &mut &[b'x'; 70][..],
            70,
            &mut so_far,
        )
        .await
        .unwrap_err();
        let CollectError::Author(msg) = err else {
            panic!("the task cap is the author's: {err:?}")
        };
        assert!(
            msg.contains("three.bin")
                && msg.contains("210 bytes")
                && msg.contains("workflow.storage.max_task_bytes")
                && msg.contains("150"),
            "{msg}"
        );
    }

    #[tokio::test]
    async fn a_name_no_uri_can_carry_is_the_authors_error_naming_it() {
        let fs = mem(Caps::default());
        let mut so_far = 0;
        let err = collect_one(
            &fs,
            &to(Naming::UnderPrefixVerbatim),
            "what?.txt",
            &mut &b"x"[..],
            1,
            &mut so_far,
        )
        .await
        .unwrap_err();
        let CollectError::Author(msg) = err else {
            panic!("{err:?}")
        };
        assert!(msg.contains("what?.txt"), "{msg}");
    }

    /// A store that refuses every write: unreachable.
    #[derive(Debug)]
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
        async fn put_stream(
            &self,
            uri: &str,
            body: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
            _: Option<u64>,
        ) -> Result<u64, StorageError> {
            // Reads a little first, as a real upload would.
            let mut b = [0u8; 1];
            let _ = tokio::io::AsyncReadExt::read(body, &mut b).await;
            Err(StorageError::S3(format!("{uri}: connection refused")))
        }
    }

    #[tokio::test]
    async fn an_unreachable_store_is_an_outage_and_a_broken_archive_is_the_source() {
        let fs = FileStore {
            store: Arc::new(Down),
            ..mem(Caps::default())
        };
        let mut so_far = 0;
        let err = collect_one(
            &fs,
            &to(Naming::KestraOutputName),
            "a.txt",
            &mut &b"abc"[..],
            3,
            &mut so_far,
        )
        .await
        .unwrap_err();
        let CollectError::Store(msg) = err else {
            panic!("an unreachable store is egret's outage: {err:?}")
        };
        assert!(msg.contains("kestra:///") && msg.contains("a.txt"), "{msg}");

        // The archive ends early: the body fails, the store did not.
        let fs = mem(Caps::default());
        let mut archive = Vec::new();
        entry(&mut archive, "a.txt", b'0', b"0123456789", "");
        archive.truncate(512 + 4);
        let mut src = &archive[..];
        let mut reader = TarReader::new(&mut src);
        let err = collect_declared(
            &fs,
            &to(Naming::KestraOutputName),
            "a.txt",
            false,
            &mut reader,
            &mut so_far,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, CollectError::Source(_)), "{err:?}");
    }

    #[tokio::test]
    async fn a_declared_symlink_is_refused_by_name_and_its_target_never_read() {
        let fs = mem(Caps::default());
        let mut archive = Vec::new();
        entry(&mut archive, "leaked.txt", b'2', b"", "/etc/passwd");
        end(&mut archive);
        let mut src = &archive[..];
        let mut reader = TarReader::new(&mut src);
        let mut so_far = 0;
        let err = collect_declared(
            &fs,
            &to(Naming::KestraOutputName),
            "leaked.txt",
            false,
            &mut reader,
            &mut so_far,
        )
        .await
        .unwrap_err();
        let CollectError::Author(msg) = err else {
            panic!("{err:?}")
        };
        assert!(
            msg.contains("SYMLINK") && msg.contains("leaked.txt") && msg.contains("/etc/passwd"),
            "{msg}"
        );
        assert!(
            fs.store.list("", None).await.unwrap().objects.is_empty(),
            "nothing -- not even an empty file -- was stored for the link"
        );
    }

    #[tokio::test]
    async fn a_declared_directory_is_refused_a_globbed_one_skipped_a_missing_file_ignored() {
        let fs = mem(Caps::default());
        let mut dir = Vec::new();
        entry(&mut dir, "results/", b'5', b"", "");
        entry(&mut dir, "results/inner", b'0', b"x", "");
        end(&mut dir);
        let mut so_far = 0;
        for (from_glob, want_err) in [(false, true), (true, false)] {
            let mut src = &dir[..];
            let mut reader = TarReader::new(&mut src);
            let got = collect_declared(
                &fs,
                &to(Naming::KestraOutputName),
                "results",
                from_glob,
                &mut reader,
                &mut so_far,
            )
            .await;
            if want_err {
                let msg = got.unwrap_err().to_string();
                assert!(
                    msg.contains("DIRECTORY") && msg.contains("results"),
                    "{msg}"
                );
            } else {
                assert_eq!(got.unwrap(), None, "a glob skips a directory");
            }
        }
        let empty = [0u8; 1024];
        let mut src = &empty[..];
        let mut reader = TarReader::new(&mut src);
        assert_eq!(
            collect_declared(
                &fs,
                &to(Naming::KestraOutputName),
                "never.txt",
                false,
                &mut reader,
                &mut so_far
            )
            .await
            .unwrap(),
            None,
            "an empty archive is a file the command did not write"
        );
        let mut nothing: &[u8] = &[];
        let mut reader = TarReader::new(&mut nothing);
        assert_eq!(
            collect_declared(
                &fs,
                &to(Naming::KestraOutputName),
                "never.txt",
                false,
                &mut reader,
                &mut so_far
            )
            .await
            .unwrap(),
            None,
            "so is no archive at all"
        );
    }

    #[tokio::test]
    async fn the_output_directory_is_collected_verbatim_keyed_by_relative_path() {
        let fs = mem(Caps::default());
        let mut archive = Vec::new();
        entry(&mut archive, "out/", b'5', b"", "");
        entry(&mut archive, "out/od.txt", b'0', b"in outdir", "");
        entry(&mut archive, "out/deep/", b'5', b"", "");
        entry(&mut archive, "out/deep/d.txt", b'0', b"deep", "");
        end(&mut archive);
        let mut src = &archive[..];
        let mut reader = TarReader::new(&mut src);
        let mut so_far = 0;
        let mut left = 10;
        let mut out = BTreeMap::new();
        collect_directory(
            &fs,
            &to(Naming::KestraOutputName),
            &mut reader,
            &mut so_far,
            &mut left,
            &mut out,
        )
        .await
        .expect("collected");
        let uris: BTreeMap<&str, &str> = out
            .iter()
            .map(|(k, v)| (k.as_str(), v.uri.as_str()))
            .collect();
        let p = prefix();
        assert_eq!(
            uris,
            BTreeMap::from([
                ("deep/d.txt", format!("{p}deep/d.txt").as_str()),
                ("od.txt", format!("{p}od.txt").as_str()),
            ])
        );
        assert_eq!(read_back(&fs, &out["deep/d.txt"].uri).await, b"deep");
        assert_eq!((so_far, left), (13, 8));

        // A link inside is refused by name; a file where the directory should
        // be is refused too.
        for (archive, want) in [
            {
                let mut a = Vec::new();
                entry(&mut a, "out/", b'5', b"", "");
                entry(&mut a, "out/l", b'2', b"", "/etc/passwd");
                end(&mut a);
                (a, "SYMLINK")
            },
            {
                let mut a = Vec::new();
                entry(&mut a, "out", b'0', b"x", "");
                end(&mut a);
                (a, "not a directory")
            },
        ] {
            let mut src = &archive[..];
            let mut reader = TarReader::new(&mut src);
            let err = collect_directory(
                &fs,
                &to(Naming::KestraOutputName),
                &mut reader,
                &mut so_far,
                &mut left,
                &mut BTreeMap::new(),
            )
            .await
            .unwrap_err();
            assert!(
                matches!(&err, CollectError::Author(m) if m.contains(want)),
                "{err:?}"
            );
        }
    }

    #[tokio::test]
    async fn the_name_budget_is_shared_with_the_output_directory() {
        let fs = mem(Caps::default());
        let mut archive = Vec::new();
        entry(&mut archive, "out/", b'5', b"", "");
        for i in 0..3 {
            entry(&mut archive, &format!("out/{i}"), b'0', b"x", "");
        }
        end(&mut archive);
        let mut src = &archive[..];
        let mut reader = TarReader::new(&mut src);
        let mut left = 2;
        let err = collect_directory(
            &fs,
            &to(Naming::KestraOutputName),
            &mut reader,
            &mut 0,
            &mut left,
            &mut BTreeMap::new(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("1000"), "{err}");
    }

    /// **Fix round 1, I1: a directory file at a declared output's URI is the
    /// author's error, refused before its upload.** A declared output is
    /// stored at `{prefix}{IdUtils.from("/egret/working-dir/<rel>")}-<name>`,
    /// a name anyone can compute; an output-directory file is stored
    /// verbatim at `{prefix}{rel}`. Writing `$OUTPUT_DIR/<that hash>-a.txt`
    /// beside a declared `a.txt` would give two outputs ONE URI -- the second
    /// upload overwriting the first, and the commit's index insert failing
    /// as egret's error, re-claiming the run for ever.
    #[tokio::test]
    async fn a_directory_file_at_a_declared_outputs_uri_is_refused_before_its_upload() {
        let fs = mem(Caps::default());
        let declared_to = to(Naming::KestraOutputName);
        let declared = uri_for(&declared_to, "a.txt").unwrap();
        let mut out = BTreeMap::from([(
            "a.txt".to_string(),
            StoredFile {
                uri: declared.clone(),
                size: 9,
            },
        )]);
        let clash = declared.strip_prefix(&prefix()).unwrap().to_string();
        let mut archive = Vec::new();
        entry(&mut archive, "out/", b'5', b"", "");
        entry(
            &mut archive,
            &format!("out/{clash}"),
            b'0',
            b"overwrite",
            "",
        );
        end(&mut archive);
        let mut src = &archive[..];
        let mut reader = TarReader::new(&mut src);
        let err = collect_directory(&fs, &declared_to, &mut reader, &mut 9, &mut 10, &mut out)
            .await
            .unwrap_err();
        let CollectError::Author(msg) = &err else {
            panic!("the author's, never egret's: {err:?}")
        };
        assert!(
            msg.contains(&clash) && msg.contains("a.txt") && msg.contains(&declared),
            "{msg}"
        );
        assert!(
            fs.store.list("", None).await.unwrap().objects.is_empty(),
            "refused before the upload: nothing overwrote the declared file's object"
        );
        assert_eq!(out.len(), 1);
    }

    /// **Fix round 1, M1: every entry of the output directory counts**, an
    /// empty directory as much as a file: an archive of a million empty
    /// directories must not keep the worker reading headers with no file
    /// ever crossing a cap.
    #[tokio::test]
    async fn every_entry_of_the_output_directory_counts_against_its_entry_budget() {
        use crate::worker::commands::MAX_OUTPUT_DIR_ENTRIES;
        let fs = mem(Caps::default());
        let mut archive = Vec::new();
        entry(&mut archive, "out/", b'5', b"", "");
        for i in 0..MAX_OUTPUT_DIR_ENTRIES {
            entry(&mut archive, &format!("out/d{i}/"), b'5', b"", "");
        }
        end(&mut archive);
        let mut src = &archive[..];
        let mut reader = TarReader::new(&mut src);
        let err = collect_directory(
            &fs,
            &to(Naming::KestraOutputName),
            &mut reader,
            &mut 0,
            &mut 10,
            &mut BTreeMap::new(),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&err, CollectError::Author(m)
                if m.contains(&MAX_OUTPUT_DIR_ENTRIES.to_string()) && m.contains("entries")),
            "{err:?}"
        );
        // The control: one entry fewer (the root counts) is collected.
        let mut archive = Vec::new();
        entry(&mut archive, "out/", b'5', b"", "");
        for i in 0..MAX_OUTPUT_DIR_ENTRIES - 1 {
            entry(&mut archive, &format!("out/d{i}/"), b'5', b"", "");
        }
        end(&mut archive);
        let mut src = &archive[..];
        let mut reader = TarReader::new(&mut src);
        collect_directory(
            &fs,
            &to(Naming::KestraOutputName),
            &mut reader,
            &mut 0,
            &mut 10,
            &mut BTreeMap::new(),
        )
        .await
        .expect("at the budget");
    }

    fn stored(uri: &str) -> StoredFile {
        StoredFile {
            uri: uri.to_string(),
            size: 1,
        }
    }

    #[test]
    fn an_output_pointing_under_the_prefix_at_nothing_collected_fails() {
        let p = prefix();
        let real = format!("{p}abc-a.txt");
        let ghost = format!("{p}ghost.txt");
        for outputs in [
            serde_json::json!({"outputFiles": {"a.txt": real}, "vars": {"x": ghost}}),
            serde_json::json!({"vars": {"note": format!("see {ghost} please")}}),
            serde_json::json!({"vars": {ghost.clone(): 1}}),
            serde_json::json!({"vars": {"list": [1, [ghost]]}}),
        ] {
            let err = check_outputs_collected(&outputs, &p, &[stored(&real)])
                .expect_err(&format!("{outputs}"));
            assert!(err.contains(&ghost), "{err}");
        }
        check_outputs_collected(
            &serde_json::json!({"outputFiles": {"a.txt": real}, "vars": {"again": real}}),
            &p,
            &[stored(&real)],
        )
        .expect("every URI under the prefix was collected");
    }

    #[test]
    fn an_output_pointing_at_another_prefix_is_not_this_checks_business() {
        let p = prefix();
        let other_run = uri::task_prefix("w9a.t6", "f", EXEC, "t", Uuid::from_u128(0x42)).unwrap();
        let input = uri::input_uri("w9a.t6", "f", EXEC, "in", "a.txt").unwrap();
        check_outputs_collected(
            &serde_json::json!({"vars": {"a": format!("{other_run}x.txt"), "b": input}}),
            &p,
            &[],
        )
        .expect("another task run's file or an input is the snapshot's business");
    }
}
