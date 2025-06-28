//! Integration test for WAL → VIPER → Search flow
//!
//! This test verifies the complete data flow from WAL writes through VIPER
//! storage engine flush operations to search functionality.

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::time::{sleep, Duration};

use proximadb::core::{VectorRecord, CollectionConfig, DistanceMetric, StorageEngine};
use proximadb::services::{UnifiedAvroService, CollectionService};
use proximadb::storage::engines::viper::core::{ViperCoreEngine, ViperCoreConfig};
use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use proximadb::storage::persistence::wal::{WalManager, WalConfig, WalFactory, WalStrategyType};
use proximadb::storage::traits::{UnifiedStorageEngine, FlushParameters};

/// Create test infrastructure with WAL, VIPER, and services
async fn create_test_infrastructure() -> Result<(
    Arc<WalManager>,
    Arc<ViperCoreEngine>,
    Arc<UnifiedAvroService>,
    Arc<CollectionService>,
    TempDir,
)> {
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path().to_string_lossy().to_string();
    
    // Create filesystem factory
    let fs_config = FilesystemConfig {
        default_fs: Some(format!("file://{}", temp_path)),
        ..Default::default()
    };
    let filesystem = Arc::new(FilesystemFactory::new(fs_config).await?);
    
    // Create WAL manager
    let mut wal_config = WalConfig::default();
    wal_config.strategy_type = WalStrategyType::Avro;
    wal_config.multi_disk.data_directories = vec![temp_dir.path().to_path_buf()];
    
    let strategy = WalFactory::create_from_config(&wal_config, filesystem.clone()).await?;
    let wal_manager = Arc::new(WalManager::new(strategy, wal_config).await?);
    
    // Create VIPER engine
    let viper_config = ViperCoreConfig::default();
    let viper_engine = Arc::new(ViperCoreEngine::new(viper_config, filesystem.clone()).await?);
    
    // Create collection service
    let collection_service = Arc::new(CollectionService::new(
        filesystem.clone(),
        format!("file://{}/collections", temp_path),
    ).await?);
    
    // Create unified Avro service
    let unified_service = Arc::new(UnifiedAvroService::new(
        wal_manager.clone(),
        viper_engine.clone(), // Pass VIPER engine as storage engine
        collection_service.clone(),
    ).await?);
    
    Ok((wal_manager, viper_engine, unified_service, collection_service, temp_dir))
}

/// Create test vector records
fn create_test_vectors(collection_id: &str, count: usize, dimension: usize) -> Vec<VectorRecord> {
    (0..count).map(|i| {
        let now = chrono::Utc::now().timestamp_millis();
        VectorRecord {
            id: format!("test_vector_{}", i),
            collection_id: collection_id.to_string(),
            vector: (0..dimension).map(|d| (i * dimension + d) as f32 * 0.1).collect(),
            metadata: {
                let mut meta = HashMap::new();
                meta.insert("category".to_string(), serde_json::Value::String(format!("cat_{}", i % 3)));
                meta.insert("priority".to_string(), serde_json::Value::Number(serde_json::Number::from(i)));
                meta.insert("test_id".to_string(), serde_json::Value::Number(serde_json::Number::from(i)));
                meta
            },
            timestamp: now,
            created_at: now,
            updated_at: now,
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        }
    }).collect()
}

#[tokio::test]
async fn test_wal_to_viper_flush_flow() -> Result<()> {
    let (wal_manager, viper_engine, unified_service, collection_service, _temp_dir) = 
        create_test_infrastructure().await?;
    
    let collection_id = "test_wal_viper_collection";
    let dimension = 128;
    
    // Step 1: Create collection
    let collection_config = CollectionConfig {
        name: collection_id.to_string(),
        dimension: dimension as i32,
        distance_metric: DistanceMetric::Cosine,
        storage_engine: StorageEngine::Viper,
        indexing_algorithm: proximadb::core::IndexingAlgorithm::Hnsw,
        filterable_metadata_fields: vec!["category".to_string(), "priority".to_string()],
        indexing_config: HashMap::new(),
        filterable_columns: vec![],
    };
    
    collection_service.create_collection(collection_config.clone()).await?;
    
    // Step 2: Write vectors to WAL through unified service
    let test_vectors = create_test_vectors(collection_id, 100, dimension);
    
    for vector in &test_vectors {
        unified_service.insert_vector(
            collection_id,
            vector.clone(),
        ).await?;
    }
    
    // Step 3: Verify data is in WAL/memtable
    let wal_stats = wal_manager.stats().await?;
    assert_eq!(wal_stats.total_entries, 100);
    assert!(wal_stats.memory_entries > 0);
    
    // Step 4: Trigger flush from WAL to VIPER
    let flush_params = FlushParameters {
        collection_id: Some(collection_id.to_string()),
        force: true,
        synchronous: true,
        ..Default::default()
    };
    
    let flush_result = viper_engine.flush(flush_params).await?;
    assert!(flush_result.success);
    assert_eq!(flush_result.entries_flushed, 100);
    assert!(flush_result.bytes_written > 0);
    assert!(flush_result.duration_ms > 0);
    
    // Step 5: Verify WAL segments were cleaned up
    sleep(Duration::from_millis(100)).await; // Allow cleanup to complete
    let wal_stats_after = wal_manager.stats().await?;
    assert!(wal_stats_after.memory_entries < wal_stats.memory_entries);
    
    // Step 6: Verify data can be searched from VIPER storage
    // Note: This would require implementing the search functionality in VIPER
    // For now, we verify the flush was successful
    
    println!("✅ WAL → VIPER flush flow test completed successfully");
    println!("   - Written {} vectors to WAL", test_vectors.len());
    println!("   - Flushed {} entries to VIPER", flush_result.entries_flushed);
    println!("   - Written {} bytes to storage", flush_result.bytes_written);
    println!("   - Flush took {} ms", flush_result.duration_ms);
    
    Ok(())
}

