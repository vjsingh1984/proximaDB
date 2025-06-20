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

use chrono::Utc;
use proximadb::core::{LsmConfig, StorageConfig, VectorRecord};
use proximadb::storage::StorageEngine;
use std::collections::HashMap;
use std::path::PathBuf;
use tempfile::TempDir;
use uuid::Uuid;

#[tokio::test]
async fn test_lsm_to_mmap_pipeline() {
    let temp_dir = TempDir::new().unwrap();
    let data_dir = temp_dir.path().join("data");
    let wal_dir = temp_dir.path().join("wal");

    let config = StorageConfig {
        data_dirs: vec![data_dir.clone()],
        wal_dir: wal_dir.clone(),
        mmap_enabled: true,
        lsm_config: LsmConfig {
            memtable_size_mb: 1, // Small to trigger flush
            level_count: 7,
            compaction_threshold: 4,
            block_size_kb: 64,
        },
        cache_size_mb: 10,
        bloom_filter_bits: 10,
    };

    let mut storage = StorageEngine::new(config).await.unwrap();
    storage.start().await.unwrap();

    let collection_id = "test_collection";
    storage
        .create_collection(collection_id.to_string())
        .await
        .unwrap();

    // Write enough data to trigger a flush to SST
    let mut vector_ids = Vec::new();
    for i in 0..1000 {
        let record = VectorRecord {
            id: Uuid::new_v4(),
            collection_id: collection_id.to_string(),
            vector: vec![i as f32; 128],
            metadata: HashMap::new(),
            timestamp: Utc::now(),
        };
        vector_ids.push(record.id);
        storage.write(record).await.unwrap();
    }

    // Force a flush by stopping and restarting
    storage.stop().await.unwrap();

    // Restart storage to ensure data is read from SST files via MMAP
    let mut storage = StorageEngine::new(StorageConfig {
        data_dirs: vec![data_dir.clone()],
        wal_dir: wal_dir.clone(),
        mmap_enabled: true,
        lsm_config: LsmConfig {
            memtable_size_mb: 1,
            level_count: 7,
            compaction_threshold: 4,
            block_size_kb: 64,
        },
        cache_size_mb: 10,
        bloom_filter_bits: 10,
    })
    .await
    .unwrap();
    storage.start().await.unwrap();

    // Verify all data is readable
    for (i, id) in vector_ids.iter().enumerate() {
        let result = storage.read(&collection_id.to_string(), id).await.unwrap();
        assert!(result.is_some(), "Vector {} should be readable", id);
        let record = result.unwrap();
        assert_eq!(record.vector[0], i as f32);
    }
}

#[tokio::test]
async fn test_mixed_lsm_mmap_reads() {
    let temp_dir = TempDir::new().unwrap();
    let data_dir = temp_dir.path().join("data");
    let wal_dir = temp_dir.path().join("wal");

    let config = StorageConfig {
        data_dirs: vec![data_dir.clone()],
        wal_dir: wal_dir.clone(),
        mmap_enabled: true,
        lsm_config: LsmConfig {
            memtable_size_mb: 10, // Large enough to not auto-flush
            level_count: 7,
            compaction_threshold: 4,
            block_size_kb: 64,
        },
        cache_size_mb: 10,
        bloom_filter_bits: 10,
    };

    let mut storage = StorageEngine::new(config).await.unwrap();
    storage.start().await.unwrap();

    let collection_id = "test_collection";
    storage
        .create_collection(collection_id.to_string())
        .await
        .unwrap();

    // Write some data and force flush
    let old_record = VectorRecord {
        id: Uuid::new_v4(),
        collection_id: collection_id.to_string(),
        vector: vec![1.0; 128],
        metadata: HashMap::new(),
        timestamp: Utc::now(),
    };
    let old_id = old_record.id;
    storage.write(old_record).await.unwrap();

    // Force flush by restarting
    storage.stop().await.unwrap();
    let mut storage = StorageEngine::new(StorageConfig {
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
    })
    .await
    .unwrap();
    storage.start().await.unwrap();

    // Write new data (stays in memtable)
    let new_record = VectorRecord {
        id: Uuid::new_v4(),
        collection_id: collection_id.to_string(),
        vector: vec![2.0; 128],
        metadata: HashMap::new(),
        timestamp: Utc::now(),
    };
    let new_id = new_record.id;
    storage.write(new_record).await.unwrap();

    // Read old data (from MMAP)
    let old_result = storage
        .read(&collection_id.to_string(), &old_id)
        .await
        .unwrap();
    assert!(old_result.is_some());
    assert_eq!(old_result.unwrap().vector[0], 1.0);

    // Read new data (from memtable)
    let new_result = storage
        .read(&collection_id.to_string(), &new_id)
        .await
        .unwrap();
    assert!(new_result.is_some());
    assert_eq!(new_result.unwrap().vector[0], 2.0);
}

#[tokio::test]
async fn test_storage_soft_delete() {
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

    let collection_id = "test_collection";
    storage
        .create_collection(collection_id.to_string())
        .await
        .unwrap();

    // Write a record
    let record = VectorRecord {
        id: Uuid::new_v4(),
        collection_id: collection_id.to_string(),
        vector: vec![1.0; 128],
        metadata: HashMap::new(),
        timestamp: Utc::now(),
    };
    let id = record.id;
    storage.write(record).await.unwrap();

    // Verify it exists
    let result = storage.read(&collection_id.to_string(), &id).await.unwrap();
    assert!(result.is_some());

    // Soft delete
    let deleted = storage
        .soft_delete(&collection_id.to_string(), &id)
        .await
        .unwrap();
    assert!(deleted);

    // TODO: Once delete is fully implemented, verify the record is marked as deleted
    // For now, soft delete just logs to WAL
}
