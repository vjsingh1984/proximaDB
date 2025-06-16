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

use proximadb::storage::{StorageEngine, CollectionMetadata};
use proximadb::core::{StorageConfig, LsmConfig, VectorRecord};
use std::collections::HashMap;
use tempfile::TempDir;
use uuid::Uuid;
use chrono::Utc;

#[tokio::test]
async fn test_vector_search_basic() {
    let temp_dir = TempDir::new().unwrap();
    let data_dir = temp_dir.path().join("data");
    let wal_dir = temp_dir.path().join("wal");
    
    let config = StorageConfig {
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
    };
    
    let mut storage = StorageEngine::new(config).await.unwrap();
    storage.start().await.unwrap();
    
    // Create collection with default settings first
    storage.create_collection("search_collection".to_string()).await.unwrap();
    
    // Add test vectors with known relationships
    let test_vectors = vec![
        (vec![1.0, 0.0, 0.0], "vector_x"),     // Along X axis
        (vec![0.0, 1.0, 0.0], "vector_y"),     // Along Y axis  
        (vec![0.0, 0.0, 1.0], "vector_z"),     // Along Z axis
        (vec![0.7071, 0.7071, 0.0], "vector_xy"), // Between X and Y
        (vec![0.5773, 0.5773, 0.5773], "vector_center"), // Center point
    ];
    
    let mut vector_ids = Vec::new();
    for (vector, name) in test_vectors {
        let mut metadata = HashMap::new();
        metadata.insert("name".to_string(), serde_json::Value::String(name.to_string()));
        
        let record = VectorRecord {
            id: Uuid::new_v4(),
            collection_id: "search_collection".to_string(),
            vector,
            metadata,
            timestamp: Utc::now(),
        };
        
        vector_ids.push(record.id);
        storage.write(record).await.unwrap();
    }
    
    // Test basic search - query for X axis vector
    let results = storage.search_vectors(&"search_collection".to_string(), vec![1.0, 0.0, 0.0], 3).await.unwrap();
    
    println!("Search results:");
    for (i, result) in results.iter().enumerate() {
        let name = result.metadata.as_ref()
            .and_then(|m| m.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("no_name");
        println!("  {}: {} (score: {})", i, name, result.score);
    }
    
    assert!(results.len() >= 1);
    // Find the exact match (vector_x) in the results
    let exact_match = results.iter().find(|r| {
        if let Some(metadata) = &r.metadata {
            if let Some(name) = metadata.get("name") {
                return name.as_str() == Some("vector_x");
            }
        }
        false
    });
    assert!(exact_match.is_some(), "Should find exact match vector_x in results");
    let exact_match = exact_match.unwrap();
    assert_eq!(exact_match.score, 1.0); // Should be exactly 1.0 for identical vectors after precision fix
    
    // Test search with more results
    let results = storage.search_vectors(&"search_collection".to_string(), vec![0.0, 1.0, 0.0], 5).await.unwrap();
    assert_eq!(results.len(), 5); // Should return all vectors
    
    // Test search with metadata filtering
    let results = storage.search_vectors_with_filter(
        &"search_collection".to_string(), 
        vec![1.0, 1.0, 0.0], 
        10,
        |metadata| {
            metadata.get("name")
                .and_then(|v| v.as_str())
                .map(|s| s.contains("vector_"))
                .unwrap_or(false)
        }
    ).await.unwrap();
    
    assert!(results.len() > 0);
    
    // Test index statistics
    let stats = storage.get_index_stats(&"search_collection".to_string()).await.unwrap();
    assert!(stats.is_some());
    let stats = stats.unwrap();
    assert_eq!(stats["size"], serde_json::json!(5));
    assert!(stats["total_bytes"].as_u64().unwrap() > 0);
    
    storage.stop().await.unwrap();
}

#[tokio::test]
async fn test_vector_search_with_deletions() {
    let temp_dir = TempDir::new().unwrap();
    let data_dir = temp_dir.path().join("data");
    let wal_dir = temp_dir.path().join("wal");
    
    let config = StorageConfig {
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
    };
    
    let mut storage = StorageEngine::new(config).await.unwrap();
    storage.start().await.unwrap();
    
    storage.create_collection("delete_test".to_string()).await.unwrap();
    
    // Add vectors
    let mut vector_ids = Vec::new();
    for i in 0..10 {
        let record = VectorRecord {
            id: Uuid::new_v4(),
            collection_id: "delete_test".to_string(),
            vector: vec![i as f32, 0.0, 0.0],
            metadata: HashMap::new(),
            timestamp: Utc::now(),
        };
        
        vector_ids.push(record.id);
        storage.write(record).await.unwrap();
    }
    
    // Verify all vectors are searchable
    let results = storage.search_vectors(&"delete_test".to_string(), vec![5.0, 0.0, 0.0], 10).await.unwrap();
    assert_eq!(results.len(), 10);
    
    // Delete some vectors
    for i in 0..5 {
        let deleted = storage.soft_delete(&"delete_test".to_string(), &vector_ids[i]).await.unwrap();
        assert!(deleted);
    }
    
    // Search again - should return fewer results
    let results = storage.search_vectors(&"delete_test".to_string(), vec![2.0, 0.0, 0.0], 10).await.unwrap();
    assert_eq!(results.len(), 5); // Only 5 vectors should remain
    
    // Verify stats are updated
    let stats = storage.get_index_stats(&"delete_test".to_string()).await.unwrap().unwrap();
    assert_eq!(stats["size"], serde_json::json!(5));
    
    storage.stop().await.unwrap();
}

#[tokio::test]
async fn test_vector_search_different_algorithms() {
    let temp_dir = TempDir::new().unwrap();
    let data_dir = temp_dir.path().join("data");
    let wal_dir = temp_dir.path().join("wal");
    
    let config = StorageConfig {
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
    };
    
    let mut storage = StorageEngine::new(config).await.unwrap();
    storage.start().await.unwrap();
    
    // Test with Brute Force algorithm
    let brute_force_metadata = CollectionMetadata {
        id: "brute_force_collection".to_string(),
        name: "Brute Force Collection".to_string(),
        dimension: 2,
        distance_metric: "euclidean".to_string(),
        indexing_algorithm: "brute_force".to_string(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        vector_count: 0,
        total_size_bytes: 0,
        config: HashMap::new(),
    };
    
    storage.create_collection_with_metadata("brute_force_collection".to_string(), Some(brute_force_metadata)).await.unwrap();
    
    // Add test vectors
    for i in 0..5 {
        let record = VectorRecord {
            id: Uuid::new_v4(),
            collection_id: "brute_force_collection".to_string(),
            vector: vec![i as f32, i as f32],
            metadata: HashMap::new(),
            timestamp: Utc::now(),
        };
        
        storage.write(record).await.unwrap();
    }
    
    // Test search
    let results = storage.search_vectors(&"brute_force_collection".to_string(), vec![2.0, 2.0], 3).await.unwrap();
    assert_eq!(results.len(), 3);
    
    // Test with different distance metrics
    let manhattan_metadata = CollectionMetadata {
        id: "manhattan_collection".to_string(),
        name: "Manhattan Collection".to_string(),
        dimension: 2,
        distance_metric: "manhattan".to_string(),
        indexing_algorithm: "hnsw".to_string(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        vector_count: 0,
        total_size_bytes: 0,
        config: HashMap::new(),
    };
    
    storage.create_collection_with_metadata("manhattan_collection".to_string(), Some(manhattan_metadata)).await.unwrap();
    
    // Add vectors and test search
    for i in 0..3 {
        let record = VectorRecord {
            id: Uuid::new_v4(),
            collection_id: "manhattan_collection".to_string(),
            vector: vec![i as f32, 0.0],
            metadata: HashMap::new(),
            timestamp: Utc::now(),
        };
        
        storage.write(record).await.unwrap();
    }
    
    let results = storage.search_vectors(&"manhattan_collection".to_string(), vec![1.0, 0.0], 2).await.unwrap();
    assert_eq!(results.len(), 2);
    
    storage.stop().await.unwrap();
}

#[tokio::test]
async fn test_index_optimization() {
    let temp_dir = TempDir::new().unwrap();
    let data_dir = temp_dir.path().join("data");
    let wal_dir = temp_dir.path().join("wal");
    
    let config = StorageConfig {
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
    };
    
    let mut storage = StorageEngine::new(config).await.unwrap();
    storage.start().await.unwrap();
    
    storage.create_collection("optimization_test".to_string()).await.unwrap();
    
    // Add many vectors
    for i in 0..100 {
        let record = VectorRecord {
            id: Uuid::new_v4(),
            collection_id: "optimization_test".to_string(),
            vector: vec![
                (i as f32 * 0.1).sin(),
                (i as f32 * 0.1).cos(),
                i as f32 * 0.01,
            ],
            metadata: HashMap::new(),
            timestamp: Utc::now(),
        };
        
        storage.write(record).await.unwrap();
    }
    
    // Get stats before optimization
    let stats_before = storage.get_index_stats(&"optimization_test".to_string()).await.unwrap().unwrap();
    assert_eq!(stats_before["size"], serde_json::json!(100));
    
    // Optimize index
    storage.optimize_index(&"optimization_test".to_string()).await.unwrap();
    
    // Get stats after optimization (should be the same for this test)
    let stats_after = storage.get_index_stats(&"optimization_test".to_string()).await.unwrap().unwrap();
    assert_eq!(stats_after["size"], serde_json::json!(100));
    
    // Search should still work after optimization
    let results = storage.search_vectors(&"optimization_test".to_string(), vec![0.0, 1.0, 0.5], 10).await.unwrap();
    assert_eq!(results.len(), 10);
    
    storage.stop().await.unwrap();
}