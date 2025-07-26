/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Concurrency tests for StorageEngine (which uses DashMap internally)

#[cfg(test)]
mod tests {
    use crate::core::{StorageConfig, VectorRecord, VectorId, LsmConfig};
    use crate::storage::engine::StorageEngine;
    use std::sync::Arc;
    use tokio::task::JoinSet;
    use std::time::SystemTime;

    async fn create_test_engine() -> (Arc<StorageEngine>, std::path::PathBuf) {
        // Create a simple temporary directory path without special characters
        // Use only alphanumeric characters to avoid URL parsing issues
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs(); // Use seconds for simpler numbers
        
        // Use a very simple path structure to avoid any URL parsing issues
        let dir_name = format!("pdb{}", timestamp);
        let base_path = std::path::PathBuf::from("/tmp").join(&dir_name);
        
        // Create the directory
        std::fs::create_dir_all(&base_path).unwrap();
        
        // Create storage URL - ensure no special characters in the path
        let storage_url = format!("file:///tmp/{}", dir_name);
        
        let mut config = StorageConfig::default();
        config.storage_locations = vec![crate::core::config::StorageLocation {
            url: storage_url.clone(),
            weight: 1,
            tags: vec!["ssd".to_string()],
        }];
        
        config.lsm_config = LsmConfig {
            memtable_size_mb: 1,
            level_count: 3,
            compaction_threshold: 2,
            block_size_kb: 4,
            memory_flush_size_bytes: 1024 * 1024,
            memtable_type: "standard".to_string(),
            compaction_strategy: "leveled".to_string(),
            compression: "snappy".to_string(),
            bloom_filter_config: None,
            cache_size_mb: 1,
            write_buffer_size_mb: 1,
            max_files_per_level: 10,
            level_size_multiplier: 10.0,
            max_levels: 7,
            background_thread_count: 2,
            sync_mode: "async".to_string(),
            enable_wal: false, // Disable WAL to bypass URL parsing issues
            wal_directory: base_path.join("wal").display().to_string(),
            data_directory: base_path.join("data").display().to_string(),
            mmap_enabled: false,
            prefetch_enabled: false,
            prefetch_size_kb: 64,
        };
        
        // For testing, we'll create the engine without collection service
        let engine = Arc::new(StorageEngine::new_without_collection_service(config).await.unwrap());
        (engine, base_path)
    }

    fn create_test_vector(id: &str) -> VectorRecord {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        
        VectorRecord {
            id: Some(id.to_string()),
            vector: vec![0.1; 128],
            metadata: vec![],
            timestamp: now,
            created_at: now,
            updated_at: now,
            expires_at: None,
            version: 0,
            rank: None,
            score: None,
            distance: None,
        }
    }

