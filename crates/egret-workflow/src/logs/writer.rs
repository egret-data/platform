//! W11: [`PgLogSink`] and the background batch writer behind it (spec §4.3).
//!
//! A handler's [`RunLog`](super::RunLog) writes each line to the registry's
//! [`LogSink`]. In production that is a [`PgLogSink`]: a bounded channel whose
//! one reader, [`spawn_writer`]'s task, stores lines in batches through
//! [`LogRepo::insert_batch`].
//!
//! # No line is dropped silently (D3)
//!
//! - **A full channel makes the writer wait -- while the writer is healthy.**
//!   [`PgLogSink::write`] then waits for room (never `try_send` and drop): a
//!   burst slows the handler producing it. While the writer is **failing**
//!   (below) a full channel drops the line instead, and counts it.
//! - **A failed batch is retried** (100 ms doubling, capped at 5 s) for
//!   [`WriterConfig::retry_for`], **if the writer was healthy when the batch
//!   started**. While it retries nothing is read from the channel. A batch
//!   whose retries run out is dropped **with a `tracing::error!` naming its
//!   executions and line count**, and the writer carries on.
//! - **SQLSTATE 23503 is not retried.** It is the concurrent-delete race
//!   ([`InsertError::ExecutionDeleted`]): an execution was deleted between the
//!   statement's snapshot and its foreign-key check. Backing off cannot help,
//!   and re-running at once can: the join then drops that execution's lines.
//!   So the batch is re-run **once**, immediately; a second 23503 drops it
//!   with the error above.
//! - **A data error isolates the line, not the batch.** SQLSTATE class `22`
//!   ([`InsertError::BadData`], e.g. a U+0000 that did not come through
//!   [`RunLog`](super::RunLog), which strips it) is a property of a LINE, so no
//!   retry can help. The batch is re-inserted **one line at a time**: the
//!   offending line(s) are dropped with a counted `tracing::error!`, every
//!   other line -- of any execution, of any workspace -- is stored. A line
//!   that meets an ordinary failure there is kept and retried as above.
//! - **Lines the join drops** (their execution is gone, or its workspace is not
//!   theirs) are counted and logged at `warn`.
//! - **Shutdown** ([`spawn_writer`]'s `cancel`): the writer stops accepting
//!   (closes the channel), stores what is already in it, and exits within
//!   [`WriterConfig::drain_within`]. Whatever it could not store in that time
//!   is counted in a `tracing::error!`. A write after that returns at once and
//!   drops its line, with one `warn` per sink (spec §11: "the process is
//!   stopping").
//!
//! Delivery is at least once: a batch retried after a failure whose commit
//! the writer did not see (the connection lost after the commit) is stored
//! twice.
//!
//! A batch is one `INSERT` statement, so one short transaction: what
//! [`SETTLE_LAG`](super::SETTLE_LAG)'s bound assumes.
//!
//! # The flush barrier
//!
//! [`PgLogSink::flush`] puts a marker in the channel behind every line the sink
//! has accepted, and waits for the writer to reach it. The writer stores its
//! current batch (with the retries and the 23503 re-run above) and then
//! acknowledges. So when `flush` returns [`Barrier::Acked`], every line sent
//! before it has been inserted -- or its batch was dropped and reported. The
//! wait is bounded by [`WriterConfig::flush_within`]; past it, or with the
//! writer gone, `flush` logs and returns. `RunLog::finish` uses it so that a
//! run's lines are in the table before its outcome commits (spec §4.3, §6).
//!
//! # The breaker: a failing writer never slows a run
//!
//! Without it, one log-only fault (a lock on the table, pool starvation, a bad
//! line) would make every run's barrier wait out a 30 s retry: one run per
//! 30 s per replica, and handlers blocked on a full channel counting that wait
//! against their own `timeout:`. So the writer and every sink share one
//! **failing** flag:
//!
//! - **set** by an insert that fails with an ordinary error
//!   ([`InsertError::Db`]), or by a barrier that timed out;
//! - **cleared** by the next insert that succeeds.
//!
//! While it is set:
//!
//! - [`PgLogSink::flush`] returns at once ([`Barrier::Skipped`], counted, one
//!   `warn` per failing spell); a barrier already waiting is released the
//!   moment the flag is set. **The run's outcome may then commit before its
//!   last lines are stored, or without them.**
//! - [`PgLogSink::write`] never waits for room: a line that finds the channel
//!   full is dropped and counted (one `error!` when the dropping starts, and
//!   the total when the writer recovers or stops). So logging cannot turn a
//!   run into a timeout.
//! - Each new batch gets **one** insert attempt, so the channel keeps
//!   draining; a batch that fails it is dropped and reported.
//!
//! In normal operation none of this changes anything.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::{InsertError, LogLine, LogRepo};
use crate::worker::handlers::LogSink;

