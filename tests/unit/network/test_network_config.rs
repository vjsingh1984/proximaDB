//! Unit tests for network configuration and related components
//! 
//! This module contains unit tests for network configuration structures,
//! authentication, rate limiting, and server configurations.

use proximadb::network::{
    NetworkConfig, AuthConfig, RateLimitConfig, 
    GrpcHttpServerConfig, RestHttpServerConfig, MultiServerConfig
};
use serde_json;

#[tokio::test]
async fn test_network_config_default() {
    let config = NetworkConfig::default();
    
    // Test default values
    assert_eq!(config.bind_address, "0.0.0.0");
    assert_eq!(config.port, 5678);
    assert!(config.enable_grpc);
    assert!(config.enable_rest);
    assert!(config.enable_dashboard);
    assert!(!config.auth.enabled);
    assert!(config.rate_limit.enabled);
    assert_eq!(config.request_timeout_secs, 30);
    assert_eq!(config.max_request_size, 32 * 1024 * 1024);
    assert_eq!(config.keep_alive_timeout_secs, 60);
    assert!(config.tcp_nodelay);
}

#[tokio::test]
async fn test_network_config_custom() {
    let custom_auth = AuthConfig {
        enabled: true,
        jwt_secret: Some("secret_key".to_string()),
        jwt_expiration_secs: 7200,
        api_keys: vec!["key1".to_string(), "key2".to_string()],
    };
    
    let custom_rate_limit = RateLimitConfig {
        enabled: true,
        requests_per_minute: 2000,
        burst_size: 200,
        by_ip: false,
    };
    
    let config = NetworkConfig {
        bind_address: "127.0.0.1".to_string(),
        port: 8080,
        enable_grpc: false,
        enable_rest: true,
        enable_dashboard: false,
        auth: custom_auth.clone(),
        rate_limit: custom_rate_limit.clone(),
        request_timeout_secs: 60,
        max_request_size: 64 * 1024 * 1024,
        keep_alive_timeout_secs: 120,
        tcp_nodelay: false,
    };
    
    assert_eq!(config.bind_address, "127.0.0.1");
    assert_eq!(config.port, 8080);
    assert!(!config.enable_grpc);
    assert!(config.enable_rest);
    assert!(!config.enable_dashboard);
    assert!(config.auth.enabled);
    assert_eq!(config.auth.jwt_secret, Some("secret_key".to_string()));
    assert_eq!(config.auth.jwt_expiration_secs, 7200);
    assert_eq!(config.auth.api_keys, vec!["key1", "key2"]);
    assert!(config.rate_limit.enabled);
    assert_eq!(config.rate_limit.requests_per_minute, 2000);
    assert_eq!(config.rate_limit.burst_size, 200);
    assert!(!config.rate_limit.by_ip);
    assert_eq!(config.request_timeout_secs, 60);
    assert_eq!(config.max_request_size, 64 * 1024 * 1024);
    assert_eq!(config.keep_alive_timeout_secs, 120);
    assert!(!config.tcp_nodelay);
}

#[tokio::test]
async fn test_auth_config_default() {
    let config = AuthConfig::default();
    
    assert!(!config.enabled);
    assert_eq!(config.jwt_secret, None);
    assert_eq!(config.jwt_expiration_secs, 3600);
    assert!(config.api_keys.is_empty());
}

#[tokio::test]
async fn test_auth_config_custom() {
    let config = AuthConfig {
        enabled: true,
        jwt_secret: Some("my_jwt_secret".to_string()),
        jwt_expiration_secs: 1800,
        api_keys: vec![
            "api_key_1".to_string(),
            "api_key_2".to_string(),
            "api_key_3".to_string(),
        ],
    };
    
    assert!(config.enabled);
    assert_eq!(config.jwt_secret, Some("my_jwt_secret".to_string()));
    assert_eq!(config.jwt_expiration_secs, 1800);
    assert_eq!(config.api_keys.len(), 3);
    assert_eq!(config.api_keys[0], "api_key_1");
    assert_eq!(config.api_keys[1], "api_key_2");
    assert_eq!(config.api_keys[2], "api_key_3");
}

#[tokio::test]
async fn test_rate_limit_config_default() {
    let config = RateLimitConfig::default();
    
    assert!(config.enabled);
    assert_eq!(config.requests_per_minute, 1000);
    assert_eq!(config.burst_size, 100);
    assert!(config.by_ip);
}

