//! Integration tests for end-to-end upsert workflows
//!
//! Tests the complete upsert pipeline across:
//! - REST API → UnifiedAvroService → WAL → Memtable → Search
//! - gRPC API → UnifiedAvroService → WAL → Memtable → Search  
//! - Multiple storage engines (LSM, VIPER, WAL)
//! - Flush and compaction scenarios
//! - Cross-tier search deduplication

use std::collections::HashMap;
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use serde_json::json;

use proximadb::core::VectorRecord;
use proximadb::services::collection_service::CollectionService;
use proximadb::services::unified_avro_service::UnifiedAvroService;
use proximadb::storage::persistence::wal::WalManager;
use proximadb::proto::proximadb::{CollectionConfig, DistanceMetric, StorageEngine, IndexingAlgorithm};

// Mock setup functions (these would need to be implemented based on your test infrastructure)

async fn setup_test_environment() -> (Arc<UnifiedAvroService>, Arc<CollectionService>) {
    // This is a mock setup - you'll need to implement based on your test infrastructure
    todo!("Implement test environment setup")
}

async fn create_test_collection(
    collection_service: &CollectionService,
    name: &str,
    storage_engine: StorageEngine,
) -> String {
    let config = CollectionConfig {
        name: name.to_string(),
        dimension: 3,
        distance_metric: DistanceMetric::Cosine as i32,
        storage_engine: storage_engine as i32,
        indexing_algorithm: IndexingAlgorithm::Hnsw as i32,
        filterable_metadata_fields: vec![],
        indexing_config: HashMap::new(),
        filterable_columns: vec![],
    };
    
    collection_service.create_collection_from_grpc(&config).await.unwrap();
    name.to_string()
}

/// Test 1: Basic upsert workflow without ID conflicts
#[tokio::test]
async fn test_basic_upsert_workflow() {
    let (unified_service, collection_service) = setup_test_environment().await;
    let collection_id = create_test_collection(&collection_service, "test_basic_upsert", StorageEngine::Wal).await;
    
    // Create test vector records
    let vector_records = vec![
        VectorRecord {
            id: "user_1".to_string(),
            collection_id: collection_id.clone(),
            vector: vec![0.1, 0.2, 0.3],
            metadata: {
                let mut meta = HashMap::new();
                meta.insert("category".to_string(), json!("user"));
                meta.insert("active".to_string(), json!(true));
                meta
            },
            timestamp: chrono::Utc::now().timestamp_millis(),
            created_at: chrono::Utc::now().timestamp_millis(),
            updated_at: chrono::Utc::now().timestamp_millis(),
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        },
        VectorRecord {
            id: "user_2".to_string(),
            collection_id: collection_id.clone(),
            vector: vec![0.4, 0.5, 0.6],
            metadata: {
                let mut meta = HashMap::new();
                meta.insert("category".to_string(), json!("user"));
                meta.insert("active".to_string(), json!(false));
                meta
            },
            timestamp: chrono::Utc::now().timestamp_millis(),
            created_at: chrono::Utc::now().timestamp_millis(),
            updated_at: chrono::Utc::now().timestamp_millis(),
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        },
    ];
    
    // Serialize to Avro batch
    let avro_data = proximadb::storage::persistence::wal::schema::serialize_vector_batch(&vector_records).unwrap();
    
    // Insert via UnifiedAvroService
    let result = unified_service.handle_vector_insert(&collection_id, false, &avro_data).await;
    assert!(result.is_ok(), "Initial insert should succeed");
    
    // Search for the vectors
    let search_query = json!({
        "collection_id": collection_id,
        "vector": [0.1, 0.2, 0.3],
        "k": 10,
        "filters": {}
    });
    let search_payload = serde_json::to_vec(&search_query).unwrap();
    
    let search_result = unified_service.search_vectors_polymorphic(&search_payload).await;
    assert!(search_result.is_ok(), "Search should succeed");
    
    let search_response: serde_json::Value = serde_json::from_slice(&search_result.unwrap()).unwrap();
    let results = search_response["results"].as_array().unwrap();
    assert_eq!(results.len(), 2, "Should find both vectors");
}

