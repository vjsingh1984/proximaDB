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
async fn test_collection_metadata_persistence() {
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
    
    let mut storage = StorageEngine::new(config.clone()).await.unwrap();
    storage.start().await.unwrap();
    
    // Create collection with custom metadata
    let collection_metadata = CollectionMetadata {
        id: "test_collection".to_string(),
        name: "Test Collection".to_string(),
        dimension: 256,
        distance_metric: "euclidean".to_string(),
        indexing_algorithm: "flat".to_string(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        vector_count: 0,
        total_size_bytes: 0,
        config: {
            let mut config = HashMap::new();
            config.insert("ef_construction".to_string(), serde_json::json!(200));
            config.insert("max_connections".to_string(), serde_json::json!(16));
            config
        },
    };
    
    storage.create_collection_with_metadata("test_collection".to_string(), Some(collection_metadata.clone())).await.unwrap();
    
    // Verify metadata was stored
    let retrieved = storage.get_collection_metadata(&"test_collection".to_string()).await.unwrap();
    assert!(retrieved.is_some());
    let metadata = retrieved.unwrap();
    assert_eq!(metadata.name, "Test Collection");
    assert_eq!(metadata.dimension, 256);
    assert_eq!(metadata.distance_metric, "euclidean");
    
    // List collections
    let collections = storage.list_collections().await.unwrap();
    assert_eq!(collections.len(), 1);
    assert_eq!(collections[0].name, "Test Collection");
    
    // Add some vectors and verify statistics update
    for i in 0..5 {
        let record = VectorRecord {
            id: Uuid::new_v4(),
            collection_id: "test_collection".to_string(),
            vector: vec![i as f32; 256],
            metadata: HashMap::new(),
            timestamp: Utc::now(),
        };
        storage.write(record).await.unwrap();
    }
    
    // Check updated statistics
    let updated_metadata = storage.get_collection_metadata(&"test_collection".to_string()).await.unwrap().unwrap();
    assert_eq!(updated_metadata.vector_count, 5);
    assert!(updated_metadata.total_size_bytes > 0);
    
    storage.stop().await.unwrap();
}

#[tokio::test]
async fn test_metadata_persistence_across_restarts() {
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
    
    // First session - create collection and add data
    {
        let mut storage = StorageEngine::new(config.clone()).await.unwrap();
        storage.start().await.unwrap();
        
        storage.create_collection("persistent_collection".to_string()).await.unwrap();
        
        // Add vectors
        for i in 0..3 {
            let record = VectorRecord {
                id: Uuid::new_v4(),
                collection_id: "persistent_collection".to_string(),
                vector: vec![i as f32; 128],
                metadata: HashMap::new(),
                timestamp: Utc::now(),
            };
            storage.write(record).await.unwrap();
        }
        
        storage.stop().await.unwrap();
    }
    
    // Second session - verify metadata persisted
    {
        let mut storage = StorageEngine::new(config.clone()).await.unwrap();
        storage.start().await.unwrap();
        
        let metadata = storage.get_collection_metadata(&"persistent_collection".to_string()).await.unwrap();
        assert!(metadata.is_some());
        let metadata = metadata.unwrap();
        assert_eq!(metadata.id, "persistent_collection");
        assert_eq!(metadata.vector_count, 3);
        
        let collections = storage.list_collections().await.unwrap();
        assert_eq!(collections.len(), 1);
        
        storage.stop().await.unwrap();
    }
}

#[tokio::test]
async fn test_collection_deletion() {
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
    
    // Create multiple collections
    storage.create_collection("collection1".to_string()).await.unwrap();
    storage.create_collection("collection2".to_string()).await.unwrap();
    
    let collections = storage.list_collections().await.unwrap();
    assert_eq!(collections.len(), 2);
    
    // Delete one collection
    let deleted = storage.delete_collection(&"collection1".to_string()).await.unwrap();
    assert!(deleted);
    
    // Verify it's gone
    let metadata = storage.get_collection_metadata(&"collection1".to_string()).await.unwrap();
    assert!(metadata.is_none());
    
    let collections = storage.list_collections().await.unwrap();
    assert_eq!(collections.len(), 1);
    assert_eq!(collections[0].id, "collection2");
    
    // Try to delete non-existent collection
    let deleted = storage.delete_collection(&"nonexistent".to_string()).await.unwrap();
    assert!(!deleted);
}

#[tokio::test]
async fn test_collection_statistics_tracking() {
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
    
    storage.create_collection("stats_collection".to_string()).await.unwrap();
    
    // Initial state
    let metadata = storage.get_collection_metadata(&"stats_collection".to_string()).await.unwrap().unwrap();
    assert_eq!(metadata.vector_count, 0);
    assert_eq!(metadata.total_size_bytes, 0);
    
    // Add vectors in batches and verify stats
    for batch in 0..3 {
        for i in 0..10 {
            let record = VectorRecord {
                id: Uuid::new_v4(),
                collection_id: "stats_collection".to_string(),
                vector: vec![(batch * 10 + i) as f32; 64],
                metadata: {
                    let mut meta = HashMap::new();
                    meta.insert("batch".to_string(), serde_json::json!(batch));
                    meta
                },
                timestamp: Utc::now(),
            };
            storage.write(record).await.unwrap();
        }
        
        // Check stats after each batch
        let metadata = storage.get_collection_metadata(&"stats_collection".to_string()).await.unwrap().unwrap();
        assert_eq!(metadata.vector_count, (batch + 1) * 10);
        assert!(metadata.total_size_bytes > 0);
    }
    
    // Final stats
    let final_metadata = storage.get_collection_metadata(&"stats_collection".to_string()).await.unwrap().unwrap();
    assert_eq!(final_metadata.vector_count, 30);
    
    storage.stop().await.unwrap();
}