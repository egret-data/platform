//! Notebook domain types + request/response DTOs.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    Daft,
    SparkConnect,
}
impl Engine {
    pub fn as_str(self) -> &'static str {
        match self {
            Engine::Daft => "daft",
            Engine::SparkConnect => "spark_connect",
        }
    }
    pub fn parse(s: &str) -> Option<Engine> {
        match s {
            "daft" => Some(Engine::Daft),
            "spark_connect" => Some(Engine::SparkConnect),
            _ => None,
        }
    }
}

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

#[derive(Debug, Clone, Serialize)]
pub struct NotebookSummary {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub engine: String,
    pub is_public: bool,
    pub owner_user_id: Uuid,
    pub is_owner: bool,
    pub updated_at: chrono::DateTime<chrono::Utc>,
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
    pub engine: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateNotebook {
    pub name: Option<String>,
    pub description: Option<String>,
    pub engine: Option<String>,
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
