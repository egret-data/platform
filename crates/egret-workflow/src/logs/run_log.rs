//! [`RunLog`]: one task-run attempt's handle on the log (spec §4.2, §5).
//!
//! Every line a handler, a runner or `claim::run_one` writes for an attempt
//! goes through one of these. It stamps the attempt's identity on the line,
//! splits a long line into [`LOG_PIECE_CHARS`] pieces exactly as Kestra's
//! `RunContextLogger` does (D6), and enforces the per-attempt cap (D4): the
//! first line that would go over [`LogLimits`] switches storing off, later
//! lines are only counted, and [`RunLog::finish`] writes one WARN summary.
//!
//! # Two lines bypass the cap
//!
//! The summary itself, and [`RunLog::failure`] — the failed-attempt line
//! (spec §4.4) — because an attempt that hit the cap must still say why it
//! failed. Both are still split.
//!
//! # Masking (W5b)
//!
//! [`RunLog::masked`] returns a handle that replaces every value a run's
//! `secret()` resolved with [`MASK`] in each message **before** it is split,
//! so a piece boundary can never cut a value in two and leave both halves
//! readable. The same replace is [`mask`], a pure function, so a caller can
//! mask a string that never becomes a log line (a plugin's failure reason).
//!
//! # Order
//!
//! The counters sit behind an async mutex that is held across the sink write,
//! so two clones of one handle writing at once cannot reorder an attempt's
//! lines or split pieces, and `finish` cannot race a line into a second
//! summary. A full sink (D3: the writer waits, it never drops) therefore slows
//! every writer of that attempt, which is the point.

use std::borrow::Cow;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::Mutex;
use uuid::Uuid;

use super::{LogLevel, LogLimits, LogLine, LOG_PIECE_CHARS};
use crate::worker::handlers::{LogSink, TracingSink};

/// Who a line is about: one attempt of one task run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunIdentity {
    pub workspace_id: Uuid,
    pub execution_id: Uuid,
    pub task_run_id: Uuid,
    pub task_id: String,
    /// **0-based**, as Kestra's `attemptNumber` is and as the execution API's
    /// per-run `attempts` array is indexed. See [`RunIdentity::stored_attempt`].
    pub attempt: i32,
}

impl RunIdentity {
    /// `TaskInput.attempt` is 1-based; a stored line's is 0-based (the
    /// controller's ruling, measured numbering in Task 1's `attempt_numbering`).
    pub fn stored_attempt(one_based: usize) -> i32 {
        i32::try_from(one_based.saturating_sub(1)).unwrap_or(i32::MAX)
    }
}

#[derive(Debug, Default)]
struct Counters {
    /// Lines and bytes stored under the cap.
    lines: u64,
    bytes: u64,
    /// Set by the first piece that would go over; never cleared.
    withholding: bool,
    withheld_lines: u64,
    withheld_bytes: u64,
    /// `finish` has run; a second call is a no-op.
    finished: bool,
}

/// One task-run attempt's log handle. Cheap to clone; clones share counters.
#[derive(Clone)]
pub struct RunLog {
    identity: RunIdentity,
    sink: Arc<dyn LogSink>,
    limits: LogLimits,
    counters: Arc<Mutex<Counters>>,
    /// The values [`mask`] replaces, prepared by [`prepare`]: no empties, no
    /// duplicates, longest first. Empty on every handle but a masked one.
    secrets: Arc<[String]>,
}

impl std::fmt::Debug for RunLog {
    /// The identity only: the sink is a port and the counters are state.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunLog")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl PartialEq for RunLog {
    /// The identity only, so `TaskInput` can keep deriving `PartialEq`.
    fn eq(&self, other: &Self) -> bool {
        self.identity == other.identity
    }
}

impl RunLog {
    pub fn new(identity: RunIdentity, sink: Arc<dyn LogSink>, limits: LogLimits) -> Self {
        Self {
            identity,
            sink,
            limits,
            counters: Arc::new(Mutex::new(Counters::default())),
            secrets: Arc::from(Vec::new()),
        }
    }

    /// This handle, replacing each of `values` (and whatever this handle
    /// already masked) by [`MASK`] in every line it writes.
    ///
    /// **Shares the counters**: a masked handle and the one it came from are
    /// one attempt, one cap, one summary. A masked line counts toward the cap
    /// at its masked length, because that is what is stored.
    pub fn masked(&self, values: &[String]) -> RunLog {
        let mut all: Vec<String> = self.secrets.to_vec();
        all.extend(values.iter().cloned());
        RunLog {
            secrets: Arc::from(prepare(&all)),
            ..self.clone()
        }
    }

