//! Credential encryption (AES-256-GCM) and a Postgres-backed secret store.

mod crypto;
mod error;
pub mod secret_type;
mod store;
mod workspace_secrets;

pub use crypto::Crypto;
pub use error::SecretsError;
pub use store::{SecretStore, TypedSecret};
pub use workspace_secrets::{NewSecret, WorkspaceSecretMeta, WorkspaceSecretStore};
