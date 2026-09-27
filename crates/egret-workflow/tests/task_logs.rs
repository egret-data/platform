//! W11 Task 2: `workflow.task_logs` (migration 0159) and `LogRepo`; W11
//! Task 4: the batch writer (`PgLogSink`), the retention pruner, and a flow
//! logging through the real runtime. Requires DATABASE_URL.
//!
//! Every test makes its own workspace and execution, so the per-execution
//! reads cannot see each other's rows. Pruning is not per-execution -- it
//! deletes by `ts` across the table -- so **the tests in this binary run one
//! at a time** (`SERIAL`), and a prune can never take another test's lines.
//!
//! The settle lag is Postgres `now()` against the row's `inserted_at`. A test
//! that needs a row to be settled moves the ROW back (`settle`), never a
//! clock, and never sleeps.
//!
//! See docs/en/specs/2026-09-27-workflow-w11-execution-logs-design.md §3, §6
//! and §7.

mod common;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Duration as ChronoDuration, SubsecRound, Utc};
use egret_metastore::Db;
use egret_workflow::logs::{
    spawn_pruner, spawn_writer, InsertError, Inserter, LogLevel, LogLimits, LogLine, LogQuery,
    LogRepo, PruneConfig, StoredLine, WriterConfig,
};
use egret_workflow::runtime::{spawn_with_logs, RuntimeConfig};
use egret_workflow::worker::handlers::LogSink;
use egret_workflow::worker::{start_execution, Handlers};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The longest piece a split line can be (user decision D6: Kestra's
/// `RunContextLogger` splits at 15,360 characters).
const PIECE_CHARS: usize = 15_360;

struct Fx {
    db: Db,
    repo: LogRepo,
    ws: Uuid,
    _serial: tokio::sync::MutexGuard<'static, ()>,
}

async fn fx() -> Option<Fx> {
    let serial = SERIAL.lock().await;
    let db = common::db().await?;
    let (ws, _user) = common::seed_workspace(&db).await;
    Some(Fx {
        repo: LogRepo::new(db.pool.clone()),
        db,
        ws,
        _serial: serial,
    })
}

impl Fx {
    /// A bare execution row in `ws` -- the lines' foreign-key parent.
    async fn execution_in(&self, ws: Uuid) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO workflow.executions \
               (workspace_id, namespace, flow_key, source, state, history) \
             VALUES ($1, 'w11', 'f', 'id: f\ntasks: []\n', 'RUNNING', \
                     '[\"CREATED\",\"RUNNING\"]'::jsonb) \
             RETURNING id",
        )
        .bind(ws)
        .fetch_one(&self.db.pool)
        .await
        .expect("seed execution")
    }

    async fn execution(&self) -> Uuid {
        self.execution_in(self.ws).await
    }

    fn line(&self, execution: Uuid, level: LogLevel, message: &str) -> LogLine {
        LogLine {
            workspace_id: self.ws,
            execution_id: execution,
            task_run_id: None,
            task_id: None,
            attempt: None,
            level,
            ts: now_us(),
            message: message.to_string(),
        }
    }

    /// Move every line of `execution` 3 s into the past, past the 2 s settle
    /// lag, as if that much time had gone by since they were inserted.
    async fn settle(&self, execution: Uuid) {
        sqlx::query(
            "UPDATE workflow.task_logs SET inserted_at = inserted_at - interval '3 seconds' \
              WHERE execution_id = $1",
        )
        .bind(execution)
        .execute(&self.db.pool)
        .await
        .expect("backdate inserted_at");
    }

    async fn count(&self, execution: Uuid) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM workflow.task_logs WHERE execution_id = $1")
            .bind(execution)
            .fetch_one(&self.db.pool)
            .await
            .unwrap()
    }

    async fn page(&self, execution: Uuid, q: &LogQuery) -> Vec<StoredLine> {
        self.repo.page(self.ws, execution, q).await.expect("page")
    }
}

/// `Utc::now()` at Postgres's precision, so a round trip compares equal.
fn now_us() -> DateTime<Utc> {
    Utc::now().trunc_subsecs(6)
}

fn q(after: Option<i64>, limit: i64) -> LogQuery {
    LogQuery {
        after,
        limit,
        task_id: None,
        task_run_id: None,
        min_level: None,
    }
}

fn messages(lines: &[StoredLine]) -> Vec<&str> {
    lines.iter().map(|l| l.message.as_str()).collect()
}

