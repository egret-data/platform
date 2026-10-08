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
use egret_workflow::flow_trigger::process_events;
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
    let (report, rounds) = rounds_of(process_events(&fx.db.pool)).await;
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
    let timing = WorkerTiming {
        working_timeout: Duration::from_millis(600),
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
