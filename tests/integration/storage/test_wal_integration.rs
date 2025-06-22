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
use proximadb::storage::{StorageEngine, WalConfig, WalManager};
use std::collections::HashMap;
use std::path::PathBuf;
use tempfile::TempDir;
use uuid::Uuid;

#[tokio::test]
async fn test_wal_persistence_and_recovery() {
    let temp_dir = TempDir::new().unwrap();
    let data_dir = temp_dir.path().join("data");
    let wal_dir = temp_dir.path().join("wal");

    // Create some test data
    let collection_id = "test_collection";
    let vectors = vec![
        VectorRecord {
            id: Uuid::new_v4(),
            collection_id: collection_id.to_string(),
            vector: vec![1.0, 2.0, 3.0, 4.0],
            metadata: HashMap::new(),
            timestamp: Utc::now(),
        },
        VectorRecord {
            id: Uuid::new_v4(),
            collection_id: collection_id.to_string(),
            vector: vec![5.0, 6.0, 7.0, 8.0],
            metadata: HashMap::new(),
            timestamp: Utc::now(),
        },
    ];

    let vector_ids: Vec<Uuid> = vectors.iter().map(|v| v.id).collect();

    // Phase 1: Write data with WAL
    {
        let config = StorageConfig {
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
        };

        let mut storage = StorageEngine::new(config).await.unwrap();
        storage.start().await.unwrap();

        // Create collection
        storage
            .create_collection(collection_id.to_string())
            .await
            .unwrap();

        // Write vectors
        for vector in &vectors {
            storage.write(vector.clone()).await.unwrap();
        }

        // Verify data is readable
        for (i, id) in vector_ids.iter().enumerate() {
            let result = storage.read(&collection_id.to_string(), id).await.unwrap();
            assert!(result.is_some());
            let record = result.unwrap();
            assert_eq!(record.vector, vectors[i].vector);
        }

        // Stop the storage engine (simulating crash without flush)
        // Note: We're NOT calling stop() to simulate an ungraceful shutdown
        drop(storage);
    }

    // Phase 2: Recover from WAL
    {
        let config = StorageConfig {
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
        };

        let mut storage = StorageEngine::new(config).await.unwrap();
        storage.start().await.unwrap();

        // Data should be recovered from WAL
        for (i, id) in vector_ids.iter().enumerate() {
            let result = storage.read(&collection_id.to_string(), id).await.unwrap();
            assert!(
                result.is_some(),
                "Vector {} should be recovered from WAL",
                id
            );
            let record = result.unwrap();
            assert_eq!(record.vector, vectors[i].vector);
        }

        // Clean shutdown this time
        storage.stop().await.unwrap();
    }
}

#[tokio::test]
async fn test_wal_rotation() {
    let temp_dir = TempDir::new().unwrap();
    let wal_dir = temp_dir.path().join("wal");

    let config = WalConfig {
        wal_dir: wal_dir.clone(),
        segment_size: 1024, // Small size to force rotation
        sync_mode: true,
        retention_segments: 2,
    };

    let wal = WalManager::new(config).await.unwrap();

    // Write enough data to trigger rotation
    for i in 0..100 {
        let entry = proximadb::storage::WalEntry::Put {
            collection_id: "test".to_string(),
            record: VectorRecord {
                id: Uuid::new_v4(),
                collection_id: "test".to_string(),
                vector: vec![i as f32; 128],
                metadata: HashMap::new(),
                timestamp: Utc::now(),
            },
            timestamp: Utc::now(),
        };
        wal.append(entry).await.unwrap();
    }

    // Check that rotation happened by listing WAL files
    let mut entries = tokio::fs::read_dir(&wal_dir).await.unwrap();
    let mut wal_files = Vec::new();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        if entry.file_name().to_str().unwrap().starts_with("wal_") {
            wal_files.push(entry.file_name());
        }
    }

    assert!(
        wal_files.len() > 1,
        "WAL should have rotated to multiple segments"
    );

    // Verify all entries can be read
    let all_entries = wal.read_all().await.unwrap();
    assert!(all_entries.len() >= 100);
}

#[tokio::test]
async fn test_wal_checksum_validation() {
    let temp_dir = TempDir::new().unwrap();
    let wal_dir = temp_dir.path().join("wal");

    let config = WalConfig {
        wal_dir: wal_dir.clone(),
        segment_size: 64 * 1024 * 1024,
        sync_mode: true,
        retention_segments: 3,
    };

    // Write a valid entry
    {
        let wal = WalManager::new(config.clone()).await.unwrap();
        let entry = proximadb::storage::WalEntry::CreateCollection {
            collection_id: "test".to_string(),
            timestamp: Utc::now(),
        };
        wal.append(entry).await.unwrap();
    }

    // Corrupt the WAL file
    let mut entries = tokio::fs::read_dir(&wal_dir).await.unwrap();
    if let Some(entry) = entries.next_entry().await.unwrap() {
        let path = entry.path();
        if path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("wal_")
        {
            let mut data = tokio::fs::read(&path).await.unwrap();
            // Corrupt some bytes in the middle
            if data.len() > 20 {
                data[15] ^= 0xFF;
                data[16] ^= 0xFF;
            }
            tokio::fs::write(&path, data).await.unwrap();
        }
    }

    // Try to read the corrupted WAL
    let wal = WalManager::new(config).await.unwrap();
    let result = wal.read_all().await;

    // Should fail with corruption error
    assert!(result.is_err());
    if let Err(e) = result {
        match e {
            proximadb::core::StorageError::Corruption(msg) => {
                assert!(msg.contains("checksum"));
            }
            _ => panic!("Expected corruption error, got: {:?}", e),
        }
    }
}
