//! REST API error types.
//!
//! Self-contained error enum for `proximadb-api` REST handlers.  No root-crate
//! concrete types cross this boundary — only proto types and standard Rust types.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

tokio::task_local! {
    /// Per-request correlation id, scoped by the request-id middleware around
    /// each request. Read by `RestError::into_response` so error envelopes
    /// carry the SAME id the `X-Request-ID` response header advertises, with
    /// zero changes to handler signatures.
    pub static REQUEST_ID: String;
}

/// The request id for the current task scope, if the request-id middleware set
/// one. `None` on paths not wrapped by the middleware (so we never emit a fake
/// id that wouldn't match the `X-Request-ID` header).
pub fn current_request_id() -> Option<String> {
    REQUEST_ID.try_with(|id| id.clone()).ok()
}

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
        let mut error_obj = json!({
            "type": error_type,
            "message": self.to_string(),
            "code": status.as_u16(),
        });
        if let Some(rid) = current_request_id() {
            error_obj["request_id"] = json!(rid);
        }
        (status, Json(json!({ "error": error_obj }))).into_response()
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

    #[tokio::test]
    async fn error_envelope_carries_request_id_when_scoped() {
        let (_status, body) = REQUEST_ID
            .scope("req-abc-123".to_string(), async {
                response_parts(RestError::NotFound("x".to_string())).await
            })
            .await;
        assert_eq!(body["error"]["request_id"], "req-abc-123");
        assert_eq!(body["error"]["type"], "not_found");
    }

    #[tokio::test]
    async fn error_envelope_omits_request_id_when_unscoped() {
        // No middleware scope → no fake id (so body never disagrees with the
        // X-Request-ID header).
        let (_status, body) = response_parts(RestError::Internal("boom".to_string())).await;
        assert!(body["error"].get("request_id").is_none());
    }
}
