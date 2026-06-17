//! Map `CatalogError` to an Iceberg REST `ErrorModel` JSON body + HTTP status.

use crate::CatalogError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use egret_iceberg_ext::error::{ErrorModel, IcebergErrorResponse};

impl IntoResponse for CatalogError {
    fn into_response(self) -> Response {
        let (status, typ) = match &self {
            CatalogError::NotFound(_) => (StatusCode::NOT_FOUND, "NoSuchObjectException"),
            CatalogError::AlreadyExists(_) => (StatusCode::CONFLICT, "AlreadyExistsException"),
            CatalogError::Conflict(_) => (StatusCode::CONFLICT, "CommitFailedException"),
            CatalogError::NamespaceNotEmpty(_) => {
                (StatusCode::CONFLICT, "NamespaceNotEmptyException")
            }
            CatalogError::BadRequest(_) => (StatusCode::BAD_REQUEST, "BadRequestException"),
            CatalogError::Db(_) | CatalogError::Storage(_) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "InternalServerError")
            }
        };
        let body = IcebergErrorResponse {
            error: ErrorModel::new(self.to_string(), typ, status.as_u16()),
        };
        (status, Json(body)).into_response()
    }
}
