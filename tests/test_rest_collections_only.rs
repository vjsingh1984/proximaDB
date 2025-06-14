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

#[tokio::test]
async fn test_collections_endpoint() {
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
    
    // Start the database
    db.start().await.unwrap();
    
    // Give the server a moment to start
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    
    let client = reqwest::Client::new();
    let http_address = db.http_address().unwrap();
    let base_url = format!("http://{}", http_address);
    
    println!("Testing against: {}", base_url);
    
    // Test: List collections (should be empty initially)
    let response = client
        .get(&format!("{}/collections", base_url))
        .send()
        .await
        .unwrap();
    
    let status = response.status();
    println!("Collections endpoint status: {}", status);
    if !status.is_success() {
        let error_text = response.text().await.unwrap();
        println!("Error response: {}", error_text);
        panic!("Expected status 200, got {}", status);
    }
    assert_eq!(status, 200);
    
    let collections: Vec<serde_json::Value> = response.json().await.unwrap();
    assert_eq!(collections.len(), 0);
    
    println!("Collections test passed!");
    
    // Stop the database
    db.stop().await.unwrap();
}