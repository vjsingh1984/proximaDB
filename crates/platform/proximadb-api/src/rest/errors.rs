//! REST API error types.
//!
//! Self-contained error enum for `proximadb-api` REST handlers.  No root-crate
//! concrete types cross this boundary — only proto types and standard Rust types.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Unified error type for REST API handlers in `proximadb-api`.
#[derive(Debug, thiserror::Error)]
pub enum RestError {
    #[error("Collection not found: {0}")]
    CollectionNotFound(String),

    #[error("Invalid argument: {0}")]
    InvalidArgument(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Already exists: {0}")]
    AlreadyExists(String),

    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("Not implemented: {0}")]
    NotImplemented(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Resource exhausted: {0}")]
    ResourceExhausted(String),
}

/// Result alias for REST handler functions.
pub type RestResult<T> = Result<T, RestError>;

impl IntoResponse for RestError {
    fn into_response(self) -> Response {
        let (status, error_type) = match &self {
            RestError::CollectionNotFound(_) => (StatusCode::NOT_FOUND, "collection_not_found"),
            RestError::InvalidArgument(_) => (StatusCode::BAD_REQUEST, "invalid_argument"),
            RestError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
            RestError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            RestError::AlreadyExists(_) => (StatusCode::CONFLICT, "already_exists"),
            RestError::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            RestError::NotImplemented(_) => (StatusCode::NOT_IMPLEMENTED, "not_implemented"),
            RestError::Unauthorized(_) => (StatusCode::UNAUTHORIZED, "unauthorized"),
            RestError::ResourceExhausted(_) => {
                (StatusCode::TOO_MANY_REQUESTS, "resource_exhausted")
            }
        };
        (
            status,
            Json(json!({
                "error": {
                    "type": error_type,
                    "message": self.to_string(),
                    "code": status.as_u16()
                }
            })),
        )
            .into_response()
    }
}

impl From<anyhow::Error> for RestError {
    fn from(e: anyhow::Error) -> Self {
        RestError::Internal(e.to_string())
    }
}
