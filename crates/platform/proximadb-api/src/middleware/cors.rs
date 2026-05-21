//! # CORS Middleware
//!
//! Cross-Origin Resource Sharing configuration.

/// CORS configuration
#[derive(Debug, Clone)]
pub struct CorsConfig {
    pub allowed_origins: Vec<String>,
    pub allowed_methods: Vec<String>,
    pub allowed_headers: Vec<String>,
    pub allow_credentials: bool,
    pub max_age: Option<usize>,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            allowed_origins: vec!["*".to_string()],
            allowed_methods: vec![
                "GET".to_string(),
                "POST".to_string(),
                "PUT".to_string(),
                "DELETE".to_string(),
            ],
            allowed_headers: vec!["*".to_string()],
            allow_credentials: false,
            max_age: Some(86400),
        }
    }
}

/// CORS middleware
pub struct CorsMiddleware {
    #[allow(dead_code)]
    config: CorsConfig,
}

impl CorsMiddleware {
    pub fn new(config: CorsConfig) -> Self {
        Self { config }
    }
}

// TODO: Move CORS logic from src/network/middleware/cors.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_cors_config_is_permissive_without_credentials() {
        let config = CorsConfig::default();

        assert_eq!(config.allowed_origins, vec!["*".to_string()]);
        assert_eq!(
            config.allowed_methods,
            vec![
                "GET".to_string(),
                "POST".to_string(),
                "PUT".to_string(),
                "DELETE".to_string()
            ]
        );
        assert_eq!(config.allowed_headers, vec!["*".to_string()]);
        assert!(!config.allow_credentials);
        assert_eq!(config.max_age, Some(86400));
    }

    #[test]
    fn cors_middleware_preserves_supplied_policy() {
        let middleware = CorsMiddleware::new(CorsConfig {
            allowed_origins: vec!["https://example.com".to_string()],
            allowed_methods: vec!["GET".to_string()],
            allowed_headers: vec!["authorization".to_string()],
            allow_credentials: true,
            max_age: None,
        });

        assert_eq!(
            middleware.config.allowed_origins,
            vec!["https://example.com".to_string()]
        );
        assert_eq!(middleware.config.allowed_methods, vec!["GET".to_string()]);
        assert_eq!(
            middleware.config.allowed_headers,
            vec!["authorization".to_string()]
        );
        assert!(middleware.config.allow_credentials);
        assert_eq!(middleware.config.max_age, None);
    }
}