    /// A handle that writes to `tracing` under a nil identity: for tests and
    /// for callers that are not running a claimed attempt.
    pub fn discard() -> Self {
        Self::new(
            RunIdentity {
                workspace_id: Uuid::nil(),
                execution_id: Uuid::nil(),
                task_run_id: Uuid::nil(),
                task_id: String::new(),
                attempt: 0,
            },
            Arc::new(TracingSink),
            LogLimits::default(),
        )
    }

    pub fn identity(&self) -> &RunIdentity {
        &self.identity
    }

    /// Write `message` at `level`, split and under the cap.
    pub async fn line(&self, level: LogLevel, message: &str) {
        let ts = Utc::now();
        // Before the split, never after: see the module doc.
        let message = mask_prepared(message, &self.secrets);
        let mut c = self.counters.lock().await;
        for piece in pieces(&message) {
            let bytes = piece.len() as u64;
            if !c.withholding
                && (c.lines + 1 > self.limits.run_max_lines
                    || c.bytes + bytes > self.limits.run_max_bytes)
            {
                c.withholding = true;
            }
            if c.withholding {
                c.withheld_lines += 1;
                c.withheld_bytes += bytes;
                continue;
            }
            c.lines += 1;
            c.bytes += bytes;
            self.sink.write(self.make(level, ts, piece)).await;
        }
    }

    pub async fn info(&self, message: &str) {
        self.line(LogLevel::Info, message).await;
    }

    /// The failed-attempt line (spec §4.4): ERROR, **outside** the cap, and
    /// still split. Written by `claim::run_one` only.
    pub async fn failure(&self, message: &str) {
        let ts = Utc::now();
        let message = mask_prepared(message, &self.secrets);
        let _c = self.counters.lock().await;
        for piece in pieces(&message) {
            self.sink.write(self.make(LogLevel::Error, ts, piece)).await;
        }
    }

    /// Write the cap summary, once, if anything was withheld (WARN, outside
    /// the cap), then **wait for the sink's flush barrier**
    /// ([`LogSink::flush`]): every line of this attempt is stored before
    /// `finish` returns, or the sink's bound has passed. A second call is a
    /// no-op.
    ///
    /// `claim::run_one` calls this before it commits the run's outcome, so an
    /// execution that has become terminal has its lines in the table -- which
    /// is what lets the logs API's `complete` tell a poller it may stop
    /// (spec §6). The exceptions -- the process dying, a dropped batch, and a
    /// failing writer whose barrier does not wait -- are spec §11's.
    pub async fn finish(&self) {
        {
            let mut c = self.counters.lock().await;
            if c.finished {
                return;
            }
            c.finished = true;
            if c.withheld_lines > 0 {
                self.write_summary(&c).await;
            }
        }
        // Outside the lock: the barrier can take up to the sink's bound, and
        // nothing the counters guard is touched by it.
        self.sink.flush().await;
    }

    async fn write_summary(&self, c: &Counters) {
        let message = format!(
            "{} lines ({} bytes) of this attempt were not stored: over the per-run log cap \
             ({} lines / {} bytes).",
            c.withheld_lines,
            c.withheld_bytes,
            self.limits.run_max_lines,
            self.limits.run_max_bytes
        );
        self.sink
            .write(self.make(LogLevel::Warn, Utc::now(), &message))
            .await;
    }

    /// Every line this handle writes is built here, so this is where U+0000
    /// is removed: Postgres `text` cannot hold it (SQLSTATE 22021), and one
    /// such line would fail its whole batch. Kestra strips it the same way
    /// before writing (`JdbcJsonbUtils.java:13`, `replace("\u0000", "")`).
    fn make(&self, level: LogLevel, ts: DateTime<Utc>, message: &str) -> LogLine {
        let message = if message.contains('\0') {
            message.replace('\0', "")
        } else {
            message.to_string()
        };
        LogLine {
            workspace_id: self.identity.workspace_id,
            execution_id: self.identity.execution_id,
            task_run_id: Some(self.identity.task_run_id),
            task_id: Some(self.identity.task_id.clone()),
            attempt: Some(self.identity.attempt),
            level,
            ts,
            message,
        }
    }
}

/// What a masked value is replaced by: Kestra's
/// `RunContextLogger.BaseAppender.replaceSecret`, measured on the artifact
/// (`oracle-exec/w5b-plugins.json`, `secret_masking`).
pub const MASK: &str = "******";

