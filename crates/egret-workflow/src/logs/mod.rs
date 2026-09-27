//! W11: the durable log of an execution's task runs (`workflow.task_logs`,
//! migration 0159).
//!
//! This module holds the vocabulary -- [`LogLevel`], [`LogLine`] -- and
//! [`LogRepo`], the only code that reads or writes the table. The writer that
//! batches lines into it and the read route are built on the repository by
//! later W11 tasks. See
//! docs/en/specs/2026-09-27-workflow-w11-execution-logs-design.md §3 and §6.

mod repo;

use chrono::{DateTime, Utc};
use std::time::Duration;
use uuid::Uuid;

pub use repo::{LogQuery, LogRepo, StoredLine};

/// How old a row's `inserted_at` must be before [`LogRepo::page`] returns it.
///
/// `bigserial` ids are allocated at insert and become visible at commit, so
/// two writers can commit id 101 before id 100. A reader that returned 101
/// and moved its cursor past it would never see 100. Holding every row back
/// until it is this old lets the lower id commit first, provided a write
/// transaction lasts less than the lag: `inserted_at` is `now()`, the
/// transaction's START, so a transaction open longer than this could still
/// commit a row the reader has already moved past. A log batch is one short
/// `INSERT` (spec §6), so the bound is generous. The price is that a line
/// shows up about this long after it is written.
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
