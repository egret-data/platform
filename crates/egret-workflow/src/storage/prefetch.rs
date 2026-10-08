//! W9c Task 9 (final review F2, the user's ruling of 2026-10-08): **no render
//! inside a transaction downloads anything or opens a connection.**
//!
//! # The window this closes
//!
//! `settle` renders `runIf`, an `If`'s condition, a `Switch`'s value, a
//! `Loop`'s `values:` (and its values file), a `Pause`'s `pauseDuration:`, a
//! `LoopUntil`'s condition, the SLA conditions and a `Subflow`'s target and
//! inputs **inside its caller's transaction**, holding a pool connection and
//! the row locks of every execution the transaction has touched (and, for
//! the scheduler and the Flow-trigger processor, the trigger, event and
//! window rows). Before this module a `kv()`, a namespace-file function, a
//! `read()` or a `Loop` over a file there downloaded, or connected, while
//! all of that was held.
//!
//! # The mechanism: a cache-only render, rolled back and replayed
//!
//! Inside a transaction the render scope is an [`InTransaction`]: it holds
//! the execution-file *resolutions* (taken in the transaction, statements
//! only: the one readability decision, and the only connection that sees the
//! index rows the transaction has just written) and a [`Prefetch`] cache. It
//! holds **no store and no connect options**, so it cannot do I/O. Every
//! body, namespace-file and KV read is a cache lookup.
//!
//! The transaction's outermost owner (`worker::txn`) checks the cache
//! **before it commits**: with a miss pending it rolls back -- every lock
//! and the pool connection released -- fetches every pending request with no
//! transaction open ([`Prefetch::fetch_pending`]), through the same live
//! code a render outside a transaction uses ([`super::scope::Snapshot`]), and
//! runs the transaction again, which now reads the fetched answers. **A
//! transaction never commits with a miss pending.**
//!
//! # Gather: a miss does not end the attempt (the user's ruling, 2026-10-08)
//!
//! A miss records the request and hands the render a neutral PLACEHOLDER
//! answer (an empty string, empty bytes, a zero-size file), so the render
//! carries on: every later read in the same expression, every later loop
//! iteration's settle and every later listener of the same event is
//! discovered in the SAME attempt, and one fetch answers them all. Whatever
//! the attempt computed from a placeholder -- a value, a branch taken, an
//! author's error, a failed run -- is discarded with its rollback: the
//! attempt that commits has no miss, so every value it rendered is the one a
//! live render gives. A placeholder may steer control flow down a branch the
//! real value would not take; that branch is only ever rolled back, and its
//! reads are fetched (and counted against the limits below) like any other.
//!
//! So one fetch round answers everything an attempt can see. A further
//! round is needed only where a read is REACHED through another read's
//! value -- `kv(kv('p'))`, or a task that exists only once an earlier task's
//! `runIf` is known to be true: one more attempt per level of real control
//! or data dependency.
//!
//! # Limits (the user's ruling, 2026-10-08)
//!
//! Per call of an owner, across every attempt and every re-[`bind`]:
//!
//! * at most [`MAX_FETCHES`] entries fetched;
//! * at most [`MAX_ROUNDS`] fetch rounds;
//! * at most [`CACHE_CAP_FACTOR`] × `workflow.storage.max_read_bytes` of
//!   cached values, each distinct entry counted once.
//!
//! A read past either count is answered with the AUTHOR's error naming the
//! limit and the read; an answer that would cross the byte cap is cached as
//! the author's error naming the cap. Either reaches the run that rendered
//! it with that message ([`InTransaction::take_limit_failure`]).
//!
//! [`bind`]: Prefetch::bind
//!
//! # What is not cached
//!
//! Only reads are. `now()`, `uuid()`, `randomInt()` and the rest are
//! evaluated again by each attempt, and only the committed attempt's values
//! persist. `secret()` stays refused in these renders. The cache lives for
//! one call of one owner -- one `tick`'s run, one `resume`, one start, one
//! trigger fire, one event -- never longer, so a `LoopUntil` reads its
//! condition's value afresh at every iteration.
//!
//! # Outages
//!
//! A fetch that meets an outage (the store or the database unreachable)
//! caches the live answer AND the outage. The replayed attempt's scope
//! reports it ([`RenderFiles::infrastructure_failure`]) exactly as the live
//! scope did, so every owner treats it exactly as before: `settle` returns
//! `Err`, the transaction rolls back, and a claimed run is re-claimed with no
//! attempt spent (spec §11).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use egret_core::{Error, Result};
use uuid::Uuid;

use super::scope::{lookup_in, resolve_in, Snapshot};
use super::{decode_utf8, IndexedFile, Unreadable};
use crate::runners::files::{
    FileMeta, FileUnavailable, KvAnswer, KvArgs, NsFileArgs, NsFileMeta, RenderFiles,
};

