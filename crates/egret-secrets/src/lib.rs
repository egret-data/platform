//! Credential encryption (AES-256-GCM) and a Postgres-backed secret store.

mod crypto;
mod error;
mod store;

pub use crypto::Crypto;
pub use error::SecretsError;
