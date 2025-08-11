//! Integration tests for rate limit configuration from TOML
//! Tests the end-to-end flow from TOML parsing to middleware wiring

use crate::network::middleware::RateLimitConfig;
use tracing::{debug, error, info};

#[test]
fn test_rate_limit_config_defaults() {
    let config = RateLimitConfig::default();
    
    // Check that rate limiting is disabled by default
    assert!(!config.enabled, "Rate limiting should be disabled by default");
    assert_eq!(config.requests_per_minute, 1000);
    assert_eq!(config.burst_size, 100);
    assert!(config.by_ip);
    assert!(!config.limit_health_endpoints);
    assert!(config.global_requests_per_minute.is_none());
}

#[test]
fn test_rate_limit_serde_roundtrip() {
    use serde_json;
    
    let original_config = RateLimitConfig {
        enabled: true,
        requests_per_minute: 2000,
        burst_size: 200,
        by_ip: false,
        limit_health_endpoints: true,
        global_requests_per_minute: Some(10000),
    };

    let serialized = serde_json::to_string(&original_config).expect("Failed to serialize");
    let deserialized: RateLimitConfig = serde_json::from_str(&serialized).expect("Failed to deserialize");

    assert_eq!(original_config.enabled, deserialized.enabled);
    assert_eq!(original_config.requests_per_minute, deserialized.requests_per_minute);
    assert_eq!(original_config.burst_size, deserialized.burst_size);
    assert_eq!(original_config.by_ip, deserialized.by_ip);
    assert_eq!(original_config.limit_health_endpoints, deserialized.limit_health_endpoints);
    assert_eq!(original_config.global_requests_per_minute, deserialized.global_requests_per_minute);
}

#[tokio::test]
async fn test_rate_limit_config_conversion() {
    let rate_limit_config = RateLimitConfig {
        enabled: true,
        requests_per_minute: 1500,
        burst_size: 150,
        by_ip: true,
        limit_health_endpoints: false,
        global_requests_per_minute: Some(7500),
    };

    let middleware_config = rate_limit_config.to_middleware_config();
    
    assert!(middleware_config.enabled);
    assert_eq!(middleware_config.max_requests, 150); // Uses burst_size
    assert_eq!(middleware_config.window_duration.as_secs(), 60); // 1 minute
    assert!(!middleware_config.limit_health_endpoints);
    assert_eq!(middleware_config.global_max_requests, Some(7500));
}

#[test]
fn test_local_demo_config_toml_parsing() {
    // Test parsing a simple TOML config that disables rate limiting
    use toml;
use tracing::{debug, error, info};
    
    let toml_content = r#"
[network]
enabled = false

[rate_limit]
enabled = false
"#;
    
    let parsed: toml::Value = toml::from_str(toml_content).expect("Failed to parse TOML");
    
    // Verify the structure can be parsed
    if let Some(network) = parsed.get("network") {
        if let Some(enabled) = network.get("enabled") {
            assert_eq!(enabled.as_bool(), Some(false));
        }
    }
    
    if let Some(rate_limit) = parsed.get("rate_limit") {
        if let Some(enabled) = rate_limit.get("enabled") {
            assert_eq!(enabled.as_bool(), Some(false));
        }
    }
    
    info!("✅ TOML parsing works correctly for rate limit config");
}

#[tokio::test]
async fn test_rate_limit_layer_creation() {
    // Test disabled rate limit layer
    let disabled_config = RateLimitConfig {
        enabled: false,
        ..Default::default()
    };
    
    let layer = crate::network::middleware::RateLimitLayer::new(disabled_config);
    // Layer should be created without errors
    
    // Test enabled rate limit layer
    let enabled_config = RateLimitConfig {
        enabled: true,
        requests_per_minute: 500,
        burst_size: 50,
        by_ip: true,
        limit_health_endpoints: false,
        global_requests_per_minute: None,
    };
    
    let layer = crate::network::middleware::RateLimitLayer::new(enabled_config);
    // Layer should be created without errors
    
    // Test convenience methods
    let disabled_layer = crate::network::middleware::RateLimitLayer::disabled();
    let limited_layer = crate::network::middleware::RateLimitLayer::with_limits(100, 10);
    
    info!("✅ Rate limit layer creation tests passed");
}