#[tokio::test]
async fn insert_batch_round_trips_every_field() {
    let Some(fx) = fx().await else { return };
    let exec = fx.execution().await;
    let run = Uuid::new_v4();
    let t0 = now_us() - ChronoDuration::minutes(5);
    let long: String = "語".repeat(PIECE_CHARS);
    assert_eq!(long.chars().count(), PIECE_CHARS);
    assert_eq!(
        long.len(),
        3 * PIECE_CHARS,
        "control: multi-byte, 46,080 bytes"
    );

    let lines = vec![
        LogLine {
            workspace_id: fx.ws,
            execution_id: exec,
            task_run_id: Some(run),
            task_id: Some("step_1".into()),
            attempt: Some(2),
            level: LogLevel::Warn,
            ts: t0,
            message: "héllo 世界 🎉 — ok".into(),
        },
        LogLine {
            workspace_id: fx.ws,
            execution_id: exec,
            task_run_id: None,
            task_id: None,
            attempt: None,
            level: LogLevel::Trace,
            ts: t0 + ChronoDuration::milliseconds(1),
            message: String::new(),
        },
        LogLine {
            workspace_id: fx.ws,
            execution_id: exec,
            task_run_id: Some(run),
            task_id: Some("step_1".into()),
            attempt: Some(0),
            level: LogLevel::Error,
            ts: t0 + ChronoDuration::microseconds(2_001),
            message: long.clone(),
        },
    ];

    let n = fx.repo.insert_batch(&lines).await.expect("insert");
    assert_eq!(n, 3, "rows affected");
    fx.settle(exec).await;

    let got = fx.page(exec, &q(None, 10)).await;
    let back: Vec<LogLine> = got
        .iter()
        .map(|s| LogLine {
            workspace_id: fx.ws,
            execution_id: exec,
            task_run_id: s.task_run_id,
            task_id: s.task_id.clone(),
            attempt: s.attempt,
            level: s.level,
            ts: s.ts,
            message: s.message.clone(),
        })
        .collect();
    assert_eq!(back, lines, "every field, in insertion order");
    assert_eq!(
        got[2].message.len(),
        46_080,
        "the long piece is stored whole"
    );

    // The stored columns themselves, not only what `page` maps them to.
    let (ws, level, attempt): (Uuid, i16, Option<i32>) =
        sqlx::query_as("SELECT workspace_id, level, attempt FROM workflow.task_logs WHERE id = $1")
            .bind(got[0].id)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!((ws, level, attempt), (fx.ws, 3, Some(2)));

    assert_eq!(fx.repo.insert_batch(&[]).await.expect("empty"), 0);
}

#[tokio::test]
async fn page_returns_nothing_younger_than_the_settle_lag() {
    let Some(fx) = fx().await else { return };
    let exec = fx.execution().await;
    fx.repo
        .insert_batch(&[fx.line(exec, LogLevel::Info, "fresh")])
        .await
        .unwrap();
    assert_eq!(fx.count(exec).await, 1, "control: the row is committed");

    assert!(
        fx.page(exec, &q(None, 10)).await.is_empty(),
        "a line younger than the lag is held back"
    );

    fx.settle(exec).await;
    assert_eq!(messages(&fx.page(exec, &q(None, 10)).await), ["fresh"]);
}

#[tokio::test]
async fn page_is_ordered_by_id_and_honours_after_and_limit() {
    let Some(fx) = fx().await else { return };
    let exec = fx.execution().await;
    // Two batches, the second produced EARLIER (`ts`), so an order by `ts`
    // would put it first.
    let mut first: Vec<LogLine> = ["l0", "l1", "l2"]
        .iter()
        .map(|m| fx.line(exec, LogLevel::Info, m))
        .collect();
    let mut second: Vec<LogLine> = ["l3", "l4"]
        .iter()
        .map(|m| fx.line(exec, LogLevel::Info, m))
        .collect();
    for l in &mut second {
        l.ts -= ChronoDuration::hours(1);
    }
    fx.repo.insert_batch(&first).await.unwrap();
    fx.repo.insert_batch(&second).await.unwrap();
    first.append(&mut second);
    fx.settle(exec).await;

    let all = fx.page(exec, &q(None, 100)).await;
    assert_eq!(messages(&all), ["l0", "l1", "l2", "l3", "l4"]);
    assert!(all.windows(2).all(|w| w[0].id < w[1].id), "ids ascend");

    let p1 = fx.page(exec, &q(None, 2)).await;
    assert_eq!(messages(&p1), ["l0", "l1"], "limit");
    let p2 = fx.page(exec, &q(Some(p1[1].id), 2)).await;
    assert_eq!(messages(&p2), ["l2", "l3"], "after is exclusive");
    let p3 = fx.page(exec, &q(Some(p2[1].id), 2)).await;
    assert_eq!(messages(&p3), ["l4"]);
    let p4 = fx.page(exec, &q(Some(p3[0].id), 2)).await;
    assert!(p4.is_empty(), "past the last id: nothing");
}

#[tokio::test]
async fn page_filters_by_task_id_task_run_id_and_min_level() {
    let Some(fx) = fx().await else { return };
    let exec = fx.execution().await;
    let (r1, r2) = (Uuid::new_v4(), Uuid::new_v4());
    let mk = |task: Option<&str>, run: Option<Uuid>, level: LogLevel, m: &str| {
        let mut l = fx.line(exec, level, m);
        l.task_id = task.map(str::to_string);
        l.task_run_id = run;
        l.attempt = run.map(|_| 0);
        l
    };
    fx.repo
        .insert_batch(&[
            mk(None, None, LogLevel::Info, "exec"),
            mk(Some("a"), Some(r1), LogLevel::Debug, "a-debug"),
            mk(Some("a"), Some(r1), LogLevel::Warn, "a-warn"),
            mk(Some("a"), Some(r2), LogLevel::Error, "a2-error"),
            mk(Some("b"), Some(Uuid::new_v4()), LogLevel::Trace, "b-trace"),
        ])
        .await
        .unwrap();
    fx.settle(exec).await;

    let with = |task_id: Option<&str>, task_run_id: Option<Uuid>, min_level| LogQuery {
        after: None,
        limit: 100,
        task_id: task_id.map(str::to_string),
        task_run_id,
        min_level,
    };
    assert_eq!(
        messages(&fx.page(exec, &with(None, None, None)).await),
        ["exec", "a-debug", "a-warn", "a2-error", "b-trace"],
        "control: no filter"
    );
    assert_eq!(
        messages(&fx.page(exec, &with(Some("a"), None, None)).await),
        ["a-debug", "a-warn", "a2-error"]
    );
    assert_eq!(
        messages(&fx.page(exec, &with(None, Some(r1), None)).await),
        ["a-debug", "a-warn"]
    );
    assert_eq!(
        messages(&fx.page(exec, &with(None, None, Some(LogLevel::Warn))).await),
        ["a-warn", "a2-error"],
        "min_level is inclusive"
    );
    assert_eq!(
        messages(
            &fx.page(exec, &with(None, None, Some(LogLevel::Trace)))
                .await
        )
        .len(),
        5,
        "TRACE is everything"
    );
    assert_eq!(
        messages(
            &fx.page(exec, &with(Some("a"), Some(r1), Some(LogLevel::Info)))
                .await
        ),
        ["a-warn"],
        "filters combine"
    );
    assert!(fx
        .page(exec, &with(Some("b"), Some(r1), None))
        .await
        .is_empty());
}