/// Test 2: Upsert with ID conflicts - multiple updates to same vector
#[tokio::test]
async fn test_upsert_with_id_conflicts() {
    let (unified_service, collection_service) = setup_test_environment().await;
    let collection_id = create_test_collection(&collection_service, "test_upsert_conflicts", StorageEngine::Wal).await;
    
    // Initial insert
    let initial_vector = VectorRecord {
        id: "conflict_test".to_string(),
        collection_id: collection_id.clone(),
        vector: vec![1.0, 2.0, 3.0],
        metadata: {
            let mut meta = HashMap::new();
            meta.insert("version".to_string(), json!("v1"));
            meta.insert("status".to_string(), json!("initial"));
            meta
        },
        timestamp: chrono::Utc::now().timestamp_millis(),
        created_at: chrono::Utc::now().timestamp_millis(),
        updated_at: chrono::Utc::now().timestamp_millis(),
        expires_at: None,
        version: 1,
        rank: None,
        score: None,
        distance: None,
    };
    
    let avro_data1 = proximadb::storage::persistence::wal::schema::serialize_vector_batch(&[initial_vector]).unwrap();
    unified_service.handle_vector_insert(&collection_id, false, &avro_data1).await.unwrap();
    
    // First update (upsert)
    sleep(Duration::from_millis(10)).await; // Ensure different timestamp
    let updated_vector_v2 = VectorRecord {
        id: "conflict_test".to_string(),
        collection_id: collection_id.clone(),
        vector: vec![1.1, 2.1, 3.1],
        metadata: {
            let mut meta = HashMap::new();
            meta.insert("version".to_string(), json!("v2"));
            meta.insert("status".to_string(), json!("updated"));
            meta
        },
        timestamp: chrono::Utc::now().timestamp_millis(),
        created_at: chrono::Utc::now().timestamp_millis(),
        updated_at: chrono::Utc::now().timestamp_millis(),
        expires_at: None,
        version: 2,
        rank: None,
        score: None,
        distance: None,
    };
    
    let avro_data2 = proximadb::storage::persistence::wal::schema::serialize_vector_batch(&[updated_vector_v2]).unwrap();
    unified_service.handle_vector_insert(&collection_id, true, &avro_data2).await.unwrap(); // upsert_mode = true
    
    // Second update (upsert)
    sleep(Duration::from_millis(10)).await;
    let updated_vector_v3 = VectorRecord {
        id: "conflict_test".to_string(),
        collection_id: collection_id.clone(),
        vector: vec![1.2, 2.2, 3.2],
        metadata: {
            let mut meta = HashMap::new();
            meta.insert("version".to_string(), json!("v3"));
            meta.insert("status".to_string(), json!("final"));
            meta
        },
        timestamp: chrono::Utc::now().timestamp_millis(),
        created_at: chrono::Utc::now().timestamp_millis(),
        updated_at: chrono::Utc::now().timestamp_millis(),
        expires_at: None,
        version: 3,
        rank: None,
        score: None,
        distance: None,
    };
    
    let avro_data3 = proximadb::storage::persistence::wal::schema::serialize_vector_batch(&[updated_vector_v3]).unwrap();
    unified_service.handle_vector_insert(&collection_id, true, &avro_data3).await.unwrap();
    
    // Search and verify only latest version is returned
    let search_query = json!({
        "collection_id": collection_id,
        "vector": [1.0, 2.0, 3.0],
        "k": 10,
        "filters": {}
    });
    let search_payload = serde_json::to_vec(&search_query).unwrap();
    
    let search_result = unified_service.search_vectors_polymorphic(&search_payload).await.unwrap();
    let search_response: serde_json::Value = serde_json::from_slice(&search_result).unwrap();
    let results = search_response["results"].as_array().unwrap();
    
    assert_eq!(results.len(), 1, "Should only find one result (deduplicated)");
    
    let result = &results[0];
    assert_eq!(result["id"].as_str().unwrap(), "conflict_test");
    
    // Verify it's the latest version
    let metadata = result["metadata"].as_object().unwrap();
    assert_eq!(metadata["version"].as_str().unwrap(), "v3");
    assert_eq!(metadata["status"].as_str().unwrap(), "final");
    
    // Verify vector values are from v3
    let vector = result["vector"].as_array().unwrap();
    assert_eq!(vector[0].as_f64().unwrap(), 1.2);
    assert_eq!(vector[1].as_f64().unwrap(), 2.2);
    assert_eq!(vector[2].as_f64().unwrap(), 3.2);
}