/// The first wait after a failed batch; doubled per failure.
const BACKOFF_FIRST: Duration = Duration::from_millis(100);
/// The longest wait between two tries of one batch.
const BACKOFF_MAX: Duration = Duration::from_secs(5);

/// What the writer stores a batch with. [`LogRepo`] in production; a test
/// substitutes one that can be held or made to fail.
#[async_trait::async_trait]
pub trait Inserter: Send + Sync + 'static {
    /// [`LogRepo::insert_batch`]'s contract: the number of lines written, the
    /// rest dropped by the join.
    async fn insert_batch(&self, lines: &[LogLine]) -> Result<u64, InsertError>;
}

#[async_trait::async_trait]
impl Inserter for LogRepo {
    async fn insert_batch(&self, lines: &[LogLine]) -> Result<u64, InsertError> {
        LogRepo::insert_batch(self, lines).await
    }
}

/// What travels the writer's channel: a line, or a flush barrier whose sender
/// is acknowledged once every line before it is stored.
#[derive(Debug)]
enum Msg {
    Line(LogLine),
    Flush(oneshot::Sender<()>),
}

/// What one [`PgLogSink::barrier`] came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Barrier {
    /// The writer reached the barrier: every line sent before it was stored,
    /// or its batch was dropped and reported.
    Acked,
    /// The writer was failing, so the barrier did not wait (module doc).
    Skipped,
    /// [`WriterConfig::flush_within`] passed first. Trips the breaker.
    TimedOut,
    /// The writer has stopped.
    WriterGone,
}

/// The breaker's state, shared by the writer and every sink clone (module
/// doc, "The breaker").
#[derive(Debug)]
struct Health {
    /// `true` while the writer is failing.
    failing: watch::Sender<bool>,
    /// Barriers that returned at once because the writer was failing.
    barrier_skips: AtomicU64,
    /// Lines a `write` dropped at a full channel while the writer was failing.
    dropped: AtomicU64,
    /// How much of `dropped` has been reported (by the writer only).
    dropped_reported: AtomicU64,
    /// One `warn` per failing spell for skipped barriers, one `error!` for
    /// dropped lines; both reset when the writer recovers.
    skip_warned: AtomicBool,
    drop_warned: AtomicBool,
}

impl Health {
    fn new() -> Self {
        Self {
            failing: watch::channel(false).0,
            barrier_skips: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            dropped_reported: AtomicU64::new(0),
            skip_warned: AtomicBool::new(false),
            drop_warned: AtomicBool::new(false),
        }
    }

    fn is_failing(&self) -> bool {
        *self.failing.borrow()
    }

    /// Set the flag; says so once, when it goes from healthy to failing.
    fn trip(&self, why: &str) {
        let tripped = self
            .failing
            .send_if_modified(|f| !std::mem::replace(f, true));
        if tripped {
            tracing::warn!(
                why,
                "workflow log writer is failing: until an insert succeeds, flush barriers \
                 do not wait, a write that finds the channel full drops its line, and each \
                 batch gets one attempt"
            );
        }
    }

