//! W11: the durable log of an execution's task runs (`workflow.task_logs`,
//! migration 0159).
//!
//! This module holds the vocabulary -- [`LogLevel`], [`LogLine`] -- and
//! [`LogRepo`], the only code that reads or writes the table; [`PgLogSink`]
//! and its batch writer ([`spawn_writer`]), which store what handlers write;
//! and the retention pruner ([`spawn_pruner`]). `runtime::spawn_with_logs`
//! starts both beside the worker. See
//! docs/en/specs/2026-09-27-workflow-w11-execution-logs-design.md §3, §4.3,
//! §6 and §7.

mod prune;
mod repo;
mod run_log;
mod writer;

use chrono::{DateTime, Utc};
use std::time::Duration;
use uuid::Uuid;

pub use prune::{spawn_pruner, PruneConfig};
pub use repo::{
    InsertError, LogFilter, LogQuery, LogRepo, PurgeQuery, StoredLine, ERROR_LOGS_PAGE,
};
pub use run_log::{mask, RunIdentity, RunLog, MASK};
pub use writer::{spawn_writer, Barrier, Inserter, PgLogSink, WriterConfig};

/// The longest piece a log line is stored as, in `char`s (D6).
///
/// Kestra's `RunContextLogger.MAX_MESSAGE_LENGTH` (`1024 * 15`), split with
/// `Splitter.fixedLength`: measured in W11 Task 1, a 100,000-character line
/// was stored as 6 x 15,360 + 7,840. A constant rather than a setting, because
/// it is Kestra's shape and not a policy. [`RunLog`] splits every line longer
/// than this before its cap is consulted. egret counts Unicode scalar values
/// (Rust `char`) where Java counts UTF-16 code units; the two agree on the
/// Basic Multilingual Plane (spec section 5).
pub const LOG_PIECE_CHARS: usize = 15_360;

/// The per-attempt log cap (D4): once an attempt has stored this many lines or
/// bytes, [`RunLog`] stops storing and [`RunLog::finish`] writes one WARN
/// summary instead. Per attempt, so a retry starts fresh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LogLimits {
    pub run_max_lines: u64,
    pub run_max_bytes: u64,
}

impl Default for LogLimits {
    /// 10,000 lines / 10 MiB, verbatim from the spec.
    fn default() -> Self {
        Self {
            run_max_lines: 10_000,
            run_max_bytes: 10_485_760,
        }
    }
}

/// How old a row's `inserted_at` must be before [`LogRepo::page`] serves it
/// -- or any row with a higher id.
///
/// `bigserial` ids are allocated at insert and become visible at commit, so
/// two writers (two replicas; one writer is sequential) can commit id 101
/// before id 100, and a statement that started later can hold a lower id than
/// one that started earlier. A reader that returned 101 and moved its cursor
/// past it would never see 100. So `page` is a **watermark**: it stops before
/// the first row after the cursor that is younger than this, and never serves
/// a higher id past it.
///
/// **The bound: a write transaction must last less than half the lag (1 s).**
/// `inserted_at` is `now()`, the transaction's START. A row that is still
/// uncommitted is invisible to the watermark; it can hold an id below a
/// served row only if its transaction started before that row's transaction
/// ended, so it commits within two write-durations of the served row's start
/// -- which must fall inside the lag. A log batch is one short `INSERT`
/// (spec §6). The price is that a line shows up about this long after it is
/// written.
pub const SETTLE_LAG: Duration = Duration::from_secs(2);

/// A log line's level: slf4j's five, which are `Log.java`'s `level:` and
/// Kestra's stored `level`. Stored as `smallint` 0..=4 (the table's CHECK), so
/// `min_level` is a plain `>=`.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
    utoipa::ToSchema,
)]
#[serde(rename_all = "UPPERCASE")]
pub enum LogLevel {
    Trace = 0,
    Debug = 1,
    Info = 2,
    Warn = 3,
    Error = 4,
}

impl LogLevel {
    /// The five names, compared **case-sensitively** as Jackson compares an
    /// enum. `None` for anything else.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "TRACE" => LogLevel::Trace,
            "DEBUG" => LogLevel::Debug,
            "INFO" => LogLevel::Info,
            "WARN" => LogLevel::Warn,
            "ERROR" => LogLevel::Error,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Trace => "TRACE",
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
    }

    /// The stored `smallint`.
    pub fn as_i16(self) -> i16 {
        self as i16
    }

    /// The inverse of [`as_i16`](Self::as_i16); `None` outside 0..=4, which
    /// the table's CHECK makes unreachable for a stored row.
    pub fn from_i16(n: i16) -> Option<Self> {
        Some(match n {
            0 => LogLevel::Trace,
            1 => LogLevel::Debug,
            2 => LogLevel::Info,
            3 => LogLevel::Warn,
            4 => LogLevel::Error,
            _ => return None,
        })
    }
}