/// How many fetch rounds one call may make (the user's ruling, 2026-10-08).
pub const MAX_ROUNDS: u32 = 64;

/// How many entries one call may fetch, over all its rounds (the user's
/// ruling, 2026-10-08).
pub const MAX_FETCHES: usize = 256;

/// One call's cache holds at most this many times
/// `workflow.storage.max_read_bytes` (the user's ruling, 2026-10-08).
pub const CACHE_CAP_FACTOR: u64 = 4;

/// Test-only: every fetch round this process has run, so a test can tell
/// one round that answered N reads from N rounds that answered one each.
#[cfg(any(test, feature = "test-util"))]
static FETCH_ROUNDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Test-only: how many fetch rounds this process has run (see
/// [`Prefetch::fetch_pending`]).
#[cfg(any(test, feature = "test-util"))]
pub fn fetch_rounds_so_far() -> u64 {
    FETCH_ROUNDS.load(std::sync::atomic::Ordering::SeqCst)
}

/// What a render asked for: a function and its arguments, as the scope
/// that would have answered it live sees them.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Request {
    /// `read()`'s or a `Loop` values file's bytes. `whole` finishes the
    /// over-cap message, so it is part of the answer.
    ExecBody {
        workspace: Uuid,
        uri: String,
        whole: String,
    },
    /// `fileSize`, `fileExists`, `isFileEmpty` or `fileURI` of a namespace file.
    NsMeta(NsKey),
    /// `read()` of a namespace file.
    NsRead(NsKey),
    /// `kv()`.
    Kv {
        workspace: Uuid,
        flow_namespace: Option<String>,
        key: String,
        namespace: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct NsKey {
    workspace: Uuid,
    flow_namespace: Option<String>,
    path: String,
    namespace: Option<String>,
    revision: Option<String>,
}

impl Request {
    /// The read as a message names it.
    fn what(&self) -> String {
        match self {
            Request::ExecBody { uri, .. } => uri.clone(),
            Request::NsMeta(k) | Request::NsRead(k) => match &k.namespace {
                Some(ns) => format!("namespace file '{}' in namespace '{ns}'", k.path),
                None => format!("namespace file '{}'", k.path),
            },
            Request::Kv { key, namespace, .. } => match namespace {
                Some(ns) => format!("key '{key}' in namespace '{ns}'"),
                None => format!("key '{key}'"),
            },
        }
    }

    /// The live snapshot this request is fetched on: one per workspace and
    /// running flow.
    fn group(&self) -> (Uuid, Option<String>) {
        match self {
            Request::ExecBody { workspace, .. } => (*workspace, None),
            Request::NsMeta(k) | Request::NsRead(k) => (k.workspace, k.flow_namespace.clone()),
            Request::Kv {
                workspace,
                flow_namespace,
                ..
            } => (*workspace, flow_namespace.clone()),
        }
    }
}

/// What the live function answered, unchanged.
#[derive(Clone, Debug)]
enum Answer {
    Bytes(std::result::Result<Vec<u8>, String>),
    NsMeta(std::result::Result<NsFileMeta, FileUnavailable>),
    NsRead(std::result::Result<String, String>),
    Kv(std::result::Result<KvAnswer, String>),
}

impl Answer {
    /// What this answer costs the cache: the bytes of the value it carries.
    fn bytes(&self) -> u64 {
        match self {
            Answer::Bytes(Ok(b)) => b.len() as u64,
            Answer::NsRead(Ok(s)) => s.len() as u64,
            Answer::Kv(Ok(KvAnswer::Value(d))) => kv_bytes(d),
            Answer::NsMeta(Ok(m)) => (m.path.len() + m.uri.len()) as u64,
            _ => 0,
        }
    }

    /// The author's error `message`, in the shape `request`'s function answers.
    fn refusal(request: &Request, message: String) -> Answer {
        match request {
            Request::ExecBody { .. } => Answer::Bytes(Err(message)),
            Request::NsMeta(_) => Answer::NsMeta(Err(FileUnavailable::Refused(message))),
            Request::NsRead(_) => Answer::NsRead(Err(message)),
            Request::Kv { .. } => Answer::Kv(Err(message)),
        }
    }

    /// What a render gets for a read not fetched yet: a neutral value of the
    /// function's shape, so the render carries on and every later read is
    /// discovered in the same attempt. Never committed: the attempt that
    /// handed it out has a miss pending, and the gate rolls it back.
    fn placeholder(request: &Request) -> Answer {
        match request {
            Request::ExecBody { .. } => Answer::Bytes(Ok(Vec::new())),
            Request::NsMeta(k) => Answer::NsMeta(Ok(NsFileMeta {
                path: k.path.clone(),
                size: 0,
                is_directory: false,
                uri: String::new(),
            })),
            Request::NsRead(_) => Answer::NsRead(Ok(String::new())),
            Request::Kv { .. } => Answer::Kv(Ok(KvAnswer::Value(
                crate::storage::kv::value::KvData::String(String::new()),
            ))),
        }
    }
}

/// A rough size of a KV value: its text and one word per node, walked with
/// an explicit stack (a value may nest deep; see `KvData`'s `Drop`).
fn kv_bytes(d: &crate::storage::kv::value::KvData) -> u64 {
    use crate::storage::kv::value::{KvData, KvNumber};
    let mut total = 0u64;
    let mut stack = vec![d];
    while let Some(d) = stack.pop() {
        total += 8;
        match d {
            KvData::String(s) => total += s.len() as u64,
            KvData::Number(KvNumber::Int(t) | KvNumber::Decimal(t)) => total += t.len() as u64,
            KvData::List(items) => stack.extend(items.iter()),
            KvData::Struct(fields) => {
                for (name, v) in fields {
                    total += name.len() as u64;
                    stack.push(v);
                }
            }
            _ => {}
        }
    }
    total
}

/// An answer, the outage the live read met giving it, if any, and whether it
/// is a limit's refusal (the byte cap) rather than the live answer. Shared:
/// a hit hands out the `Arc`, not a copy of the body.
#[derive(Debug)]
struct Stored {
    answer: Answer,
    outage: Option<String>,
    limit: bool,
}

#[derive(Default)]
struct State {
    /// This owner's answers (see [`Prefetch::bind`]).
    answers: BTreeMap<Request, Arc<Stored>>,
    /// Asked for and not answered: the request, and for an execution file
    /// the resolution the attempt made (the attempt's rollback erases any
    /// index row it wrote, so the fetch cannot resolve it again).
    pending: BTreeMap<Request, Option<IndexedFile>>,
    /// What the cap counts, per CALL: the bytes of each distinct entry ever
    /// cached, once, kept across [`Prefetch::bind`].
    counted: BTreeSet<Request>,
    bytes: u64,
    /// Entries fetched so far in this call, across [`Prefetch::bind`].
    fetched: usize,
    /// Fetch rounds so far in this call, across [`Prefetch::bind`].
    rounds: u32,
    /// What the answers are for (`Prefetch::bind`).
    owner: Option<String>,
}

/// One call's read cache. See the module doc. Cheap to clone: every clone is
/// the same cache, which is how a nested settle of another execution, in
/// the same transaction, reads and records through the owner's.
#[derive(Clone, Default)]
pub struct Prefetch(Arc<Mutex<State>>);

impl std::fmt::Debug for Prefetch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = self.state();
        f.debug_struct("Prefetch")
            .field("answers", &s.answers.len())
            .field("pending", &s.pending.len())
            .field("fetched", &s.fetched)
            .field("rounds", &s.rounds)
            .finish()
    }
}

