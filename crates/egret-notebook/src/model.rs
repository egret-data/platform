//! Notebook domain types + request/response DTOs.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// `NotebookSummary` and `EngineOption` were moved verbatim to the leaf contract
// crate `egret-analytics-api` (E5c-1); re-exported here so this crate's handlers
// and stores keep referencing `crate::model::{NotebookSummary, EngineOption}`.
pub use egret_analytics_api::{EngineOption, NotebookSummary};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CellType {
    Code,
    Markdown,
    Sql,
}
impl CellType {
    pub fn as_str(self) -> &'static str {
        match self {
            CellType::Code => "code",
            CellType::Markdown => "markdown",
            CellType::Sql => "sql",
        }
    }
    pub fn parse(s: &str) -> Option<CellType> {
        match s {
            "code" => Some(CellType::Code),
            "markdown" => Some(CellType::Markdown),
            "sql" => Some(CellType::Sql),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Cell {
    pub id: Uuid,
    pub cell_type: String,
    pub source: String,
    pub cell_order: i32,
    pub execution_count: Option<i32>,
    pub last_output: Option<serde_json::Value>,
    pub last_execution_time_ms: Option<i32>,
}

/// Body for `PATCH /notebooks/{id}/folder`: move a notebook into a folder
/// (or back to Uncategorized with `folder_id: null`).
#[derive(Debug, Deserialize)]
pub struct SetFolder {
    pub folder_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NotebookDetail {
    #[serde(flatten)]
    pub summary: NotebookSummary,
    pub cells: Vec<Cell>,
}

#[derive(Debug, Deserialize)]
pub struct CreateNotebook {
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateNotebook {
    pub name: Option<String>,
    pub description: Option<String>,
    pub is_public: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct CreateCell {
    pub cell_type: String,
    pub source: Option<String>,
    pub cell_order: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateCell {
    pub cell_type: Option<String>,
    pub source: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ReorderCells {
    pub order: Vec<Uuid>,
}

// ── Notebook settings DTOs ───────────────────────────────────────────────────

/// Response DTO for GET /ws/{workspace}/notebook-settings.
#[derive(Debug, Clone, Serialize)]
pub struct NotebookSettings {
    pub engines: Vec<EngineOption>,
}

/// Request body for PUT /ws/{workspace}/notebook-settings.
#[derive(Debug, Deserialize)]
pub struct UpdateNotebookSettings {
    pub engine_ids: Vec<Uuid>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cell_type_roundtrips() {
        assert_eq!(
            CellType::parse(CellType::Code.as_str()),
            Some(CellType::Code)
        );
        assert_eq!(
            CellType::parse(CellType::Markdown.as_str()),
            Some(CellType::Markdown)
        );
        assert_eq!(CellType::parse(CellType::Sql.as_str()), Some(CellType::Sql));
        assert_eq!(CellType::parse("nope"), None);
    }

    #[test]
    fn enums_serialize_to_expected_strings() {
        assert_eq!(
            serde_json::to_value(CellType::Markdown).unwrap(),
            json!("markdown")
        );
    }
}