    /// An insert succeeded: clear the flag, and on the way back report what
    /// the failing spell cost.
    fn recovered(&self) {
        let was = self
            .failing
            .send_if_modified(|f| std::mem::replace(f, false));
        if was {
            self.skip_warned.store(false, Ordering::Relaxed);
            self.drop_warned.store(false, Ordering::Relaxed);
            tracing::info!(
                barriers_skipped = self.barrier_skips.load(Ordering::Relaxed),
                "workflow log writer recovered: an insert succeeded"
            );
            self.report_dropped();
        }
    }

    /// `error!` the lines dropped at the channel since the last report.
    fn report_dropped(&self) {
        let total = self.dropped.load(Ordering::Relaxed);
        let before = self.dropped_reported.swap(total, Ordering::Relaxed);
        if total > before {
            tracing::error!(
                lines = total - before,
                "workflow log lines dropped at a full channel while the writer was failing; \
                 these lines are lost"
            );
        }
    }
}

/// The durable [`LogSink`]: the sending half of the writer's channel.
#[derive(Clone, Debug)]
pub struct PgLogSink {
    tx: mpsc::Sender<Msg>,
    /// [`WriterConfig::flush_within`].
    flush_within: Duration,
    /// Set by the first write that found the writer gone, so the process
    /// stopping logs one line rather than one per dropped log line.
    closed_warned: Arc<AtomicBool>,
    health: Arc<Health>,
}

impl PgLogSink {
    /// The breaker is set: the writer is failing (module doc).
    pub fn is_failing(&self) -> bool {
        self.health.is_failing()
    }

    /// How many barriers have returned at once because the writer was
    /// failing, since the writer started.
    pub fn barrier_skips(&self) -> u64 {
        self.health.barrier_skips.load(Ordering::Relaxed)
    }

    /// How many lines a `write` has dropped at a full channel while the
    /// writer was failing, since the writer started.
    pub fn dropped_while_failing(&self) -> u64 {
        self.health.dropped.load(Ordering::Relaxed)
    }

    /// The flush barrier, saying what it came to. [`LogSink::flush`] is this
    /// with the answer ignored.
    pub async fn barrier(&self) -> Barrier {
        if self.health.is_failing() {
            return self.skipped();
        }
        let mut failing = self.health.failing.subscribe();
        let (ack, acked) = oneshot::channel();
        let wait = async {
            if self.tx.send(Msg::Flush(ack)).await.is_err() {
                return Barrier::WriterGone;
            }
            match acked.await {
                Ok(()) => Barrier::Acked,
                Err(_) => Barrier::WriterGone,
            }
        };
        tokio::select! {
            biased;
            got = tokio::time::timeout(self.flush_within, wait) => match got {
                Ok(b) => b,
                Err(_) => {
                    tracing::warn!(
                        flush_within_ms = self.flush_within.as_millis() as u64,
                        "workflow log flush barrier timed out; the run's outcome is \
                         committed without waiting for its last log lines to be stored"
                    );
                    self.health.trip("a flush barrier timed out");
                    Barrier::TimedOut
                }
            },
            _ = failing.wait_for(|f| *f) => self.skipped(),
        }
    }

    fn skipped(&self) -> Barrier {
        self.health.barrier_skips.fetch_add(1, Ordering::Relaxed);
        if !self.health.skip_warned.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                "workflow log writer is failing: flush barriers return at once, so a run's \
                 outcome can commit before its last log lines are stored, or without them"
            );
        }
        Barrier::Skipped
    }

    /// The writer is gone: drop `line`, saying so once per sink.
    fn closed(&self, line: &LogLine) {
        if !self.closed_warned.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                execution = %line.execution_id,
                "workflow log writer has stopped (the process is stopping); \
                 this log line and any written after it are dropped"
            );
        }
    }

    /// The writer is failing and the channel is full: drop `line`, counted.
    fn drop_failing(&self, line: &LogLine) {
        self.health.dropped.fetch_add(1, Ordering::Relaxed);
        if !self.health.drop_warned.swap(true, Ordering::Relaxed) {
            tracing::error!(
                execution = %line.execution_id,
                "workflow log writer is failing and its channel is full: log lines are \
                 dropped, not waited for, until it recovers (the count is logged then)"
            );
        }
    }
}

