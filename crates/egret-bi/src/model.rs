//! BI domain→wire conversions, `From<Row>` impls, and re-exports of the wire
//! DTOs (which now live in `egret-analytics-api::bi`).
//!
//! The pure-serde wire DTOs now live in the leaf contract crate
//! `egret-analytics-api` (module `bi`) and are re-exported below so every
//! `crate::model::…` reference across this crate keeps resolving unchanged. What
//! stays here are the pieces that reference `egret-bi`-local domain types and so
//! cannot move to the leaf: the `*_dto` domain→wire conversions, `validate_layout`,
//! the `From<CardInput>/From<NewTabInput>` conversions into the local `NewCard`/
//! `NewTab` rows, and `ChartDataBody` (its `QuerySpec` field is `egret-bi`-local).

use crate::chart::Chart;
use crate::chartquery::QuerySpec;
use crate::dashboard::{Dashboard, DashboardCard, NewCard};
use crate::dataset::{Dataset, DatasetColumn, Metric};

pub use egret_analytics_api::bi::{
    AddColumnBody, CardInput, ChartDataResponse, ChartDto, ChartRunResponse, CreateChart,
    CreateDashboard, CreateDataset, CreateFolder, CreateSource, DashboardCardDto,
    DashboardDetailDto, DashboardSummaryDto, DashboardTabDto, DatasetColumnDto, DatasetDetailDto,
    DatasetSummaryDto, FolderDto, MetricBody, MetricDto, NewTabInput, RenameFolder, SaveQuery,
    SavedQueryDto, SchemaColumn, SearchHit, SetFolder, UpdateChart, UpdateColumnBody,
    UpdateDashboard, UpdateDataset,
};

pub fn folder_dto(f: crate::folder::Folder) -> FolderDto {
    FolderDto {
        id: f.id.to_string(),
        name: f.name,
    }
}

pub fn dataset_kind(ds: &Dataset) -> &'static str {
    if ds.sql.is_some() {
        "virtual"
    } else {
        "physical"
    }
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
        folder_id: ds.folder_id.map(|u| u.to_string()),
        updated_at: ds.updated_at,
        deleted_at: ds.deleted_at,
        bookmarked: false,
    }
}

pub fn summary_dto_bm(ds: Dataset, column_count: i64, bookmarked: bool) -> DatasetSummaryDto {
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
        folder_id: ds.folder_id.map(|u| u.to_string()),
        updated_at: ds.updated_at,
        deleted_at: ds.deleted_at,
        bookmarked,
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

pub fn chart_dto(c: Chart) -> ChartDto {
    ChartDto {
        id: c.id.to_string(),
        dataset_id: c.dataset_id.to_string(),
        name: c.name,
        description: c.description,
        viz_type: c.viz_type,
        params: c.params,
        created_by: c.created_by.map(|u| u.to_string()),
        folder_id: c.folder_id.map(|u| u.to_string()),
        updated_at: c.updated_at,
        deleted_at: c.deleted_at,
        bookmarked: false,
    }
}

pub fn chart_dto_bm(c: Chart, bookmarked: bool) -> ChartDto {
    ChartDto {
        id: c.id.to_string(),
        dataset_id: c.dataset_id.to_string(),
        name: c.name,
        description: c.description,
        viz_type: c.viz_type,
        params: c.params,
        created_by: c.created_by.map(|u| u.to_string()),
        folder_id: c.folder_id.map(|u| u.to_string()),
        updated_at: c.updated_at,
        deleted_at: c.deleted_at,
        bookmarked,
    }
}

/// Build-only data request: produce SQL for a dataset + QuerySpec; the client submits it
/// to the existing /queries endpoint and polls.
///
/// Stays in `egret-bi` (not the leaf contract crate): its `query_spec` field is the
/// `egret-bi`-local [`QuerySpec`] query builder, which references `egret-query-api`
/// and other in-crate types and so cannot move to the leaf.
#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct ChartDataBody {
    pub dataset_id: uuid::Uuid,
    pub query_spec: QuerySpec,
}

