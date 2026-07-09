//! Engine domain types + request/response DTOs.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
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

/// Execution guards.
#[derive(Debug, Clone)]
pub struct Limits {
    pub max_rows: usize,
    pub timeout: Duration,
}

// ── Request / response DTOs ───────────────────────────────────────────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SubmitQuery {
    pub database_id: Uuid,
    pub sql: String,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SubmitAck {
    pub id: String,
    pub status: QueryStatus,
}

/// Full status view (preview inline; full result via the results endpoint).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct QueryStatusDto {
    pub id: String,
    pub database_id: String,
    pub status: QueryStatus,
    /// Column metadata `[{name,type}]`; `null` until the query finishes.
    #[schema(value_type = Option<Vec<egret_core::ColumnMeta>>)]
    pub columns: Option<serde_json::Value>,
    /// Inline preview rows (bounded, row-major); `null` until the query finishes.
    #[schema(value_type = Option<Object>)]
    pub preview: Option<serde_json::Value>,
    pub row_count: Option<i64>,
    pub bytes_scanned: Option<i64>,
    pub truncated: bool,
    pub error: Option<String>,
}

/// History list item (no result bodies).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct QueryListItem {
    pub id: String,
    pub database_id: String,
    pub status: QueryStatus,
    pub sql: String,
    pub row_count: Option<i64>,
    pub submitted_at: chrono::DateTime<chrono::Utc>,
}

/// A page of the full result.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ResultsPage {
    /// Column metadata `[{name,type}]`.
    #[schema(value_type = Vec<egret_core::ColumnMeta>)]
    pub columns: serde_json::Value,
    /// Row-major cell values; each element is one row (a JSON array).
    #[schema(value_type = Vec<Object>)]
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
    fn jsonl_row_roundtrips() {
        let row = vec![json!(1), json!("x"), json!(null)];
        let line = serde_json::to_string(&row).unwrap();
        let back: Vec<serde_json::Value> = serde_json::from_str(&line).unwrap();
        assert_eq!(back, row);
    }
}
