//! Service-related error types

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Service operation errors
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum ServiceError {
    /// Service is currently unavailable
    #[error("Service not available: {service}")]
    NotAvailable {
        /// Name of the unavailable service
        service: String,
    },

    /// Service call exceeded its timeout
    #[error("Service timeout: {service} after {timeout_ms}ms")]
    Timeout {
        /// Name of the service that timed out
        service: String,
        /// Elapsed time in milliseconds
        timeout_ms: u64,
    },

    /// Authentication credentials are invalid
    #[error("Authentication failed: {reason}")]
    AuthenticationFailed {
        /// Reason the authentication failed
        reason: String,
    },

    /// Caller is not authorized for the operation
    #[error("Authorization failed: {operation} not allowed")]
    AuthorizationFailed {
        /// The operation that was denied
        operation: String,
    },

    /// Rate limit has been exceeded
    #[error("Rate limit exceeded: {requests} requests in {window_ms}ms")]
    RateLimitExceeded {
        /// Number of requests that triggered the limit
        requests: u32,
        /// Time window in milliseconds
        window_ms: u64,
    },

    /// Request payload or parameters are invalid
    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    /// Unexpected internal service error
    #[error("Internal server error: {0}")]
    InternalError(String),

    /// Service misconfiguration
    #[error("Configuration error: {0}")]
    Configuration(String),
}