#[tokio::test]
async fn test_rate_limit_config_custom() {
    let config = RateLimitConfig {
        enabled: false,
        requests_per_minute: 500,
        burst_size: 50,
        by_ip: false,
    };
    
    assert!(!config.enabled);
    assert_eq!(config.requests_per_minute, 500);
    assert_eq!(config.burst_size, 50);
    assert!(!config.by_ip);
}

#[tokio::test]
async fn test_auth_config_with_empty_api_keys() {
    let config = AuthConfig {
        enabled: true,
        jwt_secret: None,
        jwt_expiration_secs: 900,
        api_keys: Vec::new(),
    };
    
    assert!(config.enabled);
    assert_eq!(config.jwt_secret, None);
    assert_eq!(config.jwt_expiration_secs, 900);
    assert!(config.api_keys.is_empty());
}

#[tokio::test]
async fn test_network_config_serialization() {
    let config = NetworkConfig::default();
    
    // Test serialization to JSON
    let serialized = serde_json::to_string(&config);
    assert!(serialized.is_ok());
    
    let json_str = serialized.unwrap();
    assert!(json_str.contains("0.0.0.0"));
    assert!(json_str.contains("5678"));
    assert!(json_str.contains("enable_grpc"));
    assert!(json_str.contains("enable_rest"));
    
    // Test deserialization from JSON
    let deserialized: Result<NetworkConfig, _> = serde_json::from_str(&json_str);
    assert!(deserialized.is_ok());
    
    let restored_config = deserialized.unwrap();
    assert_eq!(config.bind_address, restored_config.bind_address);
    assert_eq!(config.port, restored_config.port);
    assert_eq!(config.enable_grpc, restored_config.enable_grpc);
    assert_eq!(config.enable_rest, restored_config.enable_rest);
}

#[tokio::test]
async fn test_auth_config_serialization() {
    let config = AuthConfig {
        enabled: true,
        jwt_secret: Some("test_secret".to_string()),
        jwt_expiration_secs: 7200,
        api_keys: vec!["key1".to_string(), "key2".to_string()],
    };
    
    // Test serialization
    let serialized = serde_json::to_string(&config);
    assert!(serialized.is_ok());
    
    let json_str = serialized.unwrap();
    assert!(json_str.contains("test_secret"));
    assert!(json_str.contains("7200"));
    assert!(json_str.contains("key1"));
    assert!(json_str.contains("key2"));
    
    // Test deserialization
    let deserialized: Result<AuthConfig, _> = serde_json::from_str(&json_str);
    assert!(deserialized.is_ok());
    
    let restored_config = deserialized.unwrap();
    assert_eq!(config.enabled, restored_config.enabled);
    assert_eq!(config.jwt_secret, restored_config.jwt_secret);
    assert_eq!(config.jwt_expiration_secs, restored_config.jwt_expiration_secs);
    assert_eq!(config.api_keys, restored_config.api_keys);
}

#[tokio::test]
async fn test_rate_limit_config_serialization() {
    let config = RateLimitConfig {
        enabled: true,
        requests_per_minute: 1500,
        burst_size: 150,
        by_ip: true,
    };
    
    // Test serialization
    let serialized = serde_json::to_string(&config);
    assert!(serialized.is_ok());
    
    let json_str = serialized.unwrap();
    assert!(json_str.contains("1500"));
    assert!(json_str.contains("150"));
    
    // Test deserialization
    let deserialized: Result<RateLimitConfig, _> = serde_json::from_str(&json_str);
    assert!(deserialized.is_ok());
    
    let restored_config = deserialized.unwrap();
    assert_eq!(config.enabled, restored_config.enabled);
    assert_eq!(config.requests_per_minute, restored_config.requests_per_minute);
    assert_eq!(config.burst_size, restored_config.burst_size);
    assert_eq!(config.by_ip, restored_config.by_ip);
}

#[tokio::test]
async fn test_network_config_debug_format() {
    let config = NetworkConfig::default();
    let debug_str = format!("{:?}", config);
    
    assert!(debug_str.contains("NetworkConfig"));
    assert!(debug_str.contains("bind_address"));
    assert!(debug_str.contains("port"));
    assert!(debug_str.contains("enable_grpc"));
    assert!(debug_str.contains("enable_rest"));
}

#[tokio::test]
async fn test_auth_config_debug_format() {
    let config = AuthConfig {
        enabled: true,
        jwt_secret: Some("secret".to_string()),
        jwt_expiration_secs: 3600,
        api_keys: vec!["key1".to_string()],
    };
    
    let debug_str = format!("{:?}", config);
    assert!(debug_str.contains("AuthConfig"));
    assert!(debug_str.contains("enabled"));
    assert!(debug_str.contains("jwt_secret"));
}

