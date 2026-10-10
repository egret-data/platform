//! The render snapshot: every `kestra:///…` string a render can reach,
//! resolved before it starts (W9a spec §7.2, as the user ruled 2026-10-01).
//!
//! # What the snapshot holds, and the rule it implements
//!
//! [`Snapshot::take`] reads every `kestra:///` URI out of the flow **source**
//! and out of every **JSON value** of the render context (inputs, the outputs
//! of every terminated task, `execution`, `item`) and resolves all of them at
//! once through [`super::resolve_readable_many`] -- the one readability
//! decision -- on the caller's POOL connection: a [`Snapshot`] reads, so it is
//! never built inside a transaction (below). Inside `settle` the same
//! resolution is taken by [`scope_in_transaction`] on the transaction that
//! has just indexed the finished task's files, which a pool would not see.
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
//!
//! **Only outside a transaction** (W9c Task 9, the user's ruling on W9c's
//! final review F2): a [`Snapshot`] is the scope of a handler's render and of
//! the prefetch's fetch. A render INSIDE a transaction -- `settle`'s -- gets
//! [`scope_in_transaction`], whose scope (`storage::prefetch::InTransaction`)
//! holds the same resolutions, taken the same way, but no store and no
//! connect options: every body, namespace-file and KV read there is answered
//! from the transaction owner's cache, and a miss is fetched by a
//! [`Snapshot`] with the transaction rolled back. **The types keep the two
//! apart** (final review m2): [`Snapshot::take`] and [`scope_for`] take a
//! `PoolConnection`, which a transaction is not, and
//! [`scope_in_transaction`] takes the attempt's `worker::txn::Gate`, which
//! only a gated attempt has.
//!
//! # Namespace files (W9b Task 4, 2026-10-05)
//!
//! Execution files are snapshot-only, for the reason above. **Namespace files
//! are looked up at render**, because they can be: they commit outside any
//! execution's transaction (W9b spec §6), so every connection sees every
//! committed row, and a path assembled at render reads like any other. The
//! render thread cannot borrow `settle`'s transaction, and `settle`'s callers
//! hold no pool, so the snapshot opens **one** connection of its own, on its
//! runtime `handle`, from `nsfiles::db_options()`, the first time a template
//! calls a namespace function, and keeps it for the rest of the scope. Not a
//! `PgPool`: a pool made in one `#[tokio::test]` runtime breaks in the next,
//! while the connect options are plain data. A failed connect or query is an
//! outage ([`Snapshot::outage`]), so the run is re-claimed with no attempt
//! spent (spec §11). Every lookup goes through `nsfiles::access::authorize`
//! (the running flow reads any namespace of its workspace) and
//! `nsfiles::store::resolve`; nothing here creates anything.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, LazyLock, Mutex};

use egret_core::Result;
use regex::Regex;
use sqlx::pool::PoolConnection;
use sqlx::postgres::PgConnectOptions;
use sqlx::{Connection, PgConnection, Postgres};
use tokio::io::AsyncReadExt;
use uuid::Uuid;

use super::kv::key::KvKey;
use super::kv::store as kv_store;
use super::nsfiles::access::{authorize, Access, Actor, Grant, Resource};
use super::nsfiles::path::NsPath;
use super::nsfiles::store::{self as ns_store, NsEntry, Want};
use super::nsfiles::NsError;
use super::{decode_utf8, readable_executions, resolve_readable_many, uri};
use super::{FileStore, IndexedFile, Unreadable};
use crate::runners::files::{
    FileMeta, FileScope, FileUnavailable, KvAnswer, KvArgs, NsFileArgs, NsFileMeta, RenderFiles,
    NSFILE_SCHEME,
};

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
    /// W9b: the running flow's namespace, from the execution's flow.
    flow_namespace: Option<String>,
    /// W9b: where a namespace lookup connects (`nsfiles::db_options()`).
    ns_db: Option<Arc<PgConnectOptions>>,
    /// W9b: the one connection namespace lookups use, opened on the first.
    ns_conn: Mutex<Option<PgConnection>>,
}

impl Snapshot {
    /// Every `kestra:///…` string in `texts` (regex, whole run of non-space, non-quote chars)
    /// resolved at once through resolve_readable_many on `conn`.
    ///
    /// `json` is walked: a string that starts with `kestra:///` is taken
    /// whole, and every other string, at any depth, is searched the same way.
    /// With no URI anywhere the database is not touched.
    #[allow(clippy::too_many_arguments)]
    pub async fn take(
        conn: &mut PoolConnection<Postgres>,
        store: Arc<FileStore>,
        workspace: Uuid,
        execution: Uuid,
        texts: &[&str],
        json: &[&serde_json::Value],
        task_prefix: Option<String>,
        flow_namespace: Option<String>,
    ) -> Result<Self> {
        let files = reachable(conn, workspace, execution, texts, json).await?;
        Ok(Self {
            files,
            store,
            workspace,
            handle: tokio::runtime::Handle::current(),
            task_prefix,
            outage: Mutex::new(None),
            flow_namespace,
            ns_db: super::nsfiles::db_options(),
            ns_conn: Mutex::new(None),
        })
    }

