//! Unified error handling for ProximaDB APIs
//! 
//! This module provides a single ApiError type that can be converted
//! to both gRPC Status and HTTP responses, ensuring consistent error
//! handling across all API protocols.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// Unified API error type for consistent error handling across REST and gRPC
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// Collection not found
    #[error("Collection not found: {0}")]
    CollectionNotFound(String),
    
    /// Invalid argument provided
    #[error("Invalid argument: {0}")]
    InvalidArgument(String),
    
    /// Internal server error
    #[error("Internal error: {0}")]
    Internal(String),
    
    /// Resource exhausted (rate limiting, memory, etc.)
    #[error("Resource exhausted: {0}")]
    ResourceExhausted(String),
    
    /// Unauthorized access
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    
    /// Operation not implemented
    #[error("Not implemented: {0}")]
    NotImplemented(String),
    
    /// Deadline exceeded
    #[error("Deadline exceeded: {0}")]
    DeadlineExceeded(String),
    
    /// Already exists
    #[error("Already exists: {0}")]
    AlreadyExists(String),
    
    /// Vector dimension mismatch
    #[error("Dimension mismatch: expected {expected}, got {actual}")]
    DimensionMismatch { expected: usize, actual: usize },
    
    /// Invalid vector data
    #[error("Invalid vector: {0}")]
    InvalidVector(String),
}

impl ApiError {
    /// Convert from anyhow::Error
    pub fn from_anyhow(err: anyhow::Error) -> Self {
        ApiError::Internal(err.to_string())
    }
}

/// Convert ApiError to gRPC Status
impl From<ApiError> for tonic::Status {
    fn from(err: ApiError) -> Self {
        match err {
            ApiError::CollectionNotFound(msg) => tonic::Status::not_found(msg),
            ApiError::InvalidArgument(msg) => tonic::Status::invalid_argument(msg),
            ApiError::Internal(msg) => tonic::Status::internal(msg),
            ApiError::ResourceExhausted(msg) => tonic::Status::resource_exhausted(msg),
            ApiError::Unauthorized(msg) => tonic::Status::unauthenticated(msg),
            ApiError::NotImplemented(msg) => tonic::Status::unimplemented(msg),
            ApiError::DeadlineExceeded(msg) => tonic::Status::deadline_exceeded(msg),
            ApiError::AlreadyExists(msg) => tonic::Status::already_exists(msg),
            ApiError::DimensionMismatch { expected, actual } => {
                tonic::Status::invalid_argument(format!(
                    "Vector dimension mismatch: expected {}, got {}",
                    expected, actual
                ))
            }
            ApiError::InvalidVector(msg) => tonic::Status::invalid_argument(format!("Invalid vector: {}", msg)),
        }
    }
}

/// Convert ApiError to HTTP Response
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error_type) = match &self {
            ApiError::CollectionNotFound(_) => (StatusCode::NOT_FOUND, "collection_not_found"),
            ApiError::InvalidArgument(_) => (StatusCode::BAD_REQUEST, "invalid_argument"),
            ApiError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
            ApiError::ResourceExhausted(_) => (StatusCode::TOO_MANY_REQUESTS, "resource_exhausted"),
            ApiError::Unauthorized(_) => (StatusCode::UNAUTHORIZED, "unauthorized"),
            ApiError::NotImplemented(_) => (StatusCode::NOT_IMPLEMENTED, "not_implemented"),
            ApiError::DeadlineExceeded(_) => (StatusCode::REQUEST_TIMEOUT, "deadline_exceeded"),
            ApiError::AlreadyExists(_) => (StatusCode::CONFLICT, "already_exists"),
            ApiError::DimensionMismatch { .. } => (StatusCode::BAD_REQUEST, "dimension_mismatch"),
            ApiError::InvalidVector(_) => (StatusCode::BAD_REQUEST, "invalid_vector"),
        };
        
        let body = Json(json!({
            "error": {
                "type": error_type,
                "message": self.to_string(),
                "code": status.as_u16()
            }
        }));
        
        (status, body).into_response()
    }
}

/// Result type alias for API operations
pub type ApiResult<T> = Result<T, ApiError>;

/// Helper trait for converting various error types to ApiError
pub trait IntoApiError {
    fn into_api_error(self) -> ApiError;
}

impl IntoApiError for anyhow::Error {
    fn into_api_error(self) -> ApiError {
        ApiError::Internal(self.to_string())
    }
}

impl IntoApiError for std::io::Error {
    fn into_api_error(self) -> ApiError {
        ApiError::Internal(format!("IO error: {}", self))
    }
}

impl IntoApiError for serde_json::Error {
    fn into_api_error(self) -> ApiError {
        ApiError::InvalidArgument(format!("JSON error: {}", self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_api_error_to_status() {
        let err = ApiError::CollectionNotFound("test_collection".to_string());
        let status: tonic::Status = err.into();
        assert_eq!(status.code(), tonic::Code::NotFound);
    }
    
    #[test]
    fn test_api_error_to_response() {
        let err = ApiError::InvalidArgument("bad input".to_string());
        let response = err.into_response();
        // Response will have status 400 and JSON body
    }
}