/// `message` with every occurrence of every non-empty value in `values`
/// replaced by [`MASK`].
///
/// One left-to-right scan, taking the **longest** value that matches at each
/// position, so a value that is a substring of another cannot leave the
/// longer one's tail readable, and a value made of `*` cannot re-match the
/// mask it just produced. An empty value masks nothing (replacing `""` would
/// put a mask between every character).
pub fn mask<'a>(message: &'a str, values: &[String]) -> Cow<'a, str> {
    mask_prepared(message, &prepare(values))
}

/// `values` without empties or duplicates, longest first.
fn prepare(values: &[String]) -> Vec<String> {
    let mut out: Vec<String> = values.iter().filter(|v| !v.is_empty()).cloned().collect();
    out.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    out.dedup();
    out
}

/// [`mask`] over values [`prepare`] already ordered.
fn mask_prepared<'a>(message: &'a str, prepared: &[String]) -> Cow<'a, str> {
    if !prepared.iter().any(|v| message.contains(v.as_str())) {
        return Cow::Borrowed(message);
    }
    let mut out = String::with_capacity(message.len());
    let mut rest = message;
    while let Some(c) = rest.chars().next() {
        match prepared.iter().find(|v| rest.starts_with(v.as_str())) {
            Some(v) => {
                out.push_str(MASK);
                rest = &rest[v.len()..];
            }
            None => {
                out.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    Cow::Owned(out)
}

/// `message` in consecutive pieces of [`LOG_PIECE_CHARS`] `char`s, the last
/// one shorter; one piece (possibly empty) when it is not longer than that.
///
/// Counted in `char`s and cut on `char_indices`, so a cut never falls inside
/// a character and every piece is valid UTF-8.
fn pieces(message: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut count = 0;
    for (i, _) in message.char_indices() {
        if count == LOG_PIECE_CHARS {
            out.push(&message[start..i]);
            start = i;
            count = 0;
        }
        count += 1;
    }
    out.push(&message[start..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logs::fixture;
    use crate::worker::handlers::testing::Recorder;

    fn identity(attempt: i32) -> RunIdentity {
        RunIdentity {
            workspace_id: Uuid::from_u128(1),
            execution_id: Uuid::from_u128(2),
            task_run_id: Uuid::from_u128(3),
            task_id: "t".into(),
            attempt,
        }
    }

    fn capped(rec: &Arc<Recorder>, lines: u64, bytes: u64) -> RunLog {
        RunLog::new(
            identity(0),
            rec.clone(),
            LogLimits {
                run_max_lines: lines,
                run_max_bytes: bytes,
            },
        )
    }

    fn summary(lines: u64, bytes: u64, limits: LogLimits) -> String {
        format!(
            "{lines} lines ({bytes} bytes) of this attempt were not stored: over the per-run \
             log cap ({} lines / {} bytes).",
            limits.run_max_lines, limits.run_max_bytes
        )
    }

    #[tokio::test]
    async fn a_line_is_written_with_the_runs_identity() {
        let rec = Recorder::new();
        let log = RunLog::new(identity(2), rec.clone(), LogLimits::default());
        let before = Utc::now();
        log.line(LogLevel::Warn, "hello").await;
        log.info("again").await;
        let lines = rec.lines();
        assert_eq!(lines.len(), 2, "{lines:?}");
        let l = &lines[0];
        assert_eq!(l.workspace_id, Uuid::from_u128(1));
        assert_eq!(l.execution_id, Uuid::from_u128(2));
        assert_eq!(l.task_run_id, Some(Uuid::from_u128(3)));
        assert_eq!(l.task_id.as_deref(), Some("t"));
        assert_eq!(l.attempt, Some(2));
        assert_eq!(l.level, LogLevel::Warn);
        assert_eq!(l.message, "hello");
        assert!(l.ts >= before && l.ts <= Utc::now());
        assert_eq!(
            (lines[1].level, lines[1].message.as_str()),
            (LogLevel::Info, "again")
        );
    }

    /// U+0000 cannot be stored in Postgres `text`; every path that builds a
    /// line removes it, as Kestra does.
    #[tokio::test]
    async fn a_nul_character_is_removed_from_every_line() {
        let rec = Recorder::new();
        let log = capped(&rec, 1, u64::MAX);
        log.info("a\0b\0").await;
        log.info("withheld\0").await; // over the 1-line cap: summarised
        log.failure("fail\0ed").await;
        log.finish().await;
        let got: Vec<String> = rec.lines().into_iter().map(|l| l.message).collect();
        assert_eq!(got.len(), 3, "{got:?}");
        assert_eq!(got[0], "ab", "an ordinary line");
        assert_eq!(got[1], "failed", "the failure line");
        assert!(got[2].starts_with("1 lines"), "the summary: {got:?}");
        assert!(got.iter().all(|m| !m.contains('\0')), "{got:?}");
    }

    /// The 1-based `TaskInput.attempt` becomes the 0-based stored one.
    #[test]
    fn the_stored_attempt_is_zero_based() {
        let e = fixture::expected("attempt_numbering");
        assert_eq!(e["stored_attempt_minus_task_input_attempt"], -1);
        assert_eq!(
            RunIdentity::stored_attempt(1),
            e["first_attempt"].as_i64().unwrap() as i32
        );
        assert_eq!(
            RunIdentity::stored_attempt(2),
            e["retry_attempt"].as_i64().unwrap() as i32
        );
    }

    #[tokio::test]
    async fn a_long_line_is_split_into_pieces_in_order() {
        // 40,000 chars with multi-byte characters at and around the cut
        // points: a byte split would land inside one of them.
        let mut chars: Vec<char> = (0..40_000)
            .map(|i| (b'a' + (i % 26) as u8) as char)
            .collect();
        for at in [15_358, 15_359, 15_360, 15_361, 30_719, 30_720] {
            chars[at] = if at % 2 == 0 { 'é' } else { '𝄞' };
        }
        let input: String = chars.iter().collect();
        assert!(input.len() > 40_000, "the input must hold multi-byte chars");

        let rec = Recorder::new();
        let log = RunLog::new(identity(0), rec.clone(), LogLimits::default());
        log.line(LogLevel::Info, &input).await;
        let lines = rec.lines();
        let counts: Vec<usize> = lines.iter().map(|l| l.message.chars().count()).collect();
        assert_eq!(counts, vec![15_360, 15_360, 9_280]);
        assert!(
            lines.iter().all(|l| l.ts == lines[0].ts),
            "the pieces share one ts"
        );
        assert!(lines.iter().all(|l| l.level == LogLevel::Info));
        let joined: String = lines.iter().map(|l| l.message.as_str()).collect();
        assert_eq!(joined, input, "in order, and nothing lost");

        // Task 1's measured cases, typed.
        let e = fixture::expected("long_line_split");
        assert_eq!(e["piece_chars"].as_u64().unwrap() as usize, LOG_PIECE_CHARS);
        let cases = e["cases"].as_array().unwrap();
        assert!(!cases.is_empty());
        for case in cases {
            let n = case["input_chars"].as_u64().unwrap() as usize;
            let want: Vec<usize> = case["piece_chars"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as usize)
                .collect();
            let input: String = "é".repeat(n);
            let rec = Recorder::new();
            RunLog::new(
                identity(0),
                rec.clone(),
                LogLimits {
                    run_max_lines: u64::MAX,
                    run_max_bytes: u64::MAX,
                },
            )
            .line(LogLevel::Info, &input)
            .await;
            let got: Vec<usize> = rec
                .lines()
                .iter()
                .map(|l| l.message.chars().count())
                .collect();
            assert_eq!(got, want, "input of {n} chars");
            let joined: String = rec.lines().iter().map(|l| l.message.clone()).collect();
            assert_eq!(joined, input);
        }
    }

    #[tokio::test]
    async fn the_pieces_of_a_split_line_count_toward_the_cap() {
        let rec = Recorder::new();
        let log = capped(&rec, 2, u64::MAX);
        let input = "x".repeat(40_000);
        log.line(LogLevel::Info, &input).await;
        log.finish().await;
        let lines = rec.lines();
        assert_eq!(
            lines.len(),
            3,
            "{:?}",
            lines.iter().map(|l| l.message.len()).collect::<Vec<_>>()
        );
        assert_eq!(lines[0].message.len(), 15_360);
        assert_eq!(lines[1].message.len(), 15_360);
        assert_eq!(lines[2].level, LogLevel::Warn);
        assert_eq!(
            lines[2].message,
            summary(
                1,
                9_280,
                LogLimits {
                    run_max_lines: 2,
                    run_max_bytes: u64::MAX
                }
            )
        );
    }

    #[tokio::test]
    async fn the_cap_stops_storing_and_finish_writes_one_summary() {
        let rec = Recorder::new();
        let limits = LogLimits {
            run_max_lines: 3,
            run_max_bytes: u64::MAX,
        };
        let log = RunLog::new(identity(0), rec.clone(), limits);
        for i in 1..=5 {
            log.info(&format!("line {i}")).await;
        }
        assert_eq!(rec.lines().len(), 3, "nothing past the cap is stored");
        log.finish().await;
        let got: Vec<(LogLevel, String)> = rec
            .lines()
            .into_iter()
            .map(|l| (l.level, l.message))
            .collect();
        assert_eq!(
            got,
            vec![
                (LogLevel::Info, "line 1".into()),
                (LogLevel::Info, "line 2".into()),
                (LogLevel::Info, "line 3".into()),
                (LogLevel::Warn, summary(2, 12, limits)),
            ]
        );
        assert_eq!(
            fixture::level(&fixture::w11()["level_table"]["cap_summary"]),
            LogLevel::Warn
        );
    }

    #[tokio::test]
    async fn the_byte_cap_counts_stored_length() {
        let rec = Recorder::new();
        // 10 bytes: "ééé" is 6 bytes (3 chars), "abcd" 4 → exactly 10 stored;
        // the next line of 1 byte goes over.
        let limits = LogLimits {
            run_max_lines: u64::MAX,
            run_max_bytes: 10,
        };
        let log = RunLog::new(identity(0), rec.clone(), limits);
        log.info("ééé").await;
        log.info("abcd").await;
        log.info("z").await;
        log.info("").await;
        log.finish().await;
        let got: Vec<String> = rec.lines().into_iter().map(|l| l.message).collect();
        assert_eq!(
            got,
            vec!["ééé".to_string(), "abcd".into(), summary(2, 1, limits)],
            "bytes, not chars: 'ééé' counts 6"
        );
    }

    #[tokio::test]
    async fn finish_is_idempotent_and_silent_under_the_cap() {
        let rec = Recorder::new();
        let log = capped(&rec, 2, u64::MAX);
        log.info("a").await;
        log.finish().await;
        log.finish().await;
        assert_eq!(rec.lines().len(), 1, "under the cap, finish writes nothing");

        let rec = Recorder::new();
        let log = capped(&rec, 1, u64::MAX);
        log.info("a").await;
        log.info("b").await;
        log.finish().await;
        log.clone().finish().await;
        let warns = rec
            .lines()
            .into_iter()
            .filter(|l| l.level == LogLevel::Warn)
            .count();
        assert_eq!(warns, 1, "one summary however often finish is called");
    }

    #[tokio::test]
    async fn a_retry_attempt_starts_fresh() {
        let rec = Recorder::new();
        let limits = LogLimits {
            run_max_lines: 1,
            run_max_bytes: u64::MAX,
        };
        let first = RunLog::new(identity(0), rec.clone(), limits);
        first.info("a1").await;
        first.info("a2").await;
        first.finish().await;
        let second = RunLog::new(identity(1), rec.clone(), limits);
        second.info("b1").await;
        second.finish().await;
        let got: Vec<(Option<i32>, String)> = rec
            .lines()
            .into_iter()
            .map(|l| (l.attempt, l.message))
            .collect();
        assert_eq!(
            got,
            vec![
                (Some(0), "a1".into()),
                (Some(0), summary(1, 2, limits)),
                (Some(1), "b1".into()),
            ],
            "the second attempt's first line is stored: its counters are its own"
        );
    }

    /// `failure` is ERROR, bypasses the cap and is still split.
    #[tokio::test]
    async fn the_failure_line_bypasses_the_cap_and_is_split() {
        let rec = Recorder::new();
        let log = capped(&rec, 1, u64::MAX);
        log.info("a").await;
        log.info("b").await;
        log.failure(&"f".repeat(15_361)).await;
        log.finish().await;
        let got: Vec<(LogLevel, usize)> = rec
            .lines()
            .into_iter()
            .map(|l| (l.level, l.message.len()))
            .collect();
        assert_eq!(
            got[..3],
            [
                (LogLevel::Info, 1),
                (LogLevel::Error, 15_360),
                (LogLevel::Error, 1)
            ]
        );
        assert_eq!(got[3].0, LogLevel::Warn);
        assert_eq!(
            rec.lines()[3].message,
            summary(
                1,
                1,
                LogLimits {
                    run_max_lines: 1,
                    run_max_bytes: u64::MAX
                }
            )
        );
    }

    // -----------------------------------------------------------------
    // W5b Task 4: masking
    // -----------------------------------------------------------------

    fn v(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    /// Every value is masked in every kind of line, the masked handle shares
    /// the original's counters, and a masked line counts at its masked length.
    #[tokio::test]
    async fn masked_run_log_replaces_every_value_and_counts_the_masked_line() {
        let rec = Recorder::new();
        let limits = LogLimits {
            run_max_lines: 3,
            run_max_bytes: u64::MAX,
        };
        let plain = RunLog::new(identity(0), rec.clone(), limits);
        let log = plain.masked(&v(&["s3cr3t", "hunter2"]));

        log.info("user=admin password=s3cr3t again s3cr3t").await;
        log.line(LogLevel::Warn, "hunter2|s3cr3t").await;
        // The unmasked original is the same attempt: this is line 3 of 3.
        plain.info("third").await;
        // Over the 3-line cap: withheld, and counted at its MASKED length.
        log.info("x s3cr3t").await;
        log.failure("connect failed for s3cr3t").await;
        log.finish().await;

        let got: Vec<(LogLevel, String)> = rec.texts();
        assert_eq!(
            got,
            vec![
                (
                    LogLevel::Info,
                    "user=admin password=****** again ******".into()
                ),
                (LogLevel::Warn, "******|******".into()),
                (LogLevel::Info, "third".into()),
                (LogLevel::Error, "connect failed for ******".into()),
                // "x ******" is 8 bytes; "x s3cr3t" would also be 8, so the
                // byte count is pinned by the next test, not this one.
                (LogLevel::Warn, summary(1, 8, limits)),
            ]
        );
        assert!(got
            .iter()
            .all(|(_, m)| !m.contains("s3cr3t") && !m.contains("hunter2")));
    }

    /// A masked line is counted at the length that is STORED.
    #[tokio::test]
    async fn a_masked_line_counts_its_masked_bytes() {
        let rec = Recorder::new();
        let limits = LogLimits {
            run_max_lines: 1,
            run_max_bytes: u64::MAX,
        };
        let log = RunLog::new(identity(0), rec.clone(), limits).masked(&v(&["a-long-password"]));
        log.info("first").await;
        log.info("a-long-password").await; // withheld: 6 bytes masked, 15 not
        log.finish().await;
        assert_eq!(rec.lines()[1].message, summary(1, 6, limits));
    }

    /// The mask is applied BEFORE the split: a value straddling the
    /// 15,360-char cut is masked whole, and neither half survives.
    #[tokio::test]
    async fn a_value_across_a_piece_boundary_is_masked_whole() {
        let secret = "ABCDEFGHIJ";
        // The value starts 5 chars before the cut.
        let message = format!(
            "{}{secret}{}",
            "x".repeat(LOG_PIECE_CHARS - 5),
            "y".repeat(10)
        );
        let rec = Recorder::new();
        let log = RunLog::new(identity(0), rec.clone(), LogLimits::default()).masked(&v(&[secret]));
        log.info(&message).await;
        let joined: String = rec.lines().iter().map(|l| l.message.clone()).collect();
        assert_eq!(
            joined,
            format!(
                "{}{MASK}{}",
                "x".repeat(LOG_PIECE_CHARS - 5),
                "y".repeat(10)
            )
        );
        assert!(
            !joined.contains("ABCDE") && !joined.contains("FGHIJ"),
            "a half survived"
        );
    }

    /// Longer values first: a value that is a substring of another does not
    /// leave the longer one's tail readable, and an empty value masks nothing.
    #[test]
    fn mask_prefers_the_longer_value_and_ignores_an_empty_one() {
        // Both orders of the input list: the answer cannot depend on it.
        for values in [v(&["abc", "abcdef"]), v(&["abcdef", "abc"])] {
            assert_eq!(mask("x abcdef y abc z", &values), "x ****** y ****** z");
        }
        assert_eq!(
            mask("abc", &v(&[""])),
            "abc",
            "an empty value masks nothing"
        );
        assert_eq!(mask("abc", &v(&["", "b"])), "a******c");
        assert!(
            matches!(mask("nothing here", &v(&["zzz"])), Cow::Borrowed(_)),
            "an unmatched message is not copied"
        );
        // A value made of the mask's own character cannot re-match the mask.
        assert_eq!(mask("p*q", &v(&["*"])), "p******q");
        // Multi-byte text around and inside a value.
        assert_eq!(mask("é密码é", &v(&["密码"])), "é******é");
    }
}
