//! Group C1 Task 4 (spec §2.3): `LogRepo::purge` and the
//! `io.kestra.plugin.core.log.PurgeLogs` task. Requires DATABASE_URL.
//!
//! **This deletes rows.** Every test makes its own workspaces, and every
//! purge is scoped to one of them, so no test can reach another's lines; the
//! tests that assert on another workspace seed that workspace themselves.
//! They take turns on [`SERIAL`] anyway, so a count read between two
//! statements is never another test's.

mod common;

use std::sync::Arc;

use chrono::{DateTime, Duration as ChronoDuration, TimeZone, Utc};
use egret_metastore::Db;
use egret_workflow::logs::{
    spawn_writer, LogLevel, LogLimits, LogLine, LogRepo, PurgeQuery, WriterConfig,
};
use egret_workflow::storage::{self, FileStore};
use egret_workflow::worker::{start_execution, tick, Handlers};
use serde_json::json;
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const LEVELS: [LogLevel; 5] = [
    LogLevel::Trace,
    LogLevel::Debug,
    LogLevel::Info,
    LogLevel::Warn,
    LogLevel::Error,
];

/// The first line's `ts`; the others are at +1 h and +2 h.
fn base() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
}

/// Far enough ahead that every seeded line is before it.
fn far_end() -> DateTime<Utc> {
    base() + ChronoDuration::days(365 * 10)
}

/// A bare, terminal execution row: the lines' foreign-key parent. Terminal,
/// so no worker would ever claim anything of it.
async fn execution(pool: &sqlx::PgPool, ws: Uuid, ns: &str, flow_key: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO workflow.executions \
           (workspace_id, namespace, flow_key, source, state, history) \
         VALUES ($1, $2, $3, 'id: f\ntasks: []\n', 'SUCCESS', \
                 '[\"CREATED\",\"RUNNING\",\"SUCCESS\"]'::jsonb) \
         RETURNING id",
    )
    .bind(ws)
    .bind(ns)
    .bind(flow_key)
    .fetch_one(pool)
    .await
    .expect("seed execution")
}

/// 15 lines on `ex`: every level at `base()`, `+1 h` and `+2 h`.
async fn seed_lines(pool: &sqlx::PgPool, ws: Uuid, ex: Uuid) {
    let mut lines = Vec::new();
    for h in 0..3 {
        for level in LEVELS {
            lines.push(LogLine {
                workspace_id: ws,
                execution_id: ex,
                task_run_id: None,
                task_id: Some("t".to_string()),
                attempt: Some(0),
                level,
                ts: base() + ChronoDuration::hours(h),
                message: format!("{level:?} at +{h}h"),
            });
        }
    }
    let n = LogRepo::new(pool.clone())
        .insert_batch(&lines)
        .await
        .expect("seed lines");
    assert_eq!(n, 15, "every seeded line is stored");
}

async fn seeded(pool: &sqlx::PgPool, ws: Uuid, ns: &str, flow_key: &str) -> Uuid {
    let ex = execution(pool, ws, ns, flow_key).await;
    seed_lines(pool, ws, ex).await;
    ex
}

