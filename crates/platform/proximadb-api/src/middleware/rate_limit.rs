//! # Rate Limiting Middleware
//!
//! Request rate limiting per client/IP.

/// Rate limit configuration
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    pub requests_per_second: u64,
    pub burst_size: u64,
    pub per_ip: bool,
    pub per_api_key: bool,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            requests_per_second: 100,
            burst_size: 200,
            per_ip: true,
            per_api_key: true,
        }
    }
}

/// Rate limit middleware
pub struct RateLimitMiddleware {
    #[allow(dead_code)]
    config: RateLimitConfig,
}

impl RateLimitMiddleware {
    pub fn new(config: RateLimitConfig) -> Self {
        Self { config }
    }
}

// TODO: Move rate limiting logic from src/network/middleware/rate_limit.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_rate_limit_config_matches_api_gateway_profile() {
        let config = RateLimitConfig::default();

        assert_eq!(config.requests_per_second, 100);
        assert_eq!(config.burst_size, 200);
        assert!(config.per_ip);
        assert!(config.per_api_key);
    }

    #[test]
    fn rate_limit_middleware_preserves_supplied_config() {
        let middleware = RateLimitMiddleware::new(RateLimitConfig {
            requests_per_second: 25,
            burst_size: 50,
            per_ip: false,
            per_api_key: true,
        });

        assert_eq!(middleware.config.requests_per_second, 25);
        assert_eq!(middleware.config.burst_size, 50);
        assert!(!middleware.config.per_ip);
        assert!(middleware.config.per_api_key);
    }
}
