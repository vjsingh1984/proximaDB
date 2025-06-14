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
use proximadb::ProximaDB;
use tempfile::TempDir;
use std::path::PathBuf;

#[tokio::test]
async fn test_rest_api_integration() {
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

    // Create ProximaDB instance
    let mut db = ProximaDB::new(config).await.unwrap();
    
    // Start the database (this will start the HTTP server)
    db.start().await.unwrap();
    
    // Verify that the HTTP server is running
    assert!(db.is_http_running());
    
    // Get the HTTP address
    let address = db.http_address().unwrap();
    println!("HTTP server started on: {}", address);
    
    // Stop the database
    db.stop().await.unwrap();
    
    // Verify that the HTTP server is stopped
    assert!(!db.is_http_running());
}

#[tokio::test]
async fn test_rest_api_health_check() {
    let temp_dir = TempDir::new().unwrap();
    let data_dir = temp_dir.path().join("data");
    let wal_dir = temp_dir.path().join("wal");

    let config = Config {
        server: ServerConfig {
            node_id: "test-node".to_string(),
            bind_address: "127.0.0.1".to_string(),
            port: 8081, // Fixed port for testing
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
            grpc_port: 9091,
            rest_port: 8081,
            max_request_size_mb: 100,
            timeout_seconds: 30,
        },
        monitoring: MonitoringConfig {
            metrics_enabled: false,
            dashboard_port: 3001,
            log_level: "info".to_string(),
        },
    };

    // Create ProximaDB instance
    let mut db = ProximaDB::new(config).await.unwrap();
    
    // Start the database
    db.start().await.unwrap();
    
    // Give the server a moment to start
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    
    // Test health endpoint using reqwest
    let client = reqwest::Client::new();
    let response = client
        .get("http://127.0.0.1:8081/health")
        .send()
        .await
        .unwrap();
    
    assert_eq!(response.status(), 200);
    
    let health_response: serde_json::Value = response.json().await.unwrap();
    assert_eq!(health_response["status"], "healthy");
    assert_eq!(health_response["version"], "0.1.0");
    
    // Test readiness endpoint
    let response = client
        .get("http://127.0.0.1:8081/health/ready")
        .send()
        .await
        .unwrap();
    
    assert_eq!(response.status(), 200);
    
    // Stop the database
    db.stop().await.unwrap();
}