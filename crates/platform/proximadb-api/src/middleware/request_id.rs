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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_request_id_middleware_uses_standard_header() {
        let middleware = RequestIdMiddleware::new();

        assert_eq!(middleware.header_name, "x-request-id");
    }

    #[test]
    fn request_id_middleware_allows_custom_header_name() {
        let middleware =
            RequestIdMiddleware::new().with_header_name("x-correlation-id".to_string());

        assert_eq!(middleware.header_name, "x-correlation-id");
    }

    #[test]
    fn generated_request_ids_are_valid_v4_uuids_and_unique() {
        let middleware = RequestIdMiddleware::new();

        let first = middleware.generate();
        let second = middleware.generate();

        assert_ne!(first, second);
        assert_eq!(Uuid::parse_str(&first).unwrap().get_version_num(), 4);
        assert_eq!(Uuid::parse_str(&second).unwrap().get_version_num(), 4);
    }
}
