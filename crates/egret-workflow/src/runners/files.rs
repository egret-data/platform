//! W9a Task 4: what the four file functions -- `read`, `fileSize`,
//! `fileExists`, `isFileEmpty` -- read through (spec §7.2).
//!
//! **No sqlx and no storage types here**: `runners` stays below `storage`.
//! The one implementation that answers from the index is
//! `storage::scope::Snapshot`, taken BEFORE the render on the caller's
//! connection; this module only names the seam.
//!
//! # Why a snapshot and not a lookup at render
//!
//! A render runs on its own thread (`env::render_str`), and inside `settle`
//! the only connection is the transaction's, which that thread cannot use --
//! and a pool would not see that transaction's uncommitted index rows (the
//! just-finished task's files). So every `kestra:///…` string the render can
//! reach without assembling it is resolved first, and an assembled URI that is
//! not among them is an error naming it (user ruling, 2026-10-01).
//!
//! **Updated 2026-10-05 (W9b Task 4): namespace files are looked up at
//! render.** Execution files stay snapshot-only, for the reason above. A
//! namespace file is committed outside any execution's transaction (W9b spec
//! §6), so every connection sees the same rows and nothing is lost by looking
//! it up late: [`classify`] sends a bare path, an `nsfile://` URI and a
//! `kestra:///…/_files/…` URI to [`RenderFiles::namespace_file`] and
//! [`RenderFiles::read_namespace_file`], which `storage::scope::Snapshot`
//! answers on a connection of its own, opened the first time a template
//! calls one of them -- so a path assembled at render (`read('a/' ~ x)`) is
//! read, where an assembled execution URI is not.

use std::fmt;
use std::sync::Arc;

/// Why a file is not there to answer: the one distinction `fileExists` needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileUnavailable {
    /// The URI's execution is readable and no such file was committed --
    /// `fileExists` answers `false`, the other three fail with this message
    /// (`w9a-storage.json` `file_functions.missing_in_own`).
    Missing(String),
    /// Anything else: malformed, not readable from here, not in the
    /// snapshot, no storage configured. All four fail with this message.
    Refused(String),
}

impl FileUnavailable {
    pub fn message(&self) -> &str {
        match self {
            Self::Missing(m) | Self::Refused(m) => m,
        }
    }
}

/// The files a render may read. See this module's doc.
pub trait RenderFiles: Send + Sync {
    /// The indexed file, or why it is not readable here. Never touches the database.
    fn lookup(&self, uri: &str) -> Result<FileMeta, FileUnavailable>;
    /// UTF-8 contents, capped at the store's max_read_bytes. Blocks on the runtime handle.
    fn read_utf8(&self, uri: &str) -> Result<String, String>;
    /// The bytes, capped at the store's max_read_bytes, for a reader that
    /// needs the whole file and does not decode it as text -- W9a Task 11's
    /// `Loop` over an ION file, whose file is often BINARY Ion (spec §9.3).
    /// `whole` finishes the over-cap message's "…: {whole}, so it reads only
    /// files up to that size". Blocks on the runtime handle, as `read_utf8`.
    fn read_bytes(&self, uri: &str, whole: &str) -> Result<Vec<u8>, String>;
    /// This run's baseUri (`…/tasks/{task}/{taskRunId}/`), None outside a task run.
    fn task_prefix(&self) -> Option<&str>;
    /// The first store OUTAGE a read met, naming the URI (spec §11): not the
    /// author's error, so `settle` and the worker turn it into an `Err` (the
    /// lease lapses, the run is re-claimed, no attempt is spent) instead of
    /// the failed render it also caused. `None` when every read was answered.
    fn infrastructure_failure(&self) -> Option<String> {
        None
    }
    /// `workflow.storage.max_read_bytes` of the store this scope reads, the
    /// Ion reader's cap for a `Loop` over a file; `None` with no store.
    fn max_read_bytes(&self) -> Option<u64> {
        None
    }
    /// W9b: a namespace file's metadata (a directory's included, R3).
    /// `Missing`: well-formed, readable, no live row (`fileExists` answers
    /// false). `Refused`: everything else.
    fn namespace_file(&self, args: &NsFileArgs<'_>) -> Result<NsFileMeta, FileUnavailable> {
        Err(FileUnavailable::Refused(needs_storage(args.path)))
    }
    /// W9b: UTF-8 contents, capped at max_read_bytes; non-UTF-8 is an error
    /// naming the path.
    fn read_namespace_file(&self, args: &NsFileArgs<'_>) -> Result<String, String> {
        Err(needs_storage(args.path))
    }
    /// The running flow's namespace (from the execution's flow, never a URI);
    /// `None` outside an execution. W9b Tasks 5, 6, 7 and 10 read it here
    /// and nowhere else.
    fn flow_namespace(&self) -> Option<&str> {
        None
    }
    /// W9b fix round 1 (I1): a render has ended. A scope that holds a
    /// connection for its namespace lookups releases it here, so none
    /// outlives the render that opened it. Called on the render thread by
    /// every template render (`env::render_str` and its kin, `render_bound`).
    fn end_render(&self) {}
}

/// The refusal of a scope that cannot reach namespace files.
fn needs_storage(path: &str) -> String {
    format!(
        "namespace file '{path}' cannot be read: reading a namespace file needs a running \
         execution with egret's internal storage configured"
    )
}

