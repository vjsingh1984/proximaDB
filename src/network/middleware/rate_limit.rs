/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Rate limiting middleware for ProximaDB HTTP API

use axum::{
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::{Json, Response},
};
use serde::Serialize;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Rate limiting configuration - consolidated from network module
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RateLimitConfig {
    /// Enable rate limiting (if false, all requests pass through)
    pub enabled: bool,

    /// Maximum requests per minute (TOML-friendly)
    pub requests_per_minute: u32,

    /// Burst allowance for sudden spikes
    pub burst_size: u32,

    /// Apply rate limiting per IP address
    pub by_ip: bool,

    /// Whether to apply rate limiting to health endpoints
    pub limit_health_endpoints: bool,

    /// Global rate limit (applies to all IPs combined, optional)
    pub global_requests_per_minute: Option<u32>,
}

// Default functions for serde
#[allow(dead_code)]
fn default_requests_per_minute() -> u32 {
    1000
}
#[allow(dead_code)]
fn default_burst_size() -> u32 {
    100
}
#[allow(dead_code)]
fn default_true() -> bool {
    true
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true, // Enabled by default for security
            requests_per_minute: 1000,
            burst_size: 100,
            by_ip: true,
            limit_health_endpoints: false,
            global_requests_per_minute: None,
        }
    }
}

impl RateLimitConfig {
    /// Create a disabled rate limit configuration.
    ///
    /// **WARNING**: Only use for development/testing. Production should always
    /// have rate limiting enabled to prevent abuse.
    pub fn disabled() -> Self {
        tracing::warn!("🚨 Rate limiting is DISABLED. This is a security risk in production!");
        Self {
            enabled: false,
            ..Default::default()
        }
    }

    /// Create a production rate limit configuration with specified limits.
    pub fn production(requests_per_minute: u32, burst_size: u32) -> Self {
        Self {
            enabled: true,
            requests_per_minute,
            burst_size,
            by_ip: true,
            limit_health_endpoints: false,
            global_requests_per_minute: Some(requests_per_minute * 10), // Global limit
        }
    }

    /// Create a high-throughput rate limit configuration.
    pub fn high_throughput() -> Self {
        Self {
            enabled: true,
            requests_per_minute: 10000,
            burst_size: 1000,
            by_ip: true,
            limit_health_endpoints: false,
            global_requests_per_minute: Some(100000),
        }
    }
}

impl RateLimitConfig {
    /// Convert to the internal format used by middleware
    pub fn to_middleware_config(&self) -> MiddlewareRateLimitConfig {
        MiddlewareRateLimitConfig {
            enabled: self.enabled,
            max_requests: self.burst_size, // Use burst as max for short windows
            window_duration: Duration::from_secs(60), // 1 minute window
            limit_health_endpoints: self.limit_health_endpoints,
            global_max_requests: self.global_requests_per_minute,
        }
    }
}

/// Internal rate limiting configuration used by middleware logic
#[derive(Debug, Clone)]
pub struct MiddlewareRateLimitConfig {
    /// Enable rate limiting
    pub enabled: bool,
    /// Maximum requests per IP per window
    pub max_requests: u32,
    /// Sliding window duration for rate limit tracking
    pub window_duration: Duration,
    /// Whether to apply rate limits to health check endpoints
    pub limit_health_endpoints: bool,
    /// Global maximum requests across all clients (optional)
    pub global_max_requests: Option<u32>,
}

/// Rate limit bucket for tracking requests
#[derive(Debug, Clone)]
struct RateLimitBucket {
    count: u32,
    window_start: Instant,
}

impl RateLimitBucket {
    fn new() -> Self {
        Self {
            count: 0,
            window_start: Instant::now(),
        }
    }

    fn increment(&mut self, window_duration: Duration) -> bool {
        let now = Instant::now();

        // Reset bucket if window expired
        if now.duration_since(self.window_start) >= window_duration {
            self.count = 0;
            self.window_start = now;
        }

        self.count += 1;
        true
    }

    fn is_within_limit(&self, max_requests: u32, window_duration: Duration) -> bool {
        let now = Instant::now();

        // If window expired, we're within limit
        if now.duration_since(self.window_start) >= window_duration {
            return true;
        }

        self.count <= max_requests
    }
}

/// Rate limiting state
pub struct RateLimitState {
    config: MiddlewareRateLimitConfig,
    buckets: Arc<RwLock<HashMap<IpAddr, RateLimitBucket>>>,
    global_bucket: Arc<RwLock<RateLimitBucket>>,
}

impl RateLimitState {
    /// Create a new rate limit state with the given configuration
    pub fn new(config: MiddlewareRateLimitConfig) -> Self {
        Self {
            config,
            buckets: Arc::new(RwLock::new(HashMap::new())),
            global_bucket: Arc::new(RwLock::new(RateLimitBucket::new())),
        }
    }
}

/// Rate limit error response
#[derive(Debug, Serialize)]
pub struct RateLimitErrorResponse {
    error: String,
    message: String,
    retry_after: u64, // seconds
}