#[async_trait::async_trait]
impl LogSink for PgLogSink {
    /// Waits while the channel is full and the writer is healthy (D3). Returns
    /// at once, dropping the line with a count, when the channel is full and
    /// the writer is failing -- including when it starts failing during the
    /// wait -- and, dropping it with one `warn`, when the writer has stopped.
    async fn write(&self, line: LogLine) {
        let line = match self.tx.try_send(Msg::Line(line)) {
            Ok(()) => return,
            Err(TrySendError::Closed(Msg::Line(line))) => return self.closed(&line),
            Err(TrySendError::Full(Msg::Line(line))) => line,
            Err(_) => unreachable!("a Line was sent"),
        };
        if self.health.is_failing() {
            return self.drop_failing(&line);
        }
        let mut failing = self.health.failing.subscribe();
        tokio::select! {
            biased;
            permit = self.tx.reserve() => match permit {
                Ok(permit) => permit.send(Msg::Line(line)),
                Err(_) => self.closed(&line),
            },
            _ = failing.wait_for(|f| *f) => match self.tx.try_send(Msg::Line(line)) {
                Ok(()) => {}
                Err(TrySendError::Closed(Msg::Line(line))) => self.closed(&line),
                Err(TrySendError::Full(Msg::Line(line))) => self.drop_failing(&line),
                Err(_) => unreachable!("a Line was sent"),
            },
        }
    }

    /// The barrier (module doc), bounded as a whole -- waiting for room in a
    /// full channel included -- by [`WriterConfig::flush_within`], and not
    /// waiting at all while the writer is failing.
    async fn flush(&self) {
        self.barrier().await;
    }
}

/// The writer's bounds. [`Default`] is the spec's values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WriterConfig {
    /// Lines the channel holds before a `write` waits.
    pub capacity: usize,
    /// A batch is stored once it holds this many lines ...
    pub batch: usize,
    /// ... or once this long has passed since its first line arrived.
    pub flush_every: Duration,
    /// How long one failing batch is retried before it is dropped.
    pub retry_for: Duration,
    /// How long, after cancellation, the writer may spend storing what is
    /// already in the channel.
    pub drain_within: Duration,
    /// The longest [`PgLogSink::flush`] waits for its barrier to be
    /// acknowledged before the run carries on without it (and trips the
    /// breaker).
    pub flush_within: Duration,
}

impl Default for WriterConfig {
    /// 10,000 lines; 500 lines or 1 s; 30 s of retries; a 10 s drain; a 5 s
    /// flush barrier. A healthy barrier waits for everything queued ahead of
    /// it -- up to the whole channel, about 21 inserts of 500 lines -- within
    /// this bound; a barrier behind a failing batch is released by the
    /// breaker, not by this bound.
    fn default() -> Self {
        Self {
            capacity: 10_000,
            batch: 500,
            flush_every: Duration::from_secs(1),
            retry_for: Duration::from_secs(30),
            drain_within: Duration::from_secs(10),
            flush_within: Duration::from_secs(5),
        }
    }
}

/// Start the writer; returns its sink and its task.
///
/// The task ends when `cancel` fires (after draining, see the module doc) or
/// when every [`PgLogSink`] clone has been dropped. **Cancel it only once
/// nothing can write any more**: `egret_workflow::runtime::spawn_with_logs`
/// fires it when the claim loop has stopped.
pub fn spawn_writer<I: Inserter>(
    repo: I,
    cfg: WriterConfig,
    cancel: CancellationToken,
) -> (PgLogSink, JoinHandle<()>) {
    let (tx, rx) = mpsc::channel(cfg.capacity.max(1));
    let health = Arc::new(Health::new());
    let handle = tokio::spawn(run(repo, cfg, rx, cancel, health.clone()));
    (
        PgLogSink {
            tx,
            flush_within: cfg.flush_within,
            closed_warned: Arc::default(),
            health,
        },
        handle,
    )
}