/// What the cache had for a request.
enum Lookup {
    Hit(Arc<Stored>),
    /// Recorded as pending: this attempt will not commit.
    Missed,
    /// A limit is spent: the author's error, naming it and the read.
    Final(String),
}

impl Prefetch {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Whether a render of the current attempt asked for something this
    /// cache did not hold. **An attempt with a miss pending must not commit.**
    pub fn has_pending(&self) -> bool {
        !self.state().pending.is_empty()
    }

    /// How many fetch rounds this call has made.
    pub fn rounds(&self) -> u32 {
        self.state().rounds
    }

    /// How many entries this call has fetched.
    pub fn fetched(&self) -> usize {
        self.state().fetched
    }

    /// Say what this cache's answers are for -- a trigger, an event -- before
    /// the attempt reads through it. An owner that claims its item afresh on
    /// every attempt (the scheduler, the Flow-trigger processor) may claim a
    /// different one on a retry, when another replica took the first; the
    /// ANSWERS then start over, so they are always one item's, read after
    /// that item was claimed. **The limits do not**: the rounds, the fetches
    /// and the bytes are the call's, whatever it claims, so a call whose
    /// retries keep claiming different items still ends (final review m1).
    pub fn bind(&self, owner: &str) {
        let mut s = self.state();
        if s.owner.as_deref() != Some(owner) {
            s.owner = Some(owner.to_string());
            s.answers.clear();
            s.pending.clear();
        }
    }

    fn lookup(&self, request: Request, file: Option<IndexedFile>) -> Lookup {
        let mut s = self.state();
        if let Some(stored) = s.answers.get(&request) {
            return Lookup::Hit(stored.clone());
        }
        if s.pending.contains_key(&request) {
            return Lookup::Missed;
        }
        if s.rounds >= MAX_ROUNDS {
            return Lookup::Final(over_rounds(&request));
        }
        if s.fetched + s.pending.len() >= MAX_FETCHES {
            return Lookup::Final(over_fetches(&request));
        }
        s.pending.insert(request, file);
        Lookup::Missed
    }

