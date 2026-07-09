//! Pure-serde BI wire DTOs shared across the Analytics seam.
//!
//! Moved verbatim (same fields + serde attrs) from `egret-bi::model` so the wire
//! shapes and persisted jsonb are unchanged; `egret-bi` re-exports these to keep
//! its handlers compiling. Only the pure-data TYPES live here — the domain
//! conversions (`chart_dto`, `detail_dto`, …), the `From<Row>` impls, and any DTO
//! that references an `egret-bi`-local type (e.g. `ChartDataBody`'s `QuerySpec`)
//! stay behind in `egret-bi`.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct SaveQuery {
    pub database_id: Option<Uuid>,
    pub name: String,
    pub sql: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Serialize)]
pub struct SavedQueryDto {
    pub id: String,
    pub database_id: Option<String>,
    pub name: String,
    pub sql: String,
    pub description: String,
    pub created_by: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// A single column in a DESCRIBE result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaColumn {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

#[derive(Debug, Serialize)]
pub struct FolderDto {
    pub id: String,
    pub name: String,
}
#[derive(Debug, Deserialize)]
pub struct CreateFolder {
    pub name: String,
}
#[derive(Debug, Deserialize)]
pub struct RenameFolder {
    pub name: String,
}
#[derive(Debug, Deserialize)]
pub struct SetFolder {
    pub folder_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum CreateSource {
    Physical {
        catalog: Option<String>,
        schema: String,
        table: String,
    },
    Virtual {
        sql: String,
    },
}

#[derive(Debug, Deserialize)]
pub struct CreateDataset {
    pub name: String,
    pub database_id: Uuid,
    #[serde(default)]
    pub description: String,
    pub source: CreateSource,
}

#[derive(Debug, Deserialize)]
pub struct UpdateDataset {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub sql: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DatasetColumnDto {
    pub column_name: String,
    pub data_type: String,
    pub type_generic: String,
    pub is_temporal: bool,
    pub ordinal: i32,
    pub expression: Option<String>,
    pub verbose_name: String,
    pub description: String,
    pub is_calculated: bool,
}

#[derive(Debug, Serialize)]
pub struct DatasetSummaryDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub database_id: String,
    pub kind: &'static str,
    pub catalog: Option<String>,
    pub schema: Option<String>,
    pub table: Option<String>,
    pub column_count: i64,
    pub folder_id: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
    pub bookmarked: bool,
}

#[derive(Debug, Serialize)]
pub struct DatasetDetailDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub database_id: String,
    pub kind: &'static str,
    pub catalog: Option<String>,
    pub schema: Option<String>,
    pub table: Option<String>,
    pub sql: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub columns: Vec<DatasetColumnDto>,
    pub metrics: Vec<MetricDto>,
}

#[derive(Debug, Serialize)]
pub struct MetricDto {
    pub id: String,
    pub metric_name: String,
    pub expression: String,
    pub data_type: String,
    pub type_generic: String,
    pub verbose_name: String,
    pub description: String,
    pub ordinal: i32,
}

#[derive(Debug, Deserialize)]
pub struct MetricBody {
    pub metric_name: String,
    pub expression: String,
    #[serde(default)]
    pub verbose_name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct AddColumnBody {
    pub column_name: String,
    pub expression: String,
    #[serde(default)]
    pub verbose_name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateColumnBody {
    #[serde(default)]
    pub verbose_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub expression: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct ChartDto {
    pub id: String,
    pub dataset_id: String,
    pub name: String,
    pub description: String,
    pub viz_type: String,
    pub params: serde_json::Value,
    pub created_by: Option<String>,
    pub folder_id: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
    pub bookmarked: bool,
}

#[derive(Debug, serde::Deserialize)]
pub struct CreateChart {
    pub dataset_id: uuid::Uuid,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub viz_type: String,
    pub params: serde_json::Value,
}

#[derive(Debug, serde::Deserialize)]
pub struct UpdateChart {
    pub dataset_id: uuid::Uuid,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub viz_type: String,
    pub params: serde_json::Value,
}

#[derive(Debug, serde::Serialize)]
pub struct ChartDataResponse {
    pub sql: String,
    pub database_id: String,
}

// ── Dashboard DTOs ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct DashboardSummaryDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub created_by: Option<String>,
    pub folder_id: Option<String>,
    pub card_count: i64,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub bookmarked: bool,
}

#[derive(Debug, Serialize)]
pub struct DashboardCardDto {
    pub id: String,
    pub chart_id: Option<String>,
    pub card_type: String,
    pub grid_col: i32,
    pub grid_row: i32,
    pub size_x: i32,
    pub size_y: i32,
    pub options: serde_json::Value,
    pub tab_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DashboardTabDto {
    pub id: String,
    pub name: String,
    pub position: i32,
}

#[derive(Debug, Serialize)]
pub struct DashboardDetailDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub created_by: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub width: String,
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
    pub bookmarked: bool,
    pub tabs: Vec<DashboardTabDto>,
    pub cards: Vec<DashboardCardDto>,
}

#[derive(Debug, Deserialize)]
pub struct CreateDashboard {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

/// Card as supplied by the client when saving a layout.
#[derive(Debug, Deserialize)]
pub struct CardInput {
    #[serde(default)]
    pub chart_id: Option<Uuid>,
    #[serde(default = "default_card_type")]
    pub card_type: String,
    pub grid_col: i32,
    pub grid_row: i32,
    pub size_x: i32,
    pub size_y: i32,
    #[serde(default = "empty_obj")]
    pub options: serde_json::Value,
    #[serde(default)]
    pub tab_index: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct NewTabInput {
    pub name: String,
    pub position: i32,
}

fn default_card_type() -> String {
    "chart".into()
}
fn empty_obj() -> serde_json::Value {
    serde_json::json!({})
}
fn empty_arr() -> serde_json::Value {
    serde_json::json!([])
}
fn default_width() -> String {
    "full".into()
}

#[derive(Debug, Deserialize)]
pub struct UpdateDashboard {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "empty_arr")]
    pub parameters: serde_json::Value,
    #[serde(default = "default_width")]
    pub width: String,
    #[serde(default)]
    pub tabs: Vec<NewTabInput>,
    #[serde(default)]
    pub cards: Vec<CardInput>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schema_node_serializes() {
        let c = SchemaColumn {
            name: "id".into(),
            ty: "bigint".into(),
        };
        assert_eq!(
            serde_json::to_value(&c).unwrap(),
            serde_json::json!({"name":"id","type":"bigint"})
        );
    }
}
