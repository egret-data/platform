//! BI layer: SQL Lab (saved queries + schema introspection) and dataset persistence.

mod chart;
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
pub use saved::{SavedQuery, SavedQueryRepo};
pub use schema::SchemaService;
pub use typegen::{generic_type, GenericType};