    /// Fetch every pending request **with no transaction open**: the caller
    /// has rolled its attempt back. Each is answered by the live code a
    /// render outside a transaction runs -- a [`Snapshot`] per workspace and
    /// running flow, on a thread of its own that may block, as a render's
    /// does -- and its answer cached as it came, an outage with it. Counts
    /// one round. `Err` only for egret's own failure to run the fetch.
    pub async fn fetch_pending(&self) -> Result<()> {
        let pending = std::mem::take(&mut self.state().pending);
        if pending.is_empty() {
            return Ok(());
        }
        let store = super::current().ok_or_else(|| {
            Error::Internal(anyhow::anyhow!(
                "a render recorded a read, and this process has no internal storage to fetch it from"
            ))
        })?;
        let cap = store.caps.max_read_bytes.saturating_mul(CACHE_CAP_FACTOR);
        let ns_db = super::nsfiles::db_options();
        // One live snapshot per (workspace, running flow); built here, on the
        // runtime, because it captures the runtime's handle.
        let mut groups: BTreeMap<(Uuid, Option<String>), Group> = BTreeMap::new();
        for (request, file) in pending {
            let group = groups.entry(request.group()).or_default();
            if let (Request::ExecBody { uri, .. }, Some(file)) = (&request, file) {
                group.0.insert(uri.clone(), Ok(file));
            }
            group.1.push(request);
        }
        let work: Vec<(Snapshot, Vec<Request>)> = groups
            .into_iter()
            .map(|((workspace, flow_namespace), (files, requests))| {
                (
                    Snapshot::for_fetch(
                        files,
                        store.clone(),
                        workspace,
                        flow_namespace,
                        ns_db.clone(),
                    ),
                    requests,
                )
            })
            .collect();
        let (sent, answered) = tokio::sync::oneshot::channel();
        std::thread::Builder::new()
            .name("egret-prefetch".to_string())
            .spawn(move || {
                let _ = sent.send(fetch_on_this_thread(work));
            })
            .map_err(|e| {
                Error::Internal(anyhow::anyhow!("could not start a prefetch thread: {e}"))
            })?;
        let fetched = answered.await.map_err(|_| {
            Error::Internal(anyhow::anyhow!(
                "the prefetch thread ended without answering"
            ))
        })?;
        let mut s = self.state();
        for (request, answer, outage) in fetched {
            s.store(request, answer, outage, cap);
        }
        s.rounds += 1;
        #[cfg(any(test, feature = "test-util"))]
        FETCH_ROUNDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

impl State {
    /// Cache one fetched answer -- or, when it would take what this call has
    /// cached past `cap` bytes, the author's error naming the cap in its
    /// place. Each distinct entry counts against the cap ONCE per call, and
    /// against [`MAX_FETCHES`] each time it is fetched.
    fn store(&mut self, request: Request, answer: Answer, outage: Option<String>, cap: u64) {
        self.fetched += 1;
        let counted = self.counted.contains(&request);
        let size = if counted { 0 } else { answer.bytes() };
        let stored = if self.bytes.saturating_add(size) > cap {
            Stored {
                answer: Answer::refusal(&request, over_cache_cap(&request, cap)),
                outage: None,
                limit: true,
            }
        } else {
            self.bytes += size;
            self.counted.insert(request.clone());
            Stored {
                answer,
                outage,
                limit: false,
            }
        };
        self.answers.insert(request, Arc::new(stored));
    }
}

/// One live snapshot's work: the execution-file resolutions it reads by, and
/// the requests it answers.
type Group = (
    BTreeMap<String, std::result::Result<IndexedFile, Unreadable>>,
    Vec<Request>,
);

/// Run each request on its snapshot, on this (non-runtime) thread, and close
/// each snapshot's namespace connection when its requests are done.
fn fetch_on_this_thread(
    work: Vec<(Snapshot, Vec<Request>)>,
) -> Vec<(Request, Answer, Option<String>)> {
    let mut out = Vec::new();
    for (live, requests) in work {
        for request in requests {
            let answer = match &request {
                Request::ExecBody { uri, whole, .. } => Answer::Bytes(live.read_bytes(uri, whole)),
                Request::NsMeta(k) => Answer::NsMeta(live.namespace_file(&ns_args(k))),
                Request::NsRead(k) => Answer::NsRead(live.read_namespace_file(&ns_args(k))),
                Request::Kv { key, namespace, .. } => Answer::Kv(live.kv(&KvArgs {
                    key,
                    namespace: namespace.as_deref(),
                })),
            };
            let outage = live.take_outage();
            out.push((request, answer, outage));
        }
        live.end_render();
    }
    out
}

fn ns_args(k: &NsKey) -> NsFileArgs<'_> {
    NsFileArgs {
        path: &k.path,
        namespace: k.namespace.as_deref(),
        revision: k.revision.clone(),
    }
}

/// The author's error when the rounds are spent ([`MAX_ROUNDS`]).
fn over_rounds(request: &Request) -> String {
    format!(
        "{} could not be read: the expressions rendered in this transaction need more than \
         {MAX_ROUNDS} rounds of reads (a read reached only through another read's value -- \
         kv(kv(...)), or a task that exists only once an earlier task's runIf is known -- needs \
         one more round), and {MAX_ROUNDS} is the limit",
        request.what()
    )
}

/// The author's error when the fetches are spent ([`MAX_FETCHES`]).
fn over_fetches(request: &Request) -> String {
    format!(
        "{} could not be read: the expressions rendered in this transaction read more than \
         {MAX_FETCHES} values (kv(), read() and namespace-file functions together), and \
         {MAX_FETCHES} is the limit",
        request.what()
    )
}

/// The author's error when an answer would cross the cache cap.
fn over_cache_cap(request: &Request, cap: u64) -> String {
    format!(
        "{} could not be read: the values the expressions rendered in this transaction read \
         add up to more than {cap} bytes, the limit of {CACHE_CAP_FACTOR} x \
         workflow.storage.max_read_bytes",
        request.what()
    )
}

/// The render scope inside a transaction. See the module doc. **No store and
/// no connect options**: nothing here can download or connect.
pub struct InTransaction {
    files: BTreeMap<String, std::result::Result<IndexedFile, Unreadable>>,
    workspace: Uuid,
    task_prefix: Option<String>,
    flow_namespace: Option<String>,
    max_read_bytes: u64,
    prefetch: Prefetch,
    /// A cached outage this scope handed back (see the module doc).
    outage: Mutex<Option<String>>,
    /// The latest limit's refusal this scope handed back
    /// ([`RenderFiles::take_limit_failure`]).
    limit: Mutex<Option<String>>,
}

impl InTransaction {
    pub(crate) fn new(
        files: BTreeMap<String, std::result::Result<IndexedFile, Unreadable>>,
        workspace: Uuid,
        task_prefix: Option<String>,
        flow_namespace: Option<String>,
        max_read_bytes: u64,
        prefetch: Prefetch,
    ) -> Self {
        Self {
            files,
            workspace,
            task_prefix,
            flow_namespace,
            max_read_bytes,
            prefetch,
            outage: Mutex::new(None),
            limit: Mutex::new(None),
        }
    }