#[tokio::test]
async fn page_never_returns_another_workspaces_lines() {
    let Some(fx) = fx().await else { return };
    let exec = fx.execution().await;
    fx.repo
        .insert_batch(&[fx.line(exec, LogLevel::Info, "mine")])
        .await
        .unwrap();
    fx.settle(exec).await;
    let (other_ws, _) = common::seed_workspace(&fx.db).await;

    assert_eq!(
        messages(&fx.page(exec, &q(None, 10)).await),
        ["mine"],
        "control: the right workspace sees it"
    );
    let leaked = fx
        .repo
        .page(other_ws, exec, &q(None, 10))
        .await
        .expect("page");
    assert!(leaked.is_empty(), "another workspace got {leaked:?}");
}

/// Spec §6, "the out-of-order commit trap". Two writers on two connections:
/// A inserts first (lower id) and holds its transaction open, B inserts
/// (higher id) and commits. A reader whose cursor moves past B before A
/// commits never sees A.
///
/// Part 1 shows the trap is real on this table, by defeating the lag: B is
/// backdated, so the reader gets B and its cursor passes A's id; A then
/// commits (and settles) and the reader's next poll returns nothing -- A is
/// lost. Part 2 is the same interleaving with the lag doing its job: the
/// reader polls at every step, gets nothing while the rows are younger than
/// 2 s, and once both have settled gets A then B. Drop the lag predicate and
/// Part 2's first poll returns B, which is exactly Part 1's loss.
#[tokio::test]
async fn out_of_order_commits_are_not_skipped_by_a_polling_reader() {
    let Some(fx) = fx().await else { return };

    async fn insert_in(tx: &mut sqlx::PgConnection, l: &LogLine) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO workflow.task_logs (workspace_id, execution_id, level, ts, message) \
             VALUES ($1, $2, $3, $4, $5) RETURNING id",
        )
        .bind(l.workspace_id)
        .bind(l.execution_id)
        .bind(l.level.as_i16())
        .bind(l.ts)
        .bind(&l.message)
        .fetch_one(tx)
        .await
        .expect("insert")
    }

    /// A client polling the API: it keeps a cursor and appends what it reads.
    struct Reader {
        cursor: Option<i64>,
        seen: Vec<String>,
    }
    impl Reader {
        async fn poll(&mut self, fx: &Fx, exec: Uuid) {
            let got = fx.page(exec, &q(self.cursor, 100)).await;
            if let Some(last) = got.last() {
                self.cursor = Some(last.id);
            }
            self.seen.extend(got.into_iter().map(|l| l.message));
        }
    }

    // ---- Part 1: the trap, with the lag defeated by backdating B.
    let exec = fx.execution().await;
    let mut a = fx.db.pool.begin().await.unwrap();
    let id_a = insert_in(&mut a, &fx.line(exec, LogLevel::Info, "A")).await;
    let mut b = fx.db.pool.begin().await.unwrap();
    let id_b = insert_in(&mut b, &fx.line(exec, LogLevel::Info, "B")).await;
    b.commit().await.unwrap();
    assert!(
        id_a < id_b,
        "control: A holds the lower id ({id_a} < {id_b})"
    );
    assert_eq!(fx.count(exec).await, 1, "control: only B is committed");

    fx.settle(exec).await; // B only: A is not visible to this UPDATE
    let mut r = Reader {
        cursor: None,
        seen: vec![],
    };
    r.poll(&fx, exec).await;
    assert_eq!(r.seen, ["B"]);
    assert!(r.cursor > Some(id_a), "the cursor is past A's id");

    a.commit().await.unwrap();
    fx.settle(exec).await;
    r.poll(&fx, exec).await;
    assert_eq!(
        r.seen,
        ["B"],
        "the trap: A committed behind the cursor and is never read"
    );
    assert_eq!(
        messages(&fx.page(exec, &q(None, 100)).await),
        ["A", "B"],
        "control: A is stored and settled"
    );

    // ---- Part 2: the same interleaving, the lag in force.
    let exec = fx.execution().await;
    let mut r = Reader {
        cursor: None,
        seen: vec![],
    };
    let mut a = fx.db.pool.begin().await.unwrap();
    let id_a = insert_in(&mut a, &fx.line(exec, LogLevel::Info, "A")).await;
    let mut b = fx.db.pool.begin().await.unwrap();
    let id_b = insert_in(&mut b, &fx.line(exec, LogLevel::Info, "B")).await;
    b.commit().await.unwrap();
    assert!(
        id_a < id_b,
        "control: A holds the lower id ({id_a} < {id_b})"
    );

    r.poll(&fx, exec).await; // B is committed but younger than the lag
    assert_eq!(
        r.seen,
        Vec::<String>::new(),
        "B is held back while A is open"
    );
    assert_eq!(r.cursor, None, "and the cursor has not moved");

    a.commit().await.unwrap();
    r.poll(&fx, exec).await; // both committed, neither settled
    fx.settle(exec).await; // time passes
    r.poll(&fx, exec).await;
    assert_eq!(r.seen, ["A", "B"], "both lines, in id order");
}