/// Rate limiting layer for Axum
pub struct RateLimitLayer {
    _state: Arc<RateLimitState>,
}

impl RateLimitLayer {
    /// Create a new rate limiting layer with the given configuration
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            _state: Arc::new(RateLimitState::new(config.to_middleware_config())),
        }
    }

    /// Create a disabled rate limiting layer (all requests pass through)
    pub fn disabled() -> Self {
        Self::new(RateLimitConfig {
            enabled: false,
            ..Default::default()
        })
    }

    /// Create a rate limiting layer with specific limits
    pub fn with_limits(requests_per_minute: u32, burst_size: u32) -> Self {
        Self::new(RateLimitConfig {
            enabled: true,
            requests_per_minute,
            burst_size,
            by_ip: true,
            limit_health_endpoints: false,
            global_requests_per_minute: None,
        })
    }
}

/// Rate limiting middleware function
pub async fn rate_limit_middleware<B>(
    State(rate_limit_state): State<Arc<RateLimitState>>,
    request: Request<B>,
    next: Next<B>,
) -> Result<Response, (StatusCode, Json<RateLimitErrorResponse>)> {
    // Skip rate limiting if disabled
    if !rate_limit_state.config.enabled {
        return Ok(next.run(request).await);
    }

    let path = request.uri().path();

    // Skip rate limiting for health endpoints (if configured)
    if !rate_limit_state.config.limit_health_endpoints && is_health_endpoint(path) {
        return Ok(next.run(request).await);
    }

    // Extract client IP
    let client_ip = get_client_ip(&request);

    // Check global rate limit first (if configured)
    if let Some(global_max) = rate_limit_state.config.global_max_requests {
        let mut global_bucket = rate_limit_state.global_bucket.write().await;
        global_bucket.increment(rate_limit_state.config.window_duration);

        if !global_bucket.is_within_limit(global_max, rate_limit_state.config.window_duration) {
            let retry_after = rate_limit_state.config.window_duration.as_secs();
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                Json(RateLimitErrorResponse {
                    error: "global_rate_limit_exceeded".to_string(),
                    message: "Global rate limit exceeded. Please try again later.".to_string(),
                    retry_after,
                }),
            ));
        }
    }

    // Check per-IP rate limit
    {
        let mut buckets = rate_limit_state.buckets.write().await;
        let bucket = buckets
            .entry(client_ip)
            .or_insert_with(RateLimitBucket::new);

        bucket.increment(rate_limit_state.config.window_duration);

        if !bucket.is_within_limit(
            rate_limit_state.config.max_requests,
            rate_limit_state.config.window_duration,
        ) {
            let retry_after = rate_limit_state.config.window_duration.as_secs();
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                Json(RateLimitErrorResponse {
                    error: "rate_limit_exceeded".to_string(),
                    message: format!(
                        "Rate limit exceeded. Maximum {} requests per {} seconds.",
                        rate_limit_state.config.max_requests,
                        rate_limit_state.config.window_duration.as_secs()
                    ),
                    retry_after,
                }),
            ));
        }
    }

    Ok(next.run(request).await)
}

/// Extract client IP from request
fn get_client_ip<B>(request: &Request<B>) -> IpAddr {
    // Try to get IP from X-Forwarded-For header first (for proxies)
    if let Some(forwarded_for) = request.headers().get("X-Forwarded-For")
        && let Ok(forwarded_str) = forwarded_for.to_str()
        && let Some(first_ip) = forwarded_str.split(',').next()
        && let Ok(ip) = first_ip.trim().parse::<IpAddr>()
    {
        return ip;
    }

    // Try X-Real-IP header
    if let Some(real_ip) = request.headers().get("X-Real-IP")
        && let Ok(ip_str) = real_ip.to_str()
        && let Ok(ip) = ip_str.parse::<IpAddr>()
    {
        return ip;
    }

    // Fall back to connection remote address
    // Note: This would need to be set by the server, for now use localhost
    "127.0.0.1"
        .parse()
        .unwrap_or_else(|_| std::net::IpAddr::from([127, 0, 0, 1]))
}

