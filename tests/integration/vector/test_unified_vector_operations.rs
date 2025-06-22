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

//! Integration tests for unified vector operations using UnifiedAvroService
//!
//! Tests: vector insert, update, delete, search operations through the unified service layer

use chrono::Utc;
use proximadb::core::{CollectionId, LsmConfig, StorageConfig, VectorRecord};
use proximadb::services::unified_avro_service::UnifiedAvroService;
use proximadb::storage::StorageEngine;
use proximadb::storage::metadata::backends::memory_backend::MemoryMetadataBackend;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::RwLock;
use uuid::Uuid;

/// Helper function to create test configuration
fn create_test_config(temp_dir: &TempDir) -> StorageConfig {
    let data_dir = temp_dir.path().join("data");
    let wal_dir = temp_dir.path().join("wal");
    
    StorageConfig {
        data_dirs: vec![data_dir],
        wal_dir,
        mmap_enabled: true,
        lsm_config: LsmConfig {
            memtable_size_mb: 16,
            level_count: 7,
            compaction_threshold: 4,
            block_size_kb: 64,
        },
        cache_size_mb: 32,
        bloom_filter_bits: 10,
    }
}

/// Helper function to create test UnifiedAvroService
async fn create_test_service(config: StorageConfig) -> anyhow::Result<UnifiedAvroService> {
    let storage = StorageEngine::new(config).await?;
    let storage = Arc::new(RwLock::new(storage));
    
    let metadata_backend = Arc::new(MemoryMetadataBackend::new());
    
    UnifiedAvroService::new(storage, metadata_backend).await
}

#[tokio::test]
async fn test_vector_insert_operation() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_test_config(&temp_dir);
    let mut service = create_test_service(config).await.unwrap();
    
    // Start the service
    service.start().await.unwrap();
    
    // Create a test collection
    let collection_id = CollectionId::new("test_collection");
    service.create_collection(collection_id.clone(), None).await.unwrap();
    
    // Test vector insertion
    let vector_id = Uuid::new_v4();
    let test_vector = vec![1.0, 2.0, 3.0, 4.0];
    let mut metadata = HashMap::new();
    metadata.insert("type".to_string(), serde_json::Value::String("test".to_string()));
    metadata.insert("category".to_string(), serde_json::Value::String("embedding".to_string()));
    
    let record = VectorRecord {
        id: vector_id,
        collection_id: collection_id.clone(),
        vector: test_vector.clone(),
        metadata: metadata.clone(),
        timestamp: Utc::now(),
    };
    
    // Insert vector
    let result = service.insert_vector(record.clone()).await;
    assert!(result.is_ok(), "Vector insertion should succeed: {:?}", result.err());
    
    // Verify vector was inserted by trying to get it
    let retrieved = service.get_vector(&collection_id, &vector_id).await.unwrap();
    assert!(retrieved.is_some(), "Inserted vector should be retrievable");
    
    let retrieved_record = retrieved.unwrap();
    assert_eq!(retrieved_record.id, vector_id);
    assert_eq!(retrieved_record.vector, test_vector);
    assert_eq!(retrieved_record.metadata.get("type"), metadata.get("type"));
    
    service.stop().await.unwrap();
    println!("✅ Vector insert operation test passed");
}