#[tokio::test]
async fn prune_deletes_only_lines_older_than_the_window() {
    let Some(fx) = fx().await else { return };
    let exec = fx.execution().await;
    let mut lines = Vec::new();
    for (days, m) in [(31, "old-1"), (40, "old-2"), (365, "old-3")] {
        let mut l = fx.line(exec, LogLevel::Info, m);
        l.ts -= ChronoDuration::days(days);
        lines.push(l);
    }
    let mut edge = fx.line(exec, LogLevel::Info, "29-days");
    edge.ts -= ChronoDuration::days(29);
    lines.push(edge);
    lines.push(fx.line(exec, LogLevel::Info, "new"));
    fx.repo.insert_batch(&lines).await.unwrap();
    fx.settle(exec).await;

    assert_eq!(fx.repo.prune_batch(30, 2).await.unwrap(), 2, "one batch");
    assert_eq!(fx.repo.prune_batch(30, 2).await.unwrap(), 1, "the rest");
    assert_eq!(fx.repo.prune_batch(30, 2).await.unwrap(), 0, "nothing left");
    assert_eq!(
        messages(&fx.page(exec, &q(None, 10)).await),
        ["29-days", "new"],
        "lines inside the window survive"
    );

    // A window of 0 (every line before now) or less (lines from the future)
    // is refused, and deletes nothing: `log_retention_days = 0` means keep
    // forever and must not reach the pruner as a window.
    for days in [0, -1] {
        let err = fx
            .repo
            .prune_batch(days, 1_000_000)
            .await
            .expect_err("a non-positive window is refused");
        assert!(
            matches!(err, egret_core::Error::InvalidInput(_)),
            "{days}: {err:?}"
        );
    }
    assert_eq!(fx.count(exec).await, 2, "nothing was deleted");
}

#[tokio::test]
async fn insert_batch_drops_lines_of_a_deleted_execution_and_reports_the_count() {
    let Some(fx) = fx().await else { return };
    let live = fx.execution().await;
    let gone = fx.execution().await;
    sqlx::query("DELETE FROM workflow.executions WHERE id = $1")
        .bind(gone)
        .execute(&fx.db.pool)
        .await
        .unwrap();

    let n = fx
        .repo
        .insert_batch(&[
            fx.line(live, LogLevel::Info, "one"),
            fx.line(gone, LogLevel::Info, "orphan"),
            fx.line(live, LogLevel::Info, "two"),
        ])
        .await
        .expect("an orphaned line does not fail the batch");
    assert_eq!(n, 2, "the orphan is dropped and the count says so");
    assert_eq!(fx.count(gone).await, 0);
    fx.settle(live).await;
    assert_eq!(
        messages(&fx.page(live, &q(None, 10)).await),
        ["one", "two"],
        "the batch's other lines are written, in order"
    );
}

#[tokio::test]
async fn insert_batch_drops_a_line_whose_workspace_does_not_match_its_execution() {
    let Some(fx) = fx().await else { return };
    let exec = fx.execution().await;
    let (other_ws, _) = common::seed_workspace(&fx.db).await;
    let mut wrong = fx.line(exec, LogLevel::Info, "wrong-ws");
    wrong.workspace_id = other_ws;

    let n = fx
        .repo
        .insert_batch(&[fx.line(exec, LogLevel::Info, "right-ws"), wrong])
        .await
        .expect("insert");
    assert_eq!(n, 1, "the mismatched line is dropped");
    assert_eq!(fx.count(exec).await, 1);
    fx.settle(exec).await;
    assert_eq!(messages(&fx.page(exec, &q(None, 10)).await), ["right-ws"]);
    assert!(fx
        .repo
        .page(other_ws, exec, &q(None, 10))
        .await
        .unwrap()
        .is_empty());
}

/// The race the join cannot close: the execution is deleted by a transaction
/// that commits after the insert's snapshot. The insert's foreign-key check
/// waits on the delete's row lock and then fails `23503`, and that must come
/// back typed. Staged deterministically: the delete is held open until the
/// insert is observed WAITING on a lock (`pg_stat_activity`), then committed.
#[tokio::test]
async fn insert_batch_reports_a_concurrent_delete_as_execution_deleted() {
    let Some(fx) = fx().await else { return };
    let exec = fx.execution().await;
    let mut del = fx.db.pool.begin().await.unwrap();
    sqlx::query("DELETE FROM workflow.executions WHERE id = $1")
        .bind(exec)
        .execute(&mut *del)
        .await
        .unwrap();

    let repo = fx.repo.clone();
    let line = fx.line(exec, LogLevel::Info, "raced");
    let insert = tokio::spawn(async move { repo.insert_batch(&[line]).await });

    let mut waiting = false;
    for _ in 0..500 {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity \
              WHERE datname = current_database() AND wait_event_type = 'Lock' \
                AND query LIKE 'INSERT INTO workflow.task_logs%'",
        )
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
        if n == 1 {
            waiting = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        waiting,
        "control: the insert blocks on the delete's row lock"
    );
    del.commit().await.unwrap();

    let err = insert
        .await
        .unwrap()
        .expect_err("the deleted execution fails the statement");
    assert!(
        matches!(err, InsertError::ExecutionDeleted(_)),
        "typed, not a string: {err:?}"
    );
    assert_eq!(err.sqlstate().as_deref(), Some("23503"));
    assert_eq!(fx.count(exec).await, 0);
}