    /// The indexed file, or why it is not readable here, naming the URI --
    /// what staging (T5) and the bridge (T9) call.
    pub fn resolve(&self, uri: &str) -> std::result::Result<&IndexedFile, String> {
        resolve_in(&self.files, uri)
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
            flow_namespace: None,
            ns_db: None,
            ns_conn: Mutex::new(None),
        }
    }

    /// For tests: [`Self::from_parts`] that reaches namespace files as
    /// `flow_namespace`'s flow, connecting with `db`.
    #[cfg(any(test, feature = "test-util"))]
    pub fn with_namespace_files(
        mut self,
        flow_namespace: Option<String>,
        db: Option<Arc<PgConnectOptions>>,
    ) -> Self {
        self.flow_namespace = flow_namespace;
        self.ns_db = db;
        self
    }

    /// W9c Task 9: a live snapshot for the prefetch's fetch
    /// (`storage::prefetch`), which runs with no transaction open: `files`
    /// are the resolutions the rolled-back attempt recorded (its index rows
    /// were rolled back with it), and namespace and KV lookups connect with
    /// `ns_db`, as every render's do. Built on the runtime (it captures the
    /// handle), read on a thread that may block.
    pub(crate) fn for_fetch(
        files: BTreeMap<String, std::result::Result<IndexedFile, Unreadable>>,
        store: Arc<FileStore>,
        workspace: Uuid,
        flow_namespace: Option<String>,
        ns_db: Option<Arc<PgConnectOptions>>,
    ) -> Self {
        Self {
            files,
            store,
            workspace,
            handle: tokio::runtime::Handle::current(),
            task_prefix: None,
            outage: Mutex::new(None),
            flow_namespace,
            ns_db,
            ns_conn: Mutex::new(None),
        }
    }

    /// W9c Task 9: the outage the last read recorded, cleared, so the
    /// prefetch can tell which of several reads on one snapshot met it.
    pub(crate) fn take_outage(&self) -> Option<String> {
        self.outage.lock().unwrap_or_else(|p| p.into_inner()).take()
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
pub(super) fn not_in_snapshot(uri: &str) -> String {
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

impl Snapshot {
    /// Whether this thread may block on [`Self::handle`]: what every read
    /// checks first (W9a's store reads, W9b's namespace lookups).
    fn may_block(&self, what: &str) -> std::result::Result<(), String> {
        // `block_on` panics on a thread that is inside a runtime; the render
        // thread never is, and a caller that rendered on a tokio worker gets
        // an error rather than a panic.
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(format!(
                "{what} cannot be read on this thread: a file is read on egret's render \
                 thread, not on a runtime worker"
            ));
        }
        // C1: on a current-thread runtime the one thread that drives the IO is
        // the one waiting for this render, so a store read would never finish
        // (it does on MemStore, which proves nothing about S3).
        if self.handle.runtime_flavor() != tokio::runtime::RuntimeFlavor::MultiThread {
            return Err(format!(
                "{what} cannot be read: reading a file needs the multi-thread runtime, and \
                 this render runs on a current-thread one, whose only thread is waiting for it"
            ));
        }
        Ok(())
    }

    /// W9b: what a namespace function names -- its namespace (the URI's, the
    /// authority, the `namespace` argument or the running flow's), the
    /// path, the revision -- and the [`Grant`] to read it. Pure: no database.
    fn ns_target(&self, args: &NsFileArgs<'_>) -> std::result::Result<NsTarget, FileUnavailable> {
        let raw = args.path;
        let refused = |m: String| FileUnavailable::Refused(m);
        let own = self.flow_namespace.as_deref().ok_or_else(|| {
            refused(format!(
                "namespace file '{raw}' cannot be read: this render has no running flow, \
                 whose namespace a namespace file is read as"
            ))
        })?;
        let named = ns_ref(raw, own, args.namespace).map_err(refused)?;
        if let Some(uri_revision) = named.uri_revision {
            // The coordinator's ruling on concern 1 (2026-10-05, an
            // application of R3): the URI names one revision of one
            // namespace's file, so an argument that says otherwise is an
            // error naming both, never silently ignored as Kestra's `kestra`
            // branch does.
            if let Some(given) = &args.revision {
                return Err(refused(format!(
                    "the 'revision' argument ({given}) cannot be given beside {raw}: a \
                     kestra:// URI names exactly one revision (here {uri_revision})"
                )));
            }
            if let Some(given) = args.namespace.filter(|g| *g != named.namespace) {
                return Err(refused(format!(
                    "the 'namespace' argument '{given}' differs from namespace '{}', \
                     which {raw} names",
                    named.namespace
                )));
            }
        }
        let NsRef {
            namespace,
            path,
            uri_revision,
        } = named;
        // THE access decision: any namespace of this workspace, and the
        // running flow's own namespace must itself be one that holds files.
        let grant = authorize(
            self.workspace,
            Actor::Flow { namespace: own },
            &namespace,
            Resource::NamespaceFiles,
            Access::Read,
        )
        .map_err(refused)?;
        // A `kestra:///…/_files/p[.vN]` URI names exactly one revision (R1);
        // a `revision` argument beside it was refused above.
        let want = match (uri_revision, &args.revision) {
            (Some(n), _) => Want::Revision(n),
            (None, None) => Want::Latest,
            (None, Some(given)) => match given.parse::<i64>() {
                Ok(n) => match u32::try_from(n) {
                    Ok(n) if n >= 1 => Want::Revision(n),
                    _ => {
                        return Err(FileUnavailable::Missing(format!(
                            "revision {n} of namespace file '{path}' does not exist in \
                             namespace '{namespace}'"
                        )))
                    }
                },
                Err(_) => {
                    return Err(refused(format!(
                        "the 'revision' argument must be a valid integer, and {given:?} is \
                         not (namespace file '{path}' in namespace '{namespace}')"
                    )))
                }
            },
        };
        // A path ending with '/' names a directory; any other names a file,
        // and -- for a latest read -- the directory of that name when no file
        // row is there (the coordinator's ruling E, `four_functions_ns.*.directory`).
        let (file, dir) = if path.ends_with('/') && uri_revision.is_none() {
            (None, Some(NsPath::parse_dir(&path).map_err(refused)?))
        } else {
            let file = NsPath::parse_file(&path).map_err(refused)?;
            // A directory is always revision 1 (`NamespaceFile.java:138-148`),
            // so a URI with no `.vN` names it as well as a path does (M8).
            let dir = if want == Want::Latest || uri_revision == Some(1) {
                NsPath::parse_dir(&path).ok()
            } else {
                None
            };
            (Some(file), dir)
        };
        let shown = match (&file, &dir) {
            (Some(p), _) | (None, Some(p)) => p.as_str().to_string(),
            (None, None) => path,
        };
        Ok(NsTarget {
            grant,
            file,
            dir,
            want,
            shown,
        })
    }

    /// Run `target`'s lookup on this scope's one namespace connection,
    /// opening it first if this render has not yet. Blocks the render thread
    /// on [`Self::handle`], as W9a's store reads do.
    ///
    /// W9b fix round 1 (I1): the connection lives for one render at most --
    /// [`RenderFiles::end_render`] closes it -- and a lookup that fails on a
    /// connection an earlier lookup of the same render opened (the server
    /// may have ended it since) reconnects once before it is an outage.
    fn ns_lookup(
        &self,
        target: &NsTarget,
        read_cap: Option<u64>,
    ) -> std::result::Result<(NsEntry, Option<Vec<u8>>), FileUnavailable> {
        let what = format!(
            "namespace file '{}' in namespace '{}'",
            target.shown,
            target.grant.namespace()
        );
        self.may_block(&what).map_err(FileUnavailable::Refused)?;
        let found = self.on_ns_conn(&what, |mut conn| {
            Box::pin(async move {
                let looked = match find(&mut conn, target).await {
                    Ok(entry) => match read_cap {
                        None => Ok((entry, None)),
                        Some(cap) => match self.ns_read(target, &entry, cap).await {
                            Ok(bytes) => Ok((entry, Some(bytes))),
                            Err(e) => Err(e),
                        },
                    },
                    Err(e) => Err(e),
                };
                (conn, looked)
            })
        });
        found.map_err(|e| self.ns_unavailable(&what, e))
    }

    /// Run `work` on this scope's one namespace connection, opening it first
    /// if this render has not yet, and holding the scope's connection mutex
    /// for exactly as long as `work` runs: the mutex is released when this
    /// returns. Blocks the render thread on [`Self::handle`]; the caller has
    /// checked [`Self::may_block`].
    ///
    /// `work` is handed the connection by value and hands it back, so its
    /// future borrows nothing of this frame. A database outage (a failed
    /// connect or query) closes the connection, never reused; on a connection
    /// an earlier lookup of the same render opened (the server may have ended
    /// it since), it reconnects once before it is the answer (W9b fix round
    /// 1, I1). Shared by W9b's namespace files and W9c's `kv()`.
    fn on_ns_conn<'w, T>(
        &self,
        what: &str,
        mut work: impl FnMut(PgConnection) -> OnConn<'w, T>,
    ) -> std::result::Result<T, NsError> {
        let mut held = self.ns_conn.lock().unwrap_or_else(|p| p.into_inner());
        self.handle.block_on(async {
            let mut reused = held.is_some();
            loop {
                let conn = match held.take() {
                    Some(conn) => conn,
                    None => {
                        let Some(opts) = self.ns_db.as_deref() else {
                            return Err(NsError::Author(format!(
                                "{what} cannot be read: this egret process has no database \
                                 connection for namespace files or the KV store"
                            )));
                        };
                        PgConnection::connect_with(opts).await.map_err(db_err)?
                    }
                };
                let (conn, answered) = work(conn).await;
                match answered {
                    Err(e) if is_db_outage(&e) => {
                        // The connection may be the broken thing: never reuse it.
                        let _ = conn.close().await;
                        if std::mem::take(&mut reused) {
                            continue;
                        }
                        return Err(e);
                    }
                    other => {
                        *held = Some(conn);
                        return other;
                    }
                }
            }
        })
    }

    /// For tests: whether this scope holds a namespace connection now.
    #[cfg(any(test, feature = "test-util"))]
    pub fn holds_a_namespace_connection(&self) -> bool {
        self.ns_conn
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_some()
    }

    /// `entry`'s bytes, at most `cap + 1` of them (the `+1` is how a file
    /// over the cap is told from one at it).
    async fn ns_read(
        &self,
        target: &NsTarget,
        entry: &NsEntry,
        cap: u64,
    ) -> std::result::Result<Vec<u8>, NsError> {
        let ns = target.grant.namespace();
        if entry.is_directory {
            // `read._extra.directory`, measured: Kestra's sentence.
            return Err(NsError::Author(format!(
                "'{}' is a directory in namespace '{ns}', it has no content to read",
                entry.path.lock_path()
            )));
        }
        // The row knows the size: refuse without a download.
        if entry.size > cap {
            return Err(NsError::TooLarge(ns_over_cap(entry, ns, cap)));
        }
        let stream = ns_store::open(&self.store, &target.grant, entry).await?;
        let mut bytes = Vec::new();
        stream
            .take(cap.saturating_add(1))
            .read_to_end(&mut bytes)
            .await
            .map_err(|e| {
                NsError::Store(format!(
                    "revision {} of namespace file '{}' in namespace '{ns}' could not be read: \
                     internal storage is unavailable: {e}",
                    entry.revision, entry.path
                ))
            })?;
        Ok(bytes)
    }

    /// An outage (the database or the store unreachable) is recorded and
    /// refused; a missing row is `Missing`; anything else is refused.
    fn ns_unavailable(&self, what: &str, e: NsError) -> FileUnavailable {
        if is_db_outage(&e) {
            return FileUnavailable::Refused(self.outage(format!(
                "{what} could not be looked up: the database is unavailable: {e}"
            )));
        }
        match e {
            NsError::NotFound(m) => FileUnavailable::Missing(m),
            NsError::Store(m) => FileUnavailable::Refused(self.outage(m)),
            other => FileUnavailable::Refused(other.to_string()),
        }
    }

    /// W9c: why `kv()` could not answer. An outage (the database or the
    /// store unreachable, or a value that changed while it was read) is
    /// recorded, so the run is re-claimed with no attempt spent (spec §11);
    /// anything else is the author's error, its message as the store wrote it.
    fn kv_unavailable(&self, what: &str, e: NsError) -> String {
        if is_db_outage(&e) {
            return self.outage(format!(
                "{what} could not be looked up: the database is unavailable: {e}"
            ));
        }
        match e {
            NsError::Store(m) => self.outage(m),
            other => other.to_string(),
        }
    }

    /// W9c Task 4: `kv()` (spec §6). See [`RenderFiles::kv`].
    ///
    /// 1. [`Self::may_block`]; the key rule ([`KvKey::parse`]).
    /// 2. The levels: the `namespace` argument alone, or the running flow's
    ///    namespace and its parents ([`kv_store::ancestors`]); each through
    ///    THE access decision, `authorize(…, Resource::Kv, Access::Read)`,
    ///    as the flow, in this scope's workspace (the execution's). The first
    ///    refusal is the answer.
    /// 3. One statement on the render's namespace connection
    ///    ([`Self::on_ns_conn`]): [`kv_store::resolve_first_live`]. Then the
    ///    connection mutex is released.
    /// 4. [`kv_store::read_sized`], holding no connection of this scope: it
    ///    downloads, then decodes on the blocking pool.
    /// 5. Only when the object is not the size its row records:
    ///    [`kv_store::other_size`] re-reads the row by id on the render's OWN
    ///    connection, through [`Self::on_ns_conn`] again (W9c Task 4 fix
    ///    round 1, review M2: it replaced a lazy one-connection pool private
    ///    to the call, so a mismatch no longer opens a second connection).
    ///
    /// Nothing here writes: an expired or soft-deleted level is missing
    /// there, and the walk goes on (spec §4.5, §4.6).
    fn kv_read(&self, args: &KvArgs<'_>) -> std::result::Result<KvAnswer, String> {
        let raw = args.key;
        self.may_block(&format!("key '{raw}'"))?;
        let key = KvKey::parse(raw)?;
        let own = self.flow_namespace.as_deref().ok_or_else(|| {
            format!(
                "key '{raw}' cannot be read: this render has no running flow, whose namespace \
                 the KV store is read as"
            )
        })?;
        let levels: Vec<&str> = match args.namespace {
            Some(ns) => vec![ns],
            None => kv_store::ancestors(own),
        };
        let grants = levels
            .iter()
            .map(|ns| {
                authorize(
                    self.workspace,
                    Actor::Flow { namespace: own },
                    ns,
                    Resource::Kv,
                    Access::Read,
                )
            })
            .collect::<std::result::Result<Vec<Grant>, String>>()?;
        // The namespace Kestra's missing-key sentence names: the one asked
        // for, the flow's own when walking (w9c `inheritance.kv_child`).
        let asked = levels[0];
        let what = format!("key '{key}' in namespace '{asked}'");
        let found = self.on_ns_conn(&what, |mut conn| {
            let (grants, key) = (&grants, &key);
            Box::pin(async move {
                let found = kv_store::resolve_first_live(&mut conn, grants, key).await;
                (conn, found)
            })
        });
        // The connection mutex is released: nothing below holds it.
        let entry = match found {
            Ok(Some(entry)) => entry,
            Ok(None) => {
                return Ok(KvAnswer::Missing(format!(
                    "The key '{key}' does not exist in the namespace '{asked}'."
                )))
            }
            Err(e) => return Err(self.kv_unavailable(&what, e)),
        };
        let grant = grants
            .iter()
            .find(|g| g.namespace() == entry.namespace)
            .ok_or_else(|| {
                format!(
                    "{what}: egret's KV walk answered namespace '{}', which it did not ask",
                    entry.namespace
                )
            })?;
        let read = match self
            .handle
            .block_on(kv_store::read_sized(&self.store, grant, &entry))
        {
            Ok(read) => read,
            Err(e) => return Err(self.kv_unavailable(&what, e)),
        };
        let other = match read {
            Ok(data) => return Ok(KvAnswer::Value(data)),
            Err(other) => other,
        };
        // The size-mismatch path: one statement, on this render's own
        // connection (the mutex is free again: step 3 released it).
        let judged: std::result::Result<std::convert::Infallible, NsError> =
            self.on_ns_conn(&what, |mut conn| {
                let (store, entry) = (&self.store, &entry);
                Box::pin(async move {
                    let e = kv_store::other_size(&mut conn, store, grant, entry, other).await;
                    (conn, Err(e))
                })
            });
        match judged {
            Ok(never) => match never {},
            Err(e) => Err(self.kv_unavailable(&what, e)),
        }
    }
}

