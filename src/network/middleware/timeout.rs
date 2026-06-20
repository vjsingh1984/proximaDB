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

//! Request timeout middleware for ProximaDB.
//!
//! This module provides configurable request timeout enforcement to protect
//! against slow-client attacks and ensure predictable resource usage.
//!
//! # Security Design
//!
//! Request timeouts are essential for:
//! - **DoS Protection**: Prevents slow-client attacks
//! - **Resource Management**: Ensures connections are freed
//! - **Predictability**: Guarantees bounded response times
//!
//! # Configuration
//!
//! ```rust,ignore
//! use proximadb::network::middleware::timeout::TimeoutConfig;
//!
//! // Default: 30 second timeout
//! let config = TimeoutConfig::default();
//!
//! // Custom timeout for long-running operations
//! let config = TimeoutConfig {
//!     enabled: true,
//!     request_timeout_secs: 120, // 2 minutes
//!     ..Default::default()
//! };
//! ```

use serde::{Deserialize, Serialize};
use std::time::Duration;
use axum::http::StatusCode;
use tower_http::timeout::TimeoutLayer;

/// Request timeout configuration.
///
/// Controls how long requests can run before being terminated.
/// This is a critical security control to prevent resource exhaustion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeoutConfig {
    /// Enable request timeout enforcement.
    /// When false, requests can run indefinitely (NOT recommended).
    pub enabled: bool,

    /// Maximum duration for a request in seconds.
    /// Default: 30 seconds. For bulk operations, consider 120-300 seconds.
    pub request_timeout_secs: u64,

    /// Timeout for streaming responses (per-chunk timeout).
    /// Default: 60 seconds. Set higher for slow clients.
    pub streaming_timeout_secs: u64,

    /// Timeout specifically for health check endpoints.
    /// Default: 5 seconds. Health checks should be fast.
    pub health_timeout_secs: u64,
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            request_timeout_secs: 30,
            streaming_timeout_secs: 60,
            health_timeout_secs: 5,
        }
    }
}

impl TimeoutConfig {
    /// Create a timeout configuration for high-throughput scenarios.
    ///
    /// Uses shorter timeouts to free resources quickly.
    pub fn high_throughput() -> Self {
        Self {
            enabled: true,
            request_timeout_secs: 10,
            streaming_timeout_secs: 30,
            health_timeout_secs: 2,
        }
    }

    /// Create a timeout configuration for batch/bulk operations.
    ///
    /// Uses longer timeouts to accommodate large data transfers.
    pub fn batch_operations() -> Self {
        Self {
            enabled: true,
            request_timeout_secs: 300,   // 5 minutes
            streaming_timeout_secs: 600, // 10 minutes
            health_timeout_secs: 5,
        }
    }

    /// Get the request timeout as a Duration.
    pub fn request_timeout(&self) -> Duration {
        Duration::from_secs(self.request_timeout_secs)
    }

    /// Get the streaming timeout as a Duration.
    pub fn streaming_timeout(&self) -> Duration {
        Duration::from_secs(self.streaming_timeout_secs)
    }

    /// Get the health check timeout as a Duration.
    pub fn health_timeout(&self) -> Duration {
        Duration::from_secs(self.health_timeout_secs)
    }
}

/// Create a tower-http TimeoutLayer from the configuration.
///
/// Returns None if timeouts are disabled (not recommended).
pub fn create_timeout_layer(config: &TimeoutConfig) -> Option<TimeoutLayer> {
    if !config.enabled {
        tracing::warn!(
            "Request timeout is DISABLED. This is a security risk - \
             slow clients can hold connections indefinitely."
        );
        return None;
    }

    let timeout = Duration::from_secs(config.request_timeout_secs);
    tracing::info!(
        "Request timeout configured: {} seconds",
        config.request_timeout_secs
    );

    Some(TimeoutLayer::with_status_code(StatusCode::REQUEST_TIMEOUT, timeout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = TimeoutConfig::default();
        assert!(config.enabled);
        assert_eq!(config.request_timeout_secs, 30);
        assert_eq!(config.streaming_timeout_secs, 60);
        assert_eq!(config.health_timeout_secs, 5);
    }

    #[test]
    fn test_high_throughput_config() {
        let config = TimeoutConfig::high_throughput();
        assert!(config.enabled);
        assert_eq!(config.request_timeout_secs, 10);
    }

    #[test]
    fn test_batch_operations_config() {
        let config = TimeoutConfig::batch_operations();
        assert!(config.enabled);
        assert_eq!(config.request_timeout_secs, 300);
    }

    #[test]
    fn test_timeout_durations() {
        let config = TimeoutConfig::default();
        assert_eq!(config.request_timeout(), Duration::from_secs(30));
        assert_eq!(config.streaming_timeout(), Duration::from_secs(60));
        assert_eq!(config.health_timeout(), Duration::from_secs(5));
    }

    #[test]
    fn test_create_timeout_layer_enabled() {
        let config = TimeoutConfig::default();
        let layer = create_timeout_layer(&config);
        assert!(layer.is_some());
    }

    #[test]
    fn test_create_timeout_layer_disabled() {
        let config = TimeoutConfig {
            enabled: false,
            ..Default::default()
        };
        let layer = create_timeout_layer(&config);
        assert!(layer.is_none());
    }
}
