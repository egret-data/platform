//! W11: [`PgLogSink`] and the background batch writer behind it (spec §4.3).
//!
//! A handler's [`RunLog`](super::RunLog) writes each line to the registry's
//! [`LogSink`]. In production that is a [`PgLogSink`]: a bounded channel whose
//! one reader, [`spawn_writer`]'s task, stores lines in batches through
//! [`LogRepo::insert_batch`].
//!
//! # No line is dropped silently (D3)
//!
//! - **A full channel makes the writer wait.** [`PgLogSink::write`] is
//!   `send().await`, never `try_send`: a burst slows the handler producing it.
//! - **A failed batch is retried** (100 ms doubling, capped at 5 s) for
//!   [`WriterConfig::retry_for`]. While it retries nothing is read from the
//!   channel, so the channel fills and handlers slow down rather than lines
//!   vanishing. A batch whose retries run out is dropped **with a
//!   `tracing::error!` naming its executions and line count**, and the writer
//!   carries on: a log write never changes a run's outcome and never stops the
//!   writer.
//! - **SQLSTATE 23503 is not retried.** It is the concurrent-delete race
//!   ([`InsertError::ExecutionDeleted`]): an execution was deleted between the
//!   statement's snapshot and its foreign-key check. Backing off cannot help,
//!   and re-running at once can: the join then drops that execution's lines.
//!   So the batch is re-run **once**, immediately; a second 23503 drops it
//!   with the error above.
//! - **Lines the join drops** (their execution is gone, or its workspace is not
//!   theirs) are counted and logged at `warn`.
//! - **Shutdown** ([`spawn_writer`]'s `cancel`): the writer stops accepting
//!   (closes the channel), stores what is already in it, and exits within
//!   [`WriterConfig::drain_within`]. Whatever it could not store in that time
//!   is counted in a `tracing::error!`. A write after that returns at once and
//!   drops its line, with one `warn` per sink (spec §11: "the process is
//!   stopping").
//!
//! A batch is one `INSERT` statement, so one short transaction: what
//! [`SETTLE_LAG`](super::SETTLE_LAG)'s bound assumes.
//!
//! # The flush barrier
//!
//! [`PgLogSink::flush`] puts a marker in the channel behind every line the sink
//! has accepted, and waits for the writer to reach it. The writer stores its
//! current batch (with the retries and the 23503 re-run above) and then
//! acknowledges. So when `flush` returns, every line sent before it has been
//! inserted -- or its batch was dropped after its retries and reported. The
//! wait is bounded by [`WriterConfig::flush_within`]; past it, or with the
//! writer gone, `flush` logs and returns. `RunLog::finish` uses it so that a
//! run's lines are in the table before its outcome commits (spec §4.3, §6).

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
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

/// The durable [`LogSink`]: the sending half of the writer's channel.
#[derive(Clone, Debug)]
pub struct PgLogSink {
    tx: mpsc::Sender<Msg>,
    /// [`WriterConfig::flush_within`].
    flush_within: Duration,
    /// Set by the first write that found the writer gone, so the process
    /// stopping logs one line rather than one per dropped log line.
    closed_warned: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl LogSink for PgLogSink {
    /// Waits while the channel is full (D3). Returns at once, dropping the
    /// line, only when the writer has stopped.
    async fn write(&self, line: LogLine) {
        if let Err(mpsc::error::SendError(Msg::Line(line))) = self.tx.send(Msg::Line(line)).await {
            if !self.closed_warned.swap(true, Ordering::Relaxed) {
                tracing::warn!(
                    execution = %line.execution_id,
                    "workflow log writer has stopped (the process is stopping); \
                     this log line and any written after it are dropped"
                );
            }
        }
    }

    /// The barrier (module doc). Bounded as a whole -- waiting for room in a
    /// full channel included -- by [`WriterConfig::flush_within`].
    async fn flush(&self) {
        let (ack, acked) = oneshot::channel();
        let wait = async {
            if self.tx.send(Msg::Flush(ack)).await.is_err() {
                return false;
            }
            acked.await.is_ok()
        };
        match tokio::time::timeout(self.flush_within, wait).await {
            Ok(true) => {}
            // The writer is gone: `write` has already said so, once.
            Ok(false) => {}
            Err(_) => tracing::warn!(
                flush_within_ms = self.flush_within.as_millis() as u64,
                "workflow log flush barrier timed out; the run's outcome is \
                 committed without waiting for its last log lines to be stored"
            ),
        }
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
    /// acknowledged before the run carries on without it.
    pub flush_within: Duration,
}

impl Default for WriterConfig {
    /// 10,000 lines; 500 lines or 1 s; 30 s of retries; a 10 s drain; a 30 s
    /// flush barrier (the retry window: a barrier behind a failing batch waits
    /// out that batch's retries, and no longer).
    fn default() -> Self {
        Self {
            capacity: 10_000,
            batch: 500,
            flush_every: Duration::from_secs(1),
            retry_for: Duration::from_secs(30),
            drain_within: Duration::from_secs(10),
            flush_within: Duration::from_secs(30),
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
    let handle = tokio::spawn(run(repo, cfg, rx, cancel));
    (
        PgLogSink {
            tx,
            flush_within: cfg.flush_within,
            closed_warned: Arc::default(),
        },
        handle,
    )
}

async fn run<I: Inserter>(
    repo: I,
    cfg: WriterConfig,
    mut rx: mpsc::Receiver<Msg>,
    cancel: CancellationToken,
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
                    store(&repo, &mut buf, deadline, None).await;
                }
            }
            Ok(Some(Msg::Flush(ack))) => {
                store(&repo, &mut buf, deadline, None).await;
                acks.push(ack);
                ack_all(&mut acks);
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }
    store(&repo, &mut buf, deadline, None).await;
    ack_all(&mut acks);
    let mut unstored = buf.len();
    while let Ok(msg) = rx.try_recv() {
        // A barrier left in the channel is released by being dropped here.
        if matches!(msg, Msg::Line(_)) {
            unstored += 1;
        }
    }
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
/// (always trying at least once). See the module doc for 23503.
async fn store<I: Inserter>(
    repo: &I,
    buf: &mut Vec<LogLine>,
    give_up: Instant,
    stop: Option<&CancellationToken>,
) -> Stored {
    if buf.is_empty() {
        return Stored::Written;
    }
    let mut backoff = BACKOFF_FIRST;
    let mut reran_after_delete = false;
    loop {
        let err = match repo.insert_batch(buf).await {
            Ok(written) => {
                let dropped = (buf.len() as u64).saturating_sub(written);
                if dropped > 0 {
                    tracing::warn!(
                        dropped,
                        written,
                        "workflow log lines dropped: their execution no longer exists \
                         or is not in the line's workspace"
                    );
                }
                buf.clear();
                return Stored::Written;
            }
            Err(e @ InsertError::ExecutionDeleted(_)) if !reran_after_delete => {
                reran_after_delete = true;
                tracing::debug!(error = %e, "workflow log batch met a deleted execution; re-running once");
                continue;
            }
            Err(e) => e,
        };
        let deleted = matches!(err, InsertError::ExecutionDeleted(_));
        if deleted || Instant::now() + backoff > give_up {
            lose(buf, &err);
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

/// Report a batch that will not be stored, then empty `buf`.
fn lose(buf: &mut Vec<LogLine>, err: &InsertError) {
    let executions: BTreeSet<uuid::Uuid> = buf.iter().map(|l| l.execution_id).collect();
    tracing::error!(
        error = %err,
        lines = buf.len(),
        executions = ?executions,
        "workflow log batch dropped after its retries; these lines are lost"
    );
    buf.clear();
}
