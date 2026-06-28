//! Credential encryption (AES-256-GCM) and a Postgres-backed secret store.

mod crypto;
mod error;
mod store;
mod workspace_secrets;

pub use crypto::Crypto;
pub use error::SecretsError;
pub use store::SecretStore;
pub use workspace_secrets::{NewSecret, WorkspaceSecretMeta, WorkspaceSecretStore};
