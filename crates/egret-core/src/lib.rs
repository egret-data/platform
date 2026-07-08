//! `egret-core` — shared domain primitives and traits for the Egret workspace.

pub mod context;
pub mod error;
pub mod ids;
pub mod pagination;
pub mod rbac;

pub use context::{Principal, WorkspaceContext};
pub use error::{Error, Result};
pub use ids::{UserId, WorkspaceId};
pub use pagination::Pagination;