/// The work [`Snapshot::on_ns_conn`] runs: a future that owns the connection
/// it was handed and hands it back beside its answer.
type OnConn<'w, T> = std::pin::Pin<
    Box<dyn std::future::Future<Output = (PgConnection, std::result::Result<T, NsError>)> + 'w>,
>;

/// What a namespace-file reference names, before any access decision: its
/// namespace, its path as written, and -- for a `kestra:///…/_files/…[.vN]`
/// URI, which names exactly one revision -- that revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NsRef {
    pub(crate) namespace: String,
    pub(crate) path: String,
    pub(crate) uri_revision: Option<u32>,
}

/// THE parse of a namespace-file reference (W9b spec §3, §6), shared by the
/// functions ([`Snapshot::ns_target`]) and script `inputFiles` staging
/// (`nsfiles::stage::resolve_input_uri`, Task 5):
///
/// * `kestra:///…/_files/…[.vN]`: `uri::parse_any`'s namespace and path, and
///   the revision the URI names; an execution URI is refused;
/// * `nsfile:///path`: `own`'s (or `namespace_arg`'s) namespace;
///   `nsfile://other.ns/path`: the authority's, which wins over
///   `namespace_arg` (a caller that must refuse the pair does so first);
/// * anything else is a bare path, in `namespace_arg`'s namespace or `own`'s.
///
/// Pure: no access decision (the caller's `authorize` makes it) and no path
/// rule (the caller's `NsPath::parse_*`).
pub(crate) fn ns_ref(
    raw: &str,
    own: &str,
    namespace_arg: Option<&str>,
) -> std::result::Result<NsRef, String> {
    let or_own = |arg: Option<&str>| arg.unwrap_or(own).to_string();
    if raw.starts_with(uri::SCHEME_PREFIX) {
        return match uri::parse_any(raw) {
            Ok(uri::StorageUri::NamespaceFile(n)) => Ok(NsRef {
                namespace: n.namespace,
                path: n.path,
                uri_revision: Some(n.revision),
            }),
            Ok(uri::StorageUri::Execution(_)) => Err(format!(
                "{raw} is an execution file URI, not a namespace file's"
            )),
            Err(reason) => Err(Unreadable::Malformed {
                uri: raw.to_string(),
                reason,
            }
            .to_string()),
        };
    }
    if let Some(rest) = raw.strip_prefix(NSFILE_SCHEME) {
        let (authority, path) = rest.find('/').map_or((rest, ""), |i| rest.split_at(i));
        if path.is_empty() {
            return Err(format!(
                "{raw} names no file: an nsfile URI is nsfile:///<path> (the flow's own \
                 namespace) or nsfile://<namespace>/<path>"
            ));
        }
        let namespace = if authority.is_empty() {
            or_own(namespace_arg)
        } else {
            authority.to_string()
        };
        return Ok(NsRef {
            namespace,
            path: path.to_string(),
            uri_revision: None,
        });
    }
    Ok(NsRef {
        namespace: or_own(namespace_arg),
        path: raw.to_string(),
        uri_revision: None,
    })
}

