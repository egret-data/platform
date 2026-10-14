//! `egret-core` — shared domain primitives and traits for the Egret workspace.

pub mod authz;
pub mod context;
pub mod engine_principals;
pub mod error;
#[cfg(feature = "http")]
pub mod http;
pub mod ids;
pub mod openapi;
pub mod pagination;
pub mod ratelimit;
pub mod rbac;
pub mod tabular;
#[cfg(feature = "token")]
pub mod token;

pub use authz::{require, require_object, AuthTarget, Decision, ObjectRef, Pdp, Resource, Subject};
pub use context::{Principal, WorkspaceContext};
pub use error::{Error, Result};
#[cfg(feature = "http")]
pub use http::{ApiError, ErrorBody};
pub use ids::{GroupId, UserId, WorkspaceId};
pub use openapi::{Ack, WorkspacePath};
pub use pagination::Pagination;
pub use tabular::{ColumnMeta, TabularResult};