/// Test 3: Cross-tier deduplication after flush
#[tokio::test]
async fn test_cross_tier_deduplication_after_flush() {
    let (unified_service, collection_service) = setup_test_environment().await;
    let collection_id = create_test_collection(&collection_service, "test_cross_tier", StorageEngine::Lsm).await;
    
    // Insert initial vector
    let initial_vector = VectorRecord {
        id: "cross_tier_test".to_string(),
        collection_id: collection_id.clone(),
        vector: vec![0.5, 0.6, 0.7],
        metadata: {
            let mut meta = HashMap::new();
            meta.insert("tier".to_string(), json!("initial"));
            meta
        },
        timestamp: chrono::Utc::now().timestamp_millis(),
        created_at: chrono::Utc::now().timestamp_millis(),
        updated_at: chrono::Utc::now().timestamp_millis(),
        expires_at: None,
        version: 1,
        rank: None,
        score: None,
        distance: None,
    };
    
    let avro_data1 = proximadb::storage::persistence::wal::schema::serialize_vector_batch(&[initial_vector]).unwrap();
    unified_service.handle_vector_insert(&collection_id, false, &avro_data1).await.unwrap();
    
    // Force flush to move data to flushed tier
    unified_service.force_flush_collection(&collection_id).await.unwrap();
    
    // Insert updated vector (will be in unflushed tier)
    sleep(Duration::from_millis(100)).await;
    let updated_vector = VectorRecord {
        id: "cross_tier_test".to_string(),
        collection_id: collection_id.clone(),
        vector: vec![0.51, 0.61, 0.71],
        metadata: {
            let mut meta = HashMap::new();
            meta.insert("tier".to_string(), json!("updated"));
            meta
        },
        timestamp: chrono::Utc::now().timestamp_millis(),
        created_at: chrono::Utc::now().timestamp_millis(),
        updated_at: chrono::Utc::now().timestamp_millis(),
        expires_at: None,
        version: 2,
        rank: None,
        score: None,
        distance: None,
    };
    
    let avro_data2 = proximadb::storage::persistence::wal::schema::serialize_vector_batch(&[updated_vector]).unwrap();
    unified_service.handle_vector_insert(&collection_id, true, &avro_data2).await.unwrap();
    
    // Search should return the unflushed (latest) version
    let search_query = json!({
        "collection_id": collection_id,
        "vector": [0.5, 0.6, 0.7],
        "k": 10,
        "filters": {}
    });
    let search_payload = serde_json::to_vec(&search_query).unwrap();
    
    let search_result = unified_service.search_vectors_polymorphic(&search_payload).await.unwrap();
    let search_response: serde_json::Value = serde_json::from_slice(&search_result).unwrap();
    let results = search_response["results"].as_array().unwrap();
    
    assert_eq!(results.len(), 1, "Should find only one result (deduplicated across tiers)");
    
    let result = &results[0];
    let metadata = result["metadata"].as_object().unwrap();
    assert_eq!(metadata["tier"].as_str().unwrap(), "updated", "Should return the unflushed (latest) version");
    
    // Verify vector values are from updated version
    let vector = result["vector"].as_array().unwrap();
    assert_eq!(vector[0].as_f64().unwrap(), 0.51);
    assert_eq!(vector[1].as_f64().unwrap(), 0.61);
    assert_eq!(vector[2].as_f64().unwrap(), 0.71);
}