/// What a namespace function names, parsed and authorized ([`Snapshot::ns_target`]).
struct NsTarget {
    grant: Grant,
    /// The file the path names; `None` when it ends with '/'.
    file: Option<NsPath>,
    /// The directory a latest lookup falls back to when no file row is there.
    dir: Option<NsPath>,
    want: Want,
    /// The path in its stored form, for messages.
    shown: String,
}

/// The file row, or -- for a latest lookup with no file row -- the live
/// directory of that name (R3). Every query is `resolve`'s, scoped by the
/// grant's workspace and namespace.
async fn find(conn: &mut PgConnection, t: &NsTarget) -> std::result::Result<NsEntry, NsError> {
    let missing = match &t.file {
        Some(file) => match ns_store::resolve(conn, &t.grant, file, t.want).await {
            Err(NsError::NotFound(m)) => NsError::NotFound(m),
            other => return other,
        },
        None => NsError::NotFound(format!(
            "directory '{}' does not exist in namespace '{}'",
            t.shown,
            t.grant.namespace()
        )),
    };
    let Some(dir) = &t.dir else {
        return Err(missing);
    };
    match ns_store::resolve(conn, &t.grant, dir, Want::Latest).await {
        Ok(e) => Ok(e),
        Err(NsError::NotFound(_)) => Err(missing),
        Err(e) => Err(e),
    }
}