#[tokio::test]
async fn test_vector_update_operation() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_test_config(&temp_dir);
    let mut service = create_test_service(config).await.unwrap();
    
    service.start().await.unwrap();
    
    let collection_id = CollectionId::new("update_test_collection");
    service.create_collection(collection_id.clone(), None).await.unwrap();
    
    // Insert initial vector
    let vector_id = Uuid::new_v4();
    let initial_vector = vec![1.0, 2.0, 3.0];
    let mut initial_metadata = HashMap::new();
    initial_metadata.insert("version".to_string(), serde_json::Value::String("v1".to_string()));
    
    let initial_record = VectorRecord {
        id: vector_id,
        collection_id: collection_id.clone(),
        vector: initial_vector,
        metadata: initial_metadata,
        timestamp: Utc::now(),
    };
    
    service.insert_vector(initial_record).await.unwrap();
    
    // Update the vector
    let updated_vector = vec![4.0, 5.0, 6.0];
    let mut updated_metadata = HashMap::new();
    updated_metadata.insert("version".to_string(), serde_json::Value::String("v2".to_string()));
    updated_metadata.insert("updated".to_string(), serde_json::Value::Bool(true));
    
    let updated_record = VectorRecord {
        id: vector_id,
        collection_id: collection_id.clone(),
        vector: updated_vector.clone(),
        metadata: updated_metadata.clone(),
        timestamp: Utc::now(),
    };
    
    let update_result = service.update_vector(updated_record).await;
    assert!(update_result.is_ok(), "Vector update should succeed: {:?}", update_result.err());
    
    // Verify update
    let retrieved = service.get_vector(&collection_id, &vector_id).await.unwrap().unwrap();
    assert_eq!(retrieved.vector, updated_vector);
    assert_eq!(retrieved.metadata.get("version").unwrap().as_str().unwrap(), "v2");
    assert_eq!(retrieved.metadata.get("updated").unwrap().as_bool().unwrap(), true);
    
    service.stop().await.unwrap();
    println!("✅ Vector update operation test passed");
}

#[tokio::test]
async fn test_vector_delete_operation() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_test_config(&temp_dir);
    let mut service = create_test_service(config).await.unwrap();
    
    service.start().await.unwrap();
    
    let collection_id = CollectionId::new("delete_test_collection");
    service.create_collection(collection_id.clone(), None).await.unwrap();
    
    // Insert multiple vectors
    let mut vector_ids = Vec::new();
    for i in 0..5 {
        let vector_id = Uuid::new_v4();
        let record = VectorRecord {
            id: vector_id,
            collection_id: collection_id.clone(),
            vector: vec![i as f32, (i + 1) as f32, (i + 2) as f32],
            metadata: HashMap::new(),
            timestamp: Utc::now(),
        };
        
        service.insert_vector(record).await.unwrap();
        vector_ids.push(vector_id);
    }
    
    // Verify all vectors exist
    for vector_id in &vector_ids {
        let retrieved = service.get_vector(&collection_id, vector_id).await.unwrap();
        assert!(retrieved.is_some(), "Vector {} should exist before deletion", vector_id);
    }
    
    // Delete some vectors
    let vectors_to_delete = &vector_ids[0..3];
    for vector_id in vectors_to_delete {
        let delete_result = service.delete_vector(&collection_id, vector_id).await;
        assert!(delete_result.is_ok(), "Vector deletion should succeed: {:?}", delete_result.err());
    }
    
    // Verify deleted vectors are gone
    for vector_id in vectors_to_delete {
        let retrieved = service.get_vector(&collection_id, vector_id).await.unwrap();
        assert!(retrieved.is_none(), "Deleted vector {} should not be retrievable", vector_id);
    }
    
    // Verify remaining vectors still exist
    for vector_id in &vector_ids[3..] {
        let retrieved = service.get_vector(&collection_id, vector_id).await.unwrap();
        assert!(retrieved.is_some(), "Non-deleted vector {} should still exist", vector_id);
    }
    
    service.stop().await.unwrap();
    println!("✅ Vector delete operation test passed");
}