#[tokio::test]
async fn test_rate_limit_config_debug_format() {
    let config = RateLimitConfig::default();
    let debug_str = format!("{:?}", config);
    
    assert!(debug_str.contains("RateLimitConfig"));
    assert!(debug_str.contains("enabled"));
    assert!(debug_str.contains("requests_per_minute"));
    assert!(debug_str.contains("burst_size"));
}

#[tokio::test]
async fn test_network_config_clone() {
    let config = NetworkConfig::default();
    let cloned_config = config.clone();
    
    assert_eq!(config.bind_address, cloned_config.bind_address);
    assert_eq!(config.port, cloned_config.port);
    assert_eq!(config.enable_grpc, cloned_config.enable_grpc);
    assert_eq!(config.enable_rest, cloned_config.enable_rest);
    assert_eq!(config.auth.enabled, cloned_config.auth.enabled);
    assert_eq!(config.rate_limit.enabled, cloned_config.rate_limit.enabled);
}

#[tokio::test]
async fn test_auth_config_clone() {
    let config = AuthConfig {
        enabled: true,
        jwt_secret: Some("secret".to_string()),
        jwt_expiration_secs: 3600,
        api_keys: vec!["key1".to_string(), "key2".to_string()],
    };
    
    let cloned_config = config.clone();
    assert_eq!(config.enabled, cloned_config.enabled);
    assert_eq!(config.jwt_secret, cloned_config.jwt_secret);
    assert_eq!(config.jwt_expiration_secs, cloned_config.jwt_expiration_secs);
    assert_eq!(config.api_keys, cloned_config.api_keys);
}

#[tokio::test]
async fn test_rate_limit_config_clone() {
    let config = RateLimitConfig {
        enabled: false,
        requests_per_minute: 2000,
        burst_size: 200,
        by_ip: false,
    };
    
    let cloned_config = config.clone();
    assert_eq!(config.enabled, cloned_config.enabled);
    assert_eq!(config.requests_per_minute, cloned_config.requests_per_minute);
    assert_eq!(config.burst_size, cloned_config.burst_size);
    assert_eq!(config.by_ip, cloned_config.by_ip);
}

#[tokio::test]
async fn test_network_config_edge_cases() {
    // Test with zero values
    let config = NetworkConfig {
        bind_address: "".to_string(),
        port: 0,
        enable_grpc: false,
        enable_rest: false,
        enable_dashboard: false,
        auth: AuthConfig {
            enabled: false,
            jwt_secret: None,
            jwt_expiration_secs: 0,
            api_keys: Vec::new(),
        },
        rate_limit: RateLimitConfig {
            enabled: false,
            requests_per_minute: 0,
            burst_size: 0,
            by_ip: false,
        },
        request_timeout_secs: 0,
        max_request_size: 0,
        keep_alive_timeout_secs: 0,
        tcp_nodelay: false,
    };
    
    assert_eq!(config.bind_address, "");
    assert_eq!(config.port, 0);
    assert!(!config.enable_grpc);
    assert!(!config.enable_rest);
    assert_eq!(config.auth.jwt_expiration_secs, 0);
    assert_eq!(config.rate_limit.requests_per_minute, 0);
    assert_eq!(config.request_timeout_secs, 0);
    assert_eq!(config.max_request_size, 0);
}

#[tokio::test]
async fn test_network_config_large_values() {
    // Test with large values
    let config = NetworkConfig {
        bind_address: "192.168.1.100".to_string(),
        port: 65535,
        enable_grpc: true,
        enable_rest: true,
        enable_dashboard: true,
        auth: AuthConfig {
            enabled: true,
            jwt_secret: Some("very_long_secret_key_for_testing".to_string()),
            jwt_expiration_secs: 86400 * 365, // 1 year
            api_keys: (0..100).map(|i| format!("api_key_{}", i)).collect(),
        },
        rate_limit: RateLimitConfig {
            enabled: true,
            requests_per_minute: 1_000_000,
            burst_size: 100_000,
            by_ip: true,
        },
        request_timeout_secs: 3600,
        max_request_size: 1024 * 1024 * 1024, // 1GB
        keep_alive_timeout_secs: 7200,
        tcp_nodelay: true,
    };
    
    assert_eq!(config.port, 65535);
    assert_eq!(config.auth.jwt_expiration_secs, 86400 * 365);
    assert_eq!(config.auth.api_keys.len(), 100);
    assert_eq!(config.rate_limit.requests_per_minute, 1_000_000);
    assert_eq!(config.rate_limit.burst_size, 100_000);
    assert_eq!(config.max_request_size, 1024 * 1024 * 1024);
}