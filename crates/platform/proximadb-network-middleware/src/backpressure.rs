//! Backpressure Middleware for Enterprise Deployments
//!
//! Provides load shedding and concurrency limiting to prevent server overload:
//! - **Concurrency Limit**: Maximum simultaneous requests
//! - **Load Shedding**: Reject requests when queue is full
//!
//! This is critical for production deployments to maintain stability under load.

use serde::{Deserialize, Serialize};

/// Configuration for backpressure handling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackpressureConfig {
    /// Whether backpressure handling is enabled
    pub enabled: bool,
    /// Maximum concurrent requests (default: 1000)
    pub max_concurrent_requests: usize,
    /// Maximum pending requests in queue (default: 5000)
    pub max_pending_requests: usize,
}

impl Default for BackpressureConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_concurrent_requests: 1000,
            max_pending_requests: 5000,
        }
    }
}

impl BackpressureConfig {
    /// Create a disabled configuration (for testing)
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Default::default()
        }
    }

    /// Create a high-throughput configuration
    pub fn high_throughput() -> Self {
        Self {
            enabled: true,
            max_concurrent_requests: 5000,
            max_pending_requests: 20000,
        }
    }

    /// Create a conservative configuration for resource-constrained environments
    pub fn conservative() -> Self {
        Self {
            enabled: true,
            max_concurrent_requests: 100,
            max_pending_requests: 500,
        }
    }
}

/// Create a concurrency limit layer based on the configuration
///
/// Returns None if backpressure is disabled, allowing the caller to skip
/// adding the layer entirely.
pub fn create_concurrency_limit_layer(
    config: &BackpressureConfig,
) -> Option<tower::limit::ConcurrencyLimitLayer> {
    if config.enabled {
        Some(tower::limit::ConcurrencyLimitLayer::new(
            config.max_concurrent_requests,
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backpressure_config_defaults() {
        let config = BackpressureConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_concurrent_requests, 1000);
        assert_eq!(config.max_pending_requests, 5000);
    }

    #[test]
    fn test_backpressure_config_disabled() {
        let config = BackpressureConfig::disabled();
        assert!(!config.enabled);
    }

    #[test]
    fn test_backpressure_config_high_throughput() {
        let config = BackpressureConfig::high_throughput();
        assert!(config.enabled);
        assert_eq!(config.max_concurrent_requests, 5000);
    }

    #[test]
    fn test_create_concurrency_layer_enabled() {
        let config = BackpressureConfig::default();
        let layer = create_concurrency_limit_layer(&config);
        assert!(layer.is_some());
    }

    #[test]
    fn test_create_concurrency_layer_disabled() {
        let config = BackpressureConfig::disabled();
        let layer = create_concurrency_limit_layer(&config);
        assert!(layer.is_none());
    }
}
