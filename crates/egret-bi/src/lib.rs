//! BI layer: SQL Lab (saved queries + schema introspection) and dataset persistence.

mod chart;
mod chartquery;
mod dataset;
mod error;
mod expr;
pub mod model;
mod routes;
mod saved;
mod schema;
mod typegen;

pub use chart::{Chart, ChartRepo};
pub use chartquery::{
    build as build_chart_query, Filter, FilterOp, OrderBy, QuerySpec, MAX_CHART_ROWS,
};
pub use dataset::{Dataset, DatasetColumn, DatasetRepo, Metric, NewColumn, SourceSpec};
pub use error::{BiApiError, BiError};
pub use expr::{validate_expression, DescribedExpr};
pub use routes::{chart_router, dataset_router, sqllab_router, BiState};
pub use saved::{SavedQuery, SavedQueryRepo};
pub use schema::SchemaService;
pub use typegen::{generic_type, GenericType};
