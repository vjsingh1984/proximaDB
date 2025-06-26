//! Unit tests for network configuration components

#[cfg(test)]
mod tests {
    use crate::network::{
        NetworkConfig, AuthConfig, RateLimitConfig
    };
    use serde_json;

    #[tokio::test]
    async fn test_network_config_default() {
        let config = NetworkConfig::default();
        
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
        assert!(config.auth.enabled);
        assert_eq!(config.auth.jwt_secret, Some("secret_key".to_string()));
        assert_eq!(config.rate_limit.requests_per_minute, 2000);
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
    async fn test_rate_limit_config_default() {
        let config = RateLimitConfig::default();
        
        assert!(config.enabled);
        assert_eq!(config.requests_per_minute, 1000);
        assert_eq!(config.burst_size, 100);
        assert!(config.by_ip);
    }

    #[tokio::test]
    async fn test_network_config_serialization() {
        let config = NetworkConfig::default();
        
        let serialized = serde_json::to_string(&config);
        assert!(serialized.is_ok());
        
        let json_str = serialized.unwrap();
        assert!(json_str.contains("0.0.0.0"));
        assert!(json_str.contains("5678"));
        assert!(json_str.contains("enable_grpc"));
        
        let deserialized: Result<NetworkConfig, _> = serde_json::from_str(&json_str);
        assert!(deserialized.is_ok());
        
        let restored_config = deserialized.unwrap();
        assert_eq!(config.bind_address, restored_config.bind_address);
        assert_eq!(config.port, restored_config.port);
    }

    #[tokio::test]
    async fn test_auth_config_with_api_keys() {
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
    async fn test_network_config_clone() {
        let config = NetworkConfig::default();
        let cloned_config = config.clone();
        
        assert_eq!(config.bind_address, cloned_config.bind_address);
        assert_eq!(config.port, cloned_config.port);
        assert_eq!(config.enable_grpc, cloned_config.enable_grpc);
        assert_eq!(config.auth.enabled, cloned_config.auth.enabled);
    }

    #[tokio::test]
    async fn test_network_config_debug_format() {
        let config = NetworkConfig::default();
        let debug_str = format!("{:?}", config);
        
        assert!(debug_str.contains("NetworkConfig"));
        assert!(debug_str.contains("bind_address"));
        assert!(debug_str.contains("port"));
        assert!(debug_str.contains("enable_grpc"));
    }
}