#[tokio::test]
async fn deleting_an_execution_cascades_its_lines() {
    let Some(fx) = fx().await else { return };
    let exec = fx.execution().await;
    let keep = fx.execution().await;
    fx.repo
        .insert_batch(&[
            fx.line(exec, LogLevel::Info, "x"),
            fx.line(exec, LogLevel::Info, "y"),
            fx.line(keep, LogLevel::Info, "z"),
        ])
        .await
        .unwrap();
    assert_eq!(fx.count(exec).await, 2, "control");

    sqlx::query("DELETE FROM workflow.executions WHERE id = $1")
        .bind(exec)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    assert_eq!(
        fx.count(exec).await,
        0,
        "the execution's lines went with it"
    );
    assert_eq!(fx.count(keep).await, 1, "another execution's did not");
}

// ── W11 Task 4: the writer, the pruner, and the runtime's wiring ────────────
//
// A test that needs a stored line to be past the settle lag backdates it
// (`settle`), as above; what these tests wait for is the WRITER, polled with a
// deadline that names what was missing (`wait_for_count`).

/// A writer config for tests: the spec's values except where a test says
/// otherwise.
fn wcfg() -> WriterConfig {
    WriterConfig::default()
}

/// Poll the stored count of `execution` until it is `n`, or panic saying how
/// many there were.
async fn wait_for_count(fx: &Fx, execution: Uuid, n: i64, limit: Duration) {
    let start = std::time::Instant::now();
    loop {
        let got = fx.count(execution).await;
        if got == n {
            return;
        }
        assert!(
            start.elapsed() < limit,
            "timed out after {limit:?} waiting for {n} stored lines; {got} stored"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// An [`Inserter`] over the real repository whose inserts can be held
/// (`gate`) or failed (`fail_next` calls fail with a pool error;
/// `fail_always` fails every call; `deleted_next` calls fail as
/// [`InsertError::ExecutionDeleted`]). Counts every call.
#[derive(Clone)]
struct Rigged {
    repo: LogRepo,
    gate: tokio::sync::watch::Receiver<bool>,
    fail_next: Arc<AtomicUsize>,
    fail_always: Arc<AtomicBool>,
    deleted_next: Arc<AtomicUsize>,
    calls: Arc<AtomicUsize>,
}

impl Rigged {
    fn new(repo: LogRepo, open: bool) -> (Self, tokio::sync::watch::Sender<bool>) {
        let (tx, rx) = tokio::sync::watch::channel(open);
        (
            Self {
                repo,
                gate: rx,
                fail_next: Arc::default(),
                fail_always: Arc::default(),
                deleted_next: Arc::default(),
                calls: Arc::default(),
            },
            tx,
        )
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

fn take_one(n: &AtomicUsize) -> bool {
    n.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| v.checked_sub(1))
        .is_ok()
}

#[async_trait::async_trait]
impl Inserter for Rigged {
    async fn insert_batch(&self, lines: &[LogLine]) -> Result<u64, InsertError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut gate = self.gate.clone();
        gate.wait_for(|open| *open)
            .await
            .expect("gate sender alive");
        if take_one(&self.deleted_next) {
            return Err(InsertError::ExecutionDeleted(sqlx::Error::PoolTimedOut));
        }
        if self.fail_always.load(Ordering::SeqCst) || take_one(&self.fail_next) {
            return Err(InsertError::Db(sqlx::Error::PoolTimedOut));
        }
        self.repo.insert_batch(lines).await
    }
}

#[tokio::test]
async fn lines_reach_postgres_in_order_within_a_flush() {
    let Some(fx) = fx().await else { return };
    let exec = fx.execution().await;
    let cancel = CancellationToken::new();
    let cfg = WriterConfig {
        flush_every: Duration::from_millis(300),
        ..wcfg()
    };
    let (sink, handle) = spawn_writer(fx.repo.clone(), cfg, cancel.clone());

    for m in ["first", "second", "third"] {
        sink.write(fx.line(exec, LogLevel::Info, m)).await;
    }
    assert_eq!(
        fx.count(exec).await,
        0,
        "control: three lines are far short of a 500-line batch, so only the \
         timer can flush them, and it has not fired yet"
    );
    wait_for_count(&fx, exec, 3, cfg.flush_every + Duration::from_secs(1)).await;
    fx.settle(exec).await;
    assert_eq!(
        messages(&fx.page(exec, &q(None, 10)).await),
        ["first", "second", "third"],
        "one flush, in the order they were written"
    );
    assert!(!handle.is_finished(), "the writer keeps running");
    cancel.cancel();
    handle.await.unwrap();
}

#[tokio::test]
async fn a_full_channel_makes_the_writer_wait_and_loses_nothing() {
    let Some(fx) = fx().await else { return };
    let exec = fx.execution().await;
    let (rigged, gate) = Rigged::new(fx.repo.clone(), false);
    let cancel = CancellationToken::new();
    let cfg = WriterConfig {
        capacity: 4,
        batch: 2,
        flush_every: Duration::from_millis(50),
        ..wcfg()
    };
    let (sink, handle) = spawn_writer(rigged.clone(), cfg, cancel.clone());

    let done = Arc::new(AtomicUsize::new(0));
    let lines: Vec<LogLine> = (0..20)
        .map(|i| fx.line(exec, LogLevel::Info, &format!("line-{i:02}")))
        .collect();
    let task = {
        let done = done.clone();
        let sink = sink.clone();
        tokio::spawn(async move {
            for l in lines {
                sink.write(l).await;
                done.fetch_add(1, Ordering::SeqCst);
            }
        })
    };

    // The first batch (2 lines) is held at the gate; the channel holds 4 more.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let while_held = done.load(Ordering::SeqCst);
    assert!(
        rigged.calls() >= 1,
        "control: the writer reached the held insert"
    );
    assert!(
        while_held >= cfg.capacity,
        "control: writes did go through until the channel filled ({while_held})"
    );
    assert!(
        while_held < 20,
        "a full channel must make `write` WAIT: {while_held} of 20 writes had \
         returned while the insert was held"
    );
    assert!(!task.is_finished(), "the writing task is still pending");

    gate.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("the writes complete once the insert is released")
        .unwrap();
    wait_for_count(&fx, exec, 20, Duration::from_secs(5)).await;
    fx.settle(exec).await;
    let expected: Vec<String> = (0..20).map(|i| format!("line-{i:02}")).collect();
    assert_eq!(
        messages(&fx.page(exec, &q(None, 100)).await),
        expected,
        "all 20 stored, in order: nothing was dropped"
    );
    cancel.cancel();
    handle.await.unwrap();
}

#[tokio::test]
async fn a_failing_batch_is_retried_then_reported_not_fatal() {
    let Some(fx) = fx().await else { return };
    let exec = fx.execution().await;
    let (rigged, _gate) = Rigged::new(fx.repo.clone(), true);
    let cancel = CancellationToken::new();
    let cfg = WriterConfig {
        batch: 2,
        flush_every: Duration::from_millis(50),
        retry_for: Duration::from_millis(700),
        ..wcfg()
    };
    let (sink, handle) = spawn_writer(rigged.clone(), cfg, cancel.clone());

    // 1. Two failures, then success: the batch lands after its retries.
    rigged.fail_next.store(2, Ordering::SeqCst);
    sink.write(fx.line(exec, LogLevel::Info, "retried-1")).await;
    sink.write(fx.line(exec, LogLevel::Info, "retried-2")).await;
    wait_for_count(&fx, exec, 2, Duration::from_secs(3)).await;
    assert_eq!(
        rigged.calls(),
        3,
        "two failed calls and the one that landed"
    );

    // 2. Every call fails: the batch is dropped once `retry_for` runs out.
    rigged.fail_always.store(true, Ordering::SeqCst);
    let before = rigged.calls();
    sink.write(fx.line(exec, LogLevel::Info, "lost-1")).await;
    sink.write(fx.line(exec, LogLevel::Info, "lost-2")).await;
    tokio::time::sleep(cfg.retry_for + Duration::from_millis(500)).await;
    let tried = rigged.calls() - before;
    assert!(tried >= 3, "the failing batch was retried ({tried} calls)");
    assert!(
        !handle.is_finished(),
        "a batch whose retries ran out must not end the writer"
    );

    // 3. The database is back: later batches land.
    rigged.fail_always.store(false, Ordering::SeqCst);
    sink.write(fx.line(exec, LogLevel::Info, "later-1")).await;
    sink.write(fx.line(exec, LogLevel::Info, "later-2")).await;
    wait_for_count(&fx, exec, 4, Duration::from_secs(3)).await;
    fx.settle(exec).await;
    assert_eq!(
        messages(&fx.page(exec, &q(None, 10)).await),
        ["retried-1", "retried-2", "later-1", "later-2"],
        "the dropped batch is gone; the ones around it are stored"
    );
    cancel.cancel();
    handle.await.unwrap();
}

/// SQLSTATE 23503 is the concurrent-delete race, and a retry with backoff
/// cannot help it: the batch is re-run ONCE at once (the join then drops the
/// deleted execution's lines), and a second 23503 drops the batch.
#[tokio::test]
async fn an_execution_deleted_batch_is_rerun_once_not_retried() {
    let Some(fx) = fx().await else { return };
    let exec = fx.execution().await;
    let (rigged, _gate) = Rigged::new(fx.repo.clone(), true);
    let cancel = CancellationToken::new();
    let cfg = WriterConfig {
        batch: 2,
        flush_every: Duration::from_millis(50),
        ..wcfg()
    };
    let (sink, handle) = spawn_writer(rigged.clone(), cfg, cancel.clone());

    rigged.deleted_next.store(1, Ordering::SeqCst);
    let t0 = std::time::Instant::now();
    sink.write(fx.line(exec, LogLevel::Info, "rerun-1")).await;
    sink.write(fx.line(exec, LogLevel::Info, "rerun-2")).await;
    wait_for_count(&fx, exec, 2, Duration::from_secs(3)).await;
    assert_eq!(rigged.calls(), 2, "one 23503, one re-run");
    assert!(
        t0.elapsed() < Duration::from_millis(100 + 300),
        "the re-run does not wait out a backoff ({:?})",
        t0.elapsed()
    );

    rigged.deleted_next.store(5, Ordering::SeqCst);
    sink.write(fx.line(exec, LogLevel::Info, "dropped-1")).await;
    sink.write(fx.line(exec, LogLevel::Info, "dropped-2")).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        rigged.calls(),
        4,
        "a second 23503 drops the batch rather than retrying it"
    );
    rigged.deleted_next.store(0, Ordering::SeqCst);
    sink.write(fx.line(exec, LogLevel::Info, "after-1")).await;
    sink.write(fx.line(exec, LogLevel::Info, "after-2")).await;
    wait_for_count(&fx, exec, 4, Duration::from_secs(3)).await;
    cancel.cancel();
    handle.await.unwrap();
}

#[tokio::test]
async fn shutdown_drains_what_is_in_the_channel() {
    let Some(fx) = fx().await else { return };
    let exec = fx.execution().await;
    let cancel = CancellationToken::new();
    let cfg = wcfg();
    let (sink, handle) = spawn_writer(fx.repo.clone(), cfg, cancel.clone());

    for i in 0..50 {
        sink.write(fx.line(exec, LogLevel::Info, &format!("d{i:02}")))
            .await;
    }
    cancel.cancel();
    tokio::time::timeout(cfg.drain_within + Duration::from_secs(2), handle)
        .await
        .expect("the writer exits within its drain bound")
        .unwrap();
    assert_eq!(
        fx.count(exec).await,
        50,
        "every line in the channel at cancellation was stored before exit"
    );

    // After the writer has gone, a write returns at once and drops its line.
    tokio::time::timeout(
        Duration::from_secs(1),
        sink.write(fx.line(exec, LogLevel::Info, "too-late")),
    )
    .await
    .expect("a write on a closed channel does not hang");
    assert_eq!(fx.count(exec).await, 50);
}

#[tokio::test]
async fn the_pruner_deletes_old_lines_in_batches_and_stops_on_cancel() {
    let Some(fx) = fx().await else { return };
    // Pruning is table-wide: start from no old lines at all.
    while fx.repo.prune_batch(30, 100_000).await.unwrap() > 0 {}
    let exec = fx.execution().await;
    let mut lines = Vec::new();
    for i in 0..25 {
        let mut l = fx.line(exec, LogLevel::Info, &format!("old-{i:02}"));
        l.ts -= ChronoDuration::days(31);
        lines.push(l);
    }
    lines.push(fx.line(exec, LogLevel::Info, "new-1"));
    lines.push(fx.line(exec, LogLevel::Info, "new-2"));
    fx.repo.insert_batch(&lines).await.unwrap();

    // Control: a pass with no budget stops after its first batch of 10.
    let cancel = CancellationToken::new();
    let one_batch = PruneConfig {
        batch: 10,
        pass_budget: Duration::ZERO,
        ..PruneConfig::new(30)
    };
    let h = spawn_pruner(fx.repo.clone(), one_batch, cancel.clone()).expect("30 days prunes");
    wait_for_count(&fx, exec, 27 - 10, Duration::from_secs(3)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        fx.count(exec).await,
        17,
        "control: one batch, then the pass ends"
    );
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(2), h)
        .await
        .expect("the pruner stops on cancel")
        .unwrap();

    // With a budget, one pass loops batch after batch until a short one.
    // `every` is an hour, so everything below is the FIRST pass.
    let cancel = CancellationToken::new();
    let cfg = PruneConfig {
        batch: 10,
        ..PruneConfig::new(30)
    };
    assert_eq!(cfg.every, Duration::from_secs(3600));
    let h = spawn_pruner(fx.repo.clone(), cfg, cancel.clone()).expect("30 days prunes");
    wait_for_count(&fx, exec, 2, Duration::from_secs(3)).await;
    fx.settle(exec).await;
    assert_eq!(
        messages(&fx.page(exec, &q(None, 10)).await),
        ["new-1", "new-2"],
        "the 15 old lines went in one pass of two batches; the new ones stay"
    );
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(2), h)
        .await
        .expect("the pruner stops on cancel")
        .unwrap();
}

