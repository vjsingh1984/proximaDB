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

use proximadb::core::{Config, StorageConfig, LsmConfig, ApiConfig, ServerConfig, ConsensusConfig, MonitoringConfig};
use proximadb::network::middleware::{AuthConfig, RateLimitConfig};
use proximadb::network::middleware::auth::UserInfo;
use proximadb::ProximaDB;
use tempfile::TempDir;
use std::collections::HashMap;
use std::time::Duration;

#[tokio::test]
async fn test_authentication_middleware() {
    let temp_dir = TempDir::new().unwrap();
    let data_dir = temp_dir.path().join("data");
    let wal_dir = temp_dir.path().join("wal");

    // Create API keys for testing
    let mut api_keys = HashMap::new();
    api_keys.insert(
        "test-api-key-123".to_string(),
        UserInfo {
            user_id: "test-user".to_string(),
            tenant_id: Some("test-tenant".to_string()),
            permissions: vec!["read".to_string(), "write".to_string()],
        },
    );

    let config = Config {
        server: ServerConfig {
            node_id: "test-node".to_string(),
            bind_address: "127.0.0.1".to_string(),
            port: 0, // Let OS choose port
            data_dir: data_dir.clone(),
        },
        storage: StorageConfig {
            data_dirs: vec![data_dir.clone()],
            wal_dir: wal_dir.clone(),
            mmap_enabled: true,
            lsm_config: LsmConfig {
                memtable_size_mb: 10,
                level_count: 7,
                compaction_threshold: 4,
                block_size_kb: 64,
            },
            cache_size_mb: 10,
            bloom_filter_bits: 10,
        },
        consensus: ConsensusConfig {
            cluster_peers: vec![],
            election_timeout_ms: 5000,
            heartbeat_interval_ms: 1000,
            snapshot_threshold: 1000,
        },
        api: ApiConfig {
            grpc_port: 0,
            rest_port: 0, // Let OS choose port
            max_request_size_mb: 100,
            timeout_seconds: 30,
        },
        monitoring: MonitoringConfig {
            metrics_enabled: false,
            dashboard_port: 0,
            log_level: "info".to_string(),
        },
    };

    // Create ProximaDB instance with authentication enabled
    let mut db = ProximaDB::new(config).await.unwrap();
    
    // Enable authentication in the HTTP server
    db.configure_auth(AuthConfig {
        enabled: true,
        api_keys: api_keys.clone(),
        require_auth_for_health: false,
    });
    
    // Start the database
    db.start().await.unwrap();
    
    // Give the server a moment to start
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    
    let client = reqwest::Client::new();
    let http_address = db.http_address().unwrap();
    let base_url = format!("http://{}", http_address);
    
    println!("Testing authentication against: {}", base_url);
    
    // Test 1: Health endpoint should work without authentication (when require_auth_for_health is false)
    let response = client
        .get(&format!("{}/health", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    
    // Test 2: Collections endpoint should require authentication
    let response = client
        .get(&format!("{}/collections", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401); // Unauthorized
    
    // Test 3: Valid API key should allow access
    let response = client
        .get(&format!("{}/collections", base_url))
        .header("Authorization", "Bearer test-api-key-123")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200); // Should work with valid API key
    
    // Test 4: Invalid API key should be rejected
    let response = client
        .get(&format!("{}/collections", base_url))
        .header("Authorization", "Bearer invalid-key")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401); // Unauthorized
    
    println!("Authentication middleware test passed!");
    
    // Stop the database
    db.stop().await.unwrap();
}

#[tokio::test]
async fn test_rate_limiting_middleware() {
    let temp_dir = TempDir::new().unwrap();
    let data_dir = temp_dir.path().join("data");
    let wal_dir = temp_dir.path().join("wal");

    let config = Config {
        server: ServerConfig {
            node_id: "test-node".to_string(),
            bind_address: "127.0.0.1".to_string(),
            port: 0, // Let OS choose port
            data_dir: data_dir.clone(),
        },
        storage: StorageConfig {
            data_dirs: vec![data_dir.clone()],
            wal_dir: wal_dir.clone(),
            mmap_enabled: true,
            lsm_config: LsmConfig {
                memtable_size_mb: 10,
                level_count: 7,
                compaction_threshold: 4,
                block_size_kb: 64,
            },
            cache_size_mb: 10,
            bloom_filter_bits: 10,
        },
        consensus: ConsensusConfig {
            cluster_peers: vec![],
            election_timeout_ms: 5000,
            heartbeat_interval_ms: 1000,
            snapshot_threshold: 1000,
        },
        api: ApiConfig {
            grpc_port: 0,
            rest_port: 0, // Let OS choose port
            max_request_size_mb: 100,
            timeout_seconds: 30,
        },
        monitoring: MonitoringConfig {
            metrics_enabled: false,
            dashboard_port: 0,
            log_level: "info".to_string(),
        },
    };

    // Create ProximaDB instance with rate limiting enabled
    let mut db = ProximaDB::new(config).await.unwrap();
    
    // Enable rate limiting in the HTTP server (very low limit for testing)
    db.configure_rate_limiting(RateLimitConfig {
        enabled: true,
        max_requests: 3, // Very low limit for testing
        window_duration: Duration::from_secs(60),
        limit_health_endpoints: false,
        global_max_requests: None,
    });
    
    // Start the database
    db.start().await.unwrap();
    
    // Give the server a moment to start
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    
    let client = reqwest::Client::new();
    let http_address = db.http_address().unwrap();
    let base_url = format!("http://{}", http_address);
    
    println!("Testing rate limiting against: {}", base_url);
    
    // Test 1: Health endpoint should work without rate limiting (when limit_health_endpoints is false)
    let response = client
        .get(&format!("{}/health", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    
    // Test 2: First few requests to collections should work
    for i in 1..=3 {
        let response = client
            .get(&format!("{}/collections", base_url))
            .send()
            .await
            .unwrap();
        println!("Request {}: {}", i, response.status());
        assert_eq!(response.status(), 200); // Should work within limit
    }
    
    // Test 3: Request beyond limit should be rate limited
    let response = client
        .get(&format!("{}/collections", base_url))
        .send()
        .await
        .unwrap();
    println!("Rate limited request: {}", response.status());
    assert_eq!(response.status(), 429); // Too Many Requests
    
    println!("Rate limiting middleware test passed!");
    
    // Stop the database
    db.stop().await.unwrap();
}