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
    http::{StatusCode, Request},
    middleware::Next,
    response::{Json, Response},
};
use serde::Serialize;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Rate limiting configuration
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Enable rate limiting (if false, all requests pass through)
    pub enabled: bool,
    /// Maximum requests per window
    pub max_requests: u32,
    /// Time window duration
    pub window_duration: Duration,
    /// Whether to apply rate limiting to health endpoints
    pub limit_health_endpoints: bool,
    /// Global rate limit (applies to all IPs combined)
    pub global_max_requests: Option<u32>,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_requests: 100,
            window_duration: Duration::from_secs(60), // 1 minute
            limit_health_endpoints: false,
            global_max_requests: None,
        }
    }
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
    config: RateLimitConfig,
    buckets: Arc<RwLock<HashMap<IpAddr, RateLimitBucket>>>,
    global_bucket: Arc<RwLock<RateLimitBucket>>,
}

impl RateLimitState {
    pub fn new(config: RateLimitConfig) -> Self {
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
    state: Arc<RateLimitState>,
}

impl RateLimitLayer {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            state: Arc::new(RateLimitState::new(config)),
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
    pub fn with_limits(max_requests: u32, window_duration: Duration) -> Self {
        Self::new(RateLimitConfig {
            enabled: true,
            max_requests,
            window_duration,
            limit_health_endpoints: false,
            global_max_requests: None,
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
        let bucket = buckets.entry(client_ip).or_insert_with(RateLimitBucket::new);
        
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
    if let Some(forwarded_for) = request.headers().get("X-Forwarded-For") {
        if let Ok(forwarded_str) = forwarded_for.to_str() {
            if let Some(first_ip) = forwarded_str.split(',').next() {
                if let Ok(ip) = first_ip.trim().parse::<IpAddr>() {
                    return ip;
                }
            }
        }
    }
    
    // Try X-Real-IP header
    if let Some(real_ip) = request.headers().get("X-Real-IP") {
        if let Ok(ip_str) = real_ip.to_str() {
            if let Ok(ip) = ip_str.parse::<IpAddr>() {
                return ip;
            }
        }
    }
    
    // Fall back to connection remote address
    // Note: This would need to be set by the server, for now use localhost
    "127.0.0.1".parse().unwrap()
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
        assert!(!config.enabled);
        assert_eq!(config.max_requests, 100);
        assert_eq!(config.window_duration, Duration::from_secs(60));
        assert!(!config.limit_health_endpoints);
        assert!(config.global_max_requests.is_none());
    }
}