/// One line to store. `task_run_id`, `task_id` and `attempt` are all `None`
/// for a line about the execution rather than one run. `attempt` is 0-based,
/// as Kestra's is. `ts` is when the line was produced (the worker's clock);
/// the database stamps `inserted_at` itself.
#[derive(Clone, Debug, PartialEq)]
pub struct LogLine {
    pub workspace_id: Uuid,
    pub execution_id: Uuid,
    pub task_run_id: Option<Uuid>,
    pub task_id: Option<String>,
    pub attempt: Option<i32>,
    pub level: LogLevel,
    pub ts: DateTime<Utc>,
    pub message: String,
}

/// W11 Task 1's measured fixture, for the tests that assert its typed
/// `egret_expected` values rather than a reading of its prose.
/// Package 7d D7: the most lines `GET …/executions/{id}/logs/download`
/// writes (the frontend's former client-side cap).
pub const LOG_DOWNLOAD_MAX_LINES: i64 = 100_000;
/// How many lines the download reads per statement (plan S11): it streams,
/// so 100 000 lines of up to 15 360 characters are never held at once.
pub const LOG_DOWNLOAD_BATCH: i64 = 2_000;

/// The download's last line when there were more lines than the cap.
pub fn truncated_line() -> String {
    format!("… truncated at {LOG_DOWNLOAD_MAX_LINES} lines")
}

/// One line of the download (D7; Kestra `LogEntry.toPrettyString`,
/// `LogEntry.java:136-138`): the instant in RFC 3339 UTC with
/// milliseconds, the level, the message. Lines are joined with `\n` and
/// the file has no trailing newline, as Kestra's.
pub fn download_line(line: &StoredLine) -> String {
    format!(
        "{} {} {}",
        line.ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        line.level.as_str(),
        line.message
    )
}

#[cfg(test)]
pub(crate) mod fixture {
    /// The whole of `oracle-exec/w11-logs.json`.
    pub fn w11() -> serde_json::Value {
        serde_json::from_str(include_str!("../../oracle-exec/w11-logs.json"))
            .expect("w11-logs.json parses")
    }

    /// `findings[id = id].egret_expected`.
    pub fn expected(id: &str) -> serde_json::Value {
        w11()["findings"]
            .as_array()
            .expect("findings is an array")
            .iter()
            .find(|f| f["id"] == id)
            .unwrap_or_else(|| panic!("no finding {id:?} in w11-logs.json"))["egret_expected"]
            .clone()
    }

    /// A level name from the fixture, parsed: a name outside the five is a
    /// broken fixture, not a test input.
    pub fn level(v: &serde_json::Value) -> super::LogLevel {
        let s = v
            .as_str()
            .unwrap_or_else(|| panic!("level {v} is not a string"));
        super::LogLevel::parse(s).unwrap_or_else(|| panic!("level {s:?} is not one of the five"))
    }

    /// Every `level_table` value is a member of `level_set`.
    #[test]
    fn every_level_table_value_is_a_member_of_the_level_set() {
        let w = w11();
        let set: Vec<&str> = w["level_set"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(set, ["TRACE", "DEBUG", "INFO", "WARN", "ERROR"]);
        let table = w["level_table"].as_object().unwrap();
        assert!(!table.is_empty());
        for (k, v) in table {
            let v = v.as_str().unwrap();
            assert!(
                set.contains(&v),
                "level_table.{k} = {v:?} is not in level_set"
            );
            level(&serde_json::json!(v));
        }
    }
}

#[cfg(test)]
mod download_tests {
    use super::*;
    use chrono::TimeZone;

    fn line(level: LogLevel, ms: u32, message: &str) -> StoredLine {
        StoredLine {
            id: 1,
            task_run_id: None,
            task_id: None,
            attempt: None,
            level,
            ts: chrono::Utc
                .with_ymd_and_hms(2026, 11, 16, 10, 0, 0)
                .unwrap()
                + chrono::Duration::milliseconds(i64::from(ms)),
            message: message.to_string(),
        }
    }

    #[test]
    fn a_download_line_is_kestras_pretty_string_with_millis() {
        assert_eq!(
            download_line(&line(LogLevel::Info, 123, "hello")),
            "2026-11-16T10:00:00.123Z INFO hello"
        );
        // Whole seconds keep the three digits; a multi-line message is kept as it is.
        assert_eq!(
            download_line(&line(LogLevel::Error, 0, "a\nb")),
            "2026-11-16T10:00:00.000Z ERROR a\nb"
        );
    }

    #[test]
    fn the_truncation_line_names_the_cap() {
        assert_eq!(truncated_line(), "… truncated at 100000 lines");
    }
}
