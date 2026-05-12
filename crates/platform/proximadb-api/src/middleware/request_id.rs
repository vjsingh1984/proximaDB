//! # Request ID Middleware
//!
//! Unique request identifier generation and propagation.

use uuid::Uuid;

/// Request ID middleware
pub struct RequestIdMiddleware {
    header_name: String,
}

impl Default for RequestIdMiddleware {
    fn default() -> Self {
        Self {
            header_name: "x-request-id".to_string(),
        }
    }
}

impl RequestIdMiddleware {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_header_name(mut self, name: String) -> Self {
        self.header_name = name;
        self
    }

    pub fn generate(&self) -> String {
        Uuid::new_v4().to_string()
    }
}

// TODO: Move request ID logic from src/network/middleware/request_id.rs