/// `CardInput` → local `NewCard` row. Lives here (not the leaf) because `NewCard`
/// is an `egret-bi`-local type; the orphan rule allows `impl From<ForeignDto> for
/// LocalRow` to stay in this crate.
impl From<CardInput> for NewCard {
    fn from(v: CardInput) -> Self {
        NewCard {
            chart_id: v.chart_id,
            card_type: v.card_type,
            grid_col: v.grid_col,
            grid_row: v.grid_row,
            size_x: v.size_x,
            size_y: v.size_y,
            options: v.options,
            tab_index: v.tab_index,
        }
    }
}

/// `NewTabInput` → local `NewTab` row (see [`From<CardInput> for NewCard`]).
impl From<NewTabInput> for crate::dashboard::NewTab {
    fn from(v: NewTabInput) -> Self {
        crate::dashboard::NewTab {
            name: v.name,
            position: v.position,
        }
    }
}

/// Mirror Metabase's "can't mix tabs and null-tab cards": with tabs, every card needs an
/// in-range `tab_index`; without tabs, no card may carry one.
pub fn validate_layout(tabs: &[NewTabInput], cards: &[CardInput]) -> Result<(), egret_core::Error> {
    for c in cards {
        match c.tab_index {
            Some(i) if i >= tabs.len() => {
                return Err(egret_core::Error::InvalidInput(format!(
                    "card tab_index {i} out of range (0..{})",
                    tabs.len()
                )));
            }
            None if !tabs.is_empty() => {
                return Err(egret_core::Error::InvalidInput(
                    "every card needs a tab_index when the dashboard has tabs".into(),
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

pub fn dashboard_tab_dto(t: crate::dashboard::DashboardTab) -> DashboardTabDto {
    DashboardTabDto {
        id: t.id.to_string(),
        name: t.name,
        position: t.position,
    }
}

pub fn dashboard_summary_dto(
    d: Dashboard,
    card_count: i64,
    bookmarked: bool,
) -> DashboardSummaryDto {
    DashboardSummaryDto {
        id: d.id.to_string(),
        name: d.name,
        description: d.description,
        created_by: d.created_by.map(|u| u.to_string()),
        folder_id: d.folder_id.map(|u| u.to_string()),
        card_count,
        updated_at: d.updated_at,
        bookmarked,
    }
}

pub fn dashboard_card_dto(c: DashboardCard) -> DashboardCardDto {
    DashboardCardDto {
        id: c.id.to_string(),
        chart_id: c.chart_id.map(|i| i.to_string()),
        card_type: c.card_type,
        grid_col: c.grid_col,
        grid_row: c.grid_row,
        size_x: c.size_x,
        size_y: c.size_y,
        options: c.options,
        tab_id: c.tab_id.map(|i| i.to_string()),
    }
}

pub fn dashboard_detail_dto(
    d: Dashboard,
    tabs: Vec<crate::dashboard::DashboardTab>,
    cards: Vec<DashboardCard>,
    bookmarked: bool,
) -> DashboardDetailDto {
    DashboardDetailDto {
        id: d.id.to_string(),
        name: d.name,
        description: d.description,
        parameters: d.parameters,
        created_by: d.created_by.map(|u| u.to_string()),
        updated_at: d.updated_at,
        width: d.width,
        deleted_at: d.deleted_at,
        bookmarked,
        tabs: tabs.into_iter().map(dashboard_tab_dto).collect(),
        cards: cards.into_iter().map(dashboard_card_dto).collect(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn validate_layout_rejects_mixed_and_out_of_range() {
        use super::{validate_layout, CardInput, NewTabInput};
        let tab = || NewTabInput {
            name: "T".into(),
            position: 0,
        };
        let card = |ti: Option<usize>| CardInput {
            chart_id: None,
            card_type: "chart".into(),
            grid_col: 0,
            grid_row: 0,
            size_x: 6,
            size_y: 4,
            options: serde_json::json!({}),
            tab_index: ti,
        };
        assert!(validate_layout(&[tab()], &[card(None)]).is_err()); // tabs but card has no index
        assert!(validate_layout(&[], &[card(Some(0))]).is_err()); // no tabs but card has index
        assert!(validate_layout(&[tab()], &[card(Some(1))]).is_err()); // out of range
        assert!(validate_layout(&[tab()], &[card(Some(0))]).is_ok()); // valid with tabs
        assert!(validate_layout(&[], &[card(None)]).is_ok()); // valid tabless
    }
}
