//! BI layer: SQL Lab (saved queries + schema introspection) and dataset persistence.

pub mod chart;
pub mod chartquery;
mod dashboard;
mod dataset;
mod error;
mod expr;
pub mod folder;
pub mod model;
mod routes;
mod saved;
mod schema;
pub mod shares;
mod typegen;

pub use chart::{Chart, ChartRepo};
pub use dashboard::{Dashboard, DashboardCard, DashboardRepo, DashboardTab, NewCard, NewTab};
pub use dataset::{Dataset, DatasetColumn, DatasetRepo, Metric, NewColumn, SourceSpec};
pub use error::{BiApiError, BiError};
pub use expr::{validate_expression, DescribedExpr};
pub use routes::{
    chart_router, dashboard_router, dataset_router, folder_router, sqllab_router, BiState,
};
pub use shares::shares_router;

/// The BI seam's OpenAPI document (E0·P4b-6/-7): the dataset, chart, folder,
/// sqllab, and dashboard routers merged into one `OpenApi`. `egret-server` calls
/// this and merges it into the top-level API doc — the per-crate `openapi()`
/// convention every annotated seam exposes.
pub fn openapi() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi as _;
    let mut doc = routes::BiDatasetApi::openapi();
    doc.merge(routes::BiChartApi::openapi());
    doc.merge(routes::BiFolderApi::openapi());
    doc.merge(routes::BiSqlLabApi::openapi());
    doc.merge(routes::BiDashboardApi::openapi());
    doc.merge(shares::BiSharesApi::openapi());
    doc
}
pub use saved::{SavedQuery, SavedQueryRepo};
pub use schema::SchemaService;
pub use typegen::{generic_type, GenericType};