#[tokio::test]
async fn test_vector_search_operation() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_test_config(&temp_dir);
    let mut service = create_test_service(config).await.unwrap();
    
    service.start().await.unwrap();
    
    let collection_id = CollectionId::new("search_test_collection");
    service.create_collection(collection_id.clone(), None).await.unwrap();
    
    // Insert test vectors with known relationships
    let test_data = vec![
        (vec![1.0, 0.0, 0.0], "x_axis", "reference"),
        (vec![0.0, 1.0, 0.0], "y_axis", "reference"), 
        (vec![0.0, 0.0, 1.0], "z_axis", "reference"),
        (vec![0.7071, 0.7071, 0.0], "xy_diagonal", "diagonal"),
        (vec![0.5773, 0.5773, 0.5773], "center", "center"),
        (vec![2.0, 0.0, 0.0], "x_axis_scaled", "scaled"),
        (vec![0.1, 0.0, 0.0], "x_axis_small", "scaled"),
    ];
    
    let mut inserted_ids = Vec::new();
    for (vector, name, category) in test_data {
        let vector_id = Uuid::new_v4();
        let mut metadata = HashMap::new();
        metadata.insert("name".to_string(), serde_json::Value::String(name.to_string()));
        metadata.insert("category".to_string(), serde_json::Value::String(category.to_string()));
        
        let record = VectorRecord {
            id: vector_id,
            collection_id: collection_id.clone(),
            vector,
            metadata,
            timestamp: Utc::now(),
        };
        
        service.insert_vector(record).await.unwrap();
        inserted_ids.push((vector_id, name));
    }
    
    // Test basic similarity search
    let query_vector = vec![1.0, 0.0, 0.0]; // Should be closest to x_axis vectors
    let search_results = service.search_vectors(&collection_id, query_vector, 3, None).await;
    
    assert!(search_results.is_ok(), "Search should succeed: {:?}", search_results.err());
    let results = search_results.unwrap();
    assert!(results.len() > 0, "Search should return results");
    assert!(results.len() <= 3, "Should not return more than requested");
    
    // Verify results are sorted by similarity (highest score first)
    for i in 1..results.len() {
        assert!(
            results[i-1].score >= results[i].score,
            "Results should be sorted by score: {} >= {}",
            results[i-1].score, results[i].score
        );
    }
    
    // Test the most similar result
    let best_match = &results[0];
    let best_name = best_match.metadata.as_ref()
        .and_then(|m| m.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
        
    assert!(
        best_name.contains("x_axis"),
        "Best match should be an x_axis vector, got: {}",
        best_name
    );
    
    // Test search with different query
    let diagonal_query = vec![1.0, 1.0, 0.0]; // Should be closest to xy_diagonal
    let diagonal_results = service.search_vectors(&collection_id, diagonal_query, 5, None).await.unwrap();
    assert!(diagonal_results.len() > 0);
    
    service.stop().await.unwrap();
    println!("✅ Vector search operation test passed");
}