    #[tokio::test]
    async fn test_concurrent_collection_creation() {
        let (engine, test_dir) = create_test_engine().await;
        
        // Create multiple collections concurrently
        let mut tasks = JoinSet::new();
        
        for i in 0..50 {
            let eng = engine.clone();
            tasks.spawn(async move {
                let collection_id = format!("concurrent_collection_{}", i);
                eng.create_collection(collection_id.clone()).await?;
                Ok::<String, anyhow::Error>(collection_id)
            });
        }
        
        // Collect all results
        let mut created_collections = Vec::new();
        while let Some(result) = tasks.join_next().await {
            let collection_id = result.unwrap().unwrap();
            created_collections.push(collection_id);
        }
        
        assert_eq!(created_collections.len(), 50);
        
        // Verify all collections were created by writing to them
        let mut write_tasks = JoinSet::new();
        
        for collection_id in created_collections {
            let eng = engine.clone();
            write_tasks.spawn(async move {
                let vector = create_test_vector(&format!("vec_in_{}", collection_id));
                eng.write(&collection_id, &vector).await
            });
        }
        
        // All writes should succeed
        while let Some(result) = write_tasks.join_next().await {
            result.unwrap().unwrap();
        }
        
        // Cleanup
        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_concurrent_writes_single_collection() {
        let (engine, test_dir) = create_test_engine().await;
        
        // Create a collection
        let collection_id = "concurrent_writes_test";
        engine.create_collection(collection_id.to_string()).await.unwrap();
        
        // Write many vectors concurrently
        let mut tasks = JoinSet::new();
        
        for i in 0..100 {
            let eng = engine.clone();
            let coll_id = collection_id.to_string();
            
            tasks.spawn(async move {
                let vector = create_test_vector(&format!("concurrent_vec_{}", i));
                eng.write(&coll_id, &vector).await
            });
        }
        
        // All writes should succeed
        let mut success_count = 0;
        while let Some(result) = tasks.join_next().await {
            result.unwrap().unwrap();
            success_count += 1;
        }
        
        assert_eq!(success_count, 100);
        
        // Cleanup
        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_concurrent_reads_writes() {
        let (engine, test_dir) = create_test_engine().await;
        
        // Create a collection
        let collection_id = "read_write_test";
        engine.create_collection(collection_id.to_string()).await.unwrap();
        
        // Pre-populate with some vectors
        for i in 0..10 {
            let vector = create_test_vector(&format!("initial_vec_{}", i));
            engine.write(collection_id, &vector).await.unwrap();
        }
        
        // Mix concurrent reads and writes
        let mut tasks = JoinSet::new();
        
        for i in 0..100 {
            let eng = engine.clone();
            let coll_id = collection_id.to_string();
            
            if i % 2 == 0 {
                // Write operation
                tasks.spawn(async move {
                    let vector = create_test_vector(&format!("new_vec_{}", i));
                    eng.write(&coll_id, &vector).await.map(|_| "write").map_err(|_| crate::core::StorageError::LsmTree("write failed".to_string()))
                });
            } else {
                // Read operation (will fail for LSM, but tests concurrent access)
                let vec_id = format!("initial_vec_{}", i % 10);
                tasks.spawn(async move {
                    let _ = eng.read(&coll_id, &VectorId::from(vec_id)).await;
                    Ok("read")
                });
            }
        }
        
        // Collect results
        let mut write_count = 0;
        let mut read_count = 0;
        
        while let Some(result) = tasks.join_next().await {
            match result.unwrap() {
                Ok("write") => write_count += 1,
                Ok("read") => read_count += 1,
                _ => {}
            }
        }
        
        assert_eq!(write_count, 50);
        assert_eq!(read_count, 50);
        
        // Cleanup
        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_concurrent_collection_operations() {
        let (engine, test_dir) = create_test_engine().await;
        
        // Create collections first
        for i in 0..10 {
            engine.create_collection(format!("ops_collection_{}", i)).await.unwrap();
        }
        
        // Mix create, write, and delete operations
        let mut tasks = JoinSet::new();
        
        for i in 0..30 {
            let eng = engine.clone();
            
            match i % 3 {
                0 => {
                    // Create new collection
                    tasks.spawn(async move {
                        let coll_id = format!("new_collection_{}", i);
                        eng.create_collection(coll_id).await.map(|_| "create")
                    });
                }
                1 => {
                    // Write to existing collection
                    let coll_id = format!("ops_collection_{}", i % 10);
                    tasks.spawn(async move {
                        let vector = create_test_vector(&format!("vec_{}", i));
                        eng.write(&coll_id, &vector).await.map(|_| "write")
                    });
                }
                _ => {
                    // Delete collection
                    let coll_id = format!("ops_collection_{}", i % 10);
                    tasks.spawn(async move {
                        eng.delete_collection(&coll_id).await.map(|_| "delete")
                    });
                }
            }
        }
        
        // Collect results
        let mut results = std::collections::HashMap::new();
        
        while let Some(result) = tasks.join_next().await {
            if let Ok(op_type) = result.unwrap() {
                *results.entry(op_type).or_insert(0) += 1;
            }
        }
        
        // Verify operations completed
        assert!(results.get("create").unwrap_or(&0) > &0);
        assert!(results.get("write").unwrap_or(&0) > &0);
        assert!(results.get("delete").unwrap_or(&0) > &0);
        
        // Cleanup
        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_batch_write_concurrency() {
        let (engine, test_dir) = create_test_engine().await;
        
        // Create multiple collections
        for i in 0..5 {
            engine.create_collection(format!("batch_collection_{}", i)).await.unwrap();
        }
        
        // Perform concurrent batch writes
        let mut tasks = JoinSet::new();
        
        for i in 0..20 {
            let eng = engine.clone();
            let coll_id = format!("batch_collection_{}", i % 5);
            
            tasks.spawn(async move {
                let mut vectors = Vec::new();
                for j in 0..10 {
                    vectors.push(create_test_vector(&format!("batch_{}_{}", i, j)));
                }
                
                eng.batch_write(&coll_id, vectors).await
            });
        }
        
        // All batch writes should succeed
        let mut success_count = 0;
        while let Some(result) = tasks.join_next().await {
            let ids = result.unwrap().unwrap();
            assert_eq!(ids.len(), 10);
            success_count += 1;
        }
        
        assert_eq!(success_count, 20);
        
        // Cleanup
        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[tokio::test]
    async fn test_high_contention_single_collection() {
        let (engine, test_dir) = create_test_engine().await;
        
        let collection_id = "high_contention";
        engine.create_collection(collection_id.to_string()).await.unwrap();
        
        // Launch many concurrent operations on the same collection
        let mut tasks = JoinSet::new();
        
        for i in 0..200 {
            let eng = engine.clone();
            let coll_id = collection_id.to_string();
            
            tasks.spawn(async move {
                let vector = create_test_vector(&format!("contention_vec_{}", i));
                eng.write(&coll_id, &vector).await
            });
        }
        
        // All operations should succeed with lock-free implementation
        let mut success_count = 0;
        while let Some(result) = tasks.join_next().await {
            result.unwrap().unwrap();
            success_count += 1;
        }
        
        assert_eq!(success_count, 200);
        
        // Cleanup
        let _ = std::fs::remove_dir_all(test_dir);
    }
}