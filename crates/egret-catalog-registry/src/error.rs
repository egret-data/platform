//! Registry store errors.

use crate::config::ValidationError;

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error(transparent)]
    Invalid(#[from] ValidationError),
    #[error(transparent)]
    Secrets(#[from] egret_secrets::SecretsError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    /// `ensure_warehouse_catalog`'s implicit-grant sync (SP4a Task 6 fix
    /// round 1, Important 1) failed inside the same transaction as the
    /// catalog + `catalog_workspaces` write, which is what makes this an
    /// error the whole call rolls back on rather than a partially-applied
    /// warehouse catalog with no grant.
    #[error(transparent)]
    Perm(#[from] egret_perm::PermError),
    /// The [`crate::CatalogAssignmentCompiler`] recompile that follows
    /// `ensure_warehouse_catalog`'s grant sync (SP4a Task 16, D3 defect 2),
    /// or the same port's recompile inside a warehouse teardown, failing
    /// inside the SAME transaction as the write it accompanies — rolls the
    /// whole write back, same treatment `Perm` above gets.
    #[error(transparent)]
    Compile(#[from] egret_perm::CompileError),
}
