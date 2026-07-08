//! Kernel error types — mirrors `egret_notebook::error`.

use axum::response::{IntoResponse, Response};
use egret_core::Error;

/// Domain-level kernel error.
#[derive(Debug, thiserror::Error)]
pub enum KernelError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("spawn failed: {0}")]
    Spawn(String),
    #[error("internal: {0}")]
    Internal(#[from] anyhow::Error),
    #[error("gateway error: {0}")]
    Gateway(String),
}

/// HTTP-layer wrapper that delegates to the shared `egret_core::Error` mapping.
#[derive(Debug)]
pub struct KernelApiError(pub Error);

impl From<Error> for KernelApiError {
    fn from(e: Error) -> Self {
        Self(e)
    }
}

impl From<sqlx::Error> for KernelApiError {
    fn from(e: sqlx::Error) -> Self {
        Self(Error::Internal(anyhow::anyhow!(e)))
    }
}

impl From<KernelError> for KernelApiError {
    fn from(e: KernelError) -> Self {
        Self(Error::Internal(anyhow::anyhow!(e)))
    }
}

impl IntoResponse for KernelApiError {
    fn into_response(self) -> Response {
        self.0.into_response()
    }
}
