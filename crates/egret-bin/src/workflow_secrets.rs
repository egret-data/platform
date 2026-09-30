//! W5b Task 4: the production `egret_workflow::runners::secrets::SecretSource`,
//! over `egret_secrets::WorkspaceSecretStore::resolve_typed_by_name`.
//!
//! **Why here and not in `egret-workflow`:** that crate has `egret-secrets` as
//! a dev-dependency only, and this composition root already depends on both,
//! so the adapter adds no crate dependency edge. The runtime hands it to the
//! plugin handler as an `Arc<dyn SecretSource>`.
//!
//! The adapter's whole job is the error mapping: a missing secret is
//! `NotFound` naming the secret, and every other failure is `Unavailable`
//! with `SecretsError`'s `Display`, whose variants carry no value. It logs
//! nothing.

use std::collections::BTreeMap;

use async_trait::async_trait;
use egret_secrets::{SecretsError, WorkspaceSecretStore};
use egret_workflow::runners::secrets::{SecretError, SecretSource};
use uuid::Uuid;

/// `secret('NAME')` resolved in the execution's workspace, never globally.
#[derive(Clone)]
pub struct WorkspaceSecretSource {
    store: WorkspaceSecretStore,
}

impl WorkspaceSecretSource {
    pub fn new(store: WorkspaceSecretStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl SecretSource for WorkspaceSecretSource {
    async fn get(
        &self,
        workspace: Uuid,
        name: &str,
    ) -> Result<BTreeMap<String, String>, SecretError> {
        match self.store.resolve_typed_by_name(workspace, name).await {
            Ok(typed) => Ok(typed.data),
            Err(SecretsError::NotFound) => Err(SecretError::NotFound { name: name.into() }),
            Err(e) => Err(SecretError::Unavailable {
                name: name.into(),
                reason: e.to_string(),
            }),
        }
    }
}
