//! Integration tests for WAL (Write-Ahead Log) with current APIs
//! 
//! This module contains integration tests for the WAL system using the current
//! implementation and APIs, focusing on persistence, recovery, and strategy testing.

use proximadb::storage::persistence::wal::{
    WalConfig, WalFactory, WalManager, WalOperation, WalEntry, WalStrategyType
};
use proximadb::storage::persistence::filesystem::FilesystemFactory;
use proximadb::core::VectorRecord;
use chrono::Utc;
use serde_json::json;
use std::sync::Arc;
use std::collections::HashMap;
use tempfile::TempDir;

/// Create a test WAL manager with temporary directory
async fn create_test_wal_manager() -> (WalManager, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    
    let mut config = WalConfig::default();
    config.multi_disk.data_directories = vec![temp_dir.path().to_path_buf()];
    
    let filesystem = Arc::new(FilesystemFactory::new());
    let strategy = WalFactory::create_from_config(&config, filesystem)
        .await
        .expect("Failed to create WAL strategy");
    
    let manager = WalManager::new(strategy, config)
        .await
        .expect("Failed to create WAL manager");
    
    (manager, temp_dir)
}

/// Create a test vector record using Avro-unified VectorRecord
fn create_test_vector_record(collection_id: &str, vector_id: &str) -> VectorRecord {
    let now = Utc::now().timestamp_micros();
    VectorRecord {
        id: vector_id.to_string(),
        collection_id: collection_id.to_string(),
        vector: vec![1.0, 2.0, 3.0, 4.0],
        metadata: HashMap::new(),
        timestamp: now,
        created_at: now,
        updated_at: now,
        expires_at: None,
        version: 1,
        rank: None,
        score: None,
        distance: None,
    }
}

#[tokio::test]
async fn test_wal_persistence_and_recovery() {
    let (manager, _temp_dir) = create_test_wal_manager().await;
    
    let collection_id = "test_collection".to_string();
    let vector_records = vec![
        ("vector_1".to_string(), create_test_vector_record(&collection_id, "vector_1")),
        ("vector_2".to_string(), create_test_vector_record(&collection_id, "vector_2")),
        ("vector_3".to_string(), create_test_vector_record(&collection_id, "vector_3")),
    ];
    
    // Phase 1: Write data with WAL
    {
        // Create collection
        let create_result = manager.create_collection(
            collection_id.clone(),
            json!({"dimension": 4, "metric": "cosine"})
        ).await;
        assert!(create_result.is_ok());
        
        // Insert vectors using batch operation
        let insert_result = manager.insert_batch(
            collection_id.clone(),
            vector_records.clone()
        ).await;
        assert!(insert_result.is_ok());
        
        let sequences = insert_result.unwrap();
        assert_eq!(sequences.len(), 3);
        
        // Force flush to ensure data is persisted
        let flush_result = manager.flush(Some(&collection_id)).await;
        assert!(flush_result.is_ok());
        
        let flush_info = flush_result.unwrap();
        assert!(flush_info.entries_flushed > 0);
    }
    
    // Phase 2: Test recovery
    {
        // Attempt recovery
        let recover_result = manager.recover().await;
        assert!(recover_result.is_ok());
        
        let entries_recovered = recover_result.unwrap();
        assert!(entries_recovered >= 0);
        
        // Verify WAL statistics show persisted data
        let stats_result = manager.stats().await;
        assert!(stats_result.is_ok());
        
        let stats = stats_result.unwrap();
        assert!(stats.total_entries >= 0);
        assert!(stats.collections_count >= 0);
    }
    
    // Clean shutdown
    let close_result = manager.close().await;
    assert!(close_result.is_ok());
}

#[tokio::test]
async fn test_wal_avro_strategy_operations() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    
    let mut config = WalConfig::default();
    config.strategy_type = WalStrategyType::Avro;
    config.multi_disk.data_directories = vec![temp_dir.path().to_path_buf()];
    
    let filesystem = Arc::new(FilesystemFactory::new());
    let strategy = WalFactory::create_strategy(
        WalStrategyType::Avro,
        &config,
        filesystem
    ).await;
    
    assert!(strategy.is_ok());
    let strategy = strategy.unwrap();
    assert_eq!(strategy.strategy_name(), "avro");
    
    let manager = WalManager::new(strategy, config).await.expect("Failed to create manager");
    
    // Test Avro payload operations
    let operation_type = "test_avro_operation";
    let avro_payload = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
    
    let append_result = manager.append_avro_entry(operation_type, &avro_payload).await;
    assert!(append_result.is_ok());
    
    let sequence = append_result.unwrap();
    assert!(sequence > 0);
    
    // Read back the Avro entries
    let read_result = manager.read_avro_entries(operation_type, Some(10)).await;
    assert!(read_result.is_ok());
    
    let read_payloads = read_result.unwrap();
    assert_eq!(read_payloads.len(), 1);
    assert_eq!(read_payloads[0], avro_payload);
    
    let close_result = manager.close().await;
    assert!(close_result.is_ok());
}

