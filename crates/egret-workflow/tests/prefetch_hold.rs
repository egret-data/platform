//! W9c Task 9 (final review F2, the user's ruling of 2026-10-08): **no pool
//! connection, row lock or open transaction is held while a render inside a
//! transaction downloads or connects.** Requires DATABASE_URL.
//!
//! # How a hold is seen
//!
//! The store in front of every read ([`GateStore`], a `MemStore`) can hold
//! ONE download open: [`while_downloading`] arms it for an object, starts the
//! work, waits until that object's body is being read, and then -- while the
//! body waits for its release -- probes ([`probe`]):
//!
//! * **the pool.** The work runs on a pool of ONE connection
//!   ([`setup`]'s `worker`), and `acquire` with a 1 s timeout must get it;
//! * **the rows.** From another connection, every row the transaction
//!   locked -- the execution(s), the claimed task run, the trigger, the
//!   outbox event, the window -- must take `FOR UPDATE NOWAIT`;
//! * **the transactions.** `pg_stat_activity` shows no backend of the
//!   worker's pool, nor of the namespace connection the reads use, `idle in
//!   transaction`.
//!
//! Then it releases the body and asserts on what the work did. Nothing here
//! sleeps to be right: the probe runs at a STATE (the body is being read),
//! and the waits are guards against a hang. A metadata-only namespace lookup
//! downloads nothing, so [`while_namespace_files_are_locked`] holds it
//! instead with a table lock, and probes once its connection waits on it.
//!
//! # Why the tests take turns
//!
//! A claim is global and the gate is process-wide, so the tests take turns
//! on [`SERIAL`], each in a fresh workspace; the database is this process's
//! own (`egret-test-db`).

mod common;

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use egret_core::UserId;
use egret_metastore::Db;
use egret_storage::{ObjectStore, StorageError};
use egret_workflow::flow_trigger::{process_events, process_events_with, PassLimits};
use egret_workflow::scheduler::{fire_due, PassReport};
use egret_workflow::storage::kv::key::KvKey;
use egret_workflow::storage::kv::value::KvType;
use egret_workflow::storage::nsfiles::access::{authorize, Access, Actor, Resource};
use egret_workflow::storage::nsfiles::path::NsPath;
use egret_workflow::storage::nsfiles::store::{put as ns_put, Conflict, Written};
use egret_workflow::storage::nsfiles::{self, CreatedBy};
use egret_workflow::storage::prefetch::{
    fetch_rounds_so_far, CACHE_CAP_FACTOR, MAX_FETCHES, MAX_ROUNDS,
};
use egret_workflow::storage::{self, uri, FileStore, StoredFile};
use egret_workflow::worker::{
    apply_outcome, claim, commit_outcome, reap, start_execution, start_execution_in, tick,
    tick_with, Handlers, NewExecution, Outcome, WorkerTiming,
};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The worker pool's `application_name`.
const TAG: &str = "w9c-t9-worker";
/// The namespace-file and KV lookups' `application_name`.
const NS_TAG: &str = "w9c-t9-ns";
const NS: &str = "w9c.t9";
/// This binary's `max_read_bytes`: small, so the cache cap is reachable.
const MAX_READ: u64 = 4096;

/// One armed hold: the first `get_stream` of an object whose key contains
/// `needle` says so on `started` and waits for `release`.
struct Gate {
    needle: String,
    started: Option<oneshot::Sender<()>>,
    release: Option<oneshot::Receiver<()>>,
}

static GATE: Mutex<Option<Gate>> = Mutex::new(None);
/// While set, every read fails as an unreachable S3 does.
static OUTAGE: AtomicBool = AtomicBool::new(false);
/// Every object key `get_stream` was asked for, in order.
static READS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// `MemStore`, with [`GATE`], [`OUTAGE`] and [`READS`] in front of `get_stream`.
struct GateStore(egret_storage::MemStore);

#[async_trait::async_trait]
impl ObjectStore for GateStore {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError> {
        self.0.put(uri, bytes).await
    }
    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
        self.0.get(uri).await
    }
    async fn delete(&self, uri: &str) -> Result<(), StorageError> {
        self.0.delete(uri).await
    }
    async fn copy(&self, src: &str, dst: &str) -> Result<(), StorageError> {
        self.0.copy(src, dst).await
    }
    async fn list(
        &self,
        prefix: &str,
        delimiter: Option<&str>,
    ) -> Result<egret_storage::Listing, StorageError> {
        self.0.list(prefix, delimiter).await
    }
    async fn get_stream(
        &self,
        uri: &str,
    ) -> Result<Box<dyn tokio::io::AsyncBufRead + Send + Unpin>, StorageError> {
        READS.lock().unwrap().push(uri.to_string());
        if OUTAGE.load(Ordering::SeqCst) {
            return Err(StorageError::S3(format!(
                "{uri}: dispatch failure: connection refused"
            )));
        }
        let gate = {
            let mut armed = GATE.lock().unwrap();
            match armed.as_ref() {
                Some(g) if uri.contains(&g.needle) => armed.take(),
                _ => None,
            }
        };
        let Some(gate) = gate else {
            return self.0.get_stream(uri).await;
        };
        let bytes = self.0.get(uri).await?;
        Ok(Box::new(tokio::io::BufReader::new(Gated {
            bytes,
            at: 0,
            started: gate.started,
            release: gate.release,
        })))
    }
    async fn put_stream(
        &self,
        uri: &str,
        body: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
        size_hint: Option<u64>,
    ) -> Result<u64, StorageError> {
        self.0.put_stream(uri, body, size_hint).await
    }
    async fn head(&self, uri: &str) -> Result<Option<u64>, StorageError> {
        self.0.head(uri).await
    }
    async fn delete_prefix(&self, uri: &str) -> Result<u64, StorageError> {
        self.0.delete_prefix(uri).await
    }
}

/// A body that says it has started, waits for its release, then hands over
/// its bytes: a download held open.
struct Gated {
    bytes: Vec<u8>,
    at: usize,
    started: Option<oneshot::Sender<()>>,
    release: Option<oneshot::Receiver<()>>,
}

impl AsyncRead for Gated {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if let Some(tx) = self.started.take() {
            let _ = tx.send(());
        }
        if let Some(rx) = self.release.as_mut() {
            match Pin::new(rx).poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(_) => self.release = None,
            }
        }
        let n = buf.remaining().min(self.bytes.len() - self.at);
        let at = self.at;
        buf.put_slice(&self.bytes[at..at + n]);
        self.at += n;
        Poll::Ready(Ok(()))
    }
}

struct Fx {
    db: Db,
    ws: Uuid,
    user: UserId,
    fs: Arc<FileStore>,
    /// A pool of ONE connection, tagged [`TAG`]: what the work runs on.
    worker: PgPool,
    _serial: tokio::sync::MutexGuard<'static, ()>,
}

/// The process-wide store ([`GateStore`]) and namespace connect options
/// (tagged [`NS_TAG`]), installed once; a quiet database; a fresh
/// workspace; the worker pool.
async fn setup() -> Option<Fx> {
    let serial = SERIAL.lock().await;
    let db = common::db().await?;
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = storage::install(Arc::new(FileStore {
            store: Arc::new(GateStore(egret_storage::MemStore::new())),
            bucket: "egret-test".to_string(),
            prefix: "workflow/".to_string(),
            caps: storage::Caps {
                max_read_bytes: MAX_READ,
                ..storage::Caps::default()
            },
            retention_days: 30,
        }));
        let _ = nsfiles::install_db(
            (*db.pool.connect_options())
                .clone()
                .application_name(NS_TAG),
        );
    });
    // Nothing another test left is claimable, due or unprocessed.
    for sql in [
        "DELETE FROM workflow.flow_trigger_events",
        "DELETE FROM workflow.flow_trigger_windows",
        "DELETE FROM workflow.executions",
        "DELETE FROM workflow.flow_listeners",
        "UPDATE workflow.triggers SET next_evaluation_date = NULL",
    ] {
        sqlx::query(sql).execute(&db.pool).await.expect(sql);
    }
    *GATE.lock().unwrap() = None;
    OUTAGE.store(false, Ordering::SeqCst);
    let fs = storage::current().expect("installed above");
    let (ws, user) = common::seed_workspace(&db).await;
    let worker = PgPoolOptions::new()
        .max_connections(1)
        .connect_with((*db.pool.connect_options()).clone().application_name(TAG))
        .await
        .expect("the worker pool");
    Some(Fx {
        db,
        ws,
        user,
        fs,
        worker,
        _serial: serial,
    })
}

/// A row some transaction of the work locks.
#[derive(Debug)]
enum Row {
    Execution(Uuid),
    TaskRun(Uuid),
    Trigger(Uuid, String),
    Event(i64),
    Window(Uuid, String),
}

/// What is held now, as a list of violations (empty: nothing is). See the
/// module doc.
async fn probe(db: &Db, worker: &PgPool, rows: &[Row]) -> Vec<String> {
    let mut held = Vec::new();
    match tokio::time::timeout(Duration::from_secs(1), worker.acquire()).await {
        Ok(Ok(conn)) => drop(conn),
        Ok(Err(e)) => held.push(format!("the worker pool's connection: {e}")),
        Err(_) => held.push("the worker pool's one connection is held".to_string()),
    }
    for row in rows {
        let mut tx = db.pool.begin().await.unwrap();
        let locked =
            match row {
                Row::Execution(id) => {
                    sqlx::query("SELECT 1 FROM workflow.executions WHERE id = $1 FOR UPDATE NOWAIT")
                        .bind(id)
                        .fetch_optional(&mut *tx)
                        .await
                }
                Row::TaskRun(id) => {
                    sqlx::query("SELECT 1 FROM workflow.task_runs WHERE id = $1 FOR UPDATE NOWAIT")
                        .bind(id)
                        .fetch_optional(&mut *tx)
                        .await
                }
                Row::Trigger(flow, id) => {
                    sqlx::query(
                        "SELECT 1 FROM workflow.triggers WHERE flow_id = $1 AND trigger_id = $2 \
                 FOR UPDATE NOWAIT",
                    )
                    .bind(flow)
                    .bind(id)
                    .fetch_optional(&mut *tx)
                    .await
                }
                Row::Event(id) => sqlx::query(
                    "SELECT 1 FROM workflow.flow_trigger_events WHERE id = $1 FOR UPDATE NOWAIT",
                )
                .bind(id)
                .fetch_optional(&mut *tx)
                .await,
                Row::Window(flow, id) => {
                    sqlx::query(
                        "SELECT 1 FROM workflow.flow_trigger_windows \
                  WHERE flow_id = $1 AND trigger_id = $2 FOR UPDATE NOWAIT",
                    )
                    .bind(flow)
                    .bind(id)
                    .fetch_optional(&mut *tx)
                    .await
                }
            };
        match locked {
            Ok(Some(_)) => {}
            // A row that is not there proves nothing about its lock.
            Ok(None) => held.push(format!(
                "{row:?} does not exist, so its lock was not probed"
            )),
            Err(e) => held.push(format!("{row:?} is locked: {e}")),
        }
        let _ = tx.rollback().await;
    }
    for tag in [TAG, NS_TAG] {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity \
              WHERE application_name = $1 AND state LIKE 'idle in transaction%'",
        )
        .bind(tag)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        if n > 0 {
            held.push(format!(
                "{n} backend(s) tagged {tag} are idle in a transaction"
            ));
        }
    }
    held
}

/// Run `work` with the first download of an object whose key contains
/// `needle` held open; probe while it is; release; return what `work`
/// returned, after asserting nothing was held.
async fn while_downloading<T: Send + 'static>(
    db: &Db,
    worker: &PgPool,
    needle: &str,
    rows: Vec<Row>,
    work: impl Future<Output = T> + Send + 'static,
) -> T {
    let (started_tx, started_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    *GATE.lock().unwrap() = Some(Gate {
        needle: needle.to_string(),
        started: Some(started_tx),
        release: Some(release_rx),
    });
    let mut task = tokio::spawn(work);
    // Guards against a hang, not measurements: the download starts at once
    // when the code is right.
    let started = tokio::time::timeout(Duration::from_secs(60), async {
        tokio::select! {
            s = started_rx => s.is_ok(),
            _ = &mut task => false,
        }
    })
    .await
    .unwrap_or(false);
    if !started {
        *GATE.lock().unwrap() = None;
        panic!("the work never downloaded an object named like {needle:?}");
    }
    let held = probe(db, worker, &rows).await;
    let _ = release_tx.send(());
    let out = task.await.expect("the work did not panic");
    assert!(
        held.is_empty(),
        "held while {needle:?} was downloading: {held:#?}"
    );
    out
}

/// [`while_downloading`] for a metadata-only namespace lookup, which
/// downloads nothing: `workflow.namespace_files` is locked `ACCESS
/// EXCLUSIVE` from another connection, and the probe runs once a backend
/// tagged [`NS_TAG`] waits on that lock -- the lookup in flight.
async fn while_namespace_files_are_locked<T: Send + 'static>(
    db: &Db,
    worker: &PgPool,
    rows: Vec<Row>,
    work: impl Future<Output = T> + Send + 'static,
) -> T {
    let mut lock = db.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE workflow.namespace_files IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await
        .unwrap();
    let mut task = tokio::spawn(work);
    let waiting = async {
        loop {
            let n: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity \
                  WHERE application_name = $1 AND wait_event_type = 'Lock'",
            )
            .bind(NS_TAG)
            .fetch_one(&db.pool)
            .await
            .unwrap();
            if n > 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    };
    let blocked = tokio::time::timeout(Duration::from_secs(60), async {
        tokio::select! {
            _ = waiting => true,
            _ = &mut task => false,
        }
    })
    .await
    .unwrap_or(false);
    if !blocked {
        let _ = lock.rollback().await;
        panic!("the work never looked a namespace file up");
    }
    let held = probe(db, worker, &rows).await;
    lock.rollback().await.unwrap();
    let out = task.await.expect("the work did not panic");
    assert!(
        held.is_empty(),
        "held while a namespace lookup was in flight: {held:#?}"
    );
    out
}

