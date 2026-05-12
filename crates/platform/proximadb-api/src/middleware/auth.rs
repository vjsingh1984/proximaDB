//! # Authentication Middleware
//!
//! JWT and token-based authentication for all protocols.

use std::sync::Arc;

/// Authentication middleware configuration
#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub jwt_secret: Option<String>,
    pub enable_jwt: bool,
    pub enable_api_key: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            jwt_secret: None,
            enable_jwt: false,
            enable_api_key: true,
        }
    }
}

/// Authentication middleware
pub struct AuthMiddleware {
    #[allow(dead_code)]
    config: Arc<AuthConfig>,
}

impl AuthMiddleware {
    pub fn new(config: AuthConfig) -> Self {
        Self {
            config: Arc::new(config),
        }
    }
}

// TODO: Move authentication logic from src/network/middleware/auth.rs