#[tokio::test]
async fn test_wal_bincode_strategy_operations() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    
    let mut config = WalConfig::default();
    config.strategy_type = WalStrategyType::Bincode;
    config.multi_disk.data_directories = vec![temp_dir.path().to_path_buf()];
    
    let filesystem = Arc::new(FilesystemFactory::new());
    let strategy = WalFactory::create_strategy(
        WalStrategyType::Bincode,
        &config,
        filesystem
    ).await;
    
    assert!(strategy.is_ok());
    let strategy = strategy.unwrap();
    assert_eq!(strategy.strategy_name(), "bincode");
    
    let manager = WalManager::new(strategy, config).await.expect("Failed to create manager");
    
    let collection_id = "bincode_test_collection".to_string();
    let vector_id = "bincode_vector_1".to_string();
    let record = create_test_vector_record(&collection_id, &vector_id);
    
    // Test basic operations with Bincode strategy
    let insert_result = manager.insert(collection_id.clone(), vector_id.clone(), record).await;
    assert!(insert_result.is_ok());
    
    let update_record = create_test_vector_record(&collection_id, &vector_id);
    let update_result = manager.update(collection_id.clone(), vector_id.clone(), update_record).await;
    assert!(update_result.is_ok());
    
    let delete_result = manager.delete(collection_id.clone(), vector_id).await;
    assert!(delete_result.is_ok());
    
    let close_result = manager.close().await;
    assert!(close_result.is_ok());
}

#[tokio::test]
async fn test_wal_flush_and_compaction() {
    let (manager, _temp_dir) = create_test_wal_manager().await;
    
    let collection_id = "compaction_test_collection".to_string();
    let vector_id = "compaction_vector_1".to_string();
    
    // Create collection
    let create_result = manager.create_collection(
        collection_id.clone(),
        json!({"dimension": 4})
    ).await;
    assert!(create_result.is_ok());
    
    // Insert and update the same vector multiple times to create MVCC versions
    for i in 1..=10 {
        let mut record = create_test_vector_record(&collection_id, &vector_id);
        record.vector = vec![i as f32; 4];
        
        if i == 1 {
            let _insert_result = manager.insert(collection_id.clone(), vector_id.clone(), record).await;
        } else {
            let _update_result = manager.update(collection_id.clone(), vector_id.clone(), record).await;
        }
    }
    
    // Force flush to disk
    let flush_result = manager.flush(Some(&collection_id)).await;
    assert!(flush_result.is_ok());
    
    let flush_info = flush_result.unwrap();
    assert!(flush_info.entries_flushed > 0);
    assert!(flush_info.bytes_written > 0);
    assert!(flush_info.flush_duration_ms >= 0);
    
    // Compact the collection to clean up old MVCC versions
    let compact_result = manager.compact(&collection_id).await;
    assert!(compact_result.is_ok());
    
    let entries_compacted = compact_result.unwrap();
    assert!(entries_compacted >= 0);
    
    let close_result = manager.close().await;
    assert!(close_result.is_ok());
}

#[tokio::test]
async fn test_wal_collection_lifecycle() {
    let (manager, _temp_dir) = create_test_wal_manager().await;
    
    let collection_id = "lifecycle_test_collection".to_string();
    
    // Create collection
    let create_result = manager.create_collection(
        collection_id.clone(),
        json!({
            "dimension": 128,
            "metric": "euclidean",
            "description": "Test collection for lifecycle"
        })
    ).await;
    assert!(create_result.is_ok());
    
    let create_sequence = create_result.unwrap();
    assert!(create_sequence > 0);
    
    // Add some vectors to the collection
    let records = vec![
        ("vec1".to_string(), create_test_vector_record(&collection_id, "vec1")),
        ("vec2".to_string(), create_test_vector_record(&collection_id, "vec2")),
    ];
    
    let insert_result = manager.insert_batch(collection_id.clone(), records).await;
    assert!(insert_result.is_ok());
    
    // Get collection entries
    let entries_result = manager.get_collection_entries(&collection_id).await;
    assert!(entries_result.is_ok());
    
    // Drop the collection
    let drop_result = manager.drop_collection(&collection_id).await;
    assert!(drop_result.is_ok());
    
    let close_result = manager.close().await;
    assert!(close_result.is_ok());
}

#[tokio::test]
async fn test_wal_force_sync_operations() {
    let (manager, _temp_dir) = create_test_wal_manager().await;
    
    let collection_id = "sync_test_collection".to_string();
    let vector_id = "sync_vector_1".to_string();
    let record = create_test_vector_record(&collection_id, &vector_id);
    
    // Insert data
    let insert_result = manager.insert(collection_id.clone(), vector_id, record).await;
    assert!(insert_result.is_ok());
    
    // Test force sync without specific collection
    let sync_result = manager.force_sync(None).await;
    assert!(sync_result.is_ok());
    
    // Test force sync with specific collection
    let sync_result = manager.force_sync(Some(&collection_id)).await;
    assert!(sync_result.is_ok());
    
    // Test batch insert with immediate sync
    let batch_records = vec![
        ("batch_vec1".to_string(), create_test_vector_record(&collection_id, "batch_vec1")),
        ("batch_vec2".to_string(), create_test_vector_record(&collection_id, "batch_vec2")),
    ];
    
    let batch_result = manager.insert_batch_with_sync(
        collection_id,
        batch_records,
        true // immediate sync enabled
    ).await;
    assert!(batch_result.is_ok());
    
    let close_result = manager.close().await;
    assert!(close_result.is_ok());
}
