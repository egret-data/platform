//! `egret-core` — shared domain primitives and traits for the Egret workspace.

pub mod context;
pub mod error;
#[cfg(feature = "http")]
pub mod http;
pub mod ids;
pub mod pagination;
pub mod rbac;
#[cfg(feature = "token")]
pub mod token;

pub use context::{Principal, WorkspaceContext};
pub use error::{Error, Result};
#[cfg(feature = "http")]
pub use http::ApiError;
pub use ids::{UserId, WorkspaceId};
pub use pagination::Pagination;