/// Test 4: Metadata filtering with upserts
#[tokio::test]
async fn test_upsert_with_metadata_filtering() {
    let (unified_service, collection_service) = setup_test_environment().await;
    let collection_id = create_test_collection(&collection_service, "test_metadata_filter", StorageEngine::Wal).await;
    
    // Insert vectors with different metadata
    let vectors = vec![
        VectorRecord {
            id: "doc_1".to_string(),
            collection_id: collection_id.clone(),
            vector: vec![0.1, 0.2, 0.3],
            metadata: {
                let mut meta = HashMap::new();
                meta.insert("category".to_string(), json!("important"));
                meta.insert("status".to_string(), json!("active"));
                meta.insert("author".to_string(), json!("alice"));
                meta
            },
            timestamp: chrono::Utc::now().timestamp_millis(),
            created_at: chrono::Utc::now().timestamp_millis(),
            updated_at: chrono::Utc::now().timestamp_millis(),
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        },
        VectorRecord {
            id: "doc_2".to_string(),
            collection_id: collection_id.clone(),
            vector: vec![0.4, 0.5, 0.6],
            metadata: {
                let mut meta = HashMap::new();
                meta.insert("category".to_string(), json!("normal"));
                meta.insert("status".to_string(), json!("active"));
                meta.insert("author".to_string(), json!("bob"));
                meta
            },
            timestamp: chrono::Utc::now().timestamp_millis(),
            created_at: chrono::Utc::now().timestamp_millis(),
            updated_at: chrono::Utc::now().timestamp_millis(),
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        },
    ];
    
    let avro_data = proximadb::storage::persistence::wal::schema::serialize_vector_batch(&vectors).unwrap();
    unified_service.handle_vector_insert(&collection_id, false, &avro_data).await.unwrap();
    
    // Update doc_1 to change category (should still match important filter)
    sleep(Duration::from_millis(10)).await;
    let updated_doc1 = VectorRecord {
        id: "doc_1".to_string(),
        collection_id: collection_id.clone(),
        vector: vec![0.11, 0.21, 0.31],
        metadata: {
            let mut meta = HashMap::new();
            meta.insert("category".to_string(), json!("important"));
            meta.insert("status".to_string(), json!("updated"));
            meta.insert("author".to_string(), json!("alice"));
            meta
        },
        timestamp: chrono::Utc::now().timestamp_millis(),
        created_at: chrono::Utc::now().timestamp_millis(),
        updated_at: chrono::Utc::now().timestamp_millis(),
        expires_at: None,
        version: 2,
        rank: None,
        score: None,
        distance: None,
    };
    
    let avro_data2 = proximadb::storage::persistence::wal::schema::serialize_vector_batch(&[updated_doc1]).unwrap();
    unified_service.handle_vector_insert(&collection_id, true, &avro_data2).await.unwrap();
    
    // Search with metadata filter for important category
    let search_query = json!({
        "collection_id": collection_id,
        "vector": [0.1, 0.2, 0.3],
        "k": 10,
        "filters": {
            "category": "important"
        }
    });
    let search_payload = serde_json::to_vec(&search_query).unwrap();
    
    let search_result = unified_service.search_vectors_polymorphic(&search_payload).await.unwrap();
    let search_response: serde_json::Value = serde_json::from_slice(&search_result).unwrap();
    let results = search_response["results"].as_array().unwrap();
    
    assert_eq!(results.len(), 1, "Should find only documents with important category");
    
    let result = &results[0];
    assert_eq!(result["id"].as_str().unwrap(), "doc_1");
    
    let metadata = result["metadata"].as_object().unwrap();
    assert_eq!(metadata["category"].as_str().unwrap(), "important");
    assert_eq!(metadata["status"].as_str().unwrap(), "updated"); // Should be the updated version
}

