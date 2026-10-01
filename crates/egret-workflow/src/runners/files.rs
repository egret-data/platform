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
    /// This run's baseUri (`…/tasks/{task}/{taskRunId}/`), None outside a task run.
    fn task_prefix(&self) -> Option<&str>;
    /// The first store OUTAGE a read met, naming the URI (spec §11): not the
    /// author's error, so `settle` and the worker turn it into an `Err` (the
    /// lease lapses, the run is re-claimed, no attempt is spent) instead of
    /// the failed render it also caused. `None` when every read was answered.
    fn infrastructure_failure(&self) -> Option<String> {
        None
    }
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