    /// The cached answer to `request`; a placeholder for a miss (recorded as
    /// pending, so the attempt is rolled back); or a limit's refusal, which
    /// is also noted for [`RenderFiles::take_limit_failure`].
    fn answer(&self, request: Request, file: Option<IndexedFile>) -> Answer {
        match self.prefetch.lookup(request.clone(), file) {
            Lookup::Hit(stored) => {
                if let Some(outage) = &stored.outage {
                    let mut held = self.outage.lock().unwrap_or_else(|p| p.into_inner());
                    held.get_or_insert_with(|| outage.clone());
                }
                if stored.limit {
                    if let Some(message) = refusal_message(&stored.answer) {
                        self.note_limit(message);
                    }
                }
                stored.answer.clone()
            }
            Lookup::Missed => Answer::placeholder(&request),
            Lookup::Final(message) => {
                self.note_limit(message.clone());
                Answer::refusal(&request, message)
            }
        }
    }

    fn note_limit(&self, message: String) {
        *self.limit.lock().unwrap_or_else(|p| p.into_inner()) = Some(message);
    }

    fn ns_key(&self, args: &NsFileArgs<'_>) -> NsKey {
        NsKey {
            workspace: self.workspace,
            flow_namespace: self.flow_namespace.clone(),
            path: args.path.to_string(),
            namespace: args.namespace.map(str::to_string),
            revision: args.revision.clone(),
        }
    }
}

/// The message a refusal carries.
fn refusal_message(answer: &Answer) -> Option<String> {
    match answer {
        Answer::Bytes(Err(m)) | Answer::NsRead(Err(m)) | Answer::Kv(Err(m)) => Some(m.clone()),
        Answer::NsMeta(Err(FileUnavailable::Refused(m))) => Some(m.clone()),
        _ => None,
    }
}

/// An answer of the wrong shape for its request: egret's bug, never the author's.
fn mismatched(request: &Request) -> String {
    format!(
        "{}: egret's prefetch cache answered with the wrong kind of value",
        request.what()
    )
}

impl RenderFiles for InTransaction {
    fn lookup(&self, uri: &str) -> std::result::Result<FileMeta, FileUnavailable> {
        lookup_in(&self.files, uri)
    }