/// One live STRING revision of `key` in [`NS`], as the store writes it: a
/// row and its Ion object.
async fn seed_str(fx: &Fx, key: &str, text: &str) {
    let ion = serde_json::to_string(text).unwrap();
    let rev: i32 = sqlx::query_scalar(
        "SELECT coalesce(max(revision), 0) + 1 FROM workflow.kv_entries \
          WHERE workspace_id = $1 AND namespace = $2 AND key = $3",
    )
    .bind(fx.ws)
    .bind(NS)
    .bind(key)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflow.kv_entries \
           (workspace_id, namespace, key, revision, value_type, size, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, NULL)",
    )
    .bind(fx.ws)
    .bind(NS)
    .bind(key)
    .bind(rev)
    .bind(KvType::String.as_str())
    .bind(ion.len() as i64)
    .execute(&fx.db.pool)
    .await
    .unwrap_or_else(|e| panic!("seed {key}: {e}"));
    let grant = authorize(fx.ws, Actor::Api, NS, Resource::Kv, Access::Read).unwrap();
    let object = fx
        .fs
        .namespace_kv_uri(&grant, &KvKey::parse(key).unwrap(), rev as u32);
    fx.fs.store.put(&object, ion.into_bytes()).await.unwrap();
}

/// One revision of namespace file `path` in [`NS`], as an API upload writes it.
async fn put_ns_file(fx: &Fx, path: &str, bytes: &[u8]) {
    let grant = authorize(
        fx.ws,
        Actor::Api,
        NS,
        Resource::NamespaceFiles,
        Access::Write,
    )
    .unwrap();
    let mut body: &[u8] = bytes;
    match ns_put(
        &fx.db.pool,
        &fx.fs,
        &grant,
        &NsPath::parse_file(path).unwrap(),
        &mut body,
        Conflict::Overwrite,
        CreatedBy::Execution(Uuid::nil()),
    )
    .await
    {
        Ok(Written::Written(_)) => {}
        other => panic!("put {path}: {other:?}"),
    }
}

fn flow(id: &str, prelude: &str, tasks: &str) -> String {
    let src = format!("id: {id}\nnamespace: {NS}\n{prelude}tasks:\n{tasks}");
    egret_workflow::spec::parse(&src)
        .unwrap_or_else(|e| panic!("fixture must parse: {e:?}\n{src}"));
    src
}

fn log(id: &str) -> String {
    format!("  - id: {id}\n    type: io.kestra.plugin.core.log.Log\n    message: {id}\n")
}

/// An `If` on `condition` whose `then` is one `Log` task `then`.
fn if_then(id: &str, condition: &str, then: &str) -> String {
    let condition = serde_json::to_string(condition).unwrap();
    format!(
        "  - id: {id}\n    type: io.kestra.plugin.core.flow.If\n    condition: {condition}\n    \
         then:\n      - id: {then}\n        type: io.kestra.plugin.core.log.Log\n        \
         message: {then}\n"
    )
}

/// A `Subflow` of `flow_id` in [`NS`].
fn subflow(id: &str, flow_id: &str) -> String {
    let flow_id = serde_json::to_string(flow_id).unwrap();
    format!(
        "  - id: {id}\n    type: io.kestra.plugin.core.flow.Subflow\n    namespace: {NS}\n    \
         flowId: {flow_id}\n"
    )
}

/// A `Subflow` of the flow `child` (which [`seed_child`] files) whose one
/// input `x` is `expr` (a template): its render error is the run's message.
fn subflow_with_input(id: &str, expr: &str) -> String {
    let expr = serde_json::to_string(expr).unwrap();
    format!(
        "  - id: {id}\n    type: io.kestra.plugin.core.flow.Subflow\n    namespace: {NS}\n    \
         flowId: child\n    inputs:\n      x: {expr}\n"
    )
}

/// The flow `child`, one `Log`, for a `Subflow` to resolve.
async fn seed_child(fx: &Fx) {
    let child = flow("child", "", &log("k"));
    common::seed_flow(&fx.db, fx.ws, fx.user, &child, "draft").await;
}

/// Tick until nothing is claimable.
async fn drain(pool: &PgPool) {
    let reg = Handlers::with_core_tasks(storage::current());
    for _ in 0..512 {
        if tick(pool, 16, &reg).await.expect("tick") == 0 {
            return;
        }
    }
    panic!("tick never stopped claiming");
}