fn db_err(e: sqlx::Error) -> NsError {
    NsError::Db(egret_core::Error::Internal(anyhow::Error::new(e)))
}

/// A database the lookup could not reach or query, as opposed to a row
/// egret would not have written (also `NsError::Db`, egret's own bug).
fn is_db_outage(e: &NsError) -> bool {
    matches!(e, NsError::Db(egret_core::Error::Internal(a)) if a.downcast_ref::<sqlx::Error>().is_some())
}

fn ns_over_cap(entry: &NsEntry, ns: &str, cap: u64) -> String {
    format!(
        "namespace file '{}' in namespace '{ns}' is larger than \
         workflow.storage.max_read_bytes ({cap} bytes): read() returns the whole file as a \
         string, so it reads only files up to that size",
        entry.path
    )
}

/// The resolutions a render may read: what `fileSize`, `fileExists`,
/// `isFileEmpty` and `fileURI` answer from, with no I/O. Shared by the live
/// [`Snapshot`] and the in-transaction scope (`storage::prefetch`), so the
/// two answer metadata identically.
pub(super) fn lookup_in(
    files: &BTreeMap<String, std::result::Result<IndexedFile, Unreadable>>,
    uri: &str,
) -> std::result::Result<FileMeta, FileUnavailable> {
    match files.get(uri) {
        Some(Ok(file)) => Ok(FileMeta { size: file.size }),
        Some(Err(missing @ Unreadable::Missing { .. })) => {
            Err(FileUnavailable::Missing(missing.to_string()))
        }
        Some(Err(why)) => Err(FileUnavailable::Refused(why.to_string())),
        None => Err(FileUnavailable::Refused(not_in_snapshot(uri))),
    }
}