    fn read_utf8(&self, uri: &str) -> std::result::Result<String, String> {
        let bytes = self.read_bytes(uri, "read() returns the whole file as a string")?;
        decode_utf8(uri, bytes).map_err(|e| e.to_string())
    }

    fn read_bytes(&self, uri: &str, whole: &str) -> std::result::Result<Vec<u8>, String> {
        let file = resolve_in(&self.files, uri)?.clone();
        let request = Request::ExecBody {
            workspace: self.workspace,
            uri: uri.to_string(),
            whole: whole.to_string(),
        };
        match self.answer(request.clone(), Some(file)) {
            Answer::Bytes(answer) => answer,
            _ => Err(mismatched(&request)),
        }
    }

    fn infrastructure_failure(&self) -> Option<String> {
        self.outage
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn take_limit_failure(&self) -> Option<String> {
        self.limit.lock().unwrap_or_else(|p| p.into_inner()).take()
    }

    fn task_prefix(&self) -> Option<&str> {
        self.task_prefix.as_deref()
    }

    fn max_read_bytes(&self) -> Option<u64> {
        Some(self.max_read_bytes)
    }

    fn namespace_file(
        &self,
        args: &NsFileArgs<'_>,
    ) -> std::result::Result<NsFileMeta, FileUnavailable> {
        let request = Request::NsMeta(self.ns_key(args));
        match self.answer(request.clone(), None) {
            Answer::NsMeta(answer) => answer,
            _ => Err(FileUnavailable::Refused(mismatched(&request))),
        }
    }

    fn read_namespace_file(&self, args: &NsFileArgs<'_>) -> std::result::Result<String, String> {
        let request = Request::NsRead(self.ns_key(args));
        match self.answer(request.clone(), None) {
            Answer::NsRead(answer) => answer,
            _ => Err(mismatched(&request)),
        }
    }

    fn flow_namespace(&self) -> Option<&str> {
        self.flow_namespace.as_deref()
    }

    fn kv(&self, args: &KvArgs<'_>) -> std::result::Result<KvAnswer, String> {
        let request = Request::Kv {
            workspace: self.workspace,
            flow_namespace: self.flow_namespace.clone(),
            key: args.key.to_string(),
            namespace: args.namespace.map(str::to_string),
        };
        match self.answer(request.clone(), None) {
            Answer::Kv(answer) => answer,
            _ => Err(mismatched(&request)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runners::{render_str, RenderContext};
    use crate::storage::{Caps, FileStore};

    const WS: Uuid = Uuid::from_u128(0x7e9);

    fn uri_of(name: &str) -> String {
        format!(
            "kestra:///w9c/t9/f/executions/{}/tasks/w/{}/{name}",
            Uuid::from_u128(1),
            Uuid::from_u128(2)
        )
    }

    fn file(uri: &str, size: u64) -> IndexedFile {
        IndexedFile {
            uri: uri.to_string(),
            execution_id: Uuid::from_u128(1),
            task_run_id: Some(Uuid::from_u128(2)),
            size,
        }
    }

    fn in_tx(files: &[(String, u64)], pf: &Prefetch, cap: u64) -> Arc<InTransaction> {
        let files = files
            .iter()
            .map(|(u, n)| (u.clone(), Ok(file(u, *n))))
            .collect();
        Arc::new(InTransaction::new(
            files,
            WS,
            None,
            Some("w9c.t9".into()),
            cap,
            pf.clone(),
        ))
    }

    fn scope(files: &[(String, u64)], pf: &Prefetch, cap: u64) -> crate::runners::files::FileScope {
        crate::runners::files::FileScope(in_tx(files, pf, cap))
    }

    fn render(
        template: &str,
        scope: crate::runners::files::FileScope,
    ) -> std::result::Result<String, String> {
        let cx = RenderContext::new().with_files(scope);
        render_str(template, &cx).map_err(|e| e.to_string())
    }

    fn kv_request(key: &str) -> Request {
        Request::Kv {
            workspace: WS,
            flow_namespace: Some("w9c.t9".into()),
            key: key.into(),
            namespace: None,
        }
    }

    /// The same answers a live snapshot gives, through the cache: what the
    /// fetch stores is what the in-transaction scope hands back.
    fn seeded(pf: &Prefetch, request: Request, answer: Answer) {
        pf.state().answers.insert(
            request,
            Arc::new(Stored {
                answer,
                outage: None,
                limit: false,
            }),
        );
    }

    #[test]
    fn a_miss_records_the_request_and_renders_on_with_a_placeholder() {
        let pf = Prefetch::new();
        let uri = uri_of("a.txt");
        assert_eq!(
            render(
                &format!("[{{{{ read('{uri}') }}}}]"),
                scope(&[(uri.clone(), 3)], &pf, 10),
            ),
            Ok("[]".into()),
            "the placeholder is an empty body"
        );
        assert!(pf.has_pending());
        let pending = pf.state().pending.clone();
        assert_eq!(pending.len(), 1);
        let (request, f) = pending.into_iter().next().unwrap();
        assert!(matches!(request, Request::ExecBody { .. }), "{request:?}");
        assert_eq!(
            f,
            Some(file(&uri, 3)),
            "the resolution rides with the request"
        );
    }

    /// The user's ruling of 2026-10-08 (gather): one attempt discovers every
    /// read its renders reach, so one fetch answers them all.
    #[test]
    fn every_read_of_one_expression_is_recorded_in_one_attempt() {
        let pf = Prefetch::new();
        let uri = uri_of("a.txt");
        let out = render(
            &format!(
                "{{{{ kv('a') ~ kv('b') ~ read('{uri}') ~ fileExists('nsfile:///x.txt') ~ \
                 read('nsfile:///y.txt') ~ kv('a') }}}}"
            ),
            scope(&[(uri.clone(), 3)], &pf, 10),
        );
        assert!(out.is_ok(), "every placeholder is neutral: {out:?}");
        let pending: Vec<String> = pf.state().pending.keys().map(Request::what).collect();
        assert_eq!(
            pending,
            vec![
                uri.clone(),
                "namespace file 'nsfile:///x.txt'".to_string(),
                "namespace file 'nsfile:///y.txt'".to_string(),
                "key 'a'".to_string(),
                "key 'b'".to_string(),
            ],
            "each distinct read once"
        );
    }

    #[test]
    fn metadata_is_answered_in_the_transaction_with_no_miss() {
        let pf = Prefetch::new();
        let uri = uri_of("a.txt");
        assert_eq!(
            render(
                &format!("{{{{ fileSize('{uri}') }}}} {{{{ fileExists('{uri}') }}}}"),
                scope(&[(uri.clone(), 3)], &pf, 10)
            ),
            Ok("3 true".into())
        );
        assert!(!pf.has_pending());
    }

    #[test]
    fn a_hit_hands_back_the_cached_answer_unchanged_error_or_value() {
        let pf = Prefetch::new();
        let uri = uri_of("a.txt");
        seeded(
            &pf,
            Request::ExecBody {
                workspace: WS,
                uri: uri.clone(),
                whole: "read() returns the whole file as a string".into(),
            },
            Answer::Bytes(Ok(b"abc".to_vec())),
        );
        seeded(
            &pf,
            kv_request("gone"),
            Answer::Kv(Ok(KvAnswer::Missing(
                "The key 'gone' does not exist".into(),
            ))),
        );
        let s = scope(&[(uri.clone(), 3)], &pf, 10);
        assert_eq!(
            render(&format!("{{{{ read('{uri}') }}}}"), s.clone()),
            Ok("abc".into())
        );
        let err = render("{{ kv('gone') }}", s).unwrap_err();
        assert!(err.contains("The key 'gone' does not exist"), "{err}");
        assert!(!pf.has_pending());
    }

    #[test]
    fn a_cached_outage_is_reported_by_the_scope_that_hands_it_back() {
        let pf = Prefetch::new();
        pf.state().answers.insert(
            kv_request("k"),
            Arc::new(Stored {
                answer: Answer::Kv(Err("store down".into())),
                outage: Some("store down".into()),
                limit: false,
            }),
        );
        let s = scope(&[], &pf, 10);
        assert!(render("{{ kv('k') }}", s.clone()).is_err());
        assert_eq!(s.0.infrastructure_failure(), Some("store down".into()));
    }

    #[test]
    fn after_max_rounds_a_miss_is_the_authors_error_naming_the_limit_and_the_read() {
        let pf = Prefetch::new();
        pf.state().rounds = MAX_ROUNDS;
        let t = in_tx(&[], &pf, 10);
        let err = render(
            "{{ kv('deep') }}",
            crate::runners::files::FileScope(t.clone()),
        )
        .unwrap_err();
        assert!(
            err.contains("key 'deep'") && err.contains(&format!("more than {MAX_ROUNDS} rounds")),
            "{err}"
        );
        assert!(!pf.has_pending(), "a final miss is answered, not pending");
        let noted = t.take_limit_failure().expect("noted for the run's message");
        assert!(noted.contains("key 'deep'"), "{noted}");
        assert_eq!(t.take_limit_failure(), None, "taken once");
    }

    /// [`MAX_FETCHES`] counts what this call fetched AND what this attempt
    /// has already recorded: the read past it is the author's error.
    #[test]
    fn past_max_fetches_a_miss_is_the_authors_error_naming_the_limit_and_the_read() {
        let pf = Prefetch::new();
        pf.state().fetched = MAX_FETCHES - 1;
        let err = render("{{ kv('last') ~ kv('over') }}", scope(&[], &pf, 10)).unwrap_err();
        assert!(
            err.contains("key 'over'") && err.contains(&format!("more than {MAX_FETCHES} values")),
            "{err}"
        );
        let pending: Vec<String> = pf.state().pending.keys().map(Request::what).collect();
        assert_eq!(pending, vec!["key 'last'".to_string()]);
    }

    /// m1: a re-bind clears the ANSWERS (they are one item's), never the
    /// call's limits.
    #[test]
    fn bind_clears_the_answers_for_another_owner_and_keeps_the_calls_limits() {
        let pf = Prefetch::new();
        pf.bind("a");
        seeded(&pf, kv_request("k"), Answer::Kv(Err("x".into())));
        {
            let mut s = pf.state();
            s.rounds = 3;
            s.fetched = 7;
            s.bytes = 11;
        }
        pf.bind("a");
        assert_eq!(pf.state().answers.len(), 1, "the same owner keeps them");
        pf.bind("b");
        let s = pf.state();
        assert_eq!(s.answers.len(), 0, "another owner's answers start over");
        assert_eq!(
            (s.rounds, s.fetched, s.bytes),
            (3, 7, 11),
            "the limits do not"
        );
    }

    /// m4: an entry counts against the byte cap once per call, however many
    /// times a re-bind makes it fetched again.
    #[test]
    fn the_cap_counts_each_distinct_entry_once_per_call() {
        let pf = Prefetch::new();
        let value = || {
            Answer::Kv(Ok(KvAnswer::Value(
                crate::storage::kv::value::KvData::String("abcd".into()),
            )))
        };
        pf.state().store(kv_request("k"), value(), None, 100);
        pf.bind("other");
        pf.state().store(kv_request("k"), value(), None, 100);
        let s = pf.state();
        assert_eq!(s.bytes, 8 + 4, "counted once");
        assert_eq!(s.fetched, 2, "fetched twice");
    }

    fn mem_store(max_read_bytes: u64) -> Arc<FileStore> {
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

    /// The fetch's per-request work, on the live snapshot, as
    /// `fetch_pending` runs it -- and the cap applied as it stores.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_fetch_answers_as_the_live_snapshot_and_the_cap_refuses_by_name() {
        let fs = mem_store(4);
        let (a, b) = (uri_of("a.txt"), uri_of("b.txt"));
        for (u, bytes) in [(&a, b"abcd"), (&b, b"efgh")] {
            fs.store
                .put(&fs.object_uri(WS, u).unwrap(), bytes.to_vec())
                .await
                .unwrap();
        }
        let files: BTreeMap<_, _> = [(a.clone(), Ok(file(&a, 4))), (b.clone(), Ok(file(&b, 4)))]
            .into_iter()
            .collect();
        let live = Snapshot::for_fetch(files, fs, WS, None, None);
        let whole = "read() returns the whole file as a string".to_string();
        let requests = vec![
            Request::ExecBody {
                workspace: WS,
                uri: a.clone(),
                whole: whole.clone(),
            },
            Request::ExecBody {
                workspace: WS,
                uri: b.clone(),
                whole,
            },
        ];
        let out = tokio::task::spawn_blocking(move || {
            // Not a runtime thread: what `may_block` asks of a reader.
            std::thread::spawn(move || fetch_on_this_thread(vec![(live, requests)]))
                .join()
                .unwrap()
        })
        .await
        .unwrap();
        let bytes: Vec<_> = out
            .iter()
            .map(|(_, answer, _)| match answer {
                Answer::Bytes(Ok(b)) => b.clone(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(bytes, vec![b"abcd".to_vec(), b"efgh".to_vec()]);

        // The cap: 4 x 1 bytes holds the first four-byte value only.
        let pf = Prefetch::new();
        for (request, answer, outage) in out {
            pf.state().store(request, answer, outage, CACHE_CAP_FACTOR);
        }
        let t = in_tx(&[(a.clone(), 4), (b.clone(), 4)], &pf, 1);
        let s = crate::runners::files::FileScope(t.clone());
        assert_eq!(
            render(&format!("{{{{ read('{a}') }}}}"), s.clone()),
            Ok("abcd".into())
        );
        assert_eq!(t.take_limit_failure(), None, "a plain hit is no limit");
        let err = render(&format!("{{{{ read('{b}') }}}}"), s).unwrap_err();
        assert!(
            err.contains(&b) && err.contains("more than 4 bytes") && err.contains("max_read_bytes"),
            "{err}"
        );
        let noted = t.take_limit_failure().expect("the cap is a limit");
        assert!(noted.contains("more than 4 bytes"), "{noted}");
    }

    #[test]
    fn kv_bytes_counts_text_and_nodes() {
        use crate::storage::kv::value::KvData;
        let d = KvData::Struct(vec![("ab".into(), KvData::String("xyz".into()))]);
        assert_eq!(kv_bytes(&d), 8 + 2 + 8 + 3);
    }
}
