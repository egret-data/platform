//! Engine domain types + request/response DTOs.

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
///
/// Defined in the leaf contract crate, not here: [`egret_query_api::QueryExecPort`]
/// takes it on `execute_as`, and the seam crate is exactly what the port exists
/// to keep out of the Data API's dependency graph. Re-exported so every existing
/// `egret_query::execution::model::Limits` caller keeps compiling — one struct,
/// two paths, never two definitions.
pub use egret_query_api::Limits;

// ── Request / response DTOs ───────────────────────────────────────────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SubmitQuery {
    pub database_id: Uuid,
    pub sql: String,
    #[serde(default)]
    pub limit: Option<usize>,
    /// Template parameters substituted into `{{name}}` placeholders before the
    /// query is sent to the engine.
    #[serde(default)]
    pub params: Vec<QueryParam>,
}

/// A single SQL-Lab template parameter. `value` is substituted for `{{name}}`
/// as a typed, escaped SQL literal (never raw text) so parameters cannot inject.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct QueryParam {
    pub name: String,
    /// `string` (default; single-quoted + escaped), `number`, or `boolean`.
    #[serde(default = "default_param_type", rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub value: String,
}

fn default_param_type() -> String {
    "string".to_string()
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
