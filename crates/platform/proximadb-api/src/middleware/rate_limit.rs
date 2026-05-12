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