#[tokio::test]
async fn test_complete_vector_lifecycle() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_test_config(&temp_dir);
    let mut service = create_test_service(config).await.unwrap();
    
    service.start().await.unwrap();
    
    let collection_id = CollectionId::new("lifecycle_test_collection");
    service.create_collection(collection_id.clone(), None).await.unwrap();
    
    // 1. INSERT multiple vectors
    let mut vector_ids = Vec::new();
    for i in 0..10 {
        let vector_id = Uuid::new_v4();
        let mut metadata = HashMap::new();
        metadata.insert("index".to_string(), serde_json::Value::Number(i.into()));
        metadata.insert("phase".to_string(), serde_json::Value::String("initial".to_string()));
        
        let record = VectorRecord {
            id: vector_id,
            collection_id: collection_id.clone(),
            vector: vec![i as f32, (i * 2) as f32, (i * 3) as f32],
            metadata,
            timestamp: Utc::now(),
        };
        
        service.insert_vector(record).await.unwrap();
        vector_ids.push(vector_id);
    }
    
    // 2. SEARCH to verify all inserted
    let initial_search = service.search_vectors(&collection_id, vec![5.0, 10.0, 15.0], 10, None).await.unwrap();
    assert_eq!(initial_search.len(), 10, "Should find all 10 inserted vectors");
    
    // 3. UPDATE some vectors
    for i in 0..5 {
        let vector_id = vector_ids[i];
        let mut metadata = HashMap::new();
        metadata.insert("index".to_string(), serde_json::Value::Number(i.into()));
        metadata.insert("phase".to_string(), serde_json::Value::String("updated".to_string()));
        metadata.insert("update_timestamp".to_string(), serde_json::Value::String(Utc::now().to_rfc3339()));
        
        let updated_record = VectorRecord {
            id: vector_id,
            collection_id: collection_id.clone(),
            vector: vec![(i + 10) as f32, (i + 20) as f32, (i + 30) as f32], // Different vectors
            metadata,
            timestamp: Utc::now(),
        };
        
        service.update_vector(updated_record).await.unwrap();
    }
    
    // 4. SEARCH after updates
    let post_update_search = service.search_vectors(&collection_id, vec![12.0, 22.0, 32.0], 10, None).await.unwrap();
    assert_eq!(post_update_search.len(), 10, "Should still find all vectors after update");
    
    // Verify some vectors were updated
    let updated_count = post_update_search.iter()
        .filter(|result| {
            result.metadata.as_ref()
                .and_then(|m| m.get("phase"))
                .and_then(|v| v.as_str())
                .map(|s| s == "updated")
                .unwrap_or(false)
        })
        .count();
    assert!(updated_count >= 1, "Should find some updated vectors");
    
    // 5. DELETE some vectors
    for i in 0..3 {
        service.delete_vector(&collection_id, &vector_ids[i]).await.unwrap();
    }
    
    // 6. SEARCH after deletions
    let post_delete_search = service.search_vectors(&collection_id, vec![5.0, 10.0, 15.0], 10, None).await.unwrap();
    assert_eq!(post_delete_search.len(), 7, "Should find 7 vectors after deleting 3");
    
    // 7. Verify deleted vectors are not retrievable
    for i in 0..3 {
        let retrieved = service.get_vector(&collection_id, &vector_ids[i]).await.unwrap();
        assert!(retrieved.is_none(), "Deleted vector should not be retrievable");
    }
    
    // 8. Verify remaining vectors are still retrievable
    for i in 3..10 {
        let retrieved = service.get_vector(&collection_id, &vector_ids[i]).await.unwrap();
        assert!(retrieved.is_some(), "Non-deleted vector should still be retrievable");
    }
    
    service.stop().await.unwrap();
    println!("✅ Complete vector lifecycle test passed");
}

#[tokio::test]
async fn test_vector_operations_error_handling() {
    let temp_dir = TempDir::new().unwrap();
    let config = create_test_config(&temp_dir);
    let mut service = create_test_service(config).await.unwrap();
    
    service.start().await.unwrap();
    
    let collection_id = CollectionId::new("error_test_collection");
    let nonexistent_collection = CollectionId::new("nonexistent_collection");
    
    // Create only one collection
    service.create_collection(collection_id.clone(), None).await.unwrap();
    
    let vector_id = Uuid::new_v4();
    let test_record = VectorRecord {
        id: vector_id,
        collection_id: collection_id.clone(),
        vector: vec![1.0, 2.0, 3.0],
        metadata: HashMap::new(),
        timestamp: Utc::now(),
    };
    
    // Test operations on nonexistent collection
    let insert_result = service.insert_vector(VectorRecord {
        collection_id: nonexistent_collection.clone(),
        ..test_record.clone()
    }).await;
    assert!(insert_result.is_err(), "Insert into nonexistent collection should fail");
    
    let search_result = service.search_vectors(&nonexistent_collection, vec![1.0, 2.0, 3.0], 5, None).await;
    assert!(search_result.is_err(), "Search in nonexistent collection should fail");
    
    // Test operations on nonexistent vector
    let nonexistent_vector_id = Uuid::new_v4();
    let get_result = service.get_vector(&collection_id, &nonexistent_vector_id).await.unwrap();
    assert!(get_result.is_none(), "Get nonexistent vector should return None");
    
    let delete_result = service.delete_vector(&collection_id, &nonexistent_vector_id).await;
    // This might succeed (idempotent delete) or fail depending on implementation
    
    service.stop().await.unwrap();
    println!("✅ Vector operations error handling test passed");
}