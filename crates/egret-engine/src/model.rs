//! Engine domain types + request/response DTOs.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QueryStatus {
    Queued,
    Running,
    Finished,
    Failed,
    Cancelled,
}

impl QueryStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            QueryStatus::Queued => "queued",
            QueryStatus::Running => "running",
            QueryStatus::Finished => "finished",
            QueryStatus::Failed => "failed",
            QueryStatus::Cancelled => "cancelled",
        }
    }
    pub fn parse(s: &str) -> Option<QueryStatus> {
        Some(match s {
            "queued" => QueryStatus::Queued,
            "running" => QueryStatus::Running,
            "finished" => QueryStatus::Finished,
            "failed" => QueryStatus::Failed,
            "cancelled" => QueryStatus::Cancelled,
            _ => return None,
        })
    }
}

/// A result column: a name and an engine type label.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

/// A fully-collected query result held in memory (bounded by `max_rows`).
#[derive(Debug, Clone)]
pub struct QueryResult {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<serde_json::Value>>,
    pub row_count: u64,
    pub bytes_scanned: Option<u64>,
    pub truncated: bool,
}

/// Execution guards.
#[derive(Debug, Clone)]
pub struct Limits {
    pub max_rows: usize,
    pub timeout: Duration,
}

// ── Request / response DTOs ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SubmitQuery {
    pub database_id: Uuid,
    pub sql: String,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct SubmitAck {
    pub id: String,
    pub status: QueryStatus,
}

/// Full status view (preview inline; full result via the results endpoint).
#[derive(Debug, Serialize)]
pub struct QueryStatusDto {
    pub id: String,
    pub database_id: String,
    pub status: QueryStatus,
    pub columns: Option<serde_json::Value>,
    pub preview: Option<serde_json::Value>,
    pub row_count: Option<i64>,
    pub bytes_scanned: Option<i64>,
    pub truncated: bool,
    pub error: Option<String>,
}

/// History list item (no result bodies).
#[derive(Debug, Serialize)]
pub struct QueryListItem {
    pub id: String,
    pub database_id: String,
    pub status: QueryStatus,
    pub sql: String,
    pub row_count: Option<i64>,
    pub submitted_at: chrono::DateTime<chrono::Utc>,
}

/// A page of the full result.
#[derive(Debug, Serialize)]
pub struct ResultsPage {
    pub columns: serde_json::Value,
    pub rows: Vec<serde_json::Value>,
    pub page: usize,
    pub page_size: usize,
    pub row_count: i64,
    pub truncated: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn status_roundtrips() {
        assert_eq!(QueryStatus::Finished.as_str(), "finished");
        assert_eq!(
            QueryStatus::parse("cancelled"),
            Some(QueryStatus::Cancelled)
        );
        assert_eq!(QueryStatus::parse("nope"), None);
    }

    #[test]
    fn column_serializes_type_key() {
        let c = Column {
            name: "n".into(),
            ty: "bigint".into(),
        };
        assert_eq!(
            serde_json::to_value(&c).unwrap(),
            json!({"name":"n","type":"bigint"})
        );
    }

    #[test]
    fn jsonl_row_roundtrips() {
        let row = vec![json!(1), json!("x"), json!(null)];
        let line = serde_json::to_string(&row).unwrap();
        let back: Vec<serde_json::Value> = serde_json::from_str(&line).unwrap();
        assert_eq!(back, row);
    }
}
