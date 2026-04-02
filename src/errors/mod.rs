//! Unified error handling for ProximaDB APIs
//!
//! This module provides a single ApiError type that can be converted
//! to both gRPC Status and HTTP responses, ensuring consistent error
//! handling across all API protocols.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
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

    /// Forbidden - insufficient permissions
    #[error("Forbidden: {0}")]
    Forbidden(String),

    /// Operation not implemented
    #[error("Not implemented: {0}")]
    NotImplemented(String),

    /// Deadline exceeded
    #[error("Deadline exceeded: {0}")]
    DeadlineExceeded(String),

    /// Already exists
    #[error("Already exists: {0}")]
    AlreadyExists(String),

    /// Generic resource not found (for non-collection resources like prepared statements)
    #[error("Not found: {0}")]
    NotFound(String),

    /// Resource has expired or been removed (HTTP 410 Gone)
    #[error("Gone: {0}")]
    Gone(String),

    /// Vector dimension mismatch
    #[error("Dimension mismatch: expected {expected}, got {actual}")]
    DimensionMismatch {
        /// The dimension the collection expects.
        expected: usize,
        /// The dimension that was actually provided.
        actual: usize,
    },

    /// Invalid vector data
    #[error("Invalid vector: {0}")]
    InvalidVector(String),

    /// Conflict error (e.g., schema evolution violation)
    #[error("Conflict: {0}")]
    Conflict(String),

    /// Capability not supported by storage engine
    #[error("Capability not supported: {0}")]
    UnsupportedCapability(String),
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
            ApiError::Forbidden(msg) => tonic::Status::permission_denied(msg),
            ApiError::NotImplemented(msg) => tonic::Status::unimplemented(msg),
            ApiError::DeadlineExceeded(msg) => tonic::Status::deadline_exceeded(msg),
            ApiError::AlreadyExists(msg) => tonic::Status::already_exists(msg),
            ApiError::NotFound(msg) => tonic::Status::not_found(msg),
            ApiError::Gone(msg) => tonic::Status::not_found(format!("Resource expired: {}", msg)),
            ApiError::DimensionMismatch { expected, actual } => {
                tonic::Status::invalid_argument(format!(
                    "Vector dimension mismatch: expected {}, got {}",
                    expected, actual
                ))
            }
            ApiError::InvalidVector(msg) => {
                tonic::Status::invalid_argument(format!("Invalid vector: {}", msg))
            }
            ApiError::Conflict(msg) => tonic::Status::aborted(msg),
            ApiError::UnsupportedCapability(msg) => {
                tonic::Status::invalid_argument(format!(
                    "Capability not supported: {}. Please check storage engine capabilities.",
                    msg
                ))
            }
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
            ApiError::Forbidden(_) => (StatusCode::FORBIDDEN, "forbidden"),
            ApiError::NotImplemented(_) => (StatusCode::NOT_IMPLEMENTED, "not_implemented"),
            ApiError::DeadlineExceeded(_) => (StatusCode::REQUEST_TIMEOUT, "deadline_exceeded"),
            ApiError::AlreadyExists(_) => (StatusCode::CONFLICT, "already_exists"),
            ApiError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            ApiError::Gone(_) => (StatusCode::GONE, "gone"),
            ApiError::DimensionMismatch { .. } => (StatusCode::BAD_REQUEST, "dimension_mismatch"),
            ApiError::InvalidVector(_) => (StatusCode::BAD_REQUEST, "invalid_vector"),
            ApiError::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            ApiError::UnsupportedCapability(_) => (StatusCode::BAD_REQUEST, "unsupported_capability"),
        };

        // Check if this is a capability error and use enhanced formatting
        let body = if matches!(&self, ApiError::UnsupportedCapability(_)) {
            // This is a capability error - try to extract details
            let error_msg = self.to_string();
            let cap_error = CapabilityError {
                capability: "capability".to_string(),
                available_alternatives: vec![],
                message: error_msg.clone(),
                error_type: CapabilityErrorType::UnsupportedCapability,
            };

            Json(cap_error.to_rest_response()).into_response()
        } else {
            Json(json!({
                "error": {
                    "type": error_type,
                    "message": self.to_string(),
                    "code": status.as_u16()
                }
            })).into_response()
        };

        (status, body).into_response()
    }
}

/// Capability error module for protocol-aware error mapping
pub mod capability_error;

// Re-export capability error types for convenience
pub use capability_error::{CapabilityError, CapabilityErrorType};

/// Result type alias for API operations
pub type ApiResult<T> = Result<T, ApiError>;

