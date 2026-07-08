//! Notebook HTTP error — delegates to the shared `egret_core::Error` mapping.

use axum::response::{IntoResponse, Response};
use egret_core::Error;

#[derive(Debug)]
pub struct NotebookApiError(pub Error);

impl From<Error> for NotebookApiError {
    fn from(e: Error) -> Self {
        Self(e)
    }
}

impl From<sqlx::Error> for NotebookApiError {
    fn from(e: sqlx::Error) -> Self {
        Self(Error::Internal(anyhow::anyhow!(e)))
    }
}

impl IntoResponse for NotebookApiError {
    fn into_response(self) -> Response {
        self.0.into_response()
    }
}
