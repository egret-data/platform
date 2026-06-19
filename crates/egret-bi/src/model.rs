//! SQL Lab DTOs and Dataset DTOs.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::chart::Chart;
use crate::chartquery::QuerySpec;
use crate::dataset::{Dataset, DatasetColumn, Metric};

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
    pub updated_at: chrono::DateTime<chrono::Utc>,
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

pub fn dataset_kind(ds: &Dataset) -> &'static str {
    if ds.sql.is_some() {
        "virtual"
    } else {
        "physical"
    }
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

pub fn metric_dto(m: Metric) -> MetricDto {
    MetricDto {
        id: m.id.to_string(),
        metric_name: m.metric_name,
        expression: m.expression,
        data_type: m.data_type,
        type_generic: m.type_generic,
        verbose_name: m.verbose_name,
        description: m.description,
        ordinal: m.ordinal,
    }
}

pub fn summary_dto(ds: Dataset, column_count: i64) -> DatasetSummaryDto {
    DatasetSummaryDto {
        kind: dataset_kind(&ds),
        id: ds.id.to_string(),
        name: ds.name,
        description: ds.description,
        database_id: ds.database_id.to_string(),
        catalog: ds.catalog,
        schema: ds.schema_name,
        table: ds.table_name,
        column_count,
        updated_at: ds.updated_at,
    }
}

pub fn detail_dto(
    ds: Dataset,
    columns: Vec<DatasetColumn>,
    metrics: Vec<Metric>,
) -> DatasetDetailDto {
    DatasetDetailDto {
        kind: dataset_kind(&ds),
        id: ds.id.to_string(),
        name: ds.name,
        description: ds.description,
        database_id: ds.database_id.to_string(),
        catalog: ds.catalog,
        schema: ds.schema_name,
        table: ds.table_name,
        sql: ds.sql,
        updated_at: ds.updated_at,
        columns: columns
            .into_iter()
            .map(|c| {
                let is_calculated = c.expression.is_some();
                DatasetColumnDto {
                    is_calculated,
                    column_name: c.column_name,
                    data_type: c.data_type,
                    type_generic: c.type_generic,
                    is_temporal: c.is_temporal,
                    ordinal: c.ordinal,
                    expression: c.expression,
                    verbose_name: c.verbose_name,
                    description: c.description,
                }
            })
            .collect(),
        metrics: metrics.into_iter().map(metric_dto).collect(),
    }
}

#[derive(serde::Serialize)]
pub struct ChartDto {
    pub id: String,
    pub dataset_id: String,
    pub name: String,
    pub description: String,
    pub viz_type: String,
    pub params: serde_json::Value,
    pub created_by: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub fn chart_dto(c: Chart) -> ChartDto {
    ChartDto {
        id: c.id.to_string(),
        dataset_id: c.dataset_id.to_string(),
        name: c.name,
        description: c.description,
        viz_type: c.viz_type,
        params: c.params,
        created_by: c.created_by.map(|u| u.to_string()),
        updated_at: c.updated_at,
    }
}

#[derive(serde::Deserialize)]
pub struct CreateChart {
    pub dataset_id: uuid::Uuid,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub viz_type: String,
    pub params: serde_json::Value,
}

#[derive(serde::Deserialize)]
pub struct UpdateChart {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub viz_type: String,
    pub params: serde_json::Value,
}

/// Build-only data request: produce SQL for a dataset + QuerySpec; the client submits it
/// to the existing /queries endpoint and polls.
#[derive(serde::Deserialize)]
pub struct ChartDataBody {
    pub dataset_id: uuid::Uuid,
    pub query_spec: QuerySpec,
}

#[derive(serde::Serialize)]
pub struct ChartDataResponse {
    pub sql: String,
    pub database_id: String,
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
