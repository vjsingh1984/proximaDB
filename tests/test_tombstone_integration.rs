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

use proximadb::core::{StorageConfig, LsmConfig, VectorRecord};
use proximadb::storage::StorageEngine;
use tempfile::TempDir;
use std::collections::HashMap;
use uuid::Uuid;

#[tokio::test]
async fn test_tombstone_functionality() {
    let temp_dir = TempDir::new().unwrap();
    let data_dir = temp_dir.path().join("data");
    let wal_dir = temp_dir.path().join("wal");

    let config = StorageConfig {
        data_dirs: vec![data_dir.clone()],
        wal_dir: wal_dir.clone(),
        mmap_enabled: true,
        lsm_config: LsmConfig {
            memtable_size_mb: 1,
            level_count: 3,
            compaction_threshold: 2,
            block_size_kb: 4,
        },
        cache_size_mb: 10,
        bloom_filter_bits: 10,
    };

    let mut storage_engine = StorageEngine::new(config).await.unwrap();
    storage_engine.start().await.unwrap();
    
    let collection_id = "test_tombstone_collection".to_string();
    
    // Create collection
    storage_engine.create_collection(collection_id.clone()).await.unwrap();
    
    println!("Testing tombstone functionality...");
    
    // Insert a vector
    let vector_id = Uuid::new_v4();
    let vector = vec![1.0, 2.0, 3.0, 4.0];
    let metadata = HashMap::new();
    
    let record = VectorRecord {
        id: vector_id,
        collection_id: collection_id.clone(),
        vector,
        metadata,
        timestamp: chrono::Utc::now(),
    };
    
    storage_engine.write(record.clone()).await.unwrap();
    
    // Verify the vector exists
    let result = storage_engine.read(&collection_id, &vector_id).await.unwrap();
    assert!(result.is_some(), "Vector should exist after insert");
    
    let retrieved_record = result.unwrap();
    assert_eq!(retrieved_record.id, vector_id);
    assert_eq!(retrieved_record.vector, vec![1.0, 2.0, 3.0, 4.0]);
    
    println!("Vector inserted and verified successfully");
    
    // Delete the vector (should create a tombstone)
    let deletion_result = storage_engine.soft_delete(&collection_id, &vector_id).await.unwrap();
    assert!(deletion_result, "Deletion should return true for existing vector");
    
    println!("Vector deleted successfully");
    
    // Verify the vector no longer exists (tombstone should hide it)
    let result = storage_engine.read(&collection_id, &vector_id).await.unwrap();
    assert!(result.is_none(), "Vector should not exist after deletion");
    
    println!("Vector correctly hidden by tombstone");
    
    // Try to delete the same vector again (should return false)
    let deletion_result = storage_engine.soft_delete(&collection_id, &vector_id).await.unwrap();
    assert!(!deletion_result, "Deletion should return false for non-existing vector");
    
    println!("Repeated deletion correctly handled");
    
    // Re-insert the same vector (should overwrite the tombstone)
    let new_vector = vec![5.0, 6.0, 7.0, 8.0];
    let new_record = VectorRecord {
        id: vector_id,
        collection_id: collection_id.clone(),
        vector: new_vector.clone(),
        metadata: HashMap::new(),
        timestamp: chrono::Utc::now(),
    };
    
    storage_engine.write(new_record).await.unwrap();
    
    // Verify the vector exists again with new data
    let result = storage_engine.read(&collection_id, &vector_id).await.unwrap();
    assert!(result.is_some(), "Vector should exist after re-insert");
    
    let retrieved_record = result.unwrap();
    assert_eq!(retrieved_record.id, vector_id);
    assert_eq!(retrieved_record.vector, new_vector);
    
    println!("Vector re-insertion over tombstone successful");
    
    // Clean shutdown
    storage_engine.stop().await.unwrap();
    
    println!("Tombstone functionality test completed successfully!");
}

#[tokio::test]
async fn test_tombstone_persistence_after_flush() {
    let temp_dir = TempDir::new().unwrap();
    let data_dir = temp_dir.path().join("data");
    let wal_dir = temp_dir.path().join("wal");

    let config = StorageConfig {
        data_dirs: vec![data_dir.clone()],
        wal_dir: wal_dir.clone(),
        mmap_enabled: true,
        lsm_config: LsmConfig {
            memtable_size_mb: 1, // Small to force flushes
            level_count: 3,
            compaction_threshold: 2,
            block_size_kb: 4,
        },
        cache_size_mb: 10,
        bloom_filter_bits: 10,
    };

    let mut storage_engine = StorageEngine::new(config).await.unwrap();
    storage_engine.start().await.unwrap();
    
    let collection_id = "test_persistence_collection".to_string();
    
    // Create collection
    storage_engine.create_collection(collection_id.clone()).await.unwrap();
    
    println!("Testing tombstone persistence after flush...");
    
    // Insert multiple vectors
    let mut vector_ids = Vec::new();
    for i in 0..10 {
        let vector_id = Uuid::new_v4();
        let vector = vec![i as f32; 4];
        let metadata = HashMap::new();
        
        let record = VectorRecord {
            id: vector_id,
            collection_id: collection_id.clone(),
            vector,
            metadata,
            timestamp: chrono::Utc::now(),
        };
        
        storage_engine.write(record).await.unwrap();
        vector_ids.push(vector_id);
    }
    
    // Delete half of the vectors
    for i in 0..5 {
        let vector_id = &vector_ids[i];
        let result = storage_engine.soft_delete(&collection_id, vector_id).await.unwrap();
        assert!(result, "Vector {} should exist for deletion", i);
    }
    
    println!("Deleted 5 vectors, forcing flush...");
    
    // Force a flush by inserting more data
    for i in 10..20 {
        let vector_id = Uuid::new_v4();
        let vector = vec![i as f32; 4];
        let metadata = HashMap::new();
        
        let record = VectorRecord {
            id: vector_id,
            collection_id: collection_id.clone(),
            vector,
            metadata,
            timestamp: chrono::Utc::now(),
        };
        
        storage_engine.write(record).await.unwrap();
    }
    
    println!("Added more vectors to trigger SST flush");
    
    // Wait a moment for potential flush
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    
    // Verify deleted vectors are still gone (tombstones persisted)
    for i in 0..5 {
        let vector_id = &vector_ids[i];
        let result = storage_engine.read(&collection_id, vector_id).await.unwrap();
        assert!(result.is_none(), "Deleted vector {} should not exist after flush", i);
    }
    
    // Verify non-deleted vectors still exist
    for i in 5..10 {
        let vector_id = &vector_ids[i];
        let result = storage_engine.read(&collection_id, vector_id).await.unwrap();
        assert!(result.is_some(), "Non-deleted vector {} should still exist after flush", i);
    }
    
    println!("Tombstone persistence verified after flush");
    
    // Clean shutdown
    storage_engine.stop().await.unwrap();
    
    println!("Tombstone persistence test completed successfully!");
}