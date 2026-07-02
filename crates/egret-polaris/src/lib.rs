//! Typed async HTTP client over Apache Polaris's management API.
//!
//! egret drives an external Polaris deployment as its Iceberg catalog. This
//! crate is the control-plane client: it authenticates with an admin bearer
//! token obtained from Keycloak via client-credentials (Polaris runs in MIXED
//! auth mode) and exposes typed operations over Polaris's management API.
//!
//! The mockable [`HttpTransport`] boundary and cached [`TokenSource`] back the
//! [`PolarisClient`], which exposes typed catalog / principal / role / grant
//! operations over Polaris's management API.

mod client;
mod error;
mod model;
mod token;
mod transport;

pub use client::PolarisClient;
pub use error::PolarisError;
pub use model::{
    Catalog, CatalogType, Credentials, Grant, NewCatalog, Principal, PrincipalWithCredentials,
    Role, StorageConfig,
};
pub use token::{PolarisConfig, TokenSource};
pub use transport::{HttpTransport, ReqwestTransport};