#[tokio::test]
async fn the_pruner_is_not_started_at_zero_days() {
    let Some(fx) = fx().await else { return };
    let cancel = CancellationToken::new();
    assert!(
        spawn_pruner(fx.repo.clone(), PruneConfig::new(0), cancel.clone()).is_none(),
        "0 means keep forever: no pruner"
    );
    let h = spawn_pruner(fx.repo.clone(), PruneConfig::new(30), cancel.clone());
    assert!(h.is_some(), "control: 30 days starts one");
    cancel.cancel();
}

/// A flow run through the REAL runtime -- `spawn_with_logs`, the production
/// handlers, nothing driving the tick -- stores its lines.
///
/// `#[sqlx::test]` (a scratch database), for `tests/runtime.rs`'s reason: the
/// claim is global, and this runtime would claim other tests' runs.
#[sqlx::test(migrations = "../../migrations")]
async fn end_to_end_a_flow_logs_through_the_real_runtime(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    let source = "id: w11e2e\nnamespace: t\ntasks:\n  \
        - id: hello\n    type: io.kestra.plugin.core.log.Log\n    \
          message: hello from w11\n    level: INFO\n  \
        - id: boom\n    type: io.kestra.plugin.core.execution.Fail\n    \
          errorMessage: boom from w11\n";
    let exec = start_execution(&pool, ws, None, source, serde_json::json!({}))
        .await
        .expect("start execution");

    let cancel = CancellationToken::new();
    let cfg = RuntimeConfig {
        enabled: true,
        tick_interval: Duration::from_millis(50),
        batch: 8,
        reaper_interval: Duration::from_secs(3600),
        working_timeout: Duration::from_secs(3600),
        scheduler_enabled: false,
        trigger_interval: Duration::from_secs(3600),
        log_limits: LogLimits::default(),
        log_retention_days: 30,
    };
    let rt = spawn_with_logs(
        pool.clone(),
        Handlers::with_core_tasks(),
        cfg,
        cancel.clone(),
    );

    let repo = LogRepo::new(pool.clone());
    let start = std::time::Instant::now();
    loop {
        let state: String =
            sqlx::query_scalar("SELECT state FROM workflow.executions WHERE id = $1")
                .bind(exec)
                .fetch_one(&pool)
                .await
                .unwrap();
        let stored: i64 =
            sqlx::query_scalar("SELECT count(*) FROM workflow.task_logs WHERE execution_id = $1")
                .bind(exec)
                .fetch_one(&pool)
                .await
                .unwrap();
        if state == "FAILED" && stored >= 2 {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "timed out: execution {state}, {stored} lines stored"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    sqlx::query(
        "UPDATE workflow.task_logs SET inserted_at = inserted_at - interval '3 seconds' \
          WHERE execution_id = $1",
    )
    .bind(exec)
    .execute(&pool)
    .await
    .unwrap();
    let got: Vec<(Option<String>, Option<i32>, LogLevel, String)> = repo
        .page(ws, exec, &q(None, 100))
        .await
        .unwrap()
        .into_iter()
        .map(|l| (l.task_id, l.attempt, l.level, l.message))
        .collect();
    assert_eq!(
        got,
        [
            (
                Some("hello".into()),
                Some(0),
                LogLevel::Info,
                "hello from w11".into()
            ),
            (
                Some("boom".into()),
                Some(0),
                LogLevel::Error,
                "boom from w11".into()
            ),
        ],
        "both tasks' lines, with their task id, 0-based attempt and level"
    );

    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(15), rt.join())
        .await
        .expect("the runtime stops: claim loop, then the writer's drain");
}

/// A `Log`-typed handler that signals it has started, waits for the test, and
/// only then writes its line: a run in flight when the process is told to stop.
struct GatedLog {
    started: Arc<tokio::sync::Notify>,
    go: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl egret_workflow::worker::TaskHandler for GatedLog {
    fn type_id(&self) -> &'static str {
        "io.kestra.plugin.core.log.Log"
    }

    async fn run(
        &self,
        task: &egret_workflow::worker::TaskInput,
    ) -> egret_core::Result<egret_workflow::worker::Outcome> {
        self.started.notify_one();
        self.go.notified().await;
        task.log.info("written after cancel").await;
        Ok(egret_workflow::worker::Outcome::success())
    }

    async fn cancel(&self, _task: &egret_workflow::worker::TaskInput) -> egret_core::Result<()> {
        Ok(())
    }
}

/// **The shutdown order `spawn_with_logs` promises: the writer outlives the
/// process token until the claim loop has stopped.** A run in flight when
/// `cancel` fires writes its line AFTER the cancel; that line must be stored
/// by the time `join()` returns. Were the writer stopped by `cancel` itself,
/// it would have closed its channel and the line would be dropped.
#[sqlx::test(migrations = "../../migrations")]
async fn a_line_written_after_cancel_by_an_in_flight_run_is_stored(pool: sqlx::PgPool) {
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, _user) = common::seed_workspace(&db).await;
    let source = "id: w11gate\nnamespace: t\ntasks:\n  \
        - id: slow\n    type: io.kestra.plugin.core.log.Log\n    message: unused\n";
    let exec = start_execution(&pool, ws, None, source, serde_json::json!({}))
        .await
        .expect("start execution");

    let started = Arc::new(tokio::sync::Notify::new());
    let go = Arc::new(tokio::sync::Notify::new());
    let mut handlers = Handlers::with_core_tasks();
    handlers.register(
        "io.kestra.plugin.core.log.Log",
        Arc::new(GatedLog {
            started: started.clone(),
            go: go.clone(),
        }),
    );
    let cancel = CancellationToken::new();
    let cfg = RuntimeConfig {
        enabled: true,
        tick_interval: Duration::from_millis(50),
        batch: 8,
        reaper_interval: Duration::from_secs(3600),
        working_timeout: Duration::from_secs(3600),
        scheduler_enabled: false,
        trigger_interval: Duration::from_secs(3600),
        log_limits: LogLimits::default(),
        log_retention_days: 30,
    };
    let rt = spawn_with_logs(pool.clone(), handlers, cfg, cancel.clone());

    tokio::time::timeout(Duration::from_secs(10), started.notified())
        .await
        .expect("control: the gated run was claimed and started");
    cancel.cancel();
    // Long enough for a writer that obeyed `cancel` to have closed its channel.
    tokio::time::sleep(Duration::from_millis(300)).await;
    go.notify_one();
    tokio::time::timeout(Duration::from_secs(15), rt.join())
        .await
        .expect("the runtime stops: the in-flight run, then the writer's drain");

    let stored: Vec<String> = sqlx::query_scalar(
        "SELECT message FROM workflow.task_logs WHERE execution_id = $1 ORDER BY id",
    )
    .bind(exec)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(
        stored.iter().any(|m| m == "written after cancel"),
        "a line an in-flight run wrote after cancellation must be stored before \
         the runtime's join returns; stored: {stored:?}"
    );
}
