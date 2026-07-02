//! Typed async HTTP client over Apache Polaris's management API.
//!
//! egret drives an external Polaris deployment as its Iceberg catalog. This
//! crate is the control-plane client: it authenticates with an admin bearer
//! token obtained from Keycloak via client-credentials (Polaris runs in MIXED
//! auth mode) and exposes typed operations over Polaris's management API.
//!
//! Task 1 scaffolds the crate: the mockable [`HttpTransport`] boundary and the
//! cached [`TokenSource`]. Catalog / principal / role / grant operations and
//! the top-level `PolarisClient` are added in later tasks.

mod error;
mod token;
mod transport;

pub use error::PolarisError;
pub use token::{PolarisConfig, TokenSource};
pub use transport::{HttpTransport, ReqwestTransport};
