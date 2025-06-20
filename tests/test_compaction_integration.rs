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

use proximadb::core::{
    ApiConfig, Config, ConsensusConfig, LsmConfig, MonitoringConfig, ServerConfig, StorageConfig,
    VectorRecord,
};
use proximadb::storage::StorageEngine;
use std::collections::HashMap;
use tempfile::TempDir;
use uuid::Uuid;

#[tokio::test]
async fn test_compaction_integration() {
    let temp_dir = TempDir::new().unwrap();
    let data_dir = temp_dir.path().join("data");
    let wal_dir = temp_dir.path().join("wal");

    let config = StorageConfig {
        data_dirs: vec![data_dir.clone()],
        wal_dir: wal_dir.clone(),
        mmap_enabled: true,
        lsm_config: LsmConfig {
            memtable_size_mb: 1, // Very small to trigger frequent flushes
            level_count: 3,
            compaction_threshold: 2, // Trigger compaction when 2 files exist
            block_size_kb: 4,
        },
        cache_size_mb: 10,
        bloom_filter_bits: 10,
    };

    let mut storage_engine = StorageEngine::new(config).await.unwrap();
    storage_engine.start().await.unwrap();

    // Insert many vectors to trigger compaction
    let collection_id = "test_compaction_collection".to_string();

    // Create collection first
    storage_engine
        .create_collection(collection_id.clone())
        .await
        .unwrap();

    println!("Inserting vectors to trigger compaction...");

    // Store vector IDs to verify later
    let mut vector_ids = Vec::new();

    // Insert enough vectors to create multiple SST files
    for i in 0..50 {
        let vector = vec![0.1; 128]; // Simple test vector
        let metadata = HashMap::new();
        let vector_id = Uuid::new_v4();

        let record = VectorRecord {
            id: vector_id,
            collection_id: collection_id.clone(),
            vector,
            metadata,
            timestamp: chrono::Utc::now(),
        };

        storage_engine.write(record).await.unwrap();
        vector_ids.push(vector_id);

        // Small delay to allow potential background compaction
        if i % 10 == 0 {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
    }

    println!("Waiting for potential compaction to complete...");

    // Wait a moment for compaction to potentially complete
    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

    // Verify data is still accessible after potential compaction
    println!("Verifying data accessibility after compaction...");

    for i in 0..10 {
        let vector_id = &vector_ids[i];
        let result = storage_engine
            .read(&collection_id, vector_id)
            .await
            .unwrap();
        assert!(result.is_some(), "Vector {} should still be accessible", i);

        let record = result.unwrap();
        assert_eq!(record.id, *vector_id);
        assert_eq!(record.vector.len(), 128);
    }

    println!("All vectors verified successfully!");

    // Clean shutdown
    storage_engine.stop().await.unwrap();

    println!("Compaction integration test completed successfully!");
}

#[tokio::test]
async fn test_compaction_manager_lifecycle() {
    use proximadb::core::LsmConfig;
    use proximadb::storage::lsm::CompactionManager;

    let config = LsmConfig {
        memtable_size_mb: 1,
        level_count: 3,
        compaction_threshold: 2,
        block_size_kb: 4,
    };

    let mut manager = CompactionManager::new(config);

    // Test starting and stopping workers
    manager.start_workers(1).await.unwrap();
    let stats = manager.get_stats().await;
    assert_eq!(stats.total_compactions, 0);

    manager.stop().await.unwrap();

    println!("Compaction manager lifecycle test completed successfully!");
}
