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

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::body::to_bytes;
    use serde_json::Value;

    async fn response_parts(error: RestError) -> (StatusCode, Value) {
        let response = error.into_response();
        let status = response.status();
        let body = to_bytes(response.into_body()).await.unwrap();
        let json = serde_json::from_slice(&body).unwrap();
        (status, json)
    }

    #[tokio::test]
    async fn rest_error_variants_map_to_status_type_message_and_code() {
        let cases = [
            (
                RestError::CollectionNotFound("c1".to_string()),
                StatusCode::NOT_FOUND,
                "collection_not_found",
                "Collection not found: c1",
            ),
            (
                RestError::InvalidArgument("bad field".to_string()),
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "Invalid argument: bad field",
            ),
            (
                RestError::Internal("boom".to_string()),
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "Internal error: boom",
            ),
            (
                RestError::NotFound("row".to_string()),
                StatusCode::NOT_FOUND,
                "not_found",
                "Not found: row",
            ),
            (
                RestError::AlreadyExists("collection".to_string()),
                StatusCode::CONFLICT,
                "already_exists",
                "Already exists: collection",
            ),
            (
                RestError::Conflict("write-write".to_string()),
                StatusCode::CONFLICT,
                "conflict",
                "Conflict: write-write",
            ),
            (
                RestError::NotImplemented("feature".to_string()),
                StatusCode::NOT_IMPLEMENTED,
                "not_implemented",
                "Not implemented: feature",
            ),
            (
                RestError::Unauthorized("missing token".to_string()),
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "Unauthorized: missing token",
            ),
            (
                RestError::ResourceExhausted("quota".to_string()),
                StatusCode::TOO_MANY_REQUESTS,
                "resource_exhausted",
                "Resource exhausted: quota",
            ),
        ];

        for (error, expected_status, expected_type, expected_message) in cases {
            let (status, body) = response_parts(error).await;
            assert_eq!(status, expected_status);
            assert_eq!(body["error"]["type"], expected_type);
            assert_eq!(body["error"]["message"], expected_message);
            assert_eq!(body["error"]["code"], expected_status.as_u16());
        }
    }

    #[test]
    fn anyhow_errors_lower_to_internal_rest_errors() {
        let error = RestError::from(anyhow::anyhow!("disk unavailable"));

        assert!(matches!(error, RestError::Internal(message) if message == "disk unavailable"));
    }
}