async fn run<I: Inserter>(
    repo: I,
    cfg: WriterConfig,
    mut rx: mpsc::Receiver<Msg>,
    cancel: CancellationToken,
    health: Arc<Health>,
) {
    let batch = cfg.batch.max(1);
    let mut buf: Vec<LogLine> = Vec::with_capacity(batch);
    // Barriers reached but not yet acknowledged: acknowledged right after the
    // batch that holds every line before them has been stored (or reported).
    let mut acks: Vec<oneshot::Sender<()>> = Vec::new();
    // When the oldest unflushed line must be stored by.
    let mut due: Option<Instant> = None;
    loop {
        let flush = tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep_until(due.unwrap_or_else(Instant::now)), if due.is_some() => true,
            got = rx.recv() => match got {
                Some(Msg::Line(line)) => {
                    if buf.is_empty() {
                        due = Some(Instant::now() + cfg.flush_every);
                    }
                    buf.push(line);
                    buf.len() >= batch
                }
                Some(Msg::Flush(ack)) => {
                    acks.push(ack);
                    true
                }
                // Every sink is gone: nothing more can arrive.
                None => break,
            },
        };
        if !flush {
            continue;
        }
        match store(
            &repo,
            &mut buf,
            Instant::now() + cfg.retry_for,
            Some(&cancel),
            &health,
        )
        .await
        {
            Stored::Written => due = None,
            // Not fatal (spec §4.3): the batch is reported and gone, and the
            // writer carries on with the next one.
            Stored::Lost => due = None,
            // Cancelled while backing off: `buf` is kept for the drain, and
            // so are the barriers behind it.
            Stored::Interrupted => break,
        }
        ack_all(&mut acks);
    }

    // ── Drain: stop accepting, store what is already queued, exit in time ──
    rx.close();
    let deadline = Instant::now() + cfg.drain_within;
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(Msg::Line(line))) => {
                buf.push(line);
                if buf.len() >= batch {
                    store(&repo, &mut buf, deadline, None, &health).await;
                }
            }
            Ok(Some(Msg::Flush(ack))) => {
                store(&repo, &mut buf, deadline, None, &health).await;
                acks.push(ack);
                ack_all(&mut acks);
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }
    store(&repo, &mut buf, deadline, None, &health).await;
    ack_all(&mut acks);
    let mut unstored = buf.len();
    while let Ok(msg) = rx.try_recv() {
        // A barrier left in the channel is released by being dropped here.
        if matches!(msg, Msg::Line(_)) {
            unstored += 1;
        }
    }
    health.report_dropped();
    if unstored > 0 {
        tracing::error!(
            lines = unstored,
            drain_within_ms = cfg.drain_within.as_millis() as u64,
            "workflow log writer stopped before storing every queued line; these lines are lost"
        );
    }
    tracing::info!("workflow log writer stopped");
}

/// Release every barrier waiting on the batch just stored.
fn ack_all(acks: &mut Vec<oneshot::Sender<()>>) {
    for ack in acks.drain(..) {
        // The flusher may have timed out and gone; nothing to tell it then.
        let _ = ack.send(());
    }
}

/// What [`store`] did with the batch.
#[derive(Debug, PartialEq, Eq)]
enum Stored {
    /// Written (less any lines the join dropped, which are logged); `buf` is
    /// empty.
    Written,
    /// Given up on and reported; `buf` is empty.
    Lost,
    /// `stop` fired while backing off; `buf` still holds the batch.
    Interrupted,
}

