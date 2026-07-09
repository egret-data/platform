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
mod typegen;

pub use chart::{Chart, ChartRepo};
pub use dashboard::{Dashboard, DashboardCard, DashboardRepo, DashboardTab, NewCard, NewTab};
pub use dataset::{Dataset, DatasetColumn, DatasetRepo, Metric, NewColumn, SourceSpec};
pub use error::{BiApiError, BiError};
pub use expr::{validate_expression, DescribedExpr};
pub use routes::{
    chart_router, dashboard_router, dataset_router, folder_router, sqllab_router, BiState,
};

/// The BI seam's OpenAPI document (E0·P4b-6): the dataset, chart, and folder
/// routers merged into one `OpenApi`. `egret-server` calls this and merges it
/// into the top-level API doc — the per-crate `openapi()` convention every
/// annotated seam exposes. The sqllab + dashboard routers are annotated in a
/// later batch; their doc structs get added to this merge list then.
pub fn openapi() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi as _;
    let mut doc = routes::BiDatasetApi::openapi();
    doc.merge(routes::BiChartApi::openapi());
    doc.merge(routes::BiFolderApi::openapi());
    doc
}
pub use saved::{SavedQuery, SavedQueryRepo};
pub use schema::SchemaService;
pub use typegen::{generic_type, GenericType};