/// The indexed file `uri` resolved to, or why it is not readable here: the
/// first step of every body read, live or in a transaction.
pub(super) fn resolve_in<'f>(
    files: &'f BTreeMap<String, std::result::Result<IndexedFile, Unreadable>>,
    uri: &str,
) -> std::result::Result<&'f IndexedFile, String> {
    match files.get(uri) {
        Some(Ok(file)) => Ok(file),
        Some(Err(why)) => Err(why.to_string()),
        None => Err(not_in_snapshot(uri)),
    }
}

impl RenderFiles for Snapshot {
    fn lookup(&self, uri: &str) -> std::result::Result<FileMeta, FileUnavailable> {
        lookup_in(&self.files, uri)
    }

    fn read_utf8(&self, uri: &str) -> std::result::Result<String, String> {
        let bytes = self.read_bytes(uri, "read() returns the whole file as a string")?;
        decode_utf8(uri, bytes).map_err(|e| e.to_string())
    }

    fn read_bytes(&self, uri: &str, whole: &str) -> std::result::Result<Vec<u8>, String> {
        let file = self.resolve(uri)?;
        let object = self.store.object_uri(self.workspace, uri)?;
        let cap = self.store.caps.max_read_bytes;
        let over_cap = || {
            format!(
                "{uri} is larger than workflow.storage.max_read_bytes ({cap} bytes): {whole}, \
                 so it reads only files up to that size"
            )
        };
        // The index knows the size: refuse without a download (fix round 1,
        // M7). The `cap + 1` guard below stays for an object larger than its row.
        if file.size > cap {
            return Err(over_cap());
        }
        self.may_block(uri)?;
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
        Ok(bytes)
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

    fn max_read_bytes(&self) -> Option<u64> {
        Some(self.store.caps.max_read_bytes)
    }

    fn namespace_file(
        &self,
        args: &NsFileArgs<'_>,
    ) -> std::result::Result<NsFileMeta, FileUnavailable> {
        let target = self.ns_target(args)?;
        let (entry, _) = self.ns_lookup(&target, None)?;
        let path = entry.path.lock_path().to_string();
        if entry.is_directory {
            return Ok(NsFileMeta {
                path,
                size: 0,
                is_directory: true,
                uri: String::new(),
            });
        }
        let uri = uri::namespace_file_uri(target.grant.namespace(), &entry.path, entry.revision)
            .map_err(FileUnavailable::Refused)?;
        Ok(NsFileMeta {
            path,
            size: entry.size,
            is_directory: false,
            uri,
        })
    }

    fn read_namespace_file(&self, args: &NsFileArgs<'_>) -> std::result::Result<String, String> {
        let message = |e: FileUnavailable| e.message().to_string();
        let target = self.ns_target(args).map_err(message)?;
        let cap = self.store.caps.max_read_bytes;
        let (entry, bytes) = self.ns_lookup(&target, Some(cap)).map_err(message)?;
        let bytes = bytes.unwrap_or_default();
        // The stream guard, for an object larger than its row says.
        if bytes.len() as u64 > cap {
            return Err(ns_over_cap(&entry, target.grant.namespace(), cap));
        }
        let uri = uri::namespace_file_uri(target.grant.namespace(), &entry.path, entry.revision)?;
        decode_utf8(&uri, bytes).map_err(|e| e.to_string())
    }

    fn flow_namespace(&self) -> Option<&str> {
        self.flow_namespace.as_deref()
    }

    fn kv(&self, args: &KvArgs<'_>) -> std::result::Result<KvAnswer, String> {
        self.kv_read(args)
    }

    /// Close this render's namespace connection, if it opened one (I1):
    /// `close()`, so the server sees a goodbye and not an unexpected EOF.
    fn end_render(&self) {
        let taken = self
            .ns_conn
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if let Some(conn) = taken {
            // Only a lookup on a thread that may block opened it, so this
            // thread may block too; anywhere else the connection is dropped.
            if self.may_block("a namespace connection").is_ok() {
                let _ = self.handle.block_on(conn.close());
            }
        }
    }
}

/// What the four functions answer when this process has no `[storage]`.
struct Unconfigured;

fn unconfigured_message(uri: &str) -> String {
    format!(
        "{uri} cannot be read: egret has no internal storage configured (the `[storage]` \
         section of its configuration), so read, fileSize, fileExists, isFileEmpty and \
         fileURI are unavailable"
    )
}

impl RenderFiles for Unconfigured {
    fn lookup(&self, uri: &str) -> std::result::Result<FileMeta, FileUnavailable> {
        Err(FileUnavailable::Refused(unconfigured_message(uri)))
    }
    fn read_utf8(&self, uri: &str) -> std::result::Result<String, String> {
        Err(unconfigured_message(uri))
    }
    fn read_bytes(&self, uri: &str, _whole: &str) -> std::result::Result<Vec<u8>, String> {
        Err(unconfigured_message(uri))
    }
    fn task_prefix(&self) -> Option<&str> {
        None
    }
    fn namespace_file(
        &self,
        args: &NsFileArgs<'_>,
    ) -> std::result::Result<NsFileMeta, FileUnavailable> {
        Err(FileUnavailable::Refused(unconfigured_message(args.path)))
    }
    fn read_namespace_file(&self, args: &NsFileArgs<'_>) -> std::result::Result<String, String> {
        Err(unconfigured_message(args.path))
    }
    fn flow_namespace(&self) -> Option<&str> {
        None
    }
    fn kv(&self, args: &KvArgs<'_>) -> std::result::Result<KvAnswer, String> {
        Err(format!(
            "key '{}' cannot be read: egret has no internal storage configured (the \
             `[storage]` section of its configuration), so kv is unavailable",
            args.key
        ))
    }
}

/// The scope a render inside a running execution gets OUTSIDE any
/// transaction (a handler's): a [`Snapshot`] over the installed store, or --
/// with [`super::current`] `None` -- a scope whose every answer names
/// `[storage]`. A wiring point (the module doc of `storage`): `worker::claim`
/// calls this and nothing else does. `conn` is a pool connection, so a
/// transaction cannot be handed in: a scope that downloads and connects is
/// never built where a transaction is open (W9c Task 9, final review m2).
#[allow(clippy::too_many_arguments)]
pub async fn scope_for(
    conn: &mut PoolConnection<Postgres>,
    store: Option<Arc<FileStore>>,
    workspace: Uuid,
    execution: Uuid,
    texts: &[&str],
    json: &[&serde_json::Value],
    task_prefix: Option<String>,
    flow_namespace: Option<String>,
) -> Result<FileScope> {
    match store {
        None => Ok(FileScope(Arc::new(Unconfigured))),
        Some(store) => Ok(Snapshot::take(
            conn,
            store,
            workspace,
            execution,
            texts,
            json,
            task_prefix,
            flow_namespace,
        )
        .await?
        .into_scope()),
    }
}

/// W9c Task 9: the scope a render INSIDE a transaction gets -- `settle`'s,
/// and through it every nested settle's. The execution-file resolutions are
/// taken on `conn` exactly as [`scope_for`]'s (the one readability decision,
/// statements only, and the only connection that sees the index rows this
/// transaction has just written); every body, namespace-file and KV read is
/// answered from `gate`'s cache (`storage::prefetch`), never from the store
/// or the database. The scope it returns holds no store and no connect
/// options, so it cannot do I/O; `store` is read here only for its cap.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn scope_in_transaction(
    conn: &mut PgConnection,
    store: Option<Arc<FileStore>>,
    workspace: Uuid,
    execution: Uuid,
    texts: &[&str],
    json: &[&serde_json::Value],
    task_prefix: Option<String>,
    flow_namespace: Option<String>,
    gate: &crate::worker::txn::Gate,
) -> Result<FileScope> {
    match store {
        None => Ok(FileScope(Arc::new(Unconfigured))),
        Some(store) => {
            let files = reachable(conn, workspace, execution, texts, json).await?;
            Ok(FileScope(Arc::new(super::prefetch::InTransaction::new(
                files,
                workspace,
                task_prefix,
                flow_namespace,
                store.caps.max_read_bytes,
                gate.prefetch().clone(),
            ))))
        }
    }
}

