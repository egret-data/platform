//! BI layer: SQL Lab (saved queries + schema introspection) and dataset persistence.

mod dataset;
mod error;
pub mod model;
mod routes;
mod saved;
mod schema;
mod typegen;

pub use dataset::{Dataset, DatasetColumn, DatasetRepo, NewColumn, SourceSpec};
pub use error::{BiApiError, BiError};
pub use routes::{dataset_router, sqllab_router, BiState};
pub use saved::{SavedQuery, SavedQueryRepo};
pub use schema::SchemaService;
pub use typegen::{generic_type, GenericType};