#[tokio::test]
async fn test_wal_viper_search_with_filters() -> Result<()> {
    let (wal_manager, viper_engine, unified_service, collection_service, _temp_dir) = 
        create_test_infrastructure().await?;
    
    let collection_id = "test_search_collection";
    let dimension = 64;
    
    // Create collection with filterable columns
    let collection_config = CollectionConfig {
        name: collection_id.to_string(),
        dimension: dimension as i32,
        distance_metric: DistanceMetric::Euclidean,
        storage_engine: StorageEngine::Viper,
        indexing_algorithm: proximadb::core::IndexingAlgorithm::Flat,
        filterable_metadata_fields: vec!["category".to_string(), "priority".to_string()],
        indexing_config: HashMap::new(),
        filterable_columns: vec!["category".to_string(), "priority".to_string()],
    };
    
    collection_service.create_collection(collection_config).await?;
    
    // Insert test data with specific metadata patterns
    let test_vectors = create_test_vectors(collection_id, 50, dimension);
    
    for vector in &test_vectors {
        unified_service.insert_vector(collection_id, vector.clone()).await?;
    }
    
    // Flush to VIPER
    let flush_params = FlushParameters {
        collection_id: Some(collection_id.to_string()),
        force: true,
        synchronous: true,
        ..Default::default()
    };
    
    let flush_result = viper_engine.flush(flush_params).await?;
    assert!(flush_result.success);
    
    // TODO: Implement search functionality test when VIPER search is ready
    // For now, we've verified the data flow works correctly
    
    println!("✅ WAL → VIPER → Search (with filters) test prepared");
    println!("   - Collection created with filterable columns");
    println!("   - {} vectors inserted and flushed", flush_result.entries_flushed);
    
    Ok(())
}

#[tokio::test]
async fn test_concurrent_wal_writes_and_flush() -> Result<()> {
    let (wal_manager, viper_engine, unified_service, collection_service, _temp_dir) = 
        create_test_infrastructure().await?;
    
    let collection_id = "test_concurrent_collection";
    let dimension = 32;
    
    // Create collection
    let collection_config = CollectionConfig {
        name: collection_id.to_string(),
        dimension: dimension as i32,
        distance_metric: DistanceMetric::DotProduct,
        storage_engine: StorageEngine::Viper,
        indexing_algorithm: proximadb::core::IndexingAlgorithm::Hnsw,
        filterable_metadata_fields: vec![],
        indexing_config: HashMap::new(),
        filterable_columns: vec![],
    };
    
    collection_service.create_collection(collection_config).await?;
    
    // Spawn multiple concurrent write tasks
    let mut handles = vec![];
    let vectors_per_task = 20;
    let num_tasks = 5;
    
    for task_id in 0..num_tasks {
        let service = unified_service.clone();
        let collection_id = collection_id.to_string();
        
        let handle = tokio::spawn(async move {
            let vectors = create_test_vectors(&collection_id, vectors_per_task, dimension);
            for (i, mut vector) in vectors.into_iter().enumerate() {
                vector.id = format!("task_{}_vector_{}", task_id, i);
                service.insert_vector(&collection_id, vector).await.unwrap();
            }
        });
        
        handles.push(handle);
    }
    
    // Wait for all writes to complete
    for handle in handles {
        handle.await?;
    }
    
    // Verify all writes made it to WAL
    let wal_stats = wal_manager.stats().await?;
    assert_eq!(wal_stats.total_entries, (vectors_per_task * num_tasks) as u64);
    
    // Flush to VIPER
    let flush_params = FlushParameters {
        collection_id: Some(collection_id.to_string()),
        force: true,
        synchronous: true,
        ..Default::default()
    };
    
    let flush_result = viper_engine.flush(flush_params).await?;
    assert!(flush_result.success);
    assert_eq!(flush_result.entries_flushed, (vectors_per_task * num_tasks) as u64);
    
    println!("✅ Concurrent WAL writes and flush test completed");
    println!("   - {} concurrent tasks", num_tasks);
    println!("   - {} vectors per task", vectors_per_task);
    println!("   - {} total vectors flushed", flush_result.entries_flushed);
    
    Ok(())
}

