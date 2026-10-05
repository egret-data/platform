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
    spawn_pruner, spawn_writer, Barrier, InsertError, Inserter, LogLevel, LogLimits, LogLine,
    LogQuery, LogRepo, PruneConfig, RunIdentity, RunLog, StoredLine, WriterConfig,
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
    /// A member of `ws`, with no grants until a test gives it some.
    user: egret_core::UserId,
    _serial: tokio::sync::MutexGuard<'static, ()>,
}

async fn fx() -> Option<Fx> {
    let serial = SERIAL.lock().await;
    let db = common::db().await?;
    let (ws, user) = common::seed_workspace(&db).await;
    Some(Fx {
        repo: LogRepo::new(db.pool.clone()),
        db,
        ws,
        user,
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

/// The settle lag is a WATERMARK (final review I3). Two concurrent INSERTs
/// can give a statement that STARTED later (so settles later) an id below
/// some of the earlier statement's ids. A per-row "settled" filter would then
/// serve the higher, settled id and let the cursor pass the lower one, which
/// is never served again. Staged directly: three committed rows S < A < B;
/// only S and B are backdated. The page must stop at A -- serving S and
/// holding B back -- even under a filter that A itself does not match.
#[tokio::test]
async fn the_page_stops_at_the_first_unsettled_id() {
    let Some(fx) = fx().await else { return };
    let exec = fx.execution().await;
    fx.repo
        .insert_batch(&[
            fx.line(exec, LogLevel::Info, "S"),
            fx.line(exec, LogLevel::Debug, "A"),
            fx.line(exec, LogLevel::Info, "B"),
        ])
        .await
        .unwrap();
    let ids: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM workflow.task_logs WHERE execution_id = $1 ORDER BY id")
            .bind(exec)
            .fetch_all(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(ids.len(), 3, "control: all three committed");
    let (id_s, id_a, id_b) = (ids[0], ids[1], ids[2]);
    // Settle S and B only: B stands for the earlier statement's higher id,
    // A for the later statement's lower id, still inside the lag.
    sqlx::query(
        "UPDATE workflow.task_logs SET inserted_at = inserted_at - interval '3 seconds' \
          WHERE id = ANY($1)",
    )
    .bind(vec![id_s, id_b])
    .execute(&fx.db.pool)
    .await
    .unwrap();

    let got = fx.page(exec, &q(None, 10)).await;
    assert_eq!(
        messages(&got),
        ["S"],
        "B has settled but sits above the unsettled A: it must be held back"
    );
    let cursor = got.last().map(|l| l.id);
    assert_eq!(cursor, Some(id_s));
    assert!(
        fx.page(exec, &q(cursor, 10)).await.is_empty(),
        "the reader cannot advance past A ({id_a}) while it settles"
    );
    let info_only = LogQuery {
        min_level: Some(LogLevel::Info),
        ..q(cursor, 10)
    };
    assert!(
        fx.page(exec, &info_only).await.is_empty(),
        "a filter A fails does not lift the watermark: B still waits for A"
    );

    fx.settle(exec).await;
    assert_eq!(messages(&fx.page(exec, &q(cursor, 10)).await), ["A", "B"]);
    assert_eq!(messages(&fx.page(exec, &info_only).await), ["B"]);
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
    n.try_update(Ordering::SeqCst, Ordering::SeqCst, |v| v.checked_sub(1))
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
        container_default_timeout: egret_workflow::worker::DEFAULT_CONTAINER_TIMEOUT,
        scheduler_enabled: false,
        trigger_interval: Duration::from_secs(3600),
        log_limits: LogLimits::default(),
        log_retention_days: 30,
    };
    let rt = spawn_with_logs(
        pool.clone(),
        Handlers::with_core_tasks(egret_workflow::storage::current()),
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
    let mut handlers = Handlers::with_core_tasks(egret_workflow::storage::current());
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
        container_default_timeout: egret_workflow::worker::DEFAULT_CONTAINER_TIMEOUT,
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

// ── The read API (W11 Task 5) ───────────────────────────────────────────────
//
// `GET /api/ws/{workspace}/workflows/executions/{id}/logs`, driven over the
// same composed app and REAL `CedarPdp` as `tests/executions_routes.rs`.
// Every grant is written before `common::app` builds the PDP, which loads the
// policy set once.

/// `GET …/executions/{exec}/logs?{q}` as `user`: the status and, when the body
/// is JSON, the body (`Value::Null` otherwise, e.g. a query-string rejection).
async fn get_logs(
    app: &common::App,
    ws: Uuid,
    exec: Uuid,
    user: egret_core::UserId,
    q: &str,
) -> (u16, serde_json::Value) {
    let res = common::get(
        app,
        &format!("/api/ws/{ws}/workflows/executions/{exec}/logs?{q}"),
        user,
    )
    .await;
    let status = res.status().as_u16();
    let text = common::body_text(res).await;
    (
        status,
        serde_json::from_str(&text).unwrap_or(serde_json::Value::Null),
    )
}

fn item_messages(body: &serde_json::Value) -> Vec<String> {
    body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("no items array in {body}"))
        .iter()
        .map(|i| i["message"].as_str().unwrap().to_string())
        .collect()
}

impl Fx {
    async fn set_state(&self, execution: Uuid, state: &str) {
        sqlx::query("UPDATE workflow.executions SET state = $2 WHERE id = $1")
            .bind(execution)
            .bind(state)
            .execute(&self.db.pool)
            .await
            .expect("set execution state");
    }
}

#[tokio::test]
async fn logs_route_pages_with_after_and_limit_and_reports_execution_state() {
    let Some(fx) = fx().await else { return };
    common::grant(&fx.db, fx.ws, fx.user, &["workflow:read"]).await;
    let exec = fx.execution().await;
    let run = Uuid::new_v4();
    let lines: Vec<LogLine> = (0..5)
        .map(|i| {
            let mut l = fx.line(exec, LogLevel::Info, &format!("l{i}"));
            l.task_id = Some("t".into());
            l.task_run_id = Some(run);
            l.attempt = Some(1);
            l
        })
        .collect();
    fx.repo.insert_batch(&lines).await.unwrap();
    fx.settle(exec).await;
    // Inserted AFTER the settle, so younger than the lag: the route must not
    // serve it (the lag is the repository's, and the route must not bypass it).
    fx.repo
        .insert_batch(&[fx.line(exec, LogLevel::Info, "young")])
        .await
        .unwrap();
    let ids: Vec<i64> = fx
        .page(exec, &q(None, 100))
        .await
        .iter()
        .map(|l| l.id)
        .collect();
    assert_eq!(ids.len(), 5);

    let app = common::app(&fx.db).await;

    let (status, p1) = get_logs(&app, fx.ws, exec, fx.user, "limit=2").await;
    assert_eq!(status, 200, "{p1}");
    assert_eq!(item_messages(&p1), ["l0", "l1"]);
    assert_eq!(p1["next_after"], ids[1], "{p1}");
    assert_eq!(p1["execution_state"], "RUNNING", "{p1}");
    // Every field of an item reaches the wire, by value.
    let first = &p1["items"][0];
    assert_eq!(first["id"], ids[0]);
    assert_eq!(first["task_id"], "t");
    assert_eq!(first["task_run_id"], run.to_string());
    assert_eq!(first["attempt"], 1);
    assert_eq!(first["level"], "INFO");
    assert_eq!(
        first["ts"]
            .as_str()
            .unwrap()
            .parse::<DateTime<Utc>>()
            .unwrap(),
        lines[0].ts
    );
    assert_eq!(first["message"], "l0");
    assert!(
        p1.get("truncated").is_none() && first.get("truncated").is_none(),
        "D6: {p1}"
    );

    let (_, p2) = get_logs(
        &app,
        fx.ws,
        exec,
        fx.user,
        &format!("after={}&limit=2", ids[1]),
    )
    .await;
    assert_eq!(item_messages(&p2), ["l2", "l3"]);
    assert_eq!(p2["next_after"], ids[3]);

    let (_, p3) = get_logs(
        &app,
        fx.ws,
        exec,
        fx.user,
        &format!("after={}&limit=2", ids[3]),
    )
    .await;
    assert_eq!(item_messages(&p3), ["l4"]);
    assert_eq!(p3["next_after"], ids[4]);

    let (status, p4) = get_logs(&app, fx.ws, exec, fx.user, &format!("after={}", ids[4])).await;
    assert_eq!(status, 200);
    assert_eq!(
        item_messages(&p4),
        Vec::<String>::new(),
        "the unsettled line is held back"
    );
    assert!(
        p4["next_after"].is_null(),
        "an empty page has no cursor: {p4}"
    );

    // `execution_state` is read from the row on every call, not fixed.
    fx.set_state(exec, "SUCCESS").await;
    let (_, done) = get_logs(&app, fx.ws, exec, fx.user, "").await;
    assert_eq!(done["execution_state"], "SUCCESS", "{done}");
    assert_eq!(item_messages(&done), ["l0", "l1", "l2", "l3", "l4"]);

    // The default limit is 500: 501 settled lines, no `limit`, 500 served.
    let many = fx.execution().await;
    let batch: Vec<LogLine> = (0..501)
        .map(|i| fx.line(many, LogLevel::Info, &format!("m{i}")))
        .collect();
    fx.repo.insert_batch(&batch).await.unwrap();
    fx.settle(many).await;
    let (_, big) = get_logs(&app, fx.ws, many, fx.user, "").await;
    assert_eq!(big["items"].as_array().unwrap().len(), 500, "default limit");
    let (_, rest) = get_logs(
        &app,
        fx.ws,
        many,
        fx.user,
        &format!("after={}", big["next_after"]),
    )
    .await;
    assert_eq!(item_messages(&rest), ["m500"]);
}

#[tokio::test]
async fn logs_route_filters_by_min_level_and_task() {
    let Some(fx) = fx().await else { return };
    common::grant(&fx.db, fx.ws, fx.user, &["workflow:read"]).await;
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
    let app = common::app(&fx.db).await;
    let msgs = |body: serde_json::Value| item_messages(&body);

    let (s, all) = get_logs(&app, fx.ws, exec, fx.user, "").await;
    assert_eq!(s, 200);
    assert_eq!(
        msgs(all),
        ["exec", "a-debug", "a-warn", "a2-error", "b-trace"],
        "control"
    );
    let (_, b) = get_logs(&app, fx.ws, exec, fx.user, "min_level=WARN").await;
    assert_eq!(msgs(b), ["a-warn", "a2-error"], "min_level is inclusive");
    let (_, b) = get_logs(&app, fx.ws, exec, fx.user, "task_id=a").await;
    assert_eq!(msgs(b), ["a-debug", "a-warn", "a2-error"]);
    let (_, b) = get_logs(&app, fx.ws, exec, fx.user, &format!("task_run_id={r1}")).await;
    assert_eq!(msgs(b), ["a-debug", "a-warn"]);
    let (_, b) = get_logs(
        &app,
        fx.ws,
        exec,
        fx.user,
        &format!("task_id=a&task_run_id={r1}&min_level=INFO"),
    )
    .await;
    assert_eq!(msgs(b), ["a-warn"], "filters combine");

    // A level outside the five, including a lower-case one (the names compare
    // case-sensitively, as Kestra's Jackson enum does), is a 400.
    for bad in ["min_level=LOUD", "min_level=warn", "task_run_id=nope"] {
        let (s, body) = get_logs(&app, fx.ws, exec, fx.user, bad).await;
        assert_eq!(s, 400, "{bad}: {body}");
    }
}

#[tokio::test]
async fn logs_route_answers_404_for_another_workspaces_execution() {
    let Some(fx) = fx().await else { return };
    let (ws_b, user_b) = common::seed_workspace(&fx.db).await;
    common::grant(&fx.db, fx.ws, fx.user, &["workflow:read"]).await;
    common::grant(&fx.db, ws_b, user_b, &["workflow:read"]).await;
    let theirs = fx.execution_in(ws_b).await;
    let mut l = fx.line(theirs, LogLevel::Info, "b's secret");
    l.workspace_id = ws_b;
    assert_eq!(fx.repo.insert_batch(&[l]).await.unwrap(), 1);
    fx.settle(theirs).await;
    let app = common::app(&fx.db).await;

    let (status, body) = get_logs(&app, fx.ws, theirs, fx.user, "").await;
    assert_eq!(
        status, 404,
        "another workspace's execution must be a 404, never a 403 and never an \
         empty 200: {body}"
    );
    assert!(!body.to_string().contains("b's secret"), "{body}");
    // Indistinguishable from an id that exists nowhere.
    let (nowhere, _) = get_logs(&app, fx.ws, Uuid::new_v4(), fx.user, "").await;
    assert_eq!(nowhere, 404);
    // Control: the same id in its own workspace serves its line.
    let (status, body) = get_logs(&app, ws_b, theirs, user_b, "").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(item_messages(&body), ["b's secret"]);
}

#[tokio::test]
async fn logs_route_is_gated_by_the_workflows_feature() {
    let Some(fx) = fx().await else { return };
    common::grant(&fx.db, fx.ws, fx.user, &["workflow:read"]).await;
    common::disable_feature(&fx.db, fx.ws, "workflows").await;
    let exec = fx.execution().await;
    // Control: an enabled workspace, same shape, answers 200 -- so the 403
    // below is the gate and not a missing route.
    let (ws_on, user_on) = common::seed_workspace(&fx.db).await;
    common::grant(&fx.db, ws_on, user_on, &["workflow:read"]).await;
    let exec_on = fx.execution_in(ws_on).await;
    // And a `workflow:manage`-only member of an enabled workspace: the route
    // checks `workflow:read` specifically.
    let (ws_m, user_m) = common::seed_workspace(&fx.db).await;
    common::grant(&fx.db, ws_m, user_m, &["workflow:manage"]).await;
    let exec_m = fx.execution_in(ws_m).await;
    let app = common::app(&fx.db).await;

    let (status, body) = get_logs(&app, fx.ws, exec, fx.user, "").await;
    assert_eq!(status, 403, "a disabled workflows module: {body}");
    let (status, body) = get_logs(&app, ws_m, exec_m, user_m, "").await;
    assert_eq!(
        status, 403,
        "workflow:manage does not imply workflow:read: {body}"
    );
    let (status, body) = get_logs(&app, ws_on, exec_on, user_on, "").await;
    assert_eq!(status, 200, "control: {body}");
    let res = common::anon_get(
        &app,
        &format!("/api/ws/{ws_on}/workflows/executions/{exec_on}/logs"),
    )
    .await;
    assert_eq!(res.status(), 401, "anonymous");
}

#[tokio::test]
async fn logs_route_rejects_limit_over_2000() {
    let Some(fx) = fx().await else { return };
    common::grant(&fx.db, fx.ws, fx.user, &["workflow:read"]).await;
    let exec = fx.execution().await;
    fx.repo
        .insert_batch(&[fx.line(exec, LogLevel::Info, "one")])
        .await
        .unwrap();
    fx.settle(exec).await;
    let app = common::app(&fx.db).await;

    let (status, body) = get_logs(&app, fx.ws, exec, fx.user, "limit=2001").await;
    assert_eq!(
        status, 400,
        "limit over 2000 is refused, not clamped: {body}"
    );
    for bad in ["limit=0", "limit=-1", "limit=many"] {
        let (status, body) = get_logs(&app, fx.ws, exec, fx.user, bad).await;
        assert_eq!(status, 400, "{bad}: {body}");
    }
    // Controls: both ends of the range are served.
    for ok in ["limit=2000", "limit=1"] {
        let (status, body) = get_logs(&app, fx.ws, exec, fx.user, ok).await;
        assert_eq!(status, 200, "{ok}: {body}");
        assert_eq!(item_messages(&body), ["one"]);
    }
}

// ── The flush barrier (W11 Task 5, fix round 1) ─────────────────────────────
//
// `RunLog::finish` waits until the writer has stored every line the attempt
// wrote, so an execution's outcome commits after its lines are in the table.
// That is what lets the route's `complete` tell a poller it may stop.

impl Fx {
    /// A `RunLog` for one attempt of a task run of `execution`, writing to
    /// `sink`.
    fn run_log(&self, execution: Uuid, sink: Arc<dyn LogSink>) -> RunLog {
        RunLog::new(
            RunIdentity {
                workspace_id: self.ws,
                execution_id: execution,
                task_run_id: Uuid::new_v4(),
                task_id: "t".into(),
                attempt: 0,
            },
            sink,
            LogLimits::default(),
        )
    }
}

#[tokio::test]
async fn finish_waits_until_the_runs_lines_are_inserted() {
    let Some(fx) = fx().await else { return };
    let exec = fx.execution().await;
    // The insert is held until `gate` opens: a slowed database.
    let (rigged, gate) = Rigged::new(fx.repo.clone(), false);
    let cancel = CancellationToken::new();
    let (sink, handle) = spawn_writer(rigged.clone(), wcfg(), cancel.clone());
    let log = fx.run_log(exec, Arc::new(sink));
    for m in ["a", "b", "c"] {
        log.info(m).await;
    }

    let finishing = tokio::spawn({
        let log = log.clone();
        async move { log.finish().await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !finishing.is_finished(),
        "finish must not return while the attempt's lines are not inserted"
    );
    assert_eq!(fx.count(exec).await, 0, "control: the insert is still held");

    gate.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(5), finishing)
        .await
        .expect("finish returns once the lines are inserted")
        .unwrap();
    // Read at once, not polled: the barrier's promise is that they are
    // ALREADY in the table when `finish` returns. Without it the three lines
    // would wait for the writer's 1 s timer.
    assert_eq!(
        fx.count(exec).await,
        3,
        "every line is stored when finish returns"
    );
    cancel.cancel();
    handle.await.unwrap();
}

#[tokio::test]
async fn a_writer_that_never_acks_does_not_hang_the_run() {
    let Some(fx) = fx().await else { return };
    let exec = fx.execution().await;
    let (rigged, gate) = Rigged::new(fx.repo.clone(), false);
    let cancel = CancellationToken::new();
    let cfg = WriterConfig {
        flush_within: Duration::from_millis(300),
        ..wcfg()
    };
    let (sink, handle) = spawn_writer(rigged.clone(), cfg, cancel.clone());
    let log = fx.run_log(exec, Arc::new(sink.clone()));
    log.info("held").await;

    // The insert never completes while the barrier waits: `finish` gives up
    // after `flush_within` and returns, it does not hang the run.
    let t0 = std::time::Instant::now();
    tokio::time::timeout(Duration::from_secs(5), log.finish())
        .await
        .expect("finish must be bounded");
    let took = t0.elapsed();
    assert!(
        took >= cfg.flush_within,
        "control: finish did wait for the barrier ({took:?})"
    );
    assert!(
        took < Duration::from_secs(2),
        "bounded by flush_within ({took:?})"
    );

    // Release the insert and stop the writer. The line is still stored: the
    // barrier timing out gives up waiting, not the line.
    gate.send(true).unwrap();
    cancel.cancel();
    handle.await.unwrap();
    assert_eq!(fx.count(exec).await, 1);

    // A writer that is gone answers the barrier at once.
    let after = fx.run_log(exec, Arc::new(sink));
    let t0 = std::time::Instant::now();
    after.finish().await;
    assert!(
        t0.elapsed() < Duration::from_millis(200),
        "a stopped writer does not make finish wait ({:?})",
        t0.elapsed()
    );
}

#[tokio::test]
async fn logs_route_reports_complete_only_after_the_settle_lag() {
    let Some(fx) = fx().await else { return };
    common::grant(&fx.db, fx.ws, fx.user, &["workflow:read"]).await;
    let exec = fx.execution().await;
    let app = common::app(&fx.db).await;
    // Set the state and when it was set, as `claim` does in one UPDATE.
    let at = |state: &'static str, secs_ago: i32| {
        let pool = fx.db.pool.clone();
        async move {
            sqlx::query(
                "UPDATE workflow.executions \
                    SET state = $2, updated_at = now() - make_interval(secs => $3) \
                  WHERE id = $1",
            )
            .bind(exec)
            .bind(state)
            .bind(f64::from(secs_ago))
            .execute(&pool)
            .await
            .expect("set state");
        }
    };
    let complete = |body: &serde_json::Value| {
        body["complete"]
            .as_bool()
            .unwrap_or_else(|| panic!("no boolean complete in {body}"))
    };

    at("RUNNING", 10).await;
    let (s, body) = get_logs(&app, fx.ws, exec, fx.user, "").await;
    assert_eq!(s, 200, "{body}");
    assert!(
        !complete(&body),
        "running is never complete, however old: {body}"
    );

    at("SUCCESS", 0).await;
    let (_, body) = get_logs(&app, fx.ws, exec, fx.user, "").await;
    assert_eq!(body["execution_state"], "SUCCESS");
    assert!(
        !complete(&body),
        "terminal, but inside the settle lag: its last lines may not be served yet: {body}"
    );

    at("SUCCESS", 3).await;
    let (_, body) = get_logs(&app, fx.ws, exec, fx.user, "").await;
    assert!(complete(&body), "terminal and past the settle lag: {body}");

    for terminal in ["FAILED", "KILLED", "WARNING", "CANCELLED"] {
        at(terminal, 3).await;
        let (_, body) = get_logs(&app, fx.ws, exec, fx.user, "").await;
        assert!(complete(&body), "{terminal}: {body}");
    }
    at("PAUSED", 3).await;
    let (_, body) = get_logs(&app, fx.ws, exec, fx.user, "").await;
    assert!(!complete(&body), "a pause is not an end: {body}");
}

// ── Final review I1: a line Postgres refuses costs only itself ──────────────

/// A `Log` message holding U+0000 (reachable through a rendered expression
/// such as `fromJson('"\\u0000"')`) is stored, without the NUL, through the
/// real writer and table.
#[tokio::test]
async fn a_nul_in_a_log_message_is_stored_without_it() {
    let Some(fx) = fx().await else { return };
    let exec = fx.execution().await;
    let mut raw = fx.line(exec, LogLevel::Info, "x\0y");
    let err = fx
        .repo
        .insert_batch(std::slice::from_ref(&raw))
        .await
        .expect_err("control: Postgres text refuses U+0000");
    assert!(
        matches!(err, InsertError::BadData(_)),
        "classified as a data error: {err:?} ({:?})",
        err.sqlstate()
    );
    assert_eq!(err.sqlstate().as_deref(), Some("22021"));
    raw.message = "control".into();
    assert_eq!(fx.repo.insert_batch(&[raw]).await.unwrap(), 1);

    let cancel = CancellationToken::new();
    let (sink, handle) = spawn_writer(fx.repo.clone(), wcfg(), cancel.clone());
    let log = fx.run_log(exec, Arc::new(sink));
    log.info("before\0after").await;
    log.finish().await;
    fx.settle(exec).await;
    assert_eq!(
        messages(&fx.page(exec, &q(None, 10)).await),
        ["control", "beforeafter"],
        "the NUL is removed and the line stored"
    );
    cancel.cancel();
    handle.await.unwrap();
}

/// A line that bypassed `RunLog` and still holds U+0000 fails its batch with
/// 22021. The writer must not retry that batch (no retry can fix a value), and
/// must not drop it whole: every other line -- another execution's, another
/// workspace's -- is stored, and only the bad line is lost.
#[tokio::test]
async fn a_refused_line_costs_only_itself_not_its_batch() {
    let Some(fx) = fx().await else { return };
    let exec = fx.execution().await;
    let (other_ws, _) = common::seed_workspace(&fx.db).await;
    let other_exec = fx.execution_in(other_ws).await;
    let (rigged, _gate) = Rigged::new(fx.repo.clone(), true);
    let cancel = CancellationToken::new();
    let cfg = WriterConfig { batch: 4, ..wcfg() };
    let (sink, handle) = spawn_writer(rigged.clone(), cfg, cancel.clone());

    let t0 = std::time::Instant::now();
    sink.write(fx.line(exec, LogLevel::Info, "ok-1")).await;
    sink.write(fx.line(exec, LogLevel::Info, "bad\0line")).await;
    sink.write(LogLine {
        workspace_id: other_ws,
        ..fx.line(other_exec, LogLevel::Info, "other-workspace")
    })
    .await;
    sink.write(fx.line(exec, LogLevel::Info, "ok-2")).await;
    assert_eq!(sink.barrier().await, Barrier::Acked);
    assert!(
        t0.elapsed() < Duration::from_secs(2),
        "no retry window was waited out ({:?})",
        t0.elapsed()
    );
    assert_eq!(
        rigged.calls(),
        1 + 4,
        "the batch once, then each of its four lines once"
    );
    assert_eq!(fx.count(other_exec).await, 1, "the other workspace's line");
    fx.settle(exec).await;
    assert_eq!(
        messages(&fx.page(exec, &q(None, 10)).await),
        ["ok-1", "ok-2"],
        "the refused line alone is lost, the others kept in order"
    );
    assert!(!sink.is_failing(), "a bad line is not a writer fault");
    cancel.cancel();
    handle.await.unwrap();
}

// ── Final review I2: the breaker -- a failing writer never slows a run ──────

/// While every insert fails, `finish()` must return at once, run after run:
/// the barrier is skipped, not waited out (5 s bound) behind a 30 s retry.
/// Once inserts succeed again the breaker clears and the barrier waits again.
#[tokio::test]
async fn a_failing_writer_does_not_slow_finish() {
    let Some(fx) = fx().await else { return };
    let exec = fx.execution().await;
    let (rigged, _gate) = Rigged::new(fx.repo.clone(), true);
    rigged.fail_always.store(true, Ordering::SeqCst);
    let cancel = CancellationToken::new();
    let cfg = wcfg();
    assert_eq!(
        (cfg.retry_for, cfg.flush_within),
        (Duration::from_secs(30), Duration::from_secs(5)),
        "control: the spec's bounds, which a waiting barrier would show"
    );
    let (sink, handle) = spawn_writer(rigged.clone(), cfg, cancel.clone());

    let t0 = std::time::Instant::now();
    for i in 0..5 {
        let log = fx.run_log(exec, Arc::new(sink.clone()));
        log.info(&format!("run-{i}")).await;
        let t = std::time::Instant::now();
        log.finish().await;
        assert!(
            t.elapsed() < Duration::from_millis(500),
            "run {i}: finish waited {:?} on a failing writer",
            t.elapsed()
        );
    }
    assert!(
        t0.elapsed() < Duration::from_secs(1),
        "five runs took {:?}",
        t0.elapsed()
    );
    assert!(rigged.calls() >= 1, "control: an insert was attempted");
    assert!(sink.is_failing(), "the breaker is set");
    assert_eq!(sink.barrier_skips(), 5, "every barrier was skipped");
    assert_eq!(fx.count(exec).await, 0, "control: nothing stored yet");

    // The database is back: the first batch's retry lands, the breaker
    // clears, the queued runs' lines follow, and a barrier waits again.
    rigged.fail_always.store(false, Ordering::SeqCst);
    wait_for_count(&fx, exec, 5, Duration::from_secs(8)).await;
    assert!(
        !sink.is_failing(),
        "an insert succeeded: the breaker clears"
    );
    let log = fx.run_log(exec, Arc::new(sink.clone()));
    log.info("healthy").await;
    assert_eq!(sink.barrier().await, Barrier::Acked);
    assert_eq!(
        fx.count(exec).await,
        6,
        "an acknowledged barrier means stored"
    );
    assert_eq!(sink.barrier_skips(), 5);
    cancel.cancel();
    handle.await.unwrap();
}

/// While the writer is failing, a `write` that finds the channel full drops
/// its line with a count instead of waiting -- including a write that was
/// already waiting when the writer started failing -- so logging cannot run a
/// handler into its `timeout:`.
#[tokio::test]
async fn a_failing_writer_does_not_block_write() {
    let Some(fx) = fx().await else { return };
    let exec = fx.execution().await;

    // 1. Already failing: 20 writes into a 2-line channel return at once.
    let (rigged, _gate) = Rigged::new(fx.repo.clone(), true);
    rigged.fail_always.store(true, Ordering::SeqCst);
    let cancel = CancellationToken::new();
    let cfg = WriterConfig {
        capacity: 2,
        batch: 1,
        ..wcfg()
    };
    let (sink, handle) = spawn_writer(rigged.clone(), cfg, cancel.clone());
    sink.write(fx.line(exec, LogLevel::Info, "trips")).await;
    let t = std::time::Instant::now();
    while !sink.is_failing() {
        assert!(
            t.elapsed() < Duration::from_secs(2),
            "the breaker never set"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let t = std::time::Instant::now();
    for i in 0..20 {
        sink.write(fx.line(exec, LogLevel::Info, &format!("w{i}")))
            .await;
    }
    assert!(
        t.elapsed() < Duration::from_millis(500),
        "writes waited {:?} on a failing writer",
        t.elapsed()
    );
    assert_eq!(
        sink.dropped_while_failing(),
        18,
        "the channel took 2 (its writer is backing off); 18 were dropped and counted"
    );
    cancel.cancel();
    handle.await.unwrap();

    // 2. A write waiting on a full channel while the writer is healthy is
    //    released, and its line dropped, the moment the writer starts failing.
    let (rigged, gate) = Rigged::new(fx.repo.clone(), false);
    let cancel = CancellationToken::new();
    let cfg = WriterConfig {
        capacity: 1,
        batch: 1,
        ..wcfg()
    };
    let (sink, handle) = spawn_writer(rigged.clone(), cfg, cancel.clone());
    sink.write(fx.line(exec, LogLevel::Info, "held")).await; // at the gate
    let t = std::time::Instant::now();
    while rigged.calls() == 0 {
        assert!(
            t.elapsed() < Duration::from_secs(2),
            "the insert never started"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    sink.write(fx.line(exec, LogLevel::Info, "fills")).await; // the channel
    let waiting = tokio::spawn({
        let sink = sink.clone();
        let line = fx.line(exec, LogLevel::Info, "waits");
        async move { sink.write(line).await }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !waiting.is_finished(),
        "control: a healthy writer's full channel makes the write wait (D3)"
    );
    rigged.fail_always.store(true, Ordering::SeqCst);
    gate.send(true).unwrap(); // the held insert now fails: the breaker trips
    tokio::time::timeout(Duration::from_secs(1), waiting)
        .await
        .expect("the waiting write is released when the writer starts failing")
        .unwrap();
    assert_eq!(sink.dropped_while_failing(), 1, "and its line counted");
    cancel.cancel();
    handle.await.unwrap();
}

/// A barrier behind a batch that is DROPPED (not failed over to the breaker:
/// a second 23503 is not a writer fault) is acknowledged by the writer, at
/// once -- not left to time out.
#[tokio::test]
async fn a_barrier_behind_a_lost_batch_is_acknowledged() {
    let Some(fx) = fx().await else { return };
    let exec = fx.execution().await;
    let (rigged, _gate) = Rigged::new(fx.repo.clone(), true);
    let cancel = CancellationToken::new();
    let cfg = WriterConfig {
        flush_within: Duration::from_secs(10),
        ..wcfg()
    };
    let (sink, handle) = spawn_writer(rigged.clone(), cfg, cancel.clone());
    rigged.deleted_next.store(2, Ordering::SeqCst);
    sink.write(fx.line(exec, LogLevel::Info, "lost")).await;

    let t = std::time::Instant::now();
    assert_eq!(sink.barrier().await, Barrier::Acked);
    assert!(
        t.elapsed() < Duration::from_secs(1),
        "acknowledged, not timed out ({:?})",
        t.elapsed()
    );
    assert_eq!(rigged.calls(), 2, "control: 23503, re-run, 23503: dropped");
    assert_eq!(fx.count(exec).await, 0, "control: the batch was lost");
    assert!(!sink.is_failing(), "control: the breaker played no part");
    cancel.cancel();
    handle.await.unwrap();
}

/// A barrier still in the channel when the writer is cancelled is
/// acknowledged by the drain, after the lines before it are stored.
#[tokio::test]
async fn a_barrier_queued_at_shutdown_is_acknowledged_by_the_drain() {
    let Some(fx) = fx().await else { return };
    let exec = fx.execution().await;
    let (rigged, gate) = Rigged::new(fx.repo.clone(), false);
    let cancel = CancellationToken::new();
    let cfg = WriterConfig {
        batch: 1,
        flush_within: Duration::from_secs(10),
        ..wcfg()
    };
    let (sink, handle) = spawn_writer(rigged.clone(), cfg, cancel.clone());
    sink.write(fx.line(exec, LogLevel::Info, "held")).await; // at the gate
    let t = std::time::Instant::now();
    while rigged.calls() == 0 {
        assert!(
            t.elapsed() < Duration::from_secs(2),
            "the insert never started"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    sink.write(fx.line(exec, LogLevel::Info, "queued")).await;
    let barrier = tokio::spawn({
        let sink = sink.clone();
        async move { sink.barrier().await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!barrier.is_finished(), "control: the barrier is queued");

    cancel.cancel();
    gate.send(true).unwrap();
    let got = tokio::time::timeout(Duration::from_secs(5), barrier)
        .await
        .expect("the drain answers the barrier")
        .unwrap();
    assert_eq!(
        got,
        Barrier::Acked,
        "acknowledged, not released by dropping"
    );
    assert_eq!(
        fx.count(exec).await,
        2,
        "the line before the barrier was stored before the acknowledgement"
    );
    handle.await.unwrap();
}