/// Check if the path is a health endpoint
fn is_health_endpoint(path: &str) -> bool {
    path.starts_with("/health")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limit_bucket() {
        let mut bucket = RateLimitBucket::new();
        let window = Duration::from_secs(60);

        // Within limit initially
        assert!(bucket.is_within_limit(10, window));

        // Increment and check
        bucket.increment(window);
        assert_eq!(bucket.count, 1);
        assert!(bucket.is_within_limit(10, window));
    }

    #[test]
    fn test_is_health_endpoint() {
        assert!(is_health_endpoint("/health"));
        assert!(is_health_endpoint("/health/ready"));
        assert!(is_health_endpoint("/health/live"));
        assert!(!is_health_endpoint("/collections"));
    }

    #[test]
    fn test_rate_limit_config_default() {
        let config = RateLimitConfig::default();
        // Default is now enabled for security
        assert!(config.enabled);
        assert_eq!(config.burst_size, 100);
        assert_eq!(config.requests_per_minute, 1000);
        assert!(!config.limit_health_endpoints);
        assert!(config.global_requests_per_minute.is_none());
    }

    #[test]
    fn test_rate_limit_config_disabled() {
        let config = RateLimitConfig::disabled();
        assert!(!config.enabled);
    }

    #[test]
    fn test_rate_limit_config_production() {
        let config = RateLimitConfig::production(5000, 500);
        assert!(config.enabled);
        assert_eq!(config.requests_per_minute, 5000);
        assert_eq!(config.burst_size, 500);
        assert!(config.global_requests_per_minute.is_some());
    }

    #[test]
    fn test_rate_limit_config_high_throughput() {
        let config = RateLimitConfig::high_throughput();
        assert!(config.enabled);
        assert_eq!(config.requests_per_minute, 10000);
        assert_eq!(config.burst_size, 1000);
    }

    // ============================================================
    // Extended rate limit tests (coverage improvement)
    // ============================================================

    #[test]
    fn test_rate_limit_config_production_global_limit() {
        let config = RateLimitConfig::production(2000, 200);
        assert_eq!(config.requests_per_minute, 2000);
        assert_eq!(config.burst_size, 200);
        assert_eq!(config.global_requests_per_minute, Some(20000));
        assert!(config.by_ip);
        assert!(!config.limit_health_endpoints);
    }

    #[test]
    fn test_to_middleware_config() {
        let config = RateLimitConfig {
            enabled: true,
            requests_per_minute: 500,
            burst_size: 50,
            by_ip: true,
            limit_health_endpoints: true,
            global_requests_per_minute: Some(5000),
        };
        let mw = config.to_middleware_config();
        assert!(mw.enabled);
        assert_eq!(mw.max_requests, 50); // burst_size
        assert_eq!(mw.window_duration, Duration::from_secs(60));
        assert!(mw.limit_health_endpoints);
        assert_eq!(mw.global_max_requests, Some(5000));
    }

    #[test]
    fn test_to_middleware_config_disabled() {
        let config = RateLimitConfig {
            enabled: false,
            ..Default::default()
        };
        let mw = config.to_middleware_config();
        assert!(!mw.enabled);
    }

    #[test]
    fn test_rate_limit_bucket_within_limit() {
        let mut bucket = RateLimitBucket::new();
        let window = Duration::from_secs(60);

        // Under limit
        for _ in 0..5 {
            bucket.increment(window);
        }
        assert!(bucket.is_within_limit(10, window));
        assert_eq!(bucket.count, 5);
    }

    #[test]
    fn test_rate_limit_bucket_exceeds_limit() {
        let mut bucket = RateLimitBucket::new();
        let window = Duration::from_secs(60);

        for _ in 0..11 {
            bucket.increment(window);
        }
        assert!(!bucket.is_within_limit(10, window));
        assert_eq!(bucket.count, 11);
    }

    #[test]
    fn test_rate_limit_bucket_exactly_at_limit() {
        let mut bucket = RateLimitBucket::new();
        let window = Duration::from_secs(60);

        for _ in 0..10 {
            bucket.increment(window);
        }
        assert!(bucket.is_within_limit(10, window));
        assert_eq!(bucket.count, 10);

        // One more pushes it over
        bucket.increment(window);
        assert!(!bucket.is_within_limit(10, window));
    }

    #[test]
    fn test_rate_limit_layer_disabled() {
        let layer = RateLimitLayer::disabled();
        // Should not panic
        let _ = layer;
    }

    #[test]
    fn test_rate_limit_layer_with_limits() {
        let layer = RateLimitLayer::with_limits(500, 50);
        let _ = layer;
    }

    #[test]
    fn test_is_health_endpoint_various_paths() {
        assert!(is_health_endpoint("/health"));
        assert!(is_health_endpoint("/health/ready"));
        assert!(is_health_endpoint("/health/live"));
        assert!(is_health_endpoint("/health/startup"));
        assert!(!is_health_endpoint("/api/v1/collections"));
        assert!(!is_health_endpoint("/metrics"));
        assert!(!is_health_endpoint("/"));
        assert!(is_health_endpoint("/healthcheck")); // starts with /health so it IS a health endpoint
    }

    #[test]
    fn test_rate_limit_state_creation() {
        let config = MiddlewareRateLimitConfig {
            enabled: true,
            max_requests: 100,
            window_duration: Duration::from_secs(60),
            limit_health_endpoints: false,
            global_max_requests: None,
        };
        let state = RateLimitState::new(config);
        assert!(state.config.enabled);
        assert_eq!(state.config.max_requests, 100);
    }

    #[test]
    fn test_rate_limit_error_response_serialization() {
        let err = RateLimitErrorResponse {
            error: "rate_limit_exceeded".to_string(),
            message: "Too many requests".to_string(),
            retry_after: 60,
        };
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("rate_limit_exceeded"));
        assert!(json.contains("60"));
    }

    #[test]
    fn test_high_throughput_global_limit() {
        let config = RateLimitConfig::high_throughput();
        assert_eq!(config.global_requests_per_minute, Some(100000));
    }
}
