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
use serde_json::json;

#[tokio::test]
async fn test_complete_rest_api_workflow() {
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
            grpc_port: 9092,
            rest_port: 0, // Let OS choose port
            max_request_size_mb: 100,
            timeout_seconds: 30,
        },
        monitoring: MonitoringConfig {
            metrics_enabled: false,
            dashboard_port: 3002,
            log_level: "info".to_string(),
        },
    };

    // Create ProximaDB instance
    let mut db = ProximaDB::new(config).await.unwrap();
    
    // Start the database
    db.start().await.unwrap();
    
    // Give the server a moment to start
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    
    let client = reqwest::Client::new();
    let http_address = db.http_address().unwrap();
    let base_url = format!("http://{}", http_address);
    
    // Test 1: Health check
    let response = client
        .get(&format!("{}/health", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    
    // Test 2: List collections (should be empty initially)
    let response = client
        .get(&format!("{}/collections", base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let collections: Vec<serde_json::Value> = response.json().await.unwrap();
    assert_eq!(collections.len(), 0);
    
    // Test 3: Create a new collection
    let create_request = json!({
        "name": "test_collection",
        "dimension": 3,
        "distance_metric": "cosine",
        "indexing_algorithm": "hnsw"
    });
    
    let response = client
        .post(&format!("{}/collections", base_url))
        .json(&create_request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    
    let collection_response: serde_json::Value = response.json().await.unwrap();
    let collection_id = collection_response["id"].as_str().unwrap();
    assert_eq!(collection_response["name"], "test_collection");
    assert_eq!(collection_response["dimension"], 3);
    
    // Test 4: Get the created collection
    let response = client
        .get(&format!("{}/collections/{}", base_url, collection_id))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    
    let collection: serde_json::Value = response.json().await.unwrap();
    assert_eq!(collection["name"], "test_collection");
    
    // Test 5: Insert a vector
    let insert_request = json!({
        "vector": [1.0, 2.0, 3.0],
        "metadata": {
            "type": "document",
            "category": "test"
        }
    });
    
    let response = client
        .post(&format!("{}/collections/{}/vectors", base_url, collection_id))
        .json(&insert_request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    
    let vector_response: serde_json::Value = response.json().await.unwrap();
    let vector_id = vector_response["id"].as_str().unwrap();
    assert_eq!(vector_response["vector"], json!([1.0, 2.0, 3.0]));
    
    // Test 6: Get the inserted vector
    let response = client
        .get(&format!("{}/collections/{}/vectors/{}", base_url, collection_id, vector_id))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    
    let vector: serde_json::Value = response.json().await.unwrap();
    assert_eq!(vector["vector"], json!([1.0, 2.0, 3.0]));
    assert_eq!(vector["metadata"]["type"], "document");
    
    // Test 7: Insert another vector for search testing
    let insert_request2 = json!({
        "vector": [1.1, 2.1, 3.1],
        "metadata": {
            "type": "document",
            "category": "similar"
        }
    });
    
    let response = client
        .post(&format!("{}/collections/{}/vectors", base_url, collection_id))
        .json(&insert_request2)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    
    // Test 8: Search for similar vectors
    let search_request = json!({
        "vector": [1.0, 2.0, 3.0],
        "k": 2
    });
    
    let response = client
        .post(&format!("{}/collections/{}/search", base_url, collection_id))
        .json(&search_request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    
    let search_results: Vec<serde_json::Value> = response.json().await.unwrap();
    // We should get back at least 1 result (exact match should have very high score)
    assert!(!search_results.is_empty());
    
    // Test 9: Search with metadata filter
    let search_request_with_filter = json!({
        "vector": [1.0, 2.0, 3.0],
        "k": 2,
        "filter": {
            "type": "document"
        }
    });
    
    let response = client
        .post(&format!("{}/collections/{}/search", base_url, collection_id))
        .json(&search_request_with_filter)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    
    let filtered_results: Vec<serde_json::Value> = response.json().await.unwrap();
    assert!(!filtered_results.is_empty());
    
    // Test 10: Delete a vector
    let response = client
        .delete(&format!("{}/collections/{}/vectors/{}", base_url, collection_id, vector_id))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204); // No Content
    
    // Test 11: Try to get the deleted vector (should be 404)
    let response = client
        .get(&format!("{}/collections/{}/vectors/{}", base_url, collection_id, vector_id))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    
    // Test 12: Get index statistics
    let response = client
        .get(&format!("{}/collections/{}/index/stats", base_url, collection_id))
        .send()
        .await
        .unwrap();
    // This might return 404 if index doesn't exist, which is okay for now
    assert!(response.status() == 200 || response.status() == 404);
    
    // Test 13: Delete the collection
    let response = client
        .delete(&format!("{}/collections/{}", base_url, collection_id))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204); // No Content
    
    // Test 14: Try to get the deleted collection (should be 404)
    let response = client
        .get(&format!("{}/collections/{}", base_url, collection_id))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    
    // Stop the database
    db.stop().await.unwrap();
}