async fn count(pool: &sqlx::PgPool, ex: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM workflow.task_logs WHERE execution_id = $1")
        .bind(ex)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Every line of workspace `ws`, by `(execution, level, ts)`, sorted: what a
/// purge left.
async fn remaining(pool: &sqlx::PgPool, ws: Uuid) -> Vec<(Uuid, i16, DateTime<Utc>)> {
    sqlx::query_as(
        "SELECT execution_id, level, ts FROM workflow.task_logs WHERE workspace_id = $1 \
          ORDER BY execution_id, level, ts",
    )
    .bind(ws)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn total(pool: &sqlx::PgPool, ws: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM workflow.task_logs WHERE workspace_id = $1")
        .bind(ws)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn query(ws: Uuid, namespaces: &[&str]) -> PurgeQuery {
    PurgeQuery {
        workspace_id: ws,
        namespaces: namespaces.iter().map(|s| s.to_string()).collect(),
        flow_key: None,
        execution_id: None,
        levels: None,
        start: None,
        end: far_end(),
        batch: None,
    }
}

struct Fx {
    db: Db,
    repo: LogRepo,
    _serial: tokio::sync::MutexGuard<'static, ()>,
}

async fn fx() -> Option<Fx> {
    let serial = SERIAL.lock().await;
    let db = common::db().await?;
    Some(Fx {
        repo: LogRepo::new(db.pool.clone()),
        db,
        _serial: serial,
    })
}

impl Fx {
    fn pool(&self) -> &sqlx::PgPool {
        &self.db.pool
    }

    async fn workspace(&self) -> Uuid {
        common::seed_workspace(&self.db).await.0
    }

    /// `PurgeLogs` reports the rows it deleted: the return value is checked
    /// against the workspace's count before and after, every time.
    async fn purge(&self, q: &PurgeQuery) -> u64 {
        let before = total(self.pool(), q.workspace_id).await;
        let n = self.repo.purge(q).await.expect("purge");
        let after = total(self.pool(), q.workspace_id).await;
        assert_eq!(
            n as i64,
            before - after,
            "the return value is the rows actually gone"
        );
        n
    }
}

/// The scope rule (spec §2.3): `a.b` reaches itself and `a.b.c` on a dot
/// boundary, never the sibling `a.bc`, another namespace, or another
/// workspace's `a.b`.
#[tokio::test]
async fn a_namespace_reaches_itself_and_its_children_and_nothing_else() {
    let Some(fx) = fx().await else { return };
    let (w1, w2) = (fx.workspace().await, fx.workspace().await);
    let p = fx.pool();
    let own_f1 = seeded(p, w1, "a.b", "f1").await;
    let own_f2 = seeded(p, w1, "a.b", "f2").await;
    let child = seeded(p, w1, "a.b.c", "f1").await;
    let sibling = seeded(p, w1, "a.bc", "f1").await;
    let parent = seeded(p, w1, "a", "f1").await;
    let other = seeded(p, w1, "z", "f1").await;
    let other_ws = seeded(p, w2, "a.b", "f1").await;

    assert_eq!(fx.purge(&query(w1, &["a.b"])).await, 45);

    for (ex, want, what) in [
        (own_f1, 0, "a.b f1"),
        (own_f2, 0, "a.b f2"),
        (child, 0, "a.b.c"),
        (sibling, 15, "the sibling a.bc"),
        (parent, 15, "the parent a"),
        (other, 15, "another namespace z"),
        (other_ws, 15, "W2's a.b"),
    ] {
        assert_eq!(count(p, ex).await, want, "{what}");
    }
}

/// The `sql-like-underscore` lesson: `_` and `%` in a grant are literal in
/// the child match, and so is `\`, the escape character.
#[tokio::test]
async fn like_wildcards_in_a_namespace_are_escaped() {
    let Some(fx) = fx().await else { return };
    let ws = fx.workspace().await;
    let p = fx.pool();
    let under = seeded(p, ws, "a_b", "f").await;
    let under_child = seeded(p, ws, "a_b.c", "f").await;
    let x = seeded(p, ws, "axb", "f").await;
    let x_child = seeded(p, ws, "axb.c", "f").await;

    assert_eq!(fx.purge(&query(ws, &["a_b"])).await, 30);
    assert_eq!(
        (count(p, under).await, count(p, under_child).await),
        (0, 0),
        "a_b and its child"
    );
    assert_eq!(
        (count(p, x).await, count(p, x_child).await),
        (15, 15),
        "`_` is not a wildcard: axb and axb.c stay"
    );

    // `%` and `\` (not valid in a namespace an author can write, but the
    // repository's rule must not lean on that).
    let pct = seeded(p, ws, "p%", "f").await;
    let pct_child = seeded(p, ws, "p%.c", "f").await;
    let pq = seeded(p, ws, "pq.c", "f").await;
    let p_dot = seeded(p, ws, "p.x.c", "f").await;
    assert_eq!(fx.purge(&query(ws, &["p%"])).await, 30);
    assert_eq!((count(p, pct).await, count(p, pct_child).await), (0, 0));
    assert_eq!(
        (count(p, pq).await, count(p, p_dot).await),
        (15, 15),
        "`%` is not a wildcard"
    );
    let bs = seeded(p, ws, "q\\", "f").await;
    let bs_x = seeded(p, ws, "q\\x.c", "f").await;
    assert_eq!(fx.purge(&query(ws, &["q\\"])).await, 15);
    assert_eq!((count(p, bs).await, count(p, bs_x).await), (0, 15));
}

/// `flow_key`, `execution_id` and `levels` each narrow the delete.
#[tokio::test]
async fn flow_execution_and_levels_each_narrow_the_delete() {
    let Some(fx) = fx().await else { return };
    let ws = fx.workspace().await;
    let p = fx.pool();
    let f1 = seeded(p, ws, "a.b", "f1").await;
    let f2 = seeded(p, ws, "a.b", "f2").await;
    let f1_child = seeded(p, ws, "a.b.c", "f1").await;

    // flow_key: f1 in a.b ONLY. With a flow key the namespace matches
    // exactly, as Kestra's (`AbstractJdbcLogDataStore.java:405-413`):
    // a.b.c's own f1 keeps its lines.
    let mut q = query(ws, &["a.b"]);
    q.flow_key = Some("f1".to_string());
    assert_eq!(fx.purge(&q).await, 15);
    assert_eq!(
        (
            count(p, f1).await,
            count(p, f2).await,
            count(p, f1_child).await
        ),
        (0, 15, 15),
        "a.b.c/f1 is not reached by a.b's flowId f1"
    );
    // The child's flow is reached by naming the child namespace itself.
    let mut q = query(ws, &["a.b.c"]);
    q.flow_key = Some("f1".to_string());
    assert_eq!(fx.purge(&q).await, 15);
    assert_eq!(count(p, f1_child).await, 0);

    // levels: INFO and ERROR only, on f2.
    let mut q = query(ws, &["a.b"]);
    q.levels = Some(vec![LogLevel::Info, LogLevel::Error]);
    assert_eq!(fx.purge(&q).await, 6);
    let levels: Vec<i16> = remaining(p, ws).await.into_iter().map(|r| r.1).collect();
    assert!(
        !levels.contains(&2) && !levels.contains(&4) && levels.len() == 9,
        "{levels:?}"
    );

    // execution_id: only that execution, and an execution of another
    // workspace reaches nothing even with its own id.
    let other_ws = fx.workspace().await;
    let foreign = seeded(p, other_ws, "a.b", "f2").await;
    let f2_again = seeded(p, ws, "a.b", "f2").await;
    let mut q = query(ws, &["a.b"]);
    q.execution_id = Some(foreign);
    assert_eq!(fx.purge(&q).await, 0, "another workspace's execution id");
    assert_eq!(count(p, foreign).await, 15);
    q.execution_id = Some(f2_again);
    assert_eq!(fx.purge(&q).await, 15);
    assert_eq!((count(p, f2_again).await, count(p, f2).await), (0, 9));

    // An execution id outside the namespace scope reaches nothing.
    let z = seeded(p, ws, "z", "f2").await;
    q.execution_id = Some(z);
    assert_eq!(fx.purge(&q).await, 0, "an execution outside the scope");
    assert_eq!(count(p, z).await, 15);
}

/// `[start, end]`, both inclusive as Kestra's (`greaterOrEqual`,
/// `lessOrEqual`): a line at exactly `start` goes, and so does one at
/// exactly `end`.
#[tokio::test]
async fn the_window_is_inclusive_at_both_ends() {
    let Some(fx) = fx().await else { return };
    let ws = fx.workspace().await;
    let p = fx.pool();
    let ex = seeded(p, ws, "a.b", "f").await;
    let mut q = query(ws, &["a.b"]);
    q.start = Some(base() + ChronoDuration::hours(1));
    q.end = base() + ChronoDuration::hours(2);
    assert_eq!(
        fx.purge(&q).await,
        10,
        "the +1h (start) and +2h (end) lines"
    );
    let ts: Vec<DateTime<Utc>> = remaining(p, ws).await.into_iter().map(|r| r.2).collect();
    assert_eq!(ts, vec![base(); 5], "only the lines before start stay");
    // No start: everything up to and including end.
    let mut q = query(ws, &["a.b"]);
    q.end = base();
    assert_eq!(fx.purge(&q).await, 5, "the base() lines, at exactly end");
    assert_eq!(count(p, ex).await, 0);
}

/// `batch = Some(2)` deletes exactly what one statement deletes, and returns
/// the same total, also when the total is a multiple of the batch.
#[tokio::test]
async fn batches_delete_what_one_statement_deletes() {
    let Some(fx) = fx().await else { return };
    let p = fx.pool();
    for batch in [Some(2), Some(5), Some(1000)] {
        let (one, many) = (fx.workspace().await, fx.workspace().await);
        let mut kept = Vec::new();
        for ws in [one, many] {
            for (ns, key) in [("a.b", "f1"), ("a.b.c", "f2"), ("a.bc", "f1"), ("z", "f1")] {
                seeded(p, ws, ns, key).await;
            }
            let mut q = query(ws, &["a.b"]);
            q.levels = Some(vec![LogLevel::Trace, LogLevel::Warn, LogLevel::Error]);
            if ws == many {
                q.batch = batch;
            }
            kept.push((fx.purge(&q).await, remaining(p, ws).await.len()));
        }
        assert_eq!(kept[0], kept[1], "{batch:?}");
        assert_eq!(kept[0], (18, 42), "{batch:?}");
    }
    // A batch below 1 is refused before anything is deleted.
    let ws = fx.workspace().await;
    seeded(p, ws, "a.b", "f").await;
    for bad in [0, -1] {
        let mut q = query(ws, &["a.b"]);
        q.batch = Some(bad);
        let err = fx.repo.purge(&q).await.unwrap_err().to_string();
        assert!(err.contains("batch"), "{bad}: {err}");
    }
    assert_eq!(total(p, ws).await, 15);
}

/// No namespace in the query reaches nothing: an empty scope is not "every
/// namespace".
#[tokio::test]
async fn an_empty_namespace_list_deletes_nothing() {
    let Some(fx) = fx().await else { return };
    let ws = fx.workspace().await;
    seeded(fx.pool(), ws, "a.b", "f").await;
    assert_eq!(fx.purge(&query(ws, &[])).await, 0);
    assert_eq!(total(fx.pool(), ws).await, 15);
}

// ---------------------------------------------------------------------------
// End to end, through the worker (Step 7)
// ---------------------------------------------------------------------------

/// The binary's store: `flow_namespace` reaches a handler through the render
/// scope, which only a configured store builds. A `MemStore`: nothing here
/// touches S3.
fn installed() -> Arc<FileStore> {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        storage::install(Arc::new(FileStore {
            store: Arc::new(egret_storage::MemStore::new()),
            bucket: "egret-test".to_string(),
            prefix: "workflow/".to_string(),
            caps: storage::Caps::default(),
            retention_days: 30,
        }))
        .expect("this binary installs the only store");
    });
    storage::current().expect("installed above")
}

/// The shipping registry for `container_runner = none` as far as this task
/// needs it: the core tasks and `PurgeLogs`.
fn registry(pool: &sqlx::PgPool) -> Handlers {
    let mut r = Handlers::with_core_tasks(storage::current());
    egret_workflow::worker::handlers::purge_logs::register(&mut r, pool.clone());
    r
}

async fn drain(pool: &sqlx::PgPool, reg: &Handlers) {
    for _ in 0..256 {
        let ticked = tick(pool, 16, reg).await;
        assert!(ticked.is_ok(), "a worker error: {ticked:?}");
        if ticked.unwrap() == 0 {
            return;
        }
    }
    panic!("tick never stopped claiming");
}

/// `(state, outputs, message)` of `task` in execution `ex`.
async fn result_of(
    pool: &sqlx::PgPool,
    ex: Uuid,
    task: &str,
) -> (String, Option<serde_json::Value>, Option<String>) {
    sqlx::query_as(
        "SELECT state, outputs, message FROM workflow.task_runs \
          WHERE execution_id = $1 AND task_id = $2",
    )
    .bind(ex)
    .bind(task)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// A one-task flow through the real worker, its lines stored by the real
/// writer (`PgLogSink`): SUCCESS, `outputs.count` is the lines it deleted --
/// the old lines of another execution of its namespace and of its child's
/// -- and the sibling's lines stay. None of the purging execution's own lines
/// are among them **only because this flow has no task before `purge`**
/// (measured: it has stored no line before the delete, and its one line, the
/// INFO naming the count, is written after it and survives). A flow with an
/// earlier task would lose that task's lines to the same delete, as in
/// Kestra (module doc of `worker::handlers::purge_logs`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_flow_purges_its_namespace_through_the_worker() {
    let Some(fx) = fx().await else { return };
    installed();
    let ws = fx.workspace().await;
    let other_ws = fx.workspace().await;
    let p = fx.pool();
    let old = seeded(p, ws, "p.q", "older").await;
    let child = seeded(p, ws, "p.q.c", "older").await;
    let sibling = seeded(p, ws, "p.qr", "older").await;
    let foreign = seeded(p, other_ws, "p.q", "older").await;

    let source = "id: purger\nnamespace: p.q\ntasks:\n  \
        - id: purge\n    type: io.kestra.plugin.core.log.PurgeLogs\n    \
          endDate: \"{{ now() | dateAdd(1, 'HOURS') }}\"\n";
    let ex = start_execution(p, ws, None, source, json!({}))
        .await
        .unwrap();
    let own_before = count(p, ex).await;
    let cancel = tokio_util::sync::CancellationToken::new();
    let (sink, writer) = spawn_writer(
        LogRepo::new(p.clone()),
        WriterConfig::default(),
        cancel.clone(),
    );
    let reg = registry(p).with_log_sink(Arc::new(sink), LogLimits::default());
    drain(p, &reg).await;
    drop(reg);
    cancel.cancel();
    writer.await.unwrap();

    let (state, outputs, message) = result_of(p, ex, "purge").await;
    assert_eq!(state, "SUCCESS", "{message:?}");
    assert_eq!(own_before, 0, "the purging execution had no stored line");
    assert_eq!(
        outputs,
        Some(json!({"count": 30, "executionLogsCount": 30, "nonExecutionLogsCount": 0})),
        "the seeded lines of p.q and p.q.c, and none of the purging execution's own"
    );
    assert_eq!((count(p, old).await, count(p, child).await), (0, 0));
    assert_eq!(count(p, sibling).await, 15, "the sibling p.qr");
    assert_eq!(count(p, foreign).await, 15, "another workspace's p.q");
    let own: Vec<(i16, String)> = sqlx::query_as(
        "SELECT level, message FROM workflow.task_logs WHERE execution_id = $1 ORDER BY id",
    )
    .bind(ex)
    .fetch_all(p)
    .await
    .unwrap();
    assert_eq!(
        own,
        [(
            2,
            "purged 30 log lines of namespace 'p.q' and its child namespaces".to_string()
        )],
        "the purging execution's own line, written after the delete"
    );
    let state: String = sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
        .bind(ex)
        .fetch_one(p)
        .await
        .unwrap();
    assert_eq!(state, "SUCCESS");
}

/// A `namespace:` outside the flow's own fails the run naming the rule, and
/// nothing is deleted -- not even the flow's own namespace's lines.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_namespace_fails_the_run_and_deletes_nothing() {
    let Some(fx) = fx().await else { return };
    installed();
    let ws = fx.workspace().await;
    let p = fx.pool();
    let own = seeded(p, ws, "p.q", "older").await;
    let target = seeded(p, ws, "purge.namespace", "older").await;

    let source = "id: purger\nnamespace: p.q\ntasks:\n  \
        - id: purge\n    type: io.kestra.plugin.core.log.PurgeLogs\n    \
          namespace: purge.namespace\n    \
          endDate: \"{{ now() | dateAdd(1, 'HOURS') }}\"\n";
    let ex = start_execution(p, ws, None, source, json!({}))
        .await
        .unwrap();
    drain(p, &registry(p)).await;

    let (state, _, message) = result_of(p, ex, "purge").await;
    assert_eq!(state, "FAILED");
    let message = message.unwrap_or_default();
    assert!(
        message.contains("may not purge logs of namespace 'purge.namespace'"),
        "{message}"
    );
    assert_eq!((count(p, own).await, count(p, target).await), (15, 15));
}
