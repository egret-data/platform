//! W11 Task 2: `workflow.task_logs` (migration 0159) and `LogRepo`.
//! Requires DATABASE_URL.
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

use chrono::{DateTime, Duration as ChronoDuration, SubsecRound, Utc};
use egret_metastore::Db;
use egret_workflow::logs::{InsertError, LogLevel, LogLine, LogQuery, LogRepo, StoredLine};
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
