//! Simple integration test to verify WAL testing helpers work
//!
//! This test verifies that the testing infrastructure we added to the WAL layer
//! is properly wired and functional.

use anyhow::Result;
use std::sync::Arc;
use tempfile::TempDir;

use proximadb::storage::persistence::wal::{WalManager, WalConfig, WalFactory, WalStrategyType};
use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};

/// Test the WAL testing helper integration
#[tokio::test]
async fn test_wal_testing_helper_integration() -> Result<()> {
    let temp_dir = TempDir::new()?;
    
    // Create WAL manager using the same pattern as existing tests
    let mut config = WalConfig::default();
    config.strategy_type = WalStrategyType::Avro;
    config.multi_disk.data_directories = vec![temp_dir.path().to_path_buf()];
    
    // Create filesystem factory with proper config
    let fs_config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::new(fs_config).await?);
    let strategy = WalFactory::create_from_config(&config, filesystem).await?;
    let wal_manager = WalManager::new(strategy, config).await?;
    
    // Test the testing helper method we added
    let collection_id = "test_collection".to_string();
    let records = wal_manager.__internal_get_memtable_records_for_testing(&collection_id).await?;
    
    // The default implementation should return empty records
    assert_eq!(records.len(), 0);
    println!("✅ WAL testing helper integration verified - returned {} records", records.len());
    
    // Test WAL stats work
    let stats = wal_manager.stats().await?;
    assert_eq!(stats.total_entries, 0);
    println!("✅ WAL stats integration verified - {} total entries", stats.total_entries);
    
    Ok(())
}

/// Test basic WAL write and read operations
#[tokio::test]
async fn test_basic_wal_operations() -> Result<()> {
    let temp_dir = TempDir::new()?;
    
    let mut config = WalConfig::default();
    config.strategy_type = WalStrategyType::Avro;
    config.multi_disk.data_directories = vec![temp_dir.path().to_path_buf()];
    
    // Create filesystem factory with proper config
    let fs_config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::new(fs_config).await?);
    let strategy = WalFactory::create_from_config(&config, filesystem).await?;
    let wal_manager = WalManager::new(strategy, config).await?;
    
    let collection_id = "test_collection".to_string();
    let vector_id = "test_vector_1".to_string();
    
    // Create a test vector record
    let now = chrono::Utc::now().timestamp_millis();
    let test_record = proximadb::core::VectorRecord {
        id: vector_id.clone(),
        collection_id: collection_id.clone(),
        vector: vec![1.0, 2.0, 3.0, 4.0],
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
    
    // Write to WAL using the correct insert method
    wal_manager.insert(
        collection_id.clone(),
        vector_id.clone(),
        test_record,
    ).await?;
    
    // Check stats
    let stats = wal_manager.stats().await?;
    assert_eq!(stats.total_entries, 1);
    assert!(stats.memory_entries > 0);
    
    println!("✅ Basic WAL operations verified - {} total entries, {} memory entries", 
             stats.total_entries, stats.memory_entries);
    
    Ok(())
}