/// W9b: which kind of file a function's `path` names (spec §3, §6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileTarget {
    Execution,
    Namespace,
}

/// A string with no `://`, or starting `nsfile://` -> [`FileTarget::Namespace`];
/// a `kestra:///` URI whose path has the marker ([`namespace_marker`]: a
/// `_files` segment at index >= 1, before the first `executions/{uuid}`
/// pair) -> `Namespace`; anything else -> [`FileTarget::Execution`], where
/// W9a's rules apply unchanged (and refuse `s3://`, `file://`, …).
pub fn classify(path: &str) -> FileTarget {
    if !path.contains("://") || path.starts_with("nsfile://") {
        return FileTarget::Namespace;
    }
    match path.strip_prefix(crate::storage::uri::SCHEME_PREFIX) {
        Some(rest) => {
            let segments: Vec<&str> = rest.split('/').collect();
            if namespace_marker(&segments).is_some() {
                FileTarget::Namespace
            } else {
                FileTarget::Execution
            }
        }
        None => FileTarget::Execution,
    }
}

/// What a function was asked for, unparsed: storage parses it (the path
/// rule, the `nsfile://` authority, the revision).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NsFileArgs<'a> {
    pub path: &'a str,
    /// The `namespace` argument. Beside an `nsfile://` authority it is the
    /// caller's to refuse (Kestra's sentence, in `functions::file_function`);
    /// storage takes the authority.
    pub namespace: Option<&'a str>,
    /// The `revision` argument as the template gave it (`2`, `'2'`, `'x'`).
    pub revision: Option<String>,
}

/// One namespace file, or a directory (R3), as the functions see it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NsFileMeta {
    /// The path in its stored form, a directory's without its trailing '/'
    /// (`/fu`): what a message names.
    pub path: String,
    /// A directory's is 0.
    pub size: u64,
    pub is_directory: bool,
    /// `kestra:///…/_files/…[.vN]` of the resolved revision; `""` for a directory.
    pub uri: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileMeta {
    pub size: u64,
}

/// Identity-compared (Arc::ptr_eq), so RenderContext keeps PartialEq + Eq.
#[derive(Clone)]
pub struct FileScope(pub Arc<dyn RenderFiles>);

impl PartialEq for FileScope {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for FileScope {}

impl fmt::Debug for FileScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileScope")
            .field("task_prefix", &self.0.task_prefix())
            .finish_non_exhaustive()
    }
}

/// The render-time global the four functions look up. A space in it, like
/// CONTEXT_SCOPE_NAMES (env.rs), so no template can name it.
pub(crate) const FILE_SCOPE: &str = "egret file scope";

/// [`FILE_SCOPE`]'s value: an opaque object, so `fetchContext` keeps it out
/// of its answer by identity, exactly as it does `CONTEXT_SCOPE_NAMES`.
#[derive(Debug)]
pub(crate) struct FileScopeObject(pub(crate) FileScope);

impl minijinja::value::Object for FileScopeObject {}

/// W9b: the path segment that marks a namespace file in a `kestra:///` URI
/// and in an object key (`kestra:///{ns-as-path}/_files/{path}`, W9b spec
/// §3). One definition: `storage::uri` re-exports it.
pub const NAMESPACE_FILES_SEGMENT: &str = "_files";

/// The index of the FIRST segment equal to [`NAMESPACE_FILES_SEGMENT`] at
/// index 1 or later, when it comes BEFORE the first `executions/{uuid}` pair
/// -- the namespace-file marker (W9b spec §3, the user's ruling 2026-10-05).
/// Index 0 is never the marker: a namespace file needs a namespace before
/// it. A `_files` after the pair is part of an execution file's path.
pub fn namespace_marker(segments: &[&str]) -> Option<usize> {
    let marker = segments
        .iter()
        .skip(1)
        .position(|s| *s == NAMESPACE_FILES_SEGMENT)
        .map(|i| i + 1)?;
    match crate::storage::uri::execution_pair(segments) {
        Some((pair, _)) if pair < marker => None,
        _ => Some(marker),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// W9b spec §3, §6, and the marker rule of Task 2's fix round 1 (I1):
    /// `_files` counts only before the first `executions/{uuid}` pair.
    #[test]
    fn classify_routes_namespace_files_and_leaves_everything_else_to_w9a() {
        let exec = "kestra:///a/f/executions/0191e1a1-2b3c-7d4e-8f90-0123456789ab";
        for path in [
            "data/x.csv",
            "/data/x.csv",
            "",
            "nsfile:///x",
            "nsfile://other.ns/x",
            "kestra:///w9b/fn/_files/fu/one.txt",
            "kestra:///w9b/fn/_files/fu/three.txt.v3",
            "kestra:///a/_files/x/executions/0191e1a1-2b3c-7d4e-8f90-0123456789ab/f",
        ] {
            assert_eq!(classify(path), FileTarget::Namespace, "{path}");
        }
        for path in [
            format!("{exec}/tasks/t/r/out.txt"),
            format!("{exec}/tasks/t/r/_files/report.csv"),
            format!("{exec}/inputs/_files/a.txt"),
            "kestra:///_files/x".to_string(),
            "s3://bucket/x".to_string(),
            "file:///etc/passwd".to_string(),
        ] {
            assert_eq!(classify(&path), FileTarget::Execution, "{path}");
        }
    }
}