/// Every `kestra:///…` string in `texts` and `json` (see [`Snapshot::take`]),
/// resolved at once through `resolve_readable_many` on `conn`. With no URI
/// anywhere the database is not touched.
async fn reachable(
    conn: &mut PgConnection,
    workspace: Uuid,
    execution: Uuid,
    texts: &[&str],
    json: &[&serde_json::Value],
) -> Result<BTreeMap<String, std::result::Result<IndexedFile, Unreadable>>> {
    let mut found = BTreeSet::new();
    for text in texts {
        collect(text, &mut found);
    }
    for value in json {
        walk(value, &mut found);
    }
    if found.is_empty() {
        return Ok(BTreeMap::new());
    }
    let readable = readable_executions(conn, workspace, execution).await?;
    let uris: Vec<String> = found.into_iter().collect();
    resolve_readable_many(conn, workspace, &readable, &uris).await
}

fn collect(text: &str, found: &mut BTreeSet<String>) {
    for m in URI_IN_TEXT.find_iter(text) {
        found.insert(m.as_str().to_string());
    }
}

pub(crate) fn walk(value: &serde_json::Value, found: &mut BTreeSet<String>) {
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

    /// W9b: a namespace lookup blocks the render thread on the runtime as a
    /// store read does, so it refuses a current-thread runtime the same way
    /// -- before it connects (the options here name nothing reachable, and
    /// no outage is recorded).
    #[tokio::test(flavor = "current_thread")]
    async fn a_namespace_function_on_a_current_thread_runtime_is_refused_saying_why() {
        let opts: sqlx::postgres::PgConnectOptions = "postgres://nobody:nothing@127.0.0.1:1/none"
            .parse()
            .unwrap();
        for call in ["read", "fileSize", "fileExists", "isFileEmpty", "fileURI"] {
            let snap = Snapshot::from_parts(BTreeMap::new(), store(1 << 20), WS, None)
                .with_namespace_files(Some("w9b.fn".into()), Some(Arc::new(opts.clone())));
            let (out, failure) = render_recording(&format!("{{{{ {call}('cfg/x.txt') }}}}"), snap);
            let err = out.unwrap_err();
            assert!(
                err.contains("'/cfg/x.txt'") && err.contains("multi-thread runtime"),
                "{call}: {err}"
            );
            assert_eq!(failure, None, "{call}");
        }
    }

    /// W9b: a process with no namespace-file database (egret-bin installs
    /// one) refuses saying so, and that is not an outage.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_namespace_function_with_no_database_is_refused_and_is_not_an_outage() {
        let snap = Snapshot::from_parts(BTreeMap::new(), store(1 << 20), WS, None)
            .with_namespace_files(Some("w9b.fn".into()), None);
        let (out, failure) = render_recording("{{ fileExists('cfg/x.txt') }}", snap);
        let err = out.unwrap_err();
        assert!(
            err.contains("'/cfg/x.txt'") && err.contains("no database connection"),
            "{err}"
        );
        assert_eq!(failure, None);
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