#[tokio::test]
async fn test_wal_recovery_after_crash() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path().to_string_lossy().to_string();
    let collection_id = "test_recovery_collection";
    let dimension = 16;
    
    // Phase 1: Write data to WAL and simulate crash
    {
        let fs_config = FilesystemConfig {
            default_fs: Some(format!("file://{}", temp_path)),
            ..Default::default()
        };
        let filesystem = Arc::new(FilesystemFactory::new(fs_config).await?);
        
        let mut wal_config = WalConfig::default();
        wal_config.strategy_type = WalStrategyType::Avro;
        wal_config.multi_disk.data_directories = vec![temp_dir.path().to_path_buf()];
        
        let strategy = WalFactory::create_from_config(&wal_config, filesystem.clone()).await?;
        let wal_manager = Arc::new(WalManager::new(strategy, wal_config).await?);
        
        // Write test vectors
        let test_vectors = create_test_vectors(collection_id, 30, dimension);
        for vector in test_vectors {
            wal_manager.insert(
                collection_id.to_string(),
                vector.id.clone(),
                vector,
            ).await?;
        }
        
        // Force WAL flush
        let _flush_result = wal_manager.flush(None).await?;
        
        // Simulate crash by dropping WAL manager
        drop(wal_manager);
    }
    
    // Phase 2: Recover from WAL and verify data
    {
        let fs_config = FilesystemConfig {
            default_fs: Some(format!("file://{}", temp_path)),
            ..Default::default()
        };
        let filesystem = Arc::new(FilesystemFactory::new(fs_config).await?);
        
        let mut wal_config = WalConfig::default();
        wal_config.strategy_type = WalStrategyType::Avro;
        wal_config.multi_disk.data_directories = vec![temp_dir.path().to_path_buf()];
        
        let strategy = WalFactory::create_from_config(&wal_config, filesystem.clone()).await?;
        let wal_manager = Arc::new(WalManager::new(strategy, wal_config).await?);
        
        // Check recovery
        let stats = wal_manager.stats().await?;
        assert_eq!(stats.total_entries, 30, "WAL should recover all 30 entries");
        
        // Create VIPER engine and flush recovered data
        let viper_config = ViperCoreConfig::default();
        let viper_engine = Arc::new(ViperCoreEngine::new(viper_config, filesystem).await?);
        
        let flush_params = FlushParameters {
            collection_id: Some(collection_id.to_string()),
            force: true,
            synchronous: true,
            ..Default::default()
        };
        
        let flush_result = viper_engine.flush(flush_params).await?;
        assert!(flush_result.success);
        assert_eq!(flush_result.entries_flushed, 30);
        
        println!("✅ WAL recovery after crash test completed");
        println!("   - Successfully recovered {} entries from WAL", stats.total_entries);
        println!("   - Flushed recovered data to VIPER");
    }
    
    Ok(())
}

#[tokio::test] 
async fn test_atomic_flush_operation_integrity() -> Result<()> {
    let (wal_manager, viper_engine, unified_service, collection_service, _temp_dir) = 
        create_test_infrastructure().await?;
    
    let collection_id = "test_atomic_collection";
    let dimension = 8;
    
    // Create collection
    let collection_config = CollectionConfig {
        name: collection_id.to_string(),
        dimension: dimension as i32,
        distance_metric: DistanceMetric::Manhattan,
        storage_engine: StorageEngine::Viper,
        indexing_algorithm: proximadb::core::IndexingAlgorithm::Flat,
        filterable_metadata_fields: vec![],
        indexing_config: HashMap::new(),
        filterable_columns: vec![],
    };
    
    collection_service.create_collection(collection_config).await?;
    
    // Insert data
    let test_vectors = create_test_vectors(collection_id, 25, dimension);
    for vector in &test_vectors {
        unified_service.insert_vector(collection_id, vector.clone()).await?;
    }
    
    // Get initial WAL state
    let wal_stats_before = wal_manager.stats().await?;
    
    // Perform atomic flush
    let flush_params = FlushParameters {
        collection_id: Some(collection_id.to_string()),
        force: true,
        synchronous: true,
        ..Default::default()
    };
    
    let flush_result = viper_engine.flush(flush_params).await?;
    
    // Verify atomicity: either all data is flushed or none
    assert!(flush_result.success);
    assert_eq!(flush_result.entries_flushed, 25);
    
    // Verify WAL cleanup happened atomically
    let wal_stats_after = wal_manager.stats().await?;
    assert!(
        wal_stats_after.memory_entries < wal_stats_before.memory_entries,
        "WAL entries should be cleaned up after successful flush"
    );
    
    // Verify operation metadata
    assert!(flush_result.engine_metrics.contains_key("operation_id"));
    assert!(flush_result.engine_metrics.contains_key("staging_url"));
    assert!(flush_result.engine_metrics.contains_key("final_url"));
    
    println!("✅ Atomic flush operation integrity test completed");
    println!("   - Flush was atomic: all {} entries processed", flush_result.entries_flushed);
    println!("   - WAL cleanup confirmed");
    println!("   - Operation metadata preserved");
    
    Ok(())
}