/// `(id, state, message)` of the run of `task` in `ex`, if it exists.
async fn run_of(db: &Db, ex: Uuid, task: &str) -> Option<(Uuid, String, Option<String>)> {
    sqlx::query_as(
        "SELECT id, state, message FROM workflow.task_runs \
          WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(ex)
    .bind(task)
    .fetch_optional(&db.pool)
    .await
    .unwrap()
}

async fn state_of(db: &Db, ex: Uuid) -> String {
    sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
        .bind(ex)
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

/// How many objects whose key contains `needle` were read so far.
fn reads_of(needle: &str) -> usize {
    READS
        .lock()
        .unwrap()
        .iter()
        .filter(|k| k.contains(needle))
        .count()
}

fn handlers() -> Handlers {
    Handlers::with_core_tasks(storage::current())
}

// ---------------------------------------------------------------------------
// The hold, per owner
// ---------------------------------------------------------------------------

/// `runIf` reading `kv()` in the settle that finishes the task before it
/// (`run_one`'s `apply_outcome`): the execution row and the claimed run are
/// free and the worker's one connection idle while the value downloads.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_if_reading_kv_after_a_task_holds_nothing_while_the_value_downloads() {
    let Some(fx) = setup().await else {
        return;
    };
    seed_str(&fx, "flag", "true").await;
    let src = flow(
        "runif_kv",
        "",
        &format!(
            "{}  - id: b\n    type: io.kestra.plugin.core.log.Log\n    runIf: \"{{{{ kv('flag') }}}}\"\n    message: b\n",
            log("a")
        ),
    );
    let ex = start_execution(&fx.db.pool, fx.ws, None, &src, json!({}))
        .await
        .unwrap();
    let (a, _, _) = run_of(&fx.db, ex, "a").await.unwrap();
    let worker = fx.worker.clone();
    let ticked = while_downloading(
        &fx.db,
        &fx.worker,
        "/_kv/flag.ion",
        vec![Row::Execution(ex), Row::TaskRun(a)],
        async move { tick(&worker, 4, &handlers()).await },
    )
    .await;
    assert_eq!(ticked.expect("the tick"), 1, "a's outcome committed");
    assert_eq!(run_of(&fx.db, ex, "a").await.unwrap().1, "SUCCESS");
    assert_eq!(
        run_of(&fx.db, ex, "b").await.unwrap().1,
        "RUNNING",
        "b's runIf read 'true': created and started"
    );
    drain(&fx.db.pool).await;
    assert_eq!(state_of(&fx.db, ex).await, "SUCCESS");
}

/// An `If` reading `kv()` as the FIRST task, decided in the first settle
/// (`start_execution`'s transaction): the worker's connection is free and no
/// transaction is open while the value downloads (the execution row itself
/// was rolled back with the attempt).
#[tokio::test(flavor = "multi_thread")]
async fn an_if_reading_kv_in_the_first_settle_holds_nothing_while_the_value_downloads() {
    let Some(fx) = setup().await else {
        return;
    };
    seed_str(&fx, "flag", "on").await;
    let src = flow("if_kv", "", &if_then("c", "{{ kv('flag') == 'on' }}", "t"));
    let (worker, ws) = (fx.worker.clone(), fx.ws);
    let ex = while_downloading(&fx.db, &fx.worker, "/_kv/flag.ion", vec![], async move {
        start_execution(&worker, ws, None, &src, json!({})).await
    })
    .await
    .expect("the start");
    assert!(
        run_of(&fx.db, ex, "t").await.is_some(),
        "the condition read 'on': `then` ran"
    );
    drain(&fx.db.pool).await;
    assert_eq!(state_of(&fx.db, ex).await, "SUCCESS");
}

/// A `Switch` reading `kv()` as the first task of a SCHEDULED flow: the
/// trigger row is free while the value downloads -- and the rolled-back
/// attempt is not a failed fire: one fire, no error, no `last_error`.
#[tokio::test(flavor = "multi_thread")]
async fn a_switch_reading_kv_in_a_scheduled_fire_holds_nothing_and_is_not_a_failed_fire() {
    let Some(fx) = setup().await else {
        return;
    };
    seed_str(&fx, "pick", "x").await;
    let src = flow(
        "switch_kv",
        "triggers:\n  - id: s\n    type: io.kestra.plugin.core.trigger.Schedule\n    cron: \"*/1 * * * *\"\n",
        "  - id: sw\n    type: io.kestra.plugin.core.flow.Switch\n    value: \"{{ kv('pick') }}\"\n    \
         cases:\n      x:\n        - id: on_x\n          type: io.kestra.plugin.core.log.Log\n          \
         message: x\n    defaults:\n      - id: other\n        type: io.kestra.plugin.core.log.Log\n        \
         message: other\n",
    );
    let flow_id = common::seed_flow(&fx.db, fx.ws, fx.user, &src, "draft").await;
    common::make_due(&fx.db, flow_id, "s", 1).await;
    let worker = fx.worker.clone();
    let report = while_downloading(
        &fx.db,
        &fx.worker,
        "/_kv/pick.ion",
        vec![Row::Trigger(flow_id, "s".into())],
        async move { fire_due(&worker).await },
    )
    .await
    .expect("the pass");
    assert_eq!(
        report,
        PassReport {
            fired: 1,
            ..PassReport::default()
        },
        "one fire; the miss is not an error"
    );
    let last_error: Option<String> = sqlx::query_scalar(
        "SELECT last_error FROM workflow.triggers WHERE flow_id = $1 AND trigger_id = 's'",
    )
    .bind(flow_id)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(last_error, None, "nothing recorded against the trigger");
    let ex: Uuid = sqlx::query_scalar("SELECT id FROM workflow.executions WHERE flow_id = $1")
        .bind(flow_id)
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
    assert!(run_of(&fx.db, ex, "on_x").await.is_some(), "case x ran");
    assert!(run_of(&fx.db, ex, "other").await.is_none());
    drain(&fx.db.pool).await;
}

/// The two flows of a Flow-trigger fire: a source that runs one `Log`, and
/// a listener on it whose first task is an `If` reading `kv()`.
fn flow_trigger_pair(listener_trigger: &str) -> (String, String) {
    let listener = flow(
        "listener",
        &format!(
            "triggers:\n  - id: on_done\n    type: io.kestra.plugin.core.trigger.Flow\n{listener_trigger}"
        ),
        &if_then("c", "{{ kv('flag') == 'on' }}", "t"),
    );
    (flow("p1", "", &log("a")), listener)
}

/// Every outbox event's id, oldest first.
async fn event_ids(db: &Db) -> Vec<(i64, i32)> {
    sqlx::query_as("SELECT id, attempts FROM workflow.flow_trigger_events ORDER BY id")
        .fetch_all(&db.pool)
        .await
        .unwrap()
}

/// A Flow-trigger fire whose listener's first task reads `kv()`: the event
/// row is free while the value downloads -- and the rolled-back attempt is
/// not an attempt: one fire, no error, the event done with.
#[tokio::test(flavor = "multi_thread")]
async fn an_if_reading_kv_in_a_flow_trigger_fire_holds_nothing_and_spends_no_attempt() {
    let Some(fx) = setup().await else {
        return;
    };
    seed_str(&fx, "flag", "on").await;
    let (source, listener) = flow_trigger_pair("    when: \"{{ flow.id == 'p1' }}\"\n");
    let listener_id = common::seed_flow(&fx.db, fx.ws, fx.user, &listener, "draft").await;
    let source_id = common::seed_flow(&fx.db, fx.ws, fx.user, &source, "draft").await;
    let src = start_execution(&fx.db.pool, fx.ws, Some(source_id), &source, json!({}))
        .await
        .unwrap();
    drain(&fx.db.pool).await;
    assert_eq!(state_of(&fx.db, src).await, "SUCCESS");
    let events = event_ids(&fx.db).await;
    assert!(!events.is_empty(), "the source's terminal is in the outbox");
    let worker = fx.worker.clone();
    let report = while_downloading(
        &fx.db,
        &fx.worker,
        "/_kv/flag.ion",
        events.iter().map(|(id, _)| Row::Event(*id)).collect(),
        async move { process_events(&worker).await },
    )
    .await
    .expect("the pass");
    assert_eq!(
        (report.fired, report.errored, report.born_failed),
        (1, 0, 0),
        "{report:?}"
    );
    assert_eq!(event_ids(&fx.db).await, vec![], "every event is done with");
    let fired: Uuid = sqlx::query_scalar("SELECT id FROM workflow.executions WHERE flow_id = $1")
        .bind(listener_id)
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
    assert!(run_of(&fx.db, fired, "t").await.is_some(), "`then` ran");
    drain(&fx.db.pool).await;
}

/// The `dependsOn` arm: the WINDOW row (written by the first source's event)
/// is free too while the second event's fire downloads the value.
#[tokio::test(flavor = "multi_thread")]
async fn a_depends_on_fire_holds_no_window_row_while_the_value_downloads() {
    let Some(fx) = setup().await else {
        return;
    };
    seed_str(&fx, "flag", "on").await;
    let (p1, listener) = flow_trigger_pair(&format!(
        "    dependsOn:\n      - namespace: {NS}\n        flowId: p1\n      - namespace: {NS}\n        flowId: p2\n"
    ));
    let p2 = flow("p2", "", &log("a"));
    let listener_id = common::seed_flow(&fx.db, fx.ws, fx.user, &listener, "draft").await;
    let p1_id = common::seed_flow(&fx.db, fx.ws, fx.user, &p1, "draft").await;
    let p2_id = common::seed_flow(&fx.db, fx.ws, fx.user, &p2, "draft").await;
    start_execution(&fx.db.pool, fx.ws, Some(p1_id), &p1, json!({}))
        .await
        .unwrap();
    drain(&fx.db.pool).await;
    let first = process_events(&fx.db.pool).await.expect("p1's pass");
    assert_eq!(first.accumulated, 1, "{first:?}");
    start_execution(&fx.db.pool, fx.ws, Some(p2_id), &p2, json!({}))
        .await
        .unwrap();
    drain(&fx.db.pool).await;
    let mut rows: Vec<Row> = event_ids(&fx.db)
        .await
        .into_iter()
        .map(|(id, _)| Row::Event(id))
        .collect();
    rows.push(Row::Window(listener_id, "on_done".into()));
    let worker = fx.worker.clone();
    let report = while_downloading(&fx.db, &fx.worker, "/_kv/flag.ion", rows, async move {
        process_events(&worker).await
    })
    .await
    .expect("the pass");
    assert_eq!((report.fired, report.errored), (1, 0), "{report:?}");
    assert_eq!(event_ids(&fx.db).await, vec![]);
    drain(&fx.db.pool).await;
}

/// The file the task just finished WRITING, read by its successor's `runIf`
/// in the same settle: its index row exists only in that transaction, which
/// is rolled back for the download -- so the fetch carries the resolution,
/// the body downloads with nothing held, and the retry indexes it again.
#[tokio::test(flavor = "multi_thread")]
async fn read_of_the_file_the_finished_task_wrote_holds_nothing_while_it_downloads() {
    let Some(fx) = setup().await else {
        return;
    };
    let src = flow(
        "read_written",
        "",
        &format!(
            "{}  - id: b\n    type: io.kestra.plugin.core.log.Log\n    runIf: \"{{{{ read(outputs.w.uri) == 'x' }}}}\"\n    message: b\n",
            log("w")
        ),
    );
    let ex = start_execution(&fx.db.pool, fx.ws, None, &src, json!({}))
        .await
        .unwrap();
    let c = claim(&fx.db.pool, 1).await.unwrap().remove(0);
    assert_eq!(c.task_id, "w");
    let file = format!(
        "{}out.txt",
        uri::task_prefix(NS, "read_written", ex, "w", c.id).unwrap()
    );
    fx.fs
        .store
        .put(&fx.fs.object_uri(fx.ws, &file).unwrap(), b"x".to_vec())
        .await
        .unwrap();
    let outcome = Outcome {
        files: vec![StoredFile {
            uri: file.clone(),
            size: 1,
        }],
        ..Outcome::succeeded_with(json!({ "uri": file }))
    };
    let (worker, run) = (fx.worker.clone(), c.id);
    let applied = while_downloading(
        &fx.db,
        &fx.worker,
        "/tasks/w/",
        vec![Row::Execution(ex), Row::TaskRun(run)],
        async move { commit_outcome(&worker, &c, &outcome).await },
    )
    .await;
    applied.expect("the outcome committed");
    assert_eq!(
        run_of(&fx.db, ex, "b").await.unwrap().1,
        "RUNNING",
        "read 'x'"
    );
    let indexed: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflow.execution_files WHERE uri = $1")
            .bind(&file)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(indexed, 1, "indexed once, by the attempt that committed");
    drain(&fx.db.pool).await;
}

/// A `Loop` over the values file the task before it wrote.
#[tokio::test(flavor = "multi_thread")]
async fn a_loop_over_a_file_holds_nothing_while_the_file_downloads() {
    let Some(fx) = setup().await else {
        return;
    };
    let src = flow(
        "loop_file",
        "",
        &format!(
            "{}  - id: l\n    type: io.kestra.plugin.core.flow.Loop\n    values: \"{{{{ outputs.w.uri }}}}\"\n    \
             tasks:\n      - id: each\n        type: io.kestra.plugin.core.log.Log\n        message: each\n",
            log("w")
        ),
    );
    let ex = start_execution(&fx.db.pool, fx.ws, None, &src, json!({}))
        .await
        .unwrap();
    let c = claim(&fx.db.pool, 1).await.unwrap().remove(0);
    let file = format!(
        "{}values.ion",
        uri::task_prefix(NS, "loop_file", ex, "w", c.id).unwrap()
    );
    fx.fs
        .store
        .put(&fx.fs.object_uri(fx.ws, &file).unwrap(), b"1 2 3".to_vec())
        .await
        .unwrap();
    let outcome = Outcome {
        files: vec![StoredFile {
            uri: file.clone(),
            size: 5,
        }],
        ..Outcome::succeeded_with(json!({ "uri": file }))
    };
    let (worker, run) = (fx.worker.clone(), c.id);
    while_downloading(
        &fx.db,
        &fx.worker,
        "/tasks/w/",
        vec![Row::Execution(ex), Row::TaskRun(run)],
        async move { commit_outcome(&worker, &c, &outcome).await },
    )
    .await
    .expect("the outcome committed");
    let values: Vec<String> = sqlx::query_scalar(
        "SELECT v.value FROM workflow.loop_file_values v \
           JOIN workflow.task_runs r ON r.execution_id = v.execution_id AND r.run_ix = v.run_ix \
          WHERE v.execution_id = $1 AND r.task_id = 'l' ORDER BY v.idx",
    )
    .bind(ex)
    .fetch_all(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(values, ["1", "2", "3"]);
    drain(&fx.db.pool).await;
    assert_eq!(state_of(&fx.db, ex).await, "SUCCESS");
}

/// A namespace file's `read()` in an `If`, after a task.
#[tokio::test(flavor = "multi_thread")]
async fn a_namespace_read_in_an_if_holds_nothing_while_the_file_downloads() {
    let Some(fx) = setup().await else {
        return;
    };
    put_ns_file(&fx, "cfg/x.txt", b"on").await;
    let src = flow(
        "ns_read",
        "",
        &format!(
            "{}{}",
            log("a"),
            if_then("c", "{{ read('cfg/x.txt') == 'on' }}", "t")
        ),
    );
    let ex = start_execution(&fx.db.pool, fx.ws, None, &src, json!({}))
        .await
        .unwrap();
    let (a, _, _) = run_of(&fx.db, ex, "a").await.unwrap();
    let worker = fx.worker.clone();
    while_downloading(
        &fx.db,
        &fx.worker,
        "/_files/cfg/x.txt",
        vec![Row::Execution(ex), Row::TaskRun(a)],
        async move { tick(&worker, 4, &handlers()).await },
    )
    .await
    .expect("the tick");
    assert!(run_of(&fx.db, ex, "t").await.is_some(), "read 'on'");
    drain(&fx.db.pool).await;
}

/// A namespace file's `fileURI()` in an `If`: metadata only, so nothing
/// downloads -- the namespace lookup itself is held, and the probe runs
/// while its connection waits; no transaction of the worker is open, so no
/// namespace connection is opened inside one.
#[tokio::test(flavor = "multi_thread")]
async fn a_namespace_file_uri_in_an_if_is_looked_up_with_no_transaction_open() {
    let Some(fx) = setup().await else {
        return;
    };
    put_ns_file(&fx, "cfg/y.txt", b"y").await;
    let src = flow(
        "ns_uri",
        "",
        &format!(
            "{}{}",
            log("a"),
            if_then("c", "{{ 'cfg/y.txt' in fileURI('cfg/y.txt') }}", "t")
        ),
    );
    let ex = start_execution(&fx.db.pool, fx.ws, None, &src, json!({}))
        .await
        .unwrap();
    let (a, _, _) = run_of(&fx.db, ex, "a").await.unwrap();
    let worker = fx.worker.clone();
    while_namespace_files_are_locked(
        &fx.db,
        &fx.worker,
        vec![Row::Execution(ex), Row::TaskRun(a)],
        async move { tick(&worker, 4, &handlers()).await },
    )
    .await
    .expect("the tick");
    assert!(
        run_of(&fx.db, ex, "t").await.is_some(),
        "the URI names the file"
    );
    drain(&fx.db.pool).await;
}

/// NESTED: a `Subflow` child finishes, and its terminal settles the PARENT
/// inside the child's transaction, where the parent's `If` reads `kv()`.
/// Both executions' rows and the child's claimed run are free while the
/// value downloads.
#[tokio::test(flavor = "multi_thread")]
async fn a_parent_settled_by_its_subflow_child_holds_neither_row_while_the_value_downloads() {
    let Some(fx) = setup().await else {
        return;
    };
    seed_str(&fx, "flag", "on").await;
    seed_child(&fx).await;
    let parent = flow(
        "parent",
        "",
        &format!(
            "{}{}",
            subflow("s", "child"),
            if_then("c", "{{ kv('flag') == 'on' }}", "t")
        ),
    );
    let ex = start_execution(&fx.db.pool, fx.ws, None, &parent, json!({}))
        .await
        .unwrap();
    let child_ex: Uuid =
        sqlx::query_scalar("SELECT id FROM workflow.executions WHERE trigger_execution_id = $1")
            .bind(ex)
            .fetch_one(&fx.db.pool)
            .await
            .expect("the child started");
    let (k, _, _) = run_of(&fx.db, child_ex, "k").await.unwrap();
    let worker = fx.worker.clone();
    while_downloading(
        &fx.db,
        &fx.worker,
        "/_kv/flag.ion",
        vec![
            Row::Execution(ex),
            Row::Execution(child_ex),
            Row::TaskRun(k),
        ],
        async move { tick(&worker, 4, &handlers()).await },
    )
    .await
    .expect("the tick");
    assert_eq!(state_of(&fx.db, child_ex).await, "SUCCESS");
    assert!(
        run_of(&fx.db, ex, "t").await.is_some(),
        "the parent read 'on'"
    );
    drain(&fx.db.pool).await;
    assert_eq!(state_of(&fx.db, ex).await, "SUCCESS");
}

// ---------------------------------------------------------------------------
// Rounds, the cap, an outage, freshness
// ---------------------------------------------------------------------------

/// `kv(kv(…))` nested `depth` deep over keys `{prefix}1 … {prefix}{depth}`,
/// the last holding `last`.
async fn seed_chain(fx: &Fx, prefix: &str, depth: usize, last: &str) -> String {
    for i in 1..depth {
        seed_str(fx, &format!("{prefix}{i}"), &format!("{prefix}{}", i + 1)).await;
    }
    seed_str(fx, &format!("{prefix}{depth}"), last).await;
    let mut expr = format!("'{prefix}1'");
    for _ in 0..depth {
        expr = format!("kv({expr})");
    }
    expr
}

/// A chain of dependent reads needs one round per level, and resolves while
/// it needs at most [`MAX_ROUNDS`]: each value is read once, outside the
/// transaction.
#[tokio::test(flavor = "multi_thread")]
async fn a_dependent_chain_resolves_within_the_round_limit() {
    let Some(fx) = setup().await else {
        return;
    };
    for (prefix, depth) in [("two", 2), ("atlimit", MAX_ROUNDS as usize)] {
        let expr = seed_chain(&fx, prefix, depth, "on").await;
        let before = reads_of(&format!("/_kv/{prefix}"));
        let src = flow(
            &format!("chain_{prefix}"),
            "",
            &if_then("c", &format!("{{{{ {expr} == 'on' }}}}"), "t"),
        );
        let ex = start_execution(&fx.db.pool, fx.ws, None, &src, json!({}))
            .await
            .unwrap();
        assert!(
            run_of(&fx.db, ex, "t").await.is_some(),
            "{prefix}: the chain resolved: {:?}",
            run_of(&fx.db, ex, "c").await
        );
        assert_eq!(
            reads_of(&format!("/_kv/{prefix}")) - before,
            depth,
            "{prefix}: one read per key"
        );
        drain(&fx.db.pool).await;
    }
}

/// One level more than [`MAX_ROUNDS`] is the AUTHOR's error, naming the
/// limit and the read that missed: the run fails, the start succeeds, and
/// the last key is never read.
#[tokio::test(flavor = "multi_thread")]
async fn a_chain_deeper_than_the_round_limit_is_the_authors_error_naming_it() {
    let Some(fx) = setup().await else {
        return;
    };
    let depth = MAX_ROUNDS as usize + 1;
    let expr = seed_chain(&fx, "deep", depth, "child").await;
    seed_child(&fx).await;
    let src = flow(
        "chain_deep",
        "",
        &subflow_with_input("s", &format!("{{{{ {expr} }}}}")),
    );
    let ex = start_execution(&fx.db.pool, fx.ws, None, &src, json!({}))
        .await
        .expect("an author's error is not the start's");
    let (_, state, message) = run_of(&fx.db, ex, "s").await.unwrap();
    assert_eq!(state, "FAILED");
    let message = message.expect("the subflow input's render error");
    assert!(
        message.contains(&format!("key 'deep{depth}'"))
            && message.contains(&format!("more than {MAX_ROUNDS} rounds")),
        "{message}"
    );
    assert_eq!(reads_of(&format!("/_kv/deep{depth}.ion")), 0, "never read");
    assert_eq!(reads_of("/_kv/deep"), MAX_ROUNDS as usize);
    drain(&fx.db.pool).await;
}

/// What one call caches is capped at [`CACHE_CAP_FACTOR`] x `max_read_bytes`:
/// the read that would cross it is the AUTHOR's error naming the cap.
#[tokio::test(flavor = "multi_thread")]
async fn reads_past_the_cache_cap_are_the_authors_error_naming_it() {
    let Some(fx) = setup().await else {
        return;
    };
    // Each value counts 8 + 3600 bytes: four fit under 4 x 4096, five do not.
    seed_child(&fx).await;
    let big = "v".repeat(3600);
    for i in 1..=5 {
        seed_str(&fx, &format!("big{i}"), &big).await;
    }
    let src = flow(
        "cache_cap",
        "",
        &subflow_with_input(
            "s",
            "{{ (kv('big1') ~ kv('big2') ~ kv('big3') ~ kv('big4') ~ kv('big5')) | length }}",
        ),
    );
    let ex = start_execution(&fx.db.pool, fx.ws, None, &src, json!({}))
        .await
        .expect("an author's error is not the start's");
    let (_, state, message) = run_of(&fx.db, ex, "s").await.unwrap();
    assert_eq!(state, "FAILED");
    let message = message.expect("the subflow input's render error");
    let cap = CACHE_CAP_FACTOR * MAX_READ;
    assert!(
        message.contains("key 'big5'")
            && message.contains(&format!("more than {cap} bytes"))
            && message.contains("max_read_bytes"),
        "{message}"
    );
    drain(&fx.db.pool).await;
}

/// An outage while the value is FETCHED is egret's, not the author's: the
/// tick is an `Err`, the finished task's outcome is not committed, no
/// attempt is spent, and the re-claimed run finishes once the store is back.
#[tokio::test(flavor = "multi_thread")]
async fn an_outage_during_the_fetch_re_claims_with_no_attempt_spent() {
    let Some(fx) = setup().await else {
        return;
    };
    seed_str(&fx, "flag", "true").await;
    let src = flow(
        "fetch_outage",
        "",
        &format!(
            "{}  - id: b\n    type: io.kestra.plugin.core.log.Log\n    runIf: \"{{{{ kv('flag') }}}}\"\n    message: b\n",
            log("a")
        ),
    );
    let ex = start_execution(&fx.db.pool, fx.ws, None, &src, json!({}))
        .await
        .unwrap();
    let attempts = || async {
        let n: i32 = sqlx::query_scalar(
            "SELECT jsonb_array_length(attempts) FROM workflow.task_runs \
              WHERE execution_id = $1 AND task_id = 'a'",
        )
        .bind(ex)
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
        n
    };
    let before = attempts().await;
    OUTAGE.store(true, Ordering::SeqCst);
    let ticked = tick(&fx.db.pool, 4, &handlers()).await;
    OUTAGE.store(false, Ordering::SeqCst);
    let err = ticked.expect_err("an outage is the worker's error");
    assert!(err.to_string().contains("key 'flag'"), "{err}");
    let (_, state, message) = run_of(&fx.db, ex, "a").await.unwrap();
    assert_eq!(
        (state.as_str(), message),
        ("RUNNING", None),
        "nothing committed"
    );
    assert!(run_of(&fx.db, ex, "b").await.is_none());
    assert_eq!(attempts().await, before, "no attempt spent");

    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(reap(&fx.db.pool, Duration::ZERO).await.unwrap(), 1);
    drain(&fx.db.pool).await;
    assert_eq!(state_of(&fx.db, ex).await, "SUCCESS");
    assert_eq!(attempts().await, before, "finished on the same attempt");
}

/// The cache lives for one call: a `LoopUntil` reads its condition's `kv()`
/// afresh at each iteration, so a value changed between two iterations is
/// seen by the second.
#[tokio::test(flavor = "multi_thread")]
async fn a_loop_until_reads_the_changed_value_at_its_next_iteration() {
    let Some(fx) = setup().await else {
        return;
    };
    seed_str(&fx, "n", "go").await;
    let src = flow(
        "loop_until_kv",
        "",
        "  - id: lu\n    type: io.kestra.plugin.core.flow.LoopUntil\n    \
         condition: \"{{ kv('n') == 'stop' }}\"\n    checkFrequency:\n      interval: PT0.2S\n      \
         maxIterations: 20\n    tasks:\n      - id: body\n        type: io.kestra.plugin.core.log.Log\n        \
         message: body\n",
    );
    let ex = start_execution(&fx.db.pool, fx.ws, None, &src, json!({}))
        .await
        .unwrap();
    // The first iteration: its condition reads 'go', so the loop pauses.
    tick(&fx.db.pool, 4, &handlers()).await.expect("tick");
    let history = || async {
        let h: serde_json::Value = sqlx::query_scalar(
            "SELECT history FROM workflow.task_runs WHERE execution_id = $1 AND task_id = 'lu'",
        )
        .bind(ex)
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
        h
    };
    assert_eq!(history().await, json!(["CREATED", "RUNNING", "PAUSED"]));
    seed_str(&fx, "n", "stop").await;
    // Waits out the pause (a guard, not the assertion): the next iteration
    // reads 'stop' and the loop ends after it.
    for _ in 0..200 {
        drain(&fx.db.pool).await;
        if state_of(&fx.db, ex).await == "SUCCESS" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(state_of(&fx.db, ex).await, "SUCCESS");
    let running = history()
        .await
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| *s == "RUNNING")
        .count();
    assert_eq!(running, 2, "two iterations: the second read the new value");
}

// ---------------------------------------------------------------------------
// Gather (the user's ruling on review C1, 2026-10-08): a miss does not end
// the attempt, so every read an attempt can reach is fetched in ONE round,
// and the values committed are a live render's.
// ---------------------------------------------------------------------------

/// The fetch rounds `work` ran (the process-wide test counter; the tests
/// take turns on [`SERIAL`], so nothing else fetches meanwhile).
async fn rounds_of<T>(work: impl Future<Output = T>) -> (T, u64) {
    let before = fetch_rounds_so_far();
    let out = work.await;
    (out, fetch_rounds_so_far() - before)
}

/// The flow `child_x`: one STRING input `x`, logged.
async fn seed_child_x(fx: &Fx) {
    let child = flow(
        "child_x",
        "inputs:\n  - id: x\n    type: STRING\n",
        "  - id: k\n    type: io.kestra.plugin.core.log.Log\n    message: \"{{ inputs.x }}\"\n",
    );
    common::seed_flow(&fx.db, fx.ws, fx.user, &child, "draft").await;
}

/// Review probe 1: nine INDEPENDENT `kv()` in ONE Subflow input. Every one
/// is discovered by the first attempt, fetched in one round, and the child
/// receives exactly what a live render gives.
#[tokio::test(flavor = "multi_thread")]
async fn nine_independent_reads_in_one_expression_are_fetched_in_one_round() {
    let Some(fx) = setup().await else {
        return;
    };
    seed_child_x(&fx).await;
    let mut expr = Vec::new();
    let mut live = String::new();
    for i in 1..=9 {
        seed_str(&fx, &format!("ex{i}"), &format!("v{i}")).await;
        expr.push(format!("kv('ex{i}')"));
        live.push_str(&format!("v{i}"));
    }
    let src = flow(
        "one_expr",
        "",
        &format!(
            "  - id: s\n    type: io.kestra.plugin.core.flow.Subflow\n    namespace: {NS}\n    \
             flowId: child_x\n    inputs:\n      x: \"{{{{ {} }}}}\"\n",
            expr.join(" ~ ")
        ),
    );
    let (ex, rounds) = rounds_of(start_execution(&fx.db.pool, fx.ws, None, &src, json!({}))).await;
    let ex = ex.expect("the start");
    assert_eq!(rounds, 1, "one fetch round answered all nine");
    assert_eq!(reads_of("/_kv/ex"), 9, "each value read once");
    let (_, state, message) = run_of(&fx.db, ex, "s").await.unwrap();
    assert_ne!(state, "FAILED", "{message:?}");
    let x: String = sqlx::query_scalar(
        "SELECT inputs->>'x' FROM workflow.executions WHERE trigger_execution_id = $1",
    )
    .bind(ex)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(x, live, "the value a live render gives");
    drain(&fx.db.pool).await;
    assert_eq!(state_of(&fx.db, ex).await, "SUCCESS");
}

/// Review probe 2: a `Loop` over nine values with `concurrencyLimit: 0`,
/// each iteration's `If` reading its OWN key. Every iteration's first
/// settle runs in the same attempt, so all nine reads are fetched in one
/// round; each iteration branches on its real value.
///
/// Both branches hold a task, so no iteration ENDS inside its first settle:
/// a parallel `Loop` whose iterations end there fails the start with a
/// duplicate `workflow_executions_loop_run_idx`, with or without a read
/// (fix round 1 report, concern 1), and a placeholder steering every
/// iteration to an empty branch would meet that defect in discovery.
#[tokio::test(flavor = "multi_thread")]
async fn nine_loop_iterations_each_reading_their_own_key_are_fetched_in_one_round() {
    let Some(fx) = setup().await else {
        return;
    };
    for i in 1..=9 {
        seed_str(
            &fx,
            &format!("lk{i}"),
            if i % 2 == 1 { "on" } else { "off" },
        )
        .await;
    }
    let src = flow(
        "loop_own_key",
        "",
        "  - id: loop\n    type: io.kestra.plugin.core.flow.Loop\n    \
         values: '[\"1\",\"2\",\"3\",\"4\",\"5\",\"6\",\"7\",\"8\",\"9\"]'\n    concurrencyLimit: 0\n    tasks:\n      \
         - id: branch\n        type: io.kestra.plugin.core.flow.If\n        \
         condition: \"{{ kv('lk' ~ item.value) == 'on' }}\"\n        then:\n          \
         - id: took_then\n            type: io.kestra.plugin.core.log.Log\n            \
         message: then\n        else:\n          \
         - id: took_else\n            type: io.kestra.plugin.core.log.Log\n            \
         message: else\n",
    );
    let (ex, rounds) = rounds_of(start_execution(&fx.db.pool, fx.ws, None, &src, json!({}))).await;
    let ex = ex.expect("the start");
    assert_eq!(rounds, 1, "one fetch round answered all nine iterations");
    assert_eq!(reads_of("/_kv/lk"), 9, "each value read once");
    drain(&fx.db.pool).await;
    let rows: Vec<(i32, String, bool)> = sqlx::query_as(
        "SELECT e.loop_index, r.state, \
                EXISTS (SELECT 1 FROM workflow.task_runs t \
                         WHERE t.execution_id = e.id AND t.task_id = 'took_then') \
           FROM workflow.executions e \
           JOIN workflow.task_runs r ON r.execution_id = e.id AND r.task_id = 'branch' \
          WHERE e.parent_execution_id = $1 ORDER BY e.loop_index",
    )
    .bind(ex)
    .fetch_all(&fx.db.pool)
    .await
    .unwrap();
    let expected: Vec<(i32, String, bool)> = (0..9)
        .map(|i| (i, "SUCCESS".to_string(), (i + 1) % 2 == 1))
        .collect();
    assert_eq!(rows, expected, "each iteration took its own value's branch");
    assert_eq!(state_of(&fx.db, ex).await, "SUCCESS");
}

/// Review probe 3: nine consecutive `runIf: kv(...)` tasks. Each task exists
/// only once the one before is decided; a placeholder decides it (falsy) in
/// the discovery attempt, so all nine reads are still found in one round --
/// and the committed attempt decides each on its real value.
#[tokio::test(flavor = "multi_thread")]
async fn nine_consecutive_run_ifs_are_fetched_in_one_round() {
    let Some(fx) = setup().await else {
        return;
    };
    let mut tasks = String::new();
    for i in 1..=9 {
        seed_str(&fx, &format!("rf{i}"), if i == 9 { "go" } else { "no" }).await;
        tasks.push_str(&format!(
            "  - id: t{i}\n    type: io.kestra.plugin.core.log.Log\n    runIf: \"{{{{ kv('rf{i}') == 'go' }}}}\"\n    message: t{i}\n"
        ));
    }
    let src = flow("runif_nine", "", &tasks);
    let (ex, rounds) = rounds_of(start_execution(&fx.db.pool, fx.ws, None, &src, json!({}))).await;
    let ex = ex.expect("the start");
    assert_eq!(rounds, 1, "one fetch round answered all nine");
    assert_eq!(reads_of("/_kv/rf"), 9, "each value read once");
    drain(&fx.db.pool).await;
    for i in 1..=8 {
        assert_eq!(
            run_of(&fx.db, ex, &format!("t{i}")).await.unwrap().1,
            "SKIPPED",
            "t{i} read 'no'"
        );
    }
    assert_eq!(
        run_of(&fx.db, ex, "t9").await.unwrap().1,
        "SUCCESS",
        "t9 read 'go'"
    );
    assert_eq!(state_of(&fx.db, ex).await, "SUCCESS");
}

/// The Flow-trigger arm: ONE event heard by nine listeners, each of whose
/// first settle reads its own key. Every listener is visited in the same
/// attempt, so one round answers all nine; nine fires, no error, and each
/// fired execution branched on its real value.
#[tokio::test(flavor = "multi_thread")]
async fn nine_listeners_of_one_event_each_reading_are_fetched_in_one_round() {
    let Some(fx) = setup().await else {
        return;
    };
    let mut listeners = Vec::new();
    for i in 1..=9 {
        seed_str(
            &fx,
            &format!("lsn{i}"),
            if i % 2 == 1 { "on" } else { "off" },
        )
        .await;
        let listener = flow(
            &format!("listener{i}"),
            "triggers:\n  - id: on_done\n    type: io.kestra.plugin.core.trigger.Flow\n    \
             when: \"{{ flow.id == 'p1' }}\"\n",
            &if_then("c", &format!("{{{{ kv('lsn{i}') == 'on' }}}}"), "t"),
        );
        listeners.push(common::seed_flow(&fx.db, fx.ws, fx.user, &listener, "draft").await);
    }
    let source = flow("p1", "", &log("a"));
    let source_id = common::seed_flow(&fx.db, fx.ws, fx.user, &source, "draft").await;
    start_execution(&fx.db.pool, fx.ws, Some(source_id), &source, json!({}))
        .await
        .unwrap();
    drain(&fx.db.pool).await;
    // The pass must reach the four events the `off` listeners' fires write
    // (their first settle ends them SUCCESS), so no duration cap may end it
    // first: under load the source's event alone can outrun the default 1s.
    let limits = PassLimits {
        max_duration: Duration::from_secs(3600),
        ..PassLimits::default()
    };
    let (report, rounds) = rounds_of(process_events_with(
        &fx.db.pool,
        &limits,
        &CancellationToken::new(),
    ))
    .await;
    let report = report.expect("the pass");
    assert_eq!(rounds, 1, "one fetch round answered all nine listeners");
    assert_eq!(reads_of("/_kv/lsn"), 9, "each value read once");
    assert_eq!(
        (report.fired, report.errored, report.born_failed),
        (9, 0, 0),
        "{report:?}"
    );
    assert_eq!(event_ids(&fx.db).await, vec![], "every event is done with");
    for (i, id) in listeners.iter().enumerate() {
        let fired: Uuid =
            sqlx::query_scalar("SELECT id FROM workflow.executions WHERE flow_id = $1")
                .bind(id)
                .fetch_one(&fx.db.pool)
                .await
                .unwrap();
        assert_eq!(
            run_of(&fx.db, fired, "t").await.is_some(),
            i % 2 == 0,
            "listener{} took its own value's branch",
            i + 1
        );
    }
    drain(&fx.db.pool).await;
}

/// [`MAX_FETCHES`]: one read more than the limit in one expression is the
/// AUTHOR's error, naming the limit and that read -- and, on an `If`, it is
/// the failed run's message (I2). The limit's reads are fetched, the one
/// past it never.
#[tokio::test(flavor = "multi_thread")]
async fn a_read_past_the_fetch_limit_is_the_authors_error_naming_it() {
    let Some(fx) = setup().await else {
        return;
    };
    let n = MAX_FETCHES + 1;
    let mut expr = Vec::new();
    for i in 1..=n {
        seed_str(&fx, &format!("lim{i}"), "v").await;
        expr.push(format!("kv('lim{i}')"));
    }
    let src = flow(
        "fetch_limit",
        "",
        &if_then(
            "c",
            &format!("{{{{ ({}) | length > 0 }}}}", expr.join(" ~ ")),
            "t",
        ),
    );
    let (ex, rounds) = rounds_of(start_execution(&fx.db.pool, fx.ws, None, &src, json!({}))).await;
    let ex = ex.expect("an author's error is not the start's");
    assert_eq!(rounds, 1);
    let (_, state, message) = run_of(&fx.db, ex, "c").await.unwrap();
    assert_eq!(state, "FAILED");
    let message = message.expect("the limit is the run's message");
    assert!(
        message.contains(&format!("key 'lim{n}'"))
            && message.contains(&format!("more than {MAX_FETCHES} values")),
        "{message}"
    );
    assert_eq!(reads_of(&format!("/_kv/lim{n}.ion")), 0, "never read");
    assert_eq!(reads_of("/_kv/lim"), MAX_FETCHES);
    assert!(run_of(&fx.db, ex, "t").await.is_none());
    drain(&fx.db.pool).await;
}

/// I2: a per-call limit met by a render in `settle` is the message of the
/// run it fails -- on `runIf`, an `If`, a `Switch`, a `Loop`'s `values:`, a
/// `Pause`'s `pauseDuration:` and a `LoopUntil`'s condition, not only on a
/// `Subflow`'s inputs. The limit here is the cache cap.
#[tokio::test(flavor = "multi_thread")]
async fn a_limit_met_by_any_render_in_settle_is_the_failed_runs_message() {
    let Some(fx) = setup().await else {
        return;
    };
    let big = "v".repeat(3600);
    for i in 1..=5 {
        seed_str(&fx, &format!("cap{i}"), &big).await;
    }
    // Five values of 8 + 3600 bytes: the fifth crosses 4 x 4096.
    let over = "(kv('cap1') ~ kv('cap2') ~ kv('cap3') ~ kv('cap4') ~ kv('cap5'))";
    let cases: Vec<(&str, String)> = vec![
        (
            "x",
            format!(
                "  - id: x\n    type: io.kestra.plugin.core.log.Log\n    runIf: \"{{{{ {over} | length > 0 }}}}\"\n    message: x\n"
            ),
        ),
        ("x", if_then("x", &format!("{{{{ {over} | length > 0 }}}}"), "t")),
        (
            "x",
            format!(
                "  - id: x\n    type: io.kestra.plugin.core.flow.Switch\n    value: \"{{{{ {over} | length }}}}\"\n    \
                 cases:\n      a:\n        - id: on_a\n          type: io.kestra.plugin.core.log.Log\n          message: a\n"
            ),
        ),
        (
            "x",
            format!(
                "  - id: x\n    type: io.kestra.plugin.core.flow.Loop\n    values: \"{{{{ {over} | length }}}}\"\n    \
                 tasks:\n      - id: body\n        type: io.kestra.plugin.core.log.Log\n        message: body\n"
            ),
        ),
        (
            "x",
            format!(
                "  - id: x\n    type: io.kestra.plugin.core.flow.Pause\n    pauseDuration: \"{{{{ {over} | length }}}}\"\n"
            ),
        ),
        (
            "x",
            format!(
                "  - id: x\n    type: io.kestra.plugin.core.flow.LoopUntil\n    \
                 condition: \"{{{{ {over} | length > 0 }}}}\"\n    checkFrequency:\n      interval: PT0.1S\n      \
                 maxIterations: 3\n    tasks:\n      - id: body\n        type: io.kestra.plugin.core.log.Log\n        \
                 message: body\n"
            ),
        ),
    ];
    let cap = CACHE_CAP_FACTOR * MAX_READ;
    for (n, (task, tasks)) in cases.into_iter().enumerate() {
        let src = flow(&format!("limit_msg_{n}"), "", &tasks);
        let ex = start_execution(&fx.db.pool, fx.ws, None, &src, json!({}))
            .await
            .expect("an author's error is not the start's");
        // A LoopUntil asks its condition after its body ran.
        drain(&fx.db.pool).await;
        let (_, state, message) = run_of(&fx.db, ex, task).await.unwrap();
        assert_eq!(state, "FAILED", "case {n}: {tasks}");
        let message = message.unwrap_or_else(|| panic!("case {n}: no message\n{tasks}"));
        assert!(
            message.contains("key 'cap5'") && message.contains(&format!("more than {cap} bytes")),
            "case {n}: {message}"
        );
    }
}

// ---------------------------------------------------------------------------
// The caller-owned, test-only wrappers (I3, m3): the caller is the gate.
// ---------------------------------------------------------------------------

/// `apply_outcome` and `start_execution_in` run in the CALLER's transaction,
/// which cannot be rolled back and fetched: a render that needs a read
/// answers `Err`, and the caller must not commit.
#[tokio::test(flavor = "multi_thread")]
async fn the_caller_owned_wrappers_answer_err_for_a_render_that_needs_a_read() {
    let Some(fx) = setup().await else {
        return;
    };
    seed_str(&fx, "wrapflag", "on").await;
    let src = flow(
        "wrapped",
        "",
        &if_then("c", "{{ kv('wrapflag') == 'on' }}", "t"),
    );
    let mut tx = fx.db.pool.begin().await.unwrap();
    let started = start_execution_in(
        &mut tx,
        NewExecution {
            workspace_id: fx.ws,
            flow_id: None,
            source: &src,
            inputs: json!({}),
            trigger: None,
            scheduled_date: None,
            trigger_execution_id: None,
            execution_depth: 0,
            flow_trigger_event_id: None,
            born_failed: false,
            replayed_from_execution_id: None,
            id: None,
            input_files: Vec::new(),
            labels: None,
            backfill_id: None,
            variables: None,
        },
    )
    .await;
    tx.rollback().await.unwrap();
    let err = started.expect_err("a start that needs a read is refused to its caller");
    assert!(err.to_string().contains("fetched"), "{err}");

    let src = flow(
        "wrapped_outcome",
        "",
        &format!(
            "{}  - id: b\n    type: io.kestra.plugin.core.log.Log\n    runIf: \"{{{{ kv('wrapflag') }}}}\"\n    message: b\n",
            log("a")
        ),
    );
    let ex = start_execution(&fx.db.pool, fx.ws, None, &src, json!({}))
        .await
        .unwrap();
    let c = claim(&fx.db.pool, 1).await.unwrap().remove(0);
    let mut tx = fx.db.pool.begin().await.unwrap();
    let applied = apply_outcome(&mut tx, &c, &Outcome::success()).await;
    tx.rollback().await.unwrap();
    let err = applied.expect_err("an outcome that needs a read is refused to its caller");
    assert!(err.to_string().contains("fetched"), "{err}");
    assert_eq!(run_of(&fx.db, ex, "a").await.unwrap().1, "RUNNING");
    assert_eq!(reads_of("/_kv/wrapflag"), 0, "nothing was read");
}

// ---------------------------------------------------------------------------
// The lease during the fetch (m5)
// ---------------------------------------------------------------------------

/// Arm the hold on the first download whose key contains `needle`: `started`
/// says it began, `release` lets it finish.
fn hold(needle: &str) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (started_tx, started_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    *GATE.lock().unwrap() = Some(Gate {
        needle: needle.to_string(),
        started: Some(started_tx),
        release: Some(release_rx),
    });
    (started_rx, release_tx)
}

async fn claimed_at(db: &Db, run: Uuid) -> chrono::DateTime<chrono::Utc> {
    sqlx::query_scalar("SELECT claimed_at FROM workflow.task_runs WHERE id = $1")
        .bind(run)
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

/// A run-If flow whose successor reads `kv('flag')` after `a`.
fn after_a_reads_flag(id: &str) -> String {
    flow(
        id,
        "",
        &format!(
            "{}  - id: b\n    type: io.kestra.plugin.core.log.Log\n    runIf: \"{{{{ kv('flag') }}}}\"\n    message: b\n",
            log("a")
        ),
    )
}

/// A fetch longer than `working_timeout` keeps the run: the lease is renewed
/// while the value downloads, so the reaper finds nothing to take, and the
/// outcome commits once it is released.
#[tokio::test(flavor = "multi_thread")]
async fn the_lease_is_renewed_while_a_long_fetch_downloads() {
    let Some(fx) = setup().await else {
        return;
    };
    seed_str(&fx, "flag", "true").await;
    let src = after_a_reads_flag("lease_renewed");
    let ex = start_execution(&fx.db.pool, fx.ws, None, &src, json!({}))
        .await
        .unwrap();
    let (a, _, _) = run_of(&fx.db, ex, "a").await.unwrap();
    // 1.5 s, renewed every 500 ms: a renewal may arrive up to 1 s late before
    // the `reap` below takes the lease (re-review n2; it was 600 ms and 400 ms).
    let timing = WorkerTiming {
        working_timeout: Duration::from_millis(1500),
        ..WorkerTiming::default()
    };
    let (started, release) = hold("/_kv/flag.ion");
    let worker = fx.worker.clone();
    let task = tokio::spawn(async move { tick_with(&worker, 4, &handlers(), timing).await });
    tokio::time::timeout(Duration::from_secs(60), started)
        .await
        .expect("the fetch began")
        .unwrap();
    let first = claimed_at(&fx.db, a).await;
    // Hold the download past two working timeouts: time must pass for a
    // lease to expire. What is asserted is the lease's state after it.
    tokio::time::sleep(timing.working_timeout * 2).await;
    let later = claimed_at(&fx.db, a).await;
    assert!(
        later > first,
        "renewed during the fetch: {first} -> {later}"
    );
    assert_eq!(
        reap(&fx.db.pool, timing.working_timeout).await.unwrap(),
        0,
        "a renewed lease is not reaped"
    );
    let _ = release.send(());
    let ticked = task.await.unwrap().expect("the tick");
    assert_eq!(ticked, 1, "the outcome committed");
    assert_eq!(run_of(&fx.db, ex, "a").await.unwrap().1, "SUCCESS");
    assert_eq!(run_of(&fx.db, ex, "b").await.unwrap().1, "RUNNING");
    drain(&fx.db.pool).await;
    assert_eq!(state_of(&fx.db, ex).await, "SUCCESS");
}

/// A lease lost while the value downloads (another worker took the run):
/// the worker gives the run up -- `LeaseLost`, not counted -- and records
/// NOTHING: no outcome, no message, no successor.
#[tokio::test(flavor = "multi_thread")]
async fn a_lease_lost_during_the_fetch_records_nothing() {
    let Some(fx) = setup().await else {
        return;
    };
    seed_str(&fx, "flag", "true").await;
    let src = after_a_reads_flag("lease_lost");
    let ex = start_execution(&fx.db.pool, fx.ws, None, &src, json!({}))
        .await
        .unwrap();
    let (a, _, _) = run_of(&fx.db, ex, "a").await.unwrap();
    let timing = WorkerTiming {
        working_timeout: Duration::from_millis(600),
        ..WorkerTiming::default()
    };
    let (started, release) = hold("/_kv/flag.ion");
    let worker = fx.worker.clone();
    let mut task = tokio::spawn(async move { tick_with(&worker, 4, &handlers(), timing).await });
    tokio::time::timeout(Duration::from_secs(60), started)
        .await
        .expect("the fetch began")
        .unwrap();
    // Another worker's claim: the fence moves under the fetch.
    let stolen = sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
        "UPDATE workflow.task_runs SET claimed_at = claimed_at + interval '1 hour' \
          WHERE id = $1 RETURNING claimed_at",
    )
    .bind(a)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    // The next renewal finds the fence moved and gives the run up, with the
    // download still held: the tick ends without waiting for it.
    let ticked = tokio::time::timeout(Duration::from_secs(30), &mut task)
        .await
        .expect("the worker gave the run up while the download was held")
        .unwrap();
    let _ = release.send(());
    assert_eq!(
        ticked.expect("a lost lease is not an error"),
        0,
        "not counted"
    );
    let (_, state, message) = run_of(&fx.db, ex, "a").await.unwrap();
    assert_eq!((state.as_str(), message), ("RUNNING", None), "no outcome");
    assert_eq!(
        claimed_at(&fx.db, a).await,
        stolen,
        "the other's lease stands"
    );
    let outputs: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT outputs FROM workflow.task_runs WHERE id = $1")
            .bind(a)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert!(
        outputs.is_none() || outputs == Some(json!({})) || outputs == Some(json!(null)),
        "{outputs:?}"
    );
    assert!(run_of(&fx.db, ex, "b").await.is_none(), "no successor");
}

// ---------------------------------------------------------------------------
// Fix round 2 (re-review N1): a `Loop` whose iterations END inside their
// first settle. Each iteration's terminal starts the next settle (the next
// iteration, or the loop's completion); written as recursion that nested one
// settle per iteration and a placeholder steering every iteration onto an
// empty branch aborted the process with a stack overflow. `claim::walk` runs
// that chain with an explicit stack; these run at 200 iterations, on a
// thread with tokio's worker stack (2 MiB), in a SUBPROCESS -- an overflow
// aborts the process, and the outer test then fails naming the signal
// instead of taking the whole binary down.
// ---------------------------------------------------------------------------

/// The iteration count: three times the 66 that aborted at `3d6c29e6`.
const MANY: usize = 200;
/// tokio's default worker stack.
const WORKER_STACK: usize = 2 * 1024 * 1024;

/// Set by [`in_a_subprocess`] on its child: an inner body runs only there,
/// so `cargo test -- --ignored` cannot run one in-process, where a regression
/// would abort the whole binary (re-review 2, m2).
const INNER_ENV: &str = "W9C_T9_INNER_CHILD";
/// How long a child may run before it is killed and its test fails.
const CHILD_TIMEOUT: Duration = Duration::from_secs(600);

/// Run `work` as a task on a multi-thread runtime whose workers have
/// [`WORKER_STACK`]: where a route or the claim loop polls it in production.
/// Only inside [`in_a_subprocess`]'s child.
fn on_a_worker_stack<F>(work: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    assert!(
        std::env::var_os(INNER_ENV).is_some(),
        "an inner body runs only in the child process its outer test starts; \
         run the test without the `inner_` prefix"
    );
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_stack_size(WORKER_STACK)
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(async move { tokio::spawn(work).await })
        .expect("the work panicked");
}

/// Run the `#[ignore]`d test `inner` of this binary alone in a child
/// process and fail unless it passed. A stack overflow aborts its process
/// (SIGABRT), which this reports as a failure naming the signal.
fn in_a_subprocess(inner: &str) {
    use std::io::Read;
    let mut child = std::process::Command::new(std::env::current_exe().expect("this binary"))
        .args([
            inner,
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(INNER_ENV, "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn the child test process");
    // Drained on their own threads, so a full pipe cannot stall the child.
    let drain = |mut pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let _ = pipe.read_to_end(&mut out);
            String::from_utf8_lossy(&out).into_owned()
        })
    };
    let stdout = drain(Box::new(child.stdout.take().unwrap()));
    let stderr = drain(Box::new(child.stderr.take().unwrap()));
    let deadline = std::time::Instant::now() + CHILD_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait for the child") {
            break Some(status);
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let stdout = stdout.join().unwrap();
    let stderr = stderr.join().unwrap();
    let tail = |s: &str| {
        s.lines()
            .rev()
            .take(25)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n")
    };
    let Some(status) = status else {
        panic!(
            "{inner} in a child process: killed after {CHILD_TIMEOUT:?}\n--- stdout\n{}\n--- stderr\n{}",
            tail(&stdout),
            tail(&stderr)
        );
    };
    #[cfg(unix)]
    let signal = std::os::unix::process::ExitStatusExt::signal(&status);
    #[cfg(not(unix))]
    let signal: Option<i32> = None;
    assert!(
        status.success() && stdout.contains("test result: ok. 1 passed"),
        "{inner} in a child process: status {status:?} (signal {signal:?})\n--- stdout\n{}\n--- stderr\n{}",
        tail(&stdout),
        tail(&stderr),
    );
}

/// `n` values, `"1"` .. `"n"`, as a `values:` JSON list in YAML.
fn many_values(n: usize) -> String {
    let vals: Vec<String> = (1..=n).map(|i| format!("\\\"{i}\\\"")).collect();
    format!("\"[{}]\"", vals.join(","))
}

/// A `Loop` over [`MANY`] values with `limit`, whose body is an `If` on
/// `condition` with a `then` (one `Log`, `took_then`) and NO `else`.
fn loop_no_else(limit: u32, condition: &str) -> String {
    loop_no_else_of(MANY, limit, condition)
}

/// [`loop_no_else`] over `n` values.
fn loop_no_else_of(n: usize, limit: u32, condition: &str) -> String {
    let condition = serde_json::to_string(condition).unwrap();
    format!(
        "  - id: loop\n    type: io.kestra.plugin.core.flow.Loop\n    values: {}\n    \
         concurrencyLimit: {limit}\n    tasks:\n      \
         - id: branch\n        type: io.kestra.plugin.core.flow.If\n        \
         condition: {condition}\n        then:\n          \
         - id: took_then\n            type: io.kestra.plugin.core.log.Log\n            \
         message: then\n",
        many_values(n)
    )
}

/// `(loop_index, state of its 'branch' run, whether it ran 'took_then')`,
/// per iteration of `ex`, in index order.
async fn iterations(db: &Db, ex: Uuid) -> Vec<(i32, String, bool)> {
    sqlx::query_as(
        "SELECT e.loop_index, r.state, \
                EXISTS (SELECT 1 FROM workflow.task_runs t \
                         WHERE t.execution_id = e.id AND t.task_id = 'took_then') \
           FROM workflow.executions e \
           JOIN workflow.task_runs r ON r.execution_id = e.id AND r.task_id = 'branch' \
          WHERE e.parent_execution_id = $1 ORDER BY e.loop_index",
    )
    .bind(ex)
    .fetch_all(&db.pool)
    .await
    .unwrap()
}

/// Every one of the `MANY` iterations exists once, its `If` SUCCEEDED and
/// took `then` iff `then`; the loop run and the execution SUCCEEDED.
async fn assert_every_iteration(db: &Db, ex: Uuid, then: bool) {
    assert_every_iteration_of(db, ex, MANY, then).await
}

/// [`assert_every_iteration`] for `n` iterations.
async fn assert_every_iteration_of(db: &Db, ex: Uuid, n: usize, then: bool) {
    let expected: Vec<(i32, String, bool)> = (0..n as i32)
        .map(|i| (i, "SUCCESS".to_string(), then))
        .collect();
    assert_eq!(iterations(db, ex).await, expected);
    let (_, loop_state, message) = run_of(db, ex, "loop").await.unwrap();
    assert_eq!(loop_state, "SUCCESS", "{message:?}");
    assert_eq!(state_of(db, ex).await, "SUCCESS");
}

/// No KV: a FALSE condition ends every iteration inside its first settle,
/// for real. `concurrencyLimit: 0` (the whole batch at once) and `1` (each
/// iteration started by the last one's terminal: the walk the recursion
/// nested).
async fn many_iterations_ending_in_their_first_settle(limit: u32) {
    let Some(fx) = setup().await else {
        return;
    };
    let src = flow(
        &format!("many_false_{limit}"),
        "",
        &loop_no_else(limit, "{{ item.value == 'x' }}"),
    );
    let (ex, rounds) = rounds_of(start_execution(&fx.db.pool, fx.ws, None, &src, json!({}))).await;
    let ex = ex.expect("the start");
    assert_eq!(rounds, 0, "nothing to read");
    // Everything happened in the start's one transaction.
    assert_every_iteration(&fx.db, ex, false).await;
    assert_eq!(tick(&fx.db.pool, 16, &handlers()).await.unwrap(), 0);
}

#[test]
#[ignore = "run by its outer test in a child process"]
fn inner_many_iterations_of_a_parallel_loop_end_in_their_first_settle() {
    on_a_worker_stack(many_iterations_ending_in_their_first_settle(0));
}

#[test]
fn many_iterations_of_a_parallel_loop_end_in_their_first_settle() {
    in_a_subprocess("inner_many_iterations_of_a_parallel_loop_end_in_their_first_settle");
}

#[test]
#[ignore = "run by its outer test in a child process"]
fn inner_many_iterations_of_a_sequential_loop_end_in_their_first_settle() {
    on_a_worker_stack(many_iterations_ending_in_their_first_settle(1));
}

#[test]
fn many_iterations_of_a_sequential_loop_end_in_their_first_settle() {
    in_a_subprocess("inner_many_iterations_of_a_sequential_loop_end_in_their_first_settle");
}

/// Every key `on`: live, every iteration takes `then` and waits for a
/// worker. The DISCOVERY attempt renders the placeholder `''`, so every
/// iteration's `If` is false there and ends inside its first settle: the
/// shape that aborted at 40. Started through the start ROUTE (`limit` 0).
async fn many_iterations_steered_by_a_placeholder_from_the_route() {
    let Some(fx) = setup().await else {
        return;
    };
    for i in 1..=MANY {
        seed_str(&fx, &format!("on{i}"), "on").await;
    }
    common::grant(
        &fx.db,
        fx.ws,
        fx.user,
        &["workflow:manage", "workflow:read"],
    )
    .await;
    let app = common::app(&fx.db).await;
    let src = flow(
        "many_kv_route",
        "",
        &loop_no_else(0, "{{ kv('on' ~ item.value) == 'on' }}"),
    );
    let flow_id = common::seed_flow(&fx.db, fx.ws, fx.user, &src, "draft").await;
    let (res, rounds) = rounds_of(common::post(
        &app,
        &format!("/api/ws/{}/workflows/flows/{flow_id}/executions", fx.ws),
        fx.user,
        json!({}),
    ))
    .await;
    let status = res.status();
    let body = common::json(res).await;
    assert_eq!(status, 201, "{body}");
    assert_eq!(
        rounds, 1,
        "every iteration's read found by one discovery attempt"
    );
    assert_eq!(reads_of("/_kv/on"), MANY, "each value read once");
    let ex: Uuid = body["id"].as_str().expect("the id").parse().unwrap();
    drain(&fx.db.pool).await;
    assert_every_iteration(&fx.db, ex, true).await;
}

#[test]
#[ignore = "run by its outer test in a child process"]
fn inner_many_iterations_steered_by_a_placeholder_start_from_the_route() {
    on_a_worker_stack(many_iterations_steered_by_a_placeholder_from_the_route());
}

#[test]
fn many_iterations_steered_by_a_placeholder_start_from_the_route() {
    in_a_subprocess("inner_many_iterations_steered_by_a_placeholder_start_from_the_route");
}

/// The same, with the `Loop` after a task: the discovery attempt is
/// `run_one`'s, where an abort was a crash loop (the run is re-claimed with
/// no attempt spent). `limit` 0 over [`MANY`], and 1 over [`SEQUENTIAL_MANY`]
/// (each of its iterations costs a tick and a round).
async fn many_iterations_steered_by_a_placeholder_after_a_task(limit: u32, n: usize) {
    let Some(fx) = setup().await else {
        return;
    };
    let tag = format!("t{limit}x");
    for i in 1..=n {
        seed_str(&fx, &format!("{tag}{i}"), "on").await;
    }
    let src = flow(
        &format!("many_kv_task_{limit}"),
        "",
        &format!(
            "{}{}",
            log("first"),
            loop_no_else_of(
                n,
                limit,
                &format!("{{{{ kv('{tag}' ~ item.value) == 'on' }}}}")
            )
        ),
    );
    let ex = start_execution(&fx.db.pool, fx.ws, None, &src, json!({}))
        .await
        .expect("the start");
    let (ticked, rounds) = rounds_of(tick(&fx.db.pool, 1, &handlers())).await;
    assert_eq!(ticked.expect("the tick that finishes 'first'"), 1);
    assert_eq!(
        rounds, 1,
        "every iteration's read found by one discovery attempt"
    );
    let (_, first, _) = run_of(&fx.db, ex, "first").await.unwrap();
    assert_eq!(first, "SUCCESS");
    drain(&fx.db.pool).await;
    assert_every_iteration_of(&fx.db, ex, n, true).await;
}

/// The sequential after-task test's iteration count: 1.5 x the 40 that
/// aborted at `3d6c29e6` (re-review 2, concern 2: 200 cost 5 minutes).
const SEQUENTIAL_MANY: usize = 60;

#[test]
#[ignore = "run by its outer test in a child process"]
fn inner_many_parallel_iterations_steered_by_a_placeholder_after_a_task() {
    on_a_worker_stack(many_iterations_steered_by_a_placeholder_after_a_task(
        0, MANY,
    ));
}

#[test]
fn many_parallel_iterations_steered_by_a_placeholder_after_a_task() {
    in_a_subprocess("inner_many_parallel_iterations_steered_by_a_placeholder_after_a_task");
}

#[test]
#[ignore = "run by its outer test in a child process"]
fn inner_many_sequential_iterations_steered_by_a_placeholder_after_a_task() {
    on_a_worker_stack(many_iterations_steered_by_a_placeholder_after_a_task(
        1,
        SEQUENTIAL_MANY,
    ));
}

#[test]
fn many_sequential_iterations_steered_by_a_placeholder_after_a_task() {
    in_a_subprocess("inner_many_sequential_iterations_steered_by_a_placeholder_after_a_task");
}

/// The pre-existing duplicate `workflow_executions_loop_run_idx` (fix round
/// 1, concern 1): a `concurrencyLimit: 0` loop whose EARLY iterations end
/// inside their first settle while later ones of the same batch have not
/// been created. Both shapes the base probe measured: every iteration ends
/// there (`== 'x'`), and only the first waits (`== '1'`).
#[tokio::test(flavor = "multi_thread")]
async fn a_parallel_loop_whose_iterations_end_in_their_first_settle_succeeds() {
    let Some(fx) = setup().await else {
        return;
    };
    for (name, condition, then) in [
        ("all_end", "{{ item.value == 'x' }}", [false, false, false]),
        (
            "first_waits",
            "{{ item.value == '1' }}",
            [true, false, false],
        ),
        (
            "last_waits",
            "{{ item.value == '3' }}",
            [false, false, true],
        ),
    ] {
        let condition = serde_json::to_string(condition).unwrap();
        let src = flow(
            &format!("dup_{name}"),
            "",
            &format!(
                "  - id: loop\n    type: io.kestra.plugin.core.flow.Loop\n    \
                 values: '[\"1\",\"2\",\"3\"]'\n    concurrencyLimit: 0\n    tasks:\n      \
                 - id: branch\n        type: io.kestra.plugin.core.flow.If\n        \
                 condition: {condition}\n        then:\n          \
                 - id: took_then\n            type: io.kestra.plugin.core.log.Log\n            \
                 message: then\n"
            ),
        );
        let ex = start_execution(&fx.db.pool, fx.ws, None, &src, json!({}))
            .await
            .unwrap_or_else(|e| panic!("{name}: the start: {e}"));
        drain(&fx.db.pool).await;
        let expected: Vec<(i32, String, bool)> = (0..3)
            .map(|i| (i, "SUCCESS".to_string(), then[i as usize]))
            .collect();
        assert_eq!(iterations(&fx.db, ex).await, expected, "{name}");
        assert_eq!(state_of(&fx.db, ex).await, "SUCCESS", "{name}");
    }
}

/// The re-review's reasoned case (§9): discovery that costs one round per
/// iteration meets the round limit, and the `Final` round fails an
/// iteration's `If` with the author's error -- an iteration that ends inside
/// its first settle in an attempt with NO miss pending, which used to be
/// where the duplicate key surfaced as a real database `Err` and `run_one`
/// re-claimed the run for ever.
///
/// One round per iteration: a SEQUENTIAL loop whose placeholder takes `then`
/// (`'' == ''`) where every real value (`'x'`) does not. Each attempt stops
/// at the first iteration it has not read (it waits for a worker), so round
/// `r` discovers iteration `r`; iteration `MAX_ROUNDS` meets the limit.
/// In a child process too: the walk is 65 iterations deep, which the
/// recursion of `3d6c29e6` overflowed.
async fn a_round_limit_met_inside_a_loop_iteration() {
    let Some(fx) = setup().await else {
        return;
    };
    let n = MAX_ROUNDS as usize + 6;
    let mut vals = Vec::new();
    for i in 1..=n {
        seed_str(&fx, &format!("rl{i}"), "x").await;
        vals.push(format!("\\\"{i}\\\""));
    }
    let src = flow(
        "round_limit_loop",
        "",
        &format!(
            "{}  - id: loop\n    type: io.kestra.plugin.core.flow.Loop\n    \
             values: \"[{}]\"\n    concurrencyLimit: 1\n    tasks:\n      \
             - id: branch\n        type: io.kestra.plugin.core.flow.If\n        \
             condition: \"{{{{ kv('rl' ~ item.value) == '' }}}}\"\n        then:\n          \
             - id: took_then\n            type: io.kestra.plugin.core.log.Log\n            \
             message: then\n",
            log("first"),
            vals.join(",")
        ),
    );
    let ex = start_execution(&fx.db.pool, fx.ws, None, &src, json!({}))
        .await
        .expect("the start");
    let (ticked, rounds) = rounds_of(tick(&fx.db.pool, 1, &handlers())).await;
    assert_eq!(
        ticked.expect("an author's error is not the tick's"),
        1,
        "the outcome committed"
    );
    assert_eq!(
        rounds, MAX_ROUNDS as u64,
        "one round per iteration, to the limit"
    );
    let limit = MAX_ROUNDS as i32;
    let rows = iterations(&fx.db, ex).await;
    let expected: Vec<(i32, String, bool)> = (0..=limit)
        .map(|i| {
            let state = if i == limit { "FAILED" } else { "SUCCESS" };
            (i, state.to_string(), false)
        })
        .collect();
    assert_eq!(
        rows, expected,
        "iterations 0..{limit} read 'x'; {limit} met the limit"
    );
    let message: Option<String> = sqlx::query_scalar(
        "SELECT r.message FROM workflow.task_runs r \
           JOIN workflow.executions e ON e.id = r.execution_id \
          WHERE e.parent_execution_id = $1 AND e.loop_index = $2 AND r.task_id = 'branch'",
    )
    .bind(ex)
    .bind(limit)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    let message = message.expect("the author's error");
    assert!(
        message.contains(&format!("key 'rl{}'", limit + 1))
            && message.contains(&format!("more than {MAX_ROUNDS} rounds")),
        "{message}"
    );
    let (_, loop_state, _) = run_of(&fx.db, ex, "loop").await.unwrap();
    assert_eq!(loop_state, "FAILED", "transmitFailed ends the loop");
    assert_eq!(state_of(&fx.db, ex).await, "FAILED");
    assert_eq!(
        tick(&fx.db.pool, 16, &handlers()).await.unwrap(),
        0,
        "nothing re-claimed"
    );
}

#[test]
#[ignore = "run by its outer test in a child process"]
fn inner_a_round_limit_met_inside_a_loop_iteration_fails_the_loop_and_is_not_re_claimed() {
    on_a_worker_stack(a_round_limit_met_inside_a_loop_iteration());
}

#[test]
fn a_round_limit_met_inside_a_loop_iteration_fails_the_loop_and_is_not_re_claimed() {
    in_a_subprocess(
        "inner_a_round_limit_met_inside_a_loop_iteration_fails_the_loop_and_is_not_re_claimed",
    );
}

// ---------------------------------------------------------------------------
// Fix round 3 (re-review 2, C2 and C3): settles started inside a settle's
// HEAD -- a refused `Subflow`'s completion, an inner `Loop`'s iterations --
// are jobs for the one walk too, so neither shape nests per iteration.
// ---------------------------------------------------------------------------

/// One revision of `key` typed `ty`, holding the Ion text `ion`.
async fn seed_typed(fx: &Fx, key: &str, ty: KvType, ion: &str) {
    sqlx::query(
        "INSERT INTO workflow.kv_entries \
           (workspace_id, namespace, key, revision, value_type, size, expires_at) \
         VALUES ($1, $2, $3, 1, $4, $5, NULL)",
    )
    .bind(fx.ws)
    .bind(NS)
    .bind(key)
    .bind(ty.as_str())
    .bind(ion.len() as i64)
    .execute(&fx.db.pool)
    .await
    .unwrap_or_else(|e| panic!("seed {key}: {e}"));
    let grant = authorize(fx.ws, Actor::Api, NS, Resource::Kv, Access::Read).unwrap();
    let object = fx
        .fs
        .namespace_kv_uri(&grant, &KvKey::parse(key).unwrap(), 1);
    fx.fs
        .store
        .put(&object, ion.as_bytes().to_vec())
        .await
        .unwrap();
}

/// A sequential (`concurrencyLimit: 1`), `transmitFailed: false` `Loop` over
/// `n` values whose body is one `Subflow` `sub` of `flow_id`, with the input
/// `x: <x>` when given.
fn loop_of_a_subflow(n: usize, flow_id: &str, x: Option<&str>) -> String {
    let inputs = x
        .map(|x| {
            format!(
                "        inputs:\n          x: {}\n",
                serde_json::to_string(x).unwrap()
            )
        })
        .unwrap_or_default();
    format!(
        "  - id: loop\n    type: io.kestra.plugin.core.flow.Loop\n    values: {}\n    \
         concurrencyLimit: 1\n    transmitFailed: false\n    tasks:\n      \
         - id: sub\n        type: io.kestra.plugin.core.flow.Subflow\n        \
         namespace: {NS}\n        flowId: {flow_id}\n{inputs}",
        many_values(n)
    )
}

/// `(loop_index, state of its 'sub' run, its message)` per iteration.
async fn sub_runs(db: &Db, ex: Uuid) -> Vec<(i32, String, Option<String>)> {
    sqlx::query_as(
        "SELECT e.loop_index, r.state, r.message FROM workflow.executions e \
           JOIN workflow.task_runs r ON r.execution_id = e.id AND r.task_id = 'sub' \
          WHERE e.parent_execution_id = $1 ORDER BY e.loop_index",
    )
    .bind(ex)
    .fetch_all(&db.pool)
    .await
    .unwrap()
}

/// C2, pre-existing: every iteration's `Subflow` is refused at spawn (its
/// flow does not exist), so its run is completed FAILED inside the
/// iteration's own head and the iteration ends there, starting the next.
/// That nested one head per iteration and aborted from 10.
///
/// From the 4th iteration on, the refusal is `subflow::MAX_SPAWN_CHAIN`'s
/// instead: `ExecutionCtx::depth` grows by 2 per iteration started from the
/// last one's terminal, exactly as before this round (it must refuse the same
/// flows), which is the pre-existing defect the sequential `wait: false` loop
/// shows (report, fix round 3, FOLLOW-UP). Either way the run FAILS and the
/// loop goes on.
async fn a_sequential_loop_of_a_refused_subflow() {
    let Some(fx) = setup().await else {
        return;
    };
    let src = flow("refused_sub", "", &loop_of_a_subflow(MANY, "missing", None));
    let (ex, rounds) = rounds_of(start_execution(&fx.db.pool, fx.ws, None, &src, json!({}))).await;
    let ex = ex.expect("the start");
    assert_eq!(rounds, 0);
    let runs = sub_runs(&fx.db, ex).await;
    assert_eq!(runs.len(), MANY, "every iteration ran");
    for (i, (index, state, message)) in runs.iter().enumerate() {
        assert_eq!((*index, state.as_str()), (i as i32, "FAILED"));
        let message = message.as_deref().unwrap_or_default();
        // Pinned per index (re-review 3 N2): iterations 0-2 reach the lookup
        // and are refused for the missing target; from iteration 3 on the
        // pre-existing depth growth (+3 per iteration) refuses first. A
        // regression that refused iteration 0 for depth -- a counter that no
        // longer resets per owner, say -- fails here.
        let want = if i < 3 {
            "unable to find flow 'w9c.t9'.'missing'"
        } else {
            "child execution(s) down a chain and 8 is as deep"
        };
        assert!(message.contains(want), "iteration {i}: {message}");
    }
    let (_, loop_state, _) = run_of(&fx.db, ex, "loop").await.unwrap();
    assert_eq!(loop_state, "SUCCESS", "transmitFailed: false");
    assert_eq!(state_of(&fx.db, ex).await, "SUCCESS");
}

#[test]
#[ignore = "run by its outer test in a child process"]
fn inner_a_sequential_loop_of_a_refused_subflow_succeeds() {
    on_a_worker_stack(a_sequential_loop_of_a_refused_subflow());
}

#[test]
fn a_sequential_loop_of_a_refused_subflow_succeeds() {
    in_a_subprocess("inner_a_sequential_loop_of_a_refused_subflow_succeeds");
}

/// C2, steered: the `Subflow`'s input is `{{ kv('cfg').a }}`, `cfg` a JSON
/// `{a: 5}`. Live it renders and each child waits for a worker, so each
/// iteration is started by its predecessor's child finishing, in a tick.
/// In the discovery attempt the placeholder does NOT render (`''.a`), so
/// every `Subflow` is refused and every iteration ends inside its first
/// settle, starting the next: the shape that aborted from 10 iterations.
async fn a_sequential_loop_of_a_subflow_whose_input_reads_kv() {
    let Some(fx) = setup().await else {
        return;
    };
    seed_typed(&fx, "cfg", KvType::Json, "{a:5}").await;
    let child = flow(
        "child_int",
        "inputs:\n  - id: x\n    type: INT\n",
        &log("k"),
    );
    common::seed_flow(&fx.db, fx.ws, fx.user, &child, "draft").await;
    let n = MANY / 2;
    let src = flow(
        "kv_input_sub",
        "",
        &loop_of_a_subflow(n, "child_int", Some("{{ kv('cfg').a }}")),
    );
    let (ex, rounds) = rounds_of(start_execution(&fx.db.pool, fx.ws, None, &src, json!({}))).await;
    let ex = ex.expect("the start");
    assert_eq!(rounds, 1, "one discovery round");
    // Live: iteration 0's child was started with the value and waits.
    assert_eq!(sub_runs(&fx.db, ex).await.len(), 1);
    drain(&fx.db.pool).await;
    let runs = sub_runs(&fx.db, ex).await;
    let expected: Vec<(i32, String, Option<String>)> = (0..n as i32)
        .map(|i| (i, "SUCCESS".to_string(), None))
        .collect();
    assert_eq!(runs, expected);
    let children: Vec<(String, String)> = sqlx::query_as(
        "SELECT c.inputs->>'x', c.state FROM workflow.executions c \
           JOIN workflow.executions e ON c.trigger_execution_id = e.id \
          WHERE e.parent_execution_id = $1",
    )
    .bind(ex)
    .fetch_all(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(
        children,
        vec![("5".to_string(), "SUCCESS".to_string()); n],
        "each child got the live value"
    );
    assert_eq!(state_of(&fx.db, ex).await, "SUCCESS");
}

#[test]
#[ignore = "run by its outer test in a child process"]
fn inner_a_sequential_loop_of_a_subflow_whose_input_is_a_kv_attribute() {
    on_a_worker_stack(a_sequential_loop_of_a_subflow_whose_input_reads_kv());
}

#[test]
fn a_sequential_loop_of_a_subflow_whose_input_is_a_kv_attribute() {
    in_a_subprocess("inner_a_sequential_loop_of_a_subflow_whose_input_is_a_kv_attribute");
}

/// N3 (re-review 3): the `wait: false` job path at scale. A `Loop` with
/// `concurrencyLimit: 0` over [`MANY`] values whose body is a `wait: false`
/// `Subflow` of a child that ends in its first settle (an `If` with no
/// `else`). Every iteration's `Subflows` phase returns two jobs -- the
/// child's first settle, then the iteration's own completion -- so the
/// start's one walk runs 200 children and 200 completions, and every
/// iteration ends inside it.
///
/// Parallel, because a SEQUENTIAL `wait: false` loop is refused by
/// `subflow::MAX_SPAWN_CHAIN` from its 4th iteration (pre-existing; W9c §15
/// FOLLOW-UP), so it cannot be driven to scale.
async fn a_parallel_loop_of_a_wait_false_subflow() {
    let Some(fx) = setup().await else {
        return;
    };
    let child = flow("instant", "", &if_then("gate", "{{ false }}", "never"));
    common::seed_flow(&fx.db, fx.ws, fx.user, &child, "draft").await;
    let body = format!(
        "  - id: loop\n    type: io.kestra.plugin.core.flow.Loop\n    values: {}\n    \
         concurrencyLimit: 0\n    tasks:\n      \
         - id: sub\n        type: io.kestra.plugin.core.flow.Subflow\n        \
         namespace: {NS}\n        flowId: instant\n        wait: false\n",
        many_values(MANY)
    );
    let src = flow("no_wait_sub", "", &body);
    let (ex, rounds) = rounds_of(start_execution(&fx.db.pool, fx.ws, None, &src, json!({}))).await;
    let ex = ex.expect("the start");
    assert_eq!(rounds, 0);
    let runs = sub_runs(&fx.db, ex).await;
    let expected: Vec<(i32, String, Option<String>)> = (0..MANY as i32)
        .map(|i| (i, "SUCCESS".to_string(), None))
        .collect();
    assert_eq!(runs, expected, "every iteration's `sub` completed SUCCESS");
    let children: Vec<(String, i64)> = sqlx::query_as(
        "SELECT c.state, count(*) FROM workflow.executions c \
           JOIN workflow.executions e ON c.trigger_execution_id = e.id \
          WHERE e.parent_execution_id = $1 GROUP BY c.state",
    )
    .bind(ex)
    .fetch_all(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(
        children,
        vec![("SUCCESS".to_string(), MANY as i64)],
        "one child per iteration, each ended in its first settle"
    );
    let recorded: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.task_runs r \
           JOIN workflow.executions e ON e.id = r.execution_id \
          WHERE e.parent_execution_id = $1 AND r.task_id = 'sub' \
            AND r.outputs->>'state' = 'CREATED'",
    )
    .bind(ex)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(
        recorded, MANY as i64,
        "each completion recorded the child's state at spawn, CREATED"
    );
    let (_, loop_state, _) = run_of(&fx.db, ex, "loop").await.unwrap();
    assert_eq!(loop_state, "SUCCESS");
    assert_eq!(state_of(&fx.db, ex).await, "SUCCESS");
}

#[test]
#[ignore = "run by its outer test in a child process"]
fn inner_a_parallel_loop_of_a_wait_false_subflow_succeeds() {
    on_a_worker_stack(a_parallel_loop_of_a_wait_false_subflow());
}

#[test]
fn a_parallel_loop_of_a_wait_false_subflow_succeeds() {
    in_a_subprocess("inner_a_parallel_loop_of_a_wait_false_subflow_succeeds");
}

/// C3: a sequential `Loop` over `outer` values whose body is a sequential
/// `Loop` over `inner` values whose body is an `If` with no `else` on
/// `condition`. Each outer iteration's head ran its inner loop's first
/// settles, the inner chain ended the outer iteration, and the next outer
/// iteration started inside that: one level per OUTER iteration.
fn nested_loops(outer: usize, inner: usize, condition: &str) -> String {
    format!(
        "  - id: outer\n    type: io.kestra.plugin.core.flow.Loop\n    values: {}\n    \
         concurrencyLimit: 1\n    tasks:\n      \
         - id: inner\n        type: io.kestra.plugin.core.flow.Loop\n        values: {}\n        \
         concurrencyLimit: 1\n        tasks:\n          \
         - id: branch\n            type: io.kestra.plugin.core.flow.If\n            \
         condition: {}\n            then:\n              \
         - id: took_then\n                type: io.kestra.plugin.core.log.Log\n                \
         message: then\n",
        many_values(outer),
        many_values(inner),
        serde_json::to_string(condition).unwrap()
    )
}

/// `(outer iterations, inner iterations, inner iterations that took
/// `then`)`, each counted only when SUCCESS; and whether any is not.
async fn nested_counts(db: &Db, ex: Uuid) -> (i64, i64, i64, i64) {
    sqlx::query_as(
        "WITH o AS (SELECT id, state FROM workflow.executions WHERE parent_execution_id = $1), \
              i AS (SELECT e.id, e.state FROM workflow.executions e JOIN o ON e.parent_execution_id = o.id) \
         SELECT (SELECT count(*) FROM o WHERE state = 'SUCCESS'), \
                (SELECT count(*) FROM i WHERE state = 'SUCCESS'), \
                (SELECT count(*) FROM i JOIN workflow.task_runs t \
                    ON t.execution_id = i.id AND t.task_id = 'took_then'), \
                (SELECT count(*) FROM o WHERE state <> 'SUCCESS') \
              + (SELECT count(*) FROM i WHERE state <> 'SUCCESS')",
    )
    .bind(ex)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

/// C3 with no KV: every inner `If` is false, so the whole of it runs in the
/// start's one transaction.
async fn nested_sequential_loops_ending_in_their_first_settle(outer: usize, inner: usize) {
    let Some(fx) = setup().await else {
        return;
    };
    let src = flow(
        &format!("nested_{outer}_{inner}"),
        "",
        &nested_loops(outer, inner, "{{ item.value == 'x' }}"),
    );
    let ex = start_execution(&fx.db.pool, fx.ws, None, &src, json!({}))
        .await
        .expect("the start");
    assert_eq!(
        nested_counts(&fx.db, ex).await,
        (outer as i64, (outer * inner) as i64, 0, 0)
    );
    assert_eq!(state_of(&fx.db, ex).await, "SUCCESS");
}

#[test]
#[ignore = "run by its outer test in a child process"]
fn inner_many_nested_sequential_loops_end_in_their_first_settle() {
    on_a_worker_stack(nested_sequential_loops_ending_in_their_first_settle(
        MANY / 2,
        2,
    ));
}

#[test]
fn many_nested_sequential_loops_end_in_their_first_settle() {
    in_a_subprocess("inner_many_nested_sequential_loops_end_in_their_first_settle");
}

#[test]
#[ignore = "run by its outer test in a child process"]
fn inner_thirty_by_thirty_nested_sequential_loops_end_in_their_first_settle() {
    on_a_worker_stack(nested_sequential_loops_ending_in_their_first_settle(30, 30));
}

#[test]
fn thirty_by_thirty_nested_sequential_loops_end_in_their_first_settle() {
    in_a_subprocess("inner_thirty_by_thirty_nested_sequential_loops_end_in_their_first_settle");
}

/// C3, steered: every inner `If` reads `kv('on')`, live `on`, so live each
/// inner iteration takes `then` and waits for a worker. The discovery
/// attempt's placeholder makes every one false: the whole nested chain,
/// 50 outer iterations deep, ran in that attempt and aborted.
async fn nested_sequential_loops_steered_by_a_placeholder_body() {
    let Some(fx) = setup().await else {
        return;
    };
    seed_str(&fx, "on", "on").await;
    let outer = 50;
    let src = flow(
        "nested_kv",
        "",
        &nested_loops(outer, 2, "{{ kv('on') == 'on' }}"),
    );
    let (ex, rounds) = rounds_of(start_execution(&fx.db.pool, fx.ws, None, &src, json!({}))).await;
    let ex = ex.expect("the start");
    assert_eq!(rounds, 1, "one discovery round");
    // Live: outer 0, inner 0 took `then` and waits.
    assert_eq!(nested_counts(&fx.db, ex).await, (0, 0, 1, 2));
    drain(&fx.db.pool).await;
    assert_eq!(
        nested_counts(&fx.db, ex).await,
        (outer as i64, (outer * 2) as i64, (outer * 2) as i64, 0)
    );
    assert_eq!(state_of(&fx.db, ex).await, "SUCCESS");
}

#[test]
#[ignore = "run by its outer test in a child process"]
fn inner_nested_sequential_loops_steered_by_a_placeholder() {
    on_a_worker_stack(nested_sequential_loops_steered_by_a_placeholder_body());
}

#[test]
fn nested_sequential_loops_steered_by_a_placeholder() {
    in_a_subprocess("inner_nested_sequential_loops_steered_by_a_placeholder");
}
