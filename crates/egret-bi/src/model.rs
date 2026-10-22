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
    AddColumnBody, CardInput, ChartDataResponse, ChartDto, ChartRunResponse, ConfirmQuery,
    CreateChart, CreateDashboard, CreateDataset, CreateFolder, CreateSource, DashboardCardDto,
    DashboardDetailDto, DashboardRevisionDto, DashboardSummaryDto, DashboardTabDto,
    DatasetColumnDto, DatasetDetailDto, DatasetRefreshDto, DatasetSummaryDto, DatasetUsageDto,
    DuplicateDataset, FolderDto, MetricBody, MetricDto, NewTabInput, RenameFolder, SaveQuery,
    SavedQueryDto, SchemaColumn, SearchHit, SetDashboardCertification, SetDatasetCertification,
    SetFolder, SnippetBody, SnippetDto, UpdateChart, UpdateColumnBody, UpdateDashboard,
    UpdateDataset,
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
        format: m.format,
        certified: m.certified,
        certified_by: m.certified_by,
        certification_details: m.certification_details,
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
        certified: ds.certified,
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
        certified: ds.certified,
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
                    groupby: c.groupby,
                    filterable: c.filterable,
                    format: c.format,
                    semantic_type: c.semantic_type,
                }
            })
            .collect(),
        metrics: metrics.into_iter().map(metric_dto).collect(),
        certified: ds.certified,
        certified_by: ds.certified_by,
        certification_details: ds.certification_details,
        deleted_at: ds.deleted_at,
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
            id: v.id,
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
            id: v.id,
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
    can_manage: bool,
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
        certified: d.certified,
        can_manage,
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

/// `names` is `(created_by_name, updated_by_name)` from `DashboardRepo::attribution_names` —
/// passed in rather than fetched here so this stays a pure domain→wire conversion.
pub fn dashboard_detail_dto(
    d: Dashboard,
    tabs: Vec<crate::dashboard::DashboardTab>,
    cards: Vec<DashboardCard>,
    bookmarked: bool,
    names: (Option<String>, Option<String>),
    can_manage: bool,
) -> DashboardDetailDto {
    DashboardDetailDto {
        id: d.id.to_string(),
        name: d.name,
        description: d.description,
        parameters: d.parameters,
        created_by: d.created_by.map(|u| u.to_string()),
        created_by_name: names.0,
        created_at: d.created_at,
        updated_by: d.updated_by.map(|u| u.to_string()),
        updated_by_name: names.1,
        updated_at: d.updated_at,
        folder_id: d.folder_id.map(|u| u.to_string()),
        width: d.width,
        certified: d.certified,
        certified_by: d.certified_by,
        certification_details: d.certification_details,
        can_manage,
        deleted_at: d.deleted_at,
        bookmarked,
        tabs: tabs.into_iter().map(dashboard_tab_dto).collect(),
        cards: cards.into_iter().map(dashboard_card_dto).collect(),
    }
}

pub fn dashboard_revision_dto(
    r: crate::dashboard::DashboardRevision,
    name: Option<String>,
    is_current: bool,
) -> DashboardRevisionDto {
    DashboardRevisionDto {
        id: r.id.to_string(),
        kind: r.kind,
        created_at: r.created_at,
        created_by: r.created_by.map(|u| u.to_string()),
        created_by_name: name,
        is_current,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn validate_layout_rejects_mixed_and_out_of_range() {
        use super::{validate_layout, CardInput, NewTabInput};
        let tab = || NewTabInput {
            id: None,
            name: "T".into(),
            position: 0,
        };
        let card = |ti: Option<usize>| CardInput {
            id: None,
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
