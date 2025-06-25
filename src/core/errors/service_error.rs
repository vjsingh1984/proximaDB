//! Service-related error types

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Service operation errors
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum ServiceError {
    #[error("Service not available: {service}")]
    NotAvailable { service: String },
    
    #[error("Service timeout: {service} after {timeout_ms}ms")]
    Timeout { service: String, timeout_ms: u64 },
    
    #[error("Authentication failed: {reason}")]
    AuthenticationFailed { reason: String },
    
    #[error("Authorization failed: {operation} not allowed")]
    AuthorizationFailed { operation: String },
    
    #[error("Rate limit exceeded: {requests} requests in {window_ms}ms")]
    RateLimitExceeded { requests: u32, window_ms: u64 },
    
    #[error("Invalid request: {0}")]
    InvalidRequest(String),
    
    #[error("Internal server error: {0}")]
    InternalError(String),
    
    #[error("Configuration error: {0}")]
    Configuration(String),
}