/// Test 5: Batch upserts with mixed operations
#[tokio::test]
async fn test_batch_upserts_mixed_operations() {
    let (unified_service, collection_service) = setup_test_environment().await;
    let collection_id = create_test_collection(&collection_service, "test_batch_mixed", StorageEngine::Wal).await;
    
    // Initial batch insert
    let initial_vectors = vec![
        VectorRecord {
            id: "item_1".to_string(),
            collection_id: collection_id.clone(),
            vector: vec![1.0, 1.0, 1.0],
            metadata: HashMap::new(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            created_at: chrono::Utc::now().timestamp_millis(),
            updated_at: chrono::Utc::now().timestamp_millis(),
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        },
        VectorRecord {
            id: "item_2".to_string(),
            collection_id: collection_id.clone(),
            vector: vec![2.0, 2.0, 2.0],
            metadata: HashMap::new(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            created_at: chrono::Utc::now().timestamp_millis(),
            updated_at: chrono::Utc::now().timestamp_millis(),
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        },
    ];
    
    let avro_data1 = proximadb::storage::persistence::wal::schema::serialize_vector_batch(&initial_vectors).unwrap();
    unified_service.handle_vector_insert(&collection_id, false, &avro_data1).await.unwrap();
    
    // Mixed batch: update existing + add new
    sleep(Duration::from_millis(10)).await;
    let mixed_batch = vec![
        VectorRecord {
            id: "item_1".to_string(), // Update existing
            collection_id: collection_id.clone(),
            vector: vec![1.1, 1.1, 1.1],
            metadata: {
                let mut meta = HashMap::new();
                meta.insert("updated".to_string(), json!(true));
                meta
            },
            timestamp: chrono::Utc::now().timestamp_millis(),
            created_at: chrono::Utc::now().timestamp_millis(),
            updated_at: chrono::Utc::now().timestamp_millis(),
            expires_at: None,
            version: 2,
            rank: None,
            score: None,
            distance: None,
        },
        VectorRecord {
            id: "item_3".to_string(), // New item
            collection_id: collection_id.clone(),
            vector: vec![3.0, 3.0, 3.0],
            metadata: {
                let mut meta = HashMap::new();
                meta.insert("new".to_string(), json!(true));
                meta
            },
            timestamp: chrono::Utc::now().timestamp_millis(),
            created_at: chrono::Utc::now().timestamp_millis(),
            updated_at: chrono::Utc::now().timestamp_millis(),
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        },
    ];
    
    let avro_data2 = proximadb::storage::persistence::wal::schema::serialize_vector_batch(&mixed_batch).unwrap();
    unified_service.handle_vector_insert(&collection_id, true, &avro_data2).await.unwrap(); // upsert mode
    
    // Search and verify results
    let search_query = json!({
        "collection_id": collection_id,
        "vector": [1.0, 1.0, 1.0],
        "k": 10,
        "filters": {}
    });
    let search_payload = serde_json::to_vec(&search_query).unwrap();
    
    let search_result = unified_service.search_vectors_polymorphic(&search_payload).await.unwrap();
    let search_response: serde_json::Value = serde_json::from_slice(&search_result).unwrap();
    let results = search_response["results"].as_array().unwrap();
    
    assert_eq!(results.len(), 3, "Should find all three items");
    
    // Verify item_1 was updated
    let item_1 = results.iter().find(|r| r["id"].as_str().unwrap() == "item_1").unwrap();
    let vector_1 = item_1["vector"].as_array().unwrap();
    assert_eq!(vector_1[0].as_f64().unwrap(), 1.1); // Updated value
    
    let metadata_1 = item_1["metadata"].as_object().unwrap();
    assert_eq!(metadata_1["updated"].as_bool().unwrap(), true);
    
    // Verify item_2 unchanged
    let item_2 = results.iter().find(|r| r["id"].as_str().unwrap() == "item_2").unwrap();
    let vector_2 = item_2["vector"].as_array().unwrap();
    assert_eq!(vector_2[0].as_f64().unwrap(), 2.0); // Original value
    
    // Verify item_3 was added
    let item_3 = results.iter().find(|r| r["id"].as_str().unwrap() == "item_3").unwrap();
    let metadata_3 = item_3["metadata"].as_object().unwrap();
    assert_eq!(metadata_3["new"].as_bool().unwrap(), true);
}

/// Test 6: Performance test - many concurrent upserts
#[tokio::test]
async fn test_concurrent_upserts_performance() {
    let (unified_service, collection_service) = setup_test_environment().await;
    let collection_id = create_test_collection(&collection_service, "test_concurrent_perf", StorageEngine::Wal).await;
    
    let num_vectors = 100;
    let num_updates = 5;
    
    // Initial batch
    let mut initial_vectors = Vec::new();
    for i in 0..num_vectors {
        initial_vectors.push(VectorRecord {
            id: format!("perf_test_{}", i),
            collection_id: collection_id.clone(),
            vector: vec![i as f32, (i + 1) as f32, (i + 2) as f32],
            metadata: {
                let mut meta = HashMap::new();
                meta.insert("iteration".to_string(), json!(0));
                meta
            },
            timestamp: chrono::Utc::now().timestamp_millis(),
            created_at: chrono::Utc::now().timestamp_millis(),
            updated_at: chrono::Utc::now().timestamp_millis(),
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        });
    }
    
    let avro_data = proximadb::storage::persistence::wal::schema::serialize_vector_batch(&initial_vectors).unwrap();
    let start_time = std::time::Instant::now();
    unified_service.handle_vector_insert(&collection_id, false, &avro_data).await.unwrap();
    let insert_duration = start_time.elapsed();
    
    // Multiple update iterations
    for iteration in 1..=num_updates {
        let mut update_vectors = Vec::new();
        for i in 0..num_vectors {
            update_vectors.push(VectorRecord {
                id: format!("perf_test_{}", i),
                collection_id: collection_id.clone(),
                vector: vec![
                    (i as f32) + (iteration as f32 * 0.1),
                    (i + 1) as f32 + (iteration as f32 * 0.1),
                    (i + 2) as f32 + (iteration as f32 * 0.1)
                ],
                metadata: {
                    let mut meta = HashMap::new();
                    meta.insert("iteration".to_string(), json!(iteration));
                    meta
                },
                timestamp: chrono::Utc::now().timestamp_millis(),
                created_at: chrono::Utc::now().timestamp_millis(),
                updated_at: chrono::Utc::now().timestamp_millis(),
                expires_at: None,
                version: iteration as u64 + 1,
                rank: None,
                score: None,
                distance: None,
            });
        }
        
        let avro_data = proximadb::storage::persistence::wal::schema::serialize_vector_batch(&update_vectors).unwrap();
        let start_time = std::time::Instant::now();
        unified_service.handle_vector_insert(&collection_id, true, &avro_data).await.unwrap();
        let update_duration = start_time.elapsed();
        
        println!("Update iteration {} took {:?}", iteration, update_duration);
    }
    
    // Final search to verify deduplication worked
    let search_query = json!({
        "collection_id": collection_id,
        "vector": [0.0, 1.0, 2.0],
        "k": num_vectors,
        "filters": {}
    });
    let search_payload = serde_json::to_vec(&search_query).unwrap();
    
    let search_start = std::time::Instant::now();
    let search_result = unified_service.search_vectors_polymorphic(&search_payload).await.unwrap();
    let search_duration = search_start.elapsed();
    
    let search_response: serde_json::Value = serde_json::from_slice(&search_result).unwrap();
    let results = search_response["results"].as_array().unwrap();
    
    assert_eq!(results.len(), num_vectors, "Should find all vectors despite multiple updates");
    
    // Verify all vectors are latest version
    for result in results {
        let metadata = result["metadata"].as_object().unwrap();
        assert_eq!(metadata["iteration"].as_u64().unwrap(), num_updates as u64, "All vectors should be latest version");
    }
    
    println!("Performance results:");
    println!("  Initial insert: {:?} for {} vectors", insert_duration, num_vectors);
    println!("  Search: {:?} for {} vectors with deduplication", search_duration, num_vectors);
}