/// Helper trait for converting various error types to ApiError
pub trait IntoApiError {
    /// Convert this value into an [`ApiError`].
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

/// Convert CapabilityCheckError to ApiError for unified error handling
impl From<crate::query::capability::CapabilityCheckError> for ApiError {
    fn from(err: crate::query::capability::CapabilityCheckError) -> Self {
        use crate::query::capability::CapabilityCheckError;

        match err {
            CapabilityCheckError::UnsupportedCapability {
                capability,
                available_alternatives,
            } => {
                let msg = if available_alternatives.is_empty() {
                    format!(
                        "The requested capability '{}' is not supported by the selected storage engine.",
                        capability
                    )
                } else {
                    format!(
                        "The requested capability '{}' is not supported. Available alternatives: {}",
                        capability,
                        available_alternatives.join(", ")
                    )
                };
                ApiError::UnsupportedCapability(msg)
            }

            CapabilityCheckError::MultipleUnsupportedCapabilities {
                missing_capabilities,
                available_alternatives,
            } => {
                let msg = if available_alternatives.is_empty() {
                    format!(
                        "Multiple capabilities are not supported: {}. Please check the storage engine capabilities.",
                        missing_capabilities.join(", ")
                    )
                } else {
                    format!(
                        "Multiple capabilities are not supported: {}. Available alternatives: {}",
                        missing_capabilities.join(", "),
                        available_alternatives.join(", ")
                    )
                };
                ApiError::UnsupportedCapability(msg)
            }
        }
    }
}

/// Convert CapabilityError to ApiError for rich error information
impl From<crate::errors::capability_error::CapabilityError> for ApiError {
    fn from(err: crate::errors::capability_error::CapabilityError) -> Self {
        ApiError::UnsupportedCapability(err.to_string())
    }
}

/// Convert ProtocolError to ApiError for unified error handling
impl From<crate::core::error::ProtocolError> for ApiError {
    fn from(err: crate::core::error::ProtocolError) -> Self {
        use crate::core::error::ProtocolError;
        match err {
            ProtocolError::InvalidArgument { msg, field } => {
                let message = if let Some(f) = field {
                    format!("{} (field: {})", msg, f)
                } else {
                    msg
                };
                ApiError::InvalidArgument(message)
            }
            ProtocolError::NotFound { resource, id } => {
                if resource.to_lowercase() == "collection" {
                    ApiError::CollectionNotFound(id)
                } else {
                    ApiError::InvalidArgument(format!("{} not found: {}", resource, id))
                }
            }
            ProtocolError::AlreadyExists { resource, id } => {
                ApiError::AlreadyExists(format!("{}: {}", resource, id))
            }
            ProtocolError::Internal { details } => ApiError::Internal(details),
            ProtocolError::PermissionDenied { action } => {
                ApiError::Unauthorized(format!("Permission denied: {}", action))
            }
            ProtocolError::Timeout {
                operation,
                duration_ms,
            } => ApiError::DeadlineExceeded(format!(
                "Operation '{}' timed out after {}ms",
                operation, duration_ms
            )),
            ProtocolError::ResourceExhausted { details } => ApiError::ResourceExhausted(details),
            ProtocolError::PreconditionFailed { details } => ApiError::InvalidArgument(details),
        }
    }
}

/// Convert canonical ProximaDBError to ApiError for unified error handling.
/// This enables any service-layer code returning ProximaDBError to be used
/// directly in API handlers via the ? operator.
impl From<crate::core::errors::ProximaDBError> for ApiError {
    fn from(err: crate::core::errors::ProximaDBError) -> Self {
        use crate::core::errors::ProximaDBError as E;
        match err {
            E::NotFound { resource_type, id } => {
                if resource_type.to_lowercase() == "collection" {
                    ApiError::CollectionNotFound(id)
                } else {
                    ApiError::NotFound(format!("{} not found: {}", resource_type, id))
                }
            }
            E::AlreadyExists { resource_type, id } => {
                ApiError::AlreadyExists(format!("{}: {}", resource_type, id))
            }
            E::InvalidInput(msg) => ApiError::InvalidArgument(msg),
            E::InvalidCacheKey(msg) => ApiError::InvalidArgument(msg),
            E::Authentication(msg) => ApiError::Unauthorized(msg),
            E::PermissionDenied(msg) => ApiError::Forbidden(msg),
            E::Timeout(secs) => ApiError::DeadlineExceeded(format!("Timed out after {}s", secs)),
            E::CapacityExceeded { message } => ApiError::ResourceExhausted(message),
            E::TransactionConflict { transaction, conflicting_with } => {
                ApiError::Conflict(format!("{} conflicts with {}", transaction, conflicting_with))
            }
            other => ApiError::Internal(other.to_string()),
        }
    }
}

/// Helper function to convert Result<T, ApiError> to Response
pub fn result_into_response<T>(result: Result<T, ApiError>) -> Response
where
    T: IntoResponse,
{
    match result {
        Ok(value) => value.into_response(),
        Err(error) => error.into_response(),
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
        let _response = err.into_response();
        // Response will have status 400 and JSON body
    }
}
