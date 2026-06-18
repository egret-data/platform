//! BI layer: SQL Lab (saved queries + schema introspection). Dataset/chart/dashboard come later.

mod error;
pub mod model;
mod routes;
mod saved;
mod schema;

pub use error::{BiApiError, BiError};
pub use routes::{sqllab_router, BiState};
pub use saved::{SavedQuery, SavedQueryRepo};
pub use schema::SchemaService;
