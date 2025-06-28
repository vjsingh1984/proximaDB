//! Simple integration test to verify LSM storage engine integration
//!
//! This test verifies that the LSM storage engine works correctly with the
//! unified storage engine trait and testing infrastructure.

use anyhow::Result;
use std::sync::Arc;
use tempfile::TempDir;

use proximadb::storage::engines::lsm::LsmTree;
use proximadb::storage::traits::{UnifiedStorageEngine, FlushParameters, CompactionParameters};
use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use proximadb::storage::persistence::wal::{WalManager, WalConfig, WalFactory, WalStrategyType};
use proximadb::core::{LsmConfig, VectorRecord};
use chrono::Utc;

/// Helper function to create LSM tree with WAL manager
async fn create_lsm_tree_with_wal(temp_dir: &TempDir) -> Result<LsmTree> {
    // Create filesystem factory
    let fs_config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::new(fs_config).await?);
    
    // Create WAL manager
    let mut wal_config = WalConfig::default();
    wal_config.strategy_type = WalStrategyType::Avro;
    wal_config.multi_disk.data_directories = vec![temp_dir.path().to_path_buf()];
    
    let strategy = WalFactory::create_from_config(&wal_config, filesystem).await?;
    let wal_manager = Arc::new(WalManager::new(strategy, wal_config).await?);
    
    // Create LSM config
    let lsm_config = LsmConfig {
        memtable_size_mb: 1, // Small size for testing
        level_count: 4,
        compaction_threshold: 4,
        block_size_kb: 64,
    };
    
    // Create LSM tree
    let collection_id = "test_collection".to_string();
    let lsm_tree = LsmTree::new(
        &lsm_config,
        collection_id,
        wal_manager,
        temp_dir.path().to_path_buf(),
        None, // No compaction manager for tests
    );
    
    Ok(lsm_tree)
}

/// Test LSM storage engine trait implementation
#[tokio::test]
async fn test_lsm_engine_trait_integration() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let lsm_engine = create_lsm_tree_with_wal(&temp_dir).await?;
    
    // Test basic trait methods
    assert_eq!(lsm_engine.engine_name(), "lsm");
    assert_eq!(lsm_engine.engine_version(), "1.0.0");
    
    // Test capabilities
    assert!(!lsm_engine.supports_collection_level_operations()); // LSM doesn't support collection-level ops
    assert!(!lsm_engine.supports_atomic_operations()); // LSM has eventual consistency
    assert!(lsm_engine.supports_background_operations()); // LSM supports background ops
    
    println!("✅ LSM engine trait integration verified");
    println!("   - Engine: {} v{}", lsm_engine.engine_name(), lsm_engine.engine_version());
    println!("   - Collection-level ops: {}", lsm_engine.supports_collection_level_operations());
    println!("   - Atomic ops: {}", lsm_engine.supports_atomic_operations());
    println!("   - Background ops: {}", lsm_engine.supports_background_operations());
    
    Ok(())
}

/// Test LSM engine statistics and health check
#[tokio::test]
async fn test_lsm_engine_stats_and_health() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let lsm_engine = create_lsm_tree_with_wal(&temp_dir).await?;
    
    // Test engine statistics
    let stats = lsm_engine.get_engine_stats().await?;
    assert_eq!(stats.engine_name, "lsm");
    assert_eq!(stats.engine_version, "1.0.0");
    assert_eq!(stats.total_storage_bytes, 0); // New engine should be empty
    assert_eq!(stats.collection_count, 0);
    
    println!("✅ LSM engine stats verified");
    println!("   - Storage bytes: {}", stats.total_storage_bytes);
    println!("   - Memory usage: {}", stats.memory_usage_bytes);
    println!("   - Collections: {}", stats.collection_count);
    
    // Test health check
    let health = lsm_engine.health_check().await?;
    assert!(health.healthy);
    assert_eq!(health.error_count, 0);
    assert!(health.response_time_ms >= 0.0);
    
    println!("✅ LSM engine health check verified");
    println!("   - Healthy: {}", health.healthy);
    println!("   - Status: {}", health.status);
    println!("   - Response time: {:.2}ms", health.response_time_ms);
    println!("   - Error count: {}", health.error_count);
    
    Ok(())
}

/// Test LSM engine flush operations with 10 records
#[tokio::test]
async fn test_lsm_engine_flush_operations() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let lsm_engine = create_lsm_tree_with_wal(&temp_dir).await?;
    
    // Add 10 test records to the LSM engine
    for i in 0..10 {
        let now = Utc::now().timestamp_millis();
        let vector_id = format!("vector_{}", i);
        let record = VectorRecord {
            id: vector_id.clone(),
            collection_id: "test_collection".to_string(),
            vector: vec![i as f32, (i + 1) as f32, (i + 2) as f32, (i + 3) as f32],
            metadata: std::collections::HashMap::new(),
            timestamp: now,
            created_at: now,
            updated_at: now,
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        };
        
        // Use LSM's direct put method to add records
        lsm_engine.put(vector_id, record).await?;
    }
    
    println!("📝 Added 10 records to LSM engine");
    
    // Test LSM's direct flush method (returns Result<()>)
    let flush_result = lsm_engine.flush().await;
    assert!(flush_result.is_ok());
    
    println!("✅ LSM engine flush operations verified");
    println!("   - Direct flush: Success");
    println!("   - 10 records successfully added and flushed");
    
    Ok(())
}

/// Test LSM engine compaction operations with 10 records
#[tokio::test]
async fn test_lsm_engine_compaction_operations() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let lsm_engine = create_lsm_tree_with_wal(&temp_dir).await?;
    
    // Add 10 test records to the LSM engine
    for i in 0..10 {
        let now = Utc::now().timestamp_millis();
        let vector_id = format!("compact_vector_{}", i);
        let record = VectorRecord {
            id: vector_id.clone(),
            collection_id: "test_collection".to_string(),
            vector: vec![i as f32 + 10.0, (i + 1) as f32 + 10.0, (i + 2) as f32 + 10.0, (i + 3) as f32 + 10.0],
            metadata: std::collections::HashMap::new(),
            timestamp: now,
            created_at: now,
            updated_at: now,
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        };
        
        // Use LSM's direct put method to add records
        lsm_engine.put(vector_id, record).await?;
    }
    
    println!("📝 Added 10 records for compaction test");
    
    // First flush to create some SST files
    let _flush_result = lsm_engine.flush().await?;
    
    // Test unified storage engine trait compaction
    let compact_params = CompactionParameters::new()
        .force()
        .synchronous();
        
    let compact_result = lsm_engine.compact(compact_params).await?;
    assert!(compact_result.success);
    assert!(compact_result.duration_ms >= 0);
    
    println!("✅ LSM engine compaction operations verified");
    println!("   - Success: {}", compact_result.success);
    println!("   - Duration: {}ms", compact_result.duration_ms);
    println!("   - Entries processed: {}", if compact_result.entries_processed == u64::MAX { 
        "uninitialized".to_string() 
    } else { 
        compact_result.entries_processed.to_string() 
    });
    println!("   - Entries removed: {}", if compact_result.entries_removed == u64::MAX {
        "uninitialized".to_string()
    } else {
        compact_result.entries_removed.to_string()
    });
    
    Ok(())
}

