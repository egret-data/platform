//! W9b: namespace files (`docs/en/specs/2026-10-05-workflow-w9b-namespace-files-design.md`).
//!
//! - [`path`]: THE path rule ([`path::NsPath`], spec §3).
//! - [`access`]: THE access decision ([`access::authorize`], spec §4.6, §5)
//!   and the one namespace rule ([`access::check_namespace`]).
//! - [`store`]: the metadata table `workflow.namespace_files` (migration
//!   0169) and the objects behind it. Every function takes the
//!   [`access::Grant`] `authorize` returned; the workspace is the grant's.
//! - [`glob`]: Kestra's `PathMatcherPredicate` and Commons-IO's wildcard
//!   (Task 5; Tasks 6 and 7 reuse them).
//! - [`routes`]: the namespace file API (spec §9, Task 8).
//! - [`stage`]: `namespaceFiles` on script tasks and `inputFiles` values that
//!   name a namespace file (spec §7, Task 5).
//! - [`staging`]: the staging objects a write uploads its body to before it
//!   takes any lock, and their sweep (spec §4.2, §8.1; Task 8 fix round 1).
//!
//! The objects live in W9a's [`super::FileStore`], under
//! `{prefix}{workspace}/{ns-as-path}/_files{path}[.v{N}]`
//! ([`super::FileStore::namespace_object_uri`]).

pub mod access;
pub mod glob;
pub mod path;
pub mod routes;
pub mod stage;
pub mod staging;
pub mod store;

use std::sync::{Arc, OnceLock};

use sqlx::postgres::PgConnectOptions;
use uuid::Uuid;

/// Why a namespace-file operation failed. `Author` .. `TooLarge` are the
/// author's (a failed task, a 4xx); `Store` and `Db` are infrastructure (a
/// handler returns `Err`, a route answers 503), spec §11.
#[derive(Debug, thiserror::Error)]
pub enum NsError {
    /// A rule, a refused namespace, a missing object behind a row.
    #[error("{0}")]
    Author(String),
    /// No live row; the message names the path, never another workspace.
    #[error("{0}")]
    NotFound(String),
    /// ERROR conflict, file/directory clash, move onto a live target.
    #[error("{0}")]
    Conflict(String),
    /// A cap, named.
    #[error("{0}")]
    TooLarge(String),
    /// The object store failed: infrastructure.
    #[error("{0}")]
    Store(String),
    /// The database failed: infrastructure.
    #[error(transparent)]
    Db(#[from] egret_core::Error),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreatedBy {
    User(Uuid),
    Execution(Uuid),
    /// A task run of an execution (W9b Task 6 fix round 1, I1): the row
    /// records both, so `UploadFiles` re-claimed after an infrastructure
    /// failure recognises the files its earlier attempt wrote
    /// ([`store::latest_written`]).
    TaskRun {
        execution: Uuid,
        task_run: Uuid,
    },
}

static DB_OPTIONS: OnceLock<Arc<PgConnectOptions>> = OnceLock::new();

/// Connect options for render-time lookups (Task 4). Set once by egret-bin
/// from its pool. A second call returns the options it was given back, and
/// the first stays.
// The interface hands the refused options back, as `storage::install` hands
// back its store; a boot error, never a hot path.
#[allow(clippy::result_large_err)]
pub fn install_db(opts: PgConnectOptions) -> Result<(), PgConnectOptions> {
    let mut given = Some(opts);
    DB_OPTIONS.get_or_init(|| Arc::new(given.take().expect("taken once")));
    match given {
        None => Ok(()),
        Some(refused) => Err(refused),
    }
}

/// The installed connect options, `None` when this process has none.
pub fn db_options() -> Option<Arc<PgConnectOptions>> {
    DB_OPTIONS.get().cloned()
}

/// Idempotent: `pool.connect_options()` (one database per test process).
#[cfg(any(test, feature = "test-util"))]
pub fn install_db_for_tests(pool: &sqlx::PgPool) {
    DB_OPTIONS.get_or_init(|| Arc::new((*pool.connect_options()).clone()));
}