/// Store `buf` as one batch, retrying ordinary failures until `give_up`
/// (always trying at least once) -- or trying **once** if the writer was
/// failing when the batch started. See the module doc for 23503 and for data
/// errors.
async fn store<I: Inserter>(
    repo: &I,
    buf: &mut Vec<LogLine>,
    give_up: Instant,
    stop: Option<&CancellationToken>,
    health: &Health,
) -> Stored {
    if buf.is_empty() {
        return Stored::Written;
    }
    let retry = !health.is_failing();
    let mut backoff = BACKOFF_FIRST;
    let mut reran_after_delete = false;
    loop {
        let err = match repo.insert_batch(buf).await {
            Ok(written) => {
                report_join_drops(buf.len() as u64, written);
                buf.clear();
                health.recovered();
                return Stored::Written;
            }
            Err(e @ InsertError::ExecutionDeleted(_)) if !reran_after_delete => {
                reran_after_delete = true;
                tracing::debug!(error = %e, "workflow log batch met a deleted execution; re-running once");
                continue;
            }
            Err(InsertError::BadData(e)) => {
                tracing::debug!(error = %e, "workflow log batch holds a line Postgres refuses; isolating it");
                match isolate(repo, buf, health).await {
                    None => return Stored::Written,
                    // Some lines met an ordinary failure: `buf` now holds
                    // just those, handled as any failed batch.
                    Some(e) => e,
                }
            }
            Err(e) => e,
        };
        if matches!(err, InsertError::Db(_)) {
            health.trip("a log batch insert failed");
        }
        let deleted = matches!(err, InsertError::ExecutionDeleted(_));
        if deleted || !retry || Instant::now() + backoff > give_up {
            let why = if deleted || retry {
                "after its retries"
            } else {
                "after one attempt: the writer is failing"
            };
            lose(buf, &err, why);
            return Stored::Lost;
        }
        tracing::warn!(
            error = %err,
            lines = buf.len(),
            retry_in_ms = backoff.as_millis() as u64,
            "workflow log batch failed; retrying"
        );
        match stop {
            Some(stop) => tokio::select! {
                _ = stop.cancelled() => return Stored::Interrupted,
                _ = tokio::time::sleep(backoff) => {}
            },
            None => tokio::time::sleep(backoff).await,
        }
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// `warn` about lines the join dropped (`written < sent`).
fn report_join_drops(sent: u64, written: u64) {
    let dropped = sent.saturating_sub(written);
    if dropped > 0 {
        tracing::warn!(
            dropped,
            written,
            "workflow log lines dropped: their execution no longer exists \
             or is not in the line's workspace"
        );
    }
}

/// A batch failed with a data error: insert its lines one at a time, in order,
/// so only the line(s) Postgres refuses are lost. Refused lines are dropped
/// with one counted `error!`; a line whose execution was deleted meanwhile is
/// dropped as the join would drop it. Lines that meet an ordinary failure stay
/// in `buf` and that error is returned; `None` when `buf` is empty.
async fn isolate<I: Inserter>(
    repo: &I,
    buf: &mut Vec<LogLine>,
    health: &Health,
) -> Option<InsertError> {
    let mut kept = Vec::new();
    let mut last = None;
    let mut refused = 0u64;
    let mut refused_in: BTreeSet<uuid::Uuid> = BTreeSet::new();
    let mut first_refusal = None;
    for line in buf.drain(..) {
        match repo.insert_batch(std::slice::from_ref(&line)).await {
            Ok(written) => {
                report_join_drops(1, written);
                health.recovered();
            }
            Err(InsertError::ExecutionDeleted(_)) => report_join_drops(1, 0),
            Err(InsertError::BadData(e)) => {
                refused += 1;
                refused_in.insert(line.execution_id);
                first_refusal.get_or_insert(e);
            }
            Err(e @ InsertError::Db(_)) => {
                kept.push(line);
                last = Some(e);
            }
        }
    }
    if let Some(e) = first_refusal {
        tracing::error!(
            error = %e,
            lines = refused,
            executions = ?refused_in,
            "workflow log lines refused by Postgres (a data error such as a NUL byte) \
             were dropped; the rest of their batch was stored one line at a time"
        );
    }
    *buf = kept;
    if buf.is_empty() {
        None
    } else {
        last
    }
}

/// Report a batch that will not be stored, then empty `buf`.
fn lose(buf: &mut Vec<LogLine>, err: &InsertError, why: &str) {
    let executions: BTreeSet<uuid::Uuid> = buf.iter().map(|l| l.execution_id).collect();
    tracing::error!(
        error = %err,
        lines = buf.len(),
        executions = ?executions,
        why,
        "workflow log batch dropped; these lines are lost"
    );
    buf.clear();
}
