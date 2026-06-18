//! BI layer: SQL Lab (saved queries + schema introspection). Dataset/chart/dashboard come later.

mod error;
pub mod model;
mod routes;
mod saved;
mod schema;

pub use error::{BiApiError, BiError};
