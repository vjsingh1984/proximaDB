//! Integration tests for WAL Manager operations
//! 
//! This module contains comprehensive integration tests for the WAL Manager,
//! covering transaction management, atomic operations, and high-level WAL operations.

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
async fn test_wal_manager_creation() {
    let (manager, _temp_dir) = create_test_wal_manager().await;
    
    // Manager should be created successfully
    assert_eq!(format!("{:?}", manager).contains("WalManager"), true);
}

#[tokio::test]
async fn test_wal_manager_insert_single_record() {
    let (manager, _temp_dir) = create_test_wal_manager().await;
    
    let collection_id = "test_collection".to_string();
    let vector_id = "test_vector_1".to_string();
    let record = create_test_vector_record(&collection_id, &vector_id);
    
    let result = manager.insert(collection_id, vector_id, record).await;
    
    assert!(result.is_ok());
    let sequence = result.unwrap();
    assert!(sequence > 0);
}

#[tokio::test]
async fn test_wal_manager_insert_batch() {
    let (manager, _temp_dir) = create_test_wal_manager().await;
    
    let collection_id = "test_collection".to_string();
    let records = vec![
        ("vector_1".to_string(), create_test_vector_record(&collection_id, "vector_1")),
        ("vector_2".to_string(), create_test_vector_record(&collection_id, "vector_2")),
        ("vector_3".to_string(), create_test_vector_record(&collection_id, "vector_3")),
    ];
    
    let result = manager.insert_batch(collection_id, records).await;
    
    assert!(result.is_ok());
    let sequences = result.unwrap();
    assert_eq!(sequences.len(), 3);
    
    // All sequences should be positive and unique
    for sequence in &sequences {
        assert!(*sequence > 0);
    }
    
    // Sequences should be in order
    for i in 1..sequences.len() {
        assert!(sequences[i] > sequences[i-1]);
    }
}

#[tokio::test]
async fn test_wal_manager_insert_batch_with_sync() {
    let (manager, _temp_dir) = create_test_wal_manager().await;
    
    let collection_id = "test_collection".to_string();
    let records = vec![
        ("vector_1".to_string(), create_test_vector_record(&collection_id, "vector_1")),
        ("vector_2".to_string(), create_test_vector_record(&collection_id, "vector_2")),
    ];
    
    // Test with immediate sync enabled
    let result = manager.insert_batch_with_sync(collection_id.clone(), records.clone(), true).await;
    assert!(result.is_ok());
    
    // Test with immediate sync disabled
    let result = manager.insert_batch_with_sync(collection_id, records, false).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_wal_manager_update_record() {
    let (manager, _temp_dir) = create_test_wal_manager().await;
    
    let collection_id = "test_collection".to_string();
    let vector_id = "test_vector_1".to_string();
    let original_record = create_test_vector_record(&collection_id, &vector_id);
    
    // Insert original record
    let insert_result = manager.insert(collection_id.clone(), vector_id.clone(), original_record).await;
    assert!(insert_result.is_ok());
    
    // Update the record
    let mut updated_record = create_test_vector_record(&collection_id, &vector_id);
    updated_record.vector = vec![5.0, 6.0, 7.0, 8.0];
    
    let update_result = manager.update(collection_id, vector_id, updated_record).await;
    assert!(update_result.is_ok());
    
    let update_sequence = update_result.unwrap();
    assert!(update_sequence > insert_result.unwrap());
}

#[tokio::test]
async fn test_wal_manager_delete_record() {
    let (manager, _temp_dir) = create_test_wal_manager().await;
    
    let collection_id = "test_collection".to_string();
    let vector_id = "test_vector_1".to_string();
    let record = create_test_vector_record(&collection_id, &vector_id);
    
    // Insert record first
    let insert_result = manager.insert(collection_id.clone(), vector_id.clone(), record).await;
    assert!(insert_result.is_ok());
    
    // Delete the record
    let delete_result = manager.delete(collection_id, vector_id).await;
    assert!(delete_result.is_ok());
    
    let delete_sequence = delete_result.unwrap();
    assert!(delete_sequence > insert_result.unwrap());
}

#[tokio::test]
async fn test_wal_manager_create_collection() {
    let (manager, _temp_dir) = create_test_wal_manager().await;
    
    let collection_id = "test_collection".to_string();
    let config = json!({
        "dimension": 128,
        "metric": "cosine",
        "description": "Test collection"
    });
    
    let result = manager.create_collection(collection_id, config).await;
    assert!(result.is_ok());
    
    let sequence = result.unwrap();
    assert!(sequence > 0);
}

#[tokio::test]
async fn test_wal_manager_drop_collection() {
    let (manager, _temp_dir) = create_test_wal_manager().await;
    
    let collection_id = "test_collection".to_string();
    let config = json!({"dimension": 128});
    
    // Create collection first
    let create_result = manager.create_collection(collection_id.clone(), config).await;
    assert!(create_result.is_ok());
    
    // Drop the collection
    let drop_result = manager.drop_collection(&collection_id).await;
    assert!(drop_result.is_ok());
}

#[tokio::test]
async fn test_wal_manager_force_sync() {
    let (manager, _temp_dir) = create_test_wal_manager().await;
    
    let collection_id = "test_collection".to_string();
    let vector_id = "test_vector_1".to_string();
    let record = create_test_vector_record(&collection_id, &vector_id);
    
    // Insert some data
    let _insert_result = manager.insert(collection_id.clone(), vector_id, record).await;
    
    // Force sync without specific collection
    let sync_result = manager.force_sync(None).await;
    assert!(sync_result.is_ok());
    
    // Force sync with specific collection
    let sync_result = manager.force_sync(Some(&collection_id)).await;
    assert!(sync_result.is_ok());
}

#[tokio::test]
async fn test_wal_manager_flush() {
    let (manager, _temp_dir) = create_test_wal_manager().await;
    
    let collection_id = "test_collection".to_string();
    let vector_id = "test_vector_1".to_string();
    let record = create_test_vector_record(&collection_id, &vector_id);
    
    // Insert some data
    let _insert_result = manager.insert(collection_id.clone(), vector_id, record).await;
    
    // Flush without specific collection
    let flush_result = manager.flush(None).await;
    assert!(flush_result.is_ok());
    
    let flush_info = flush_result.unwrap();
    assert!(flush_info.entries_flushed >= 0);
    assert!(flush_info.bytes_written >= 0);
    assert!(flush_info.segments_created >= 0);
    assert!(flush_info.flush_duration_ms >= 0);
    
    // Flush with specific collection
    let flush_result = manager.flush(Some(&collection_id)).await;
    assert!(flush_result.is_ok());
}

#[tokio::test]
async fn test_wal_manager_compact() {
    let (manager, _temp_dir) = create_test_wal_manager().await;
    
    let collection_id = "test_collection".to_string();
    let vector_id = "test_vector_1".to_string();
    
    // Insert and update the same vector multiple times to create versions
    for i in 1..=5 {
        let mut record = create_test_vector_record(&collection_id, &vector_id);
        record.vector = vec![i as f32; 4];
        
        if i == 1 {
            let _insert_result = manager.insert(collection_id.clone(), vector_id.clone(), record).await;
        } else {
            let _update_result = manager.update(collection_id.clone(), vector_id.clone(), record).await;
        }
    }
    
    // Compact the collection
    let compact_result = manager.compact(&collection_id).await;
    assert!(compact_result.is_ok());
    
    let entries_compacted = compact_result.unwrap();
    assert!(entries_compacted >= 0);
}

#[tokio::test]
async fn test_wal_manager_append_avro_entry() {
    let (manager, _temp_dir) = create_test_wal_manager().await;
    
    let operation_type = "test_operation";
    let avro_payload = vec![1, 2, 3, 4, 5, 6, 7, 8];
    
    let result = manager.append_avro_entry(operation_type, &avro_payload).await;
    assert!(result.is_ok());
    
    let sequence = result.unwrap();
    assert!(sequence > 0);
}

#[tokio::test]
async fn test_wal_manager_read_avro_entries() {
    let (manager, _temp_dir) = create_test_wal_manager().await;
    
    let operation_type = "test_operation";
    let avro_payloads = vec![
        vec![1, 2, 3, 4],
        vec![5, 6, 7, 8],
        vec![9, 10, 11, 12],
    ];
    
    // Append multiple Avro entries
    for payload in &avro_payloads {
        let _result = manager.append_avro_entry(operation_type, payload).await;
    }
    
    // Read back the entries
    let read_result = manager.read_avro_entries(operation_type, Some(10)).await;
    assert!(read_result.is_ok());
    
    let read_payloads = read_result.unwrap();
    assert_eq!(read_payloads.len(), avro_payloads.len());
    
    // Verify the payloads match
    for (original, read) in avro_payloads.iter().zip(read_payloads.iter()) {
        assert_eq!(original, read);
    }
}

#[tokio::test]
async fn test_wal_manager_stats() {
    let (manager, _temp_dir) = create_test_wal_manager().await;
    
    let collection_id = "test_collection".to_string();
    let vector_id = "test_vector_1".to_string();
    let record = create_test_vector_record(&collection_id, &vector_id);
    
    // Insert some data
    let _insert_result = manager.insert(collection_id, vector_id, record).await;
    
    // Get stats
    let stats_result = manager.stats().await;
    assert!(stats_result.is_ok());
    
    let stats = stats_result.unwrap();
    assert!(stats.total_entries >= 0);
    assert!(stats.memory_entries >= 0);
    assert!(stats.disk_segments >= 0);
    assert!(stats.total_disk_size_bytes >= 0);
    assert!(stats.memory_size_bytes >= 0);
    assert!(stats.collections_count >= 0);
    assert!(stats.write_throughput_entries_per_sec >= 0.0);
    assert!(stats.read_throughput_entries_per_sec >= 0.0);
    assert!(stats.compression_ratio >= 0.0);
}

#[tokio::test]
async fn test_wal_manager_recover() {
    let (manager, _temp_dir) = create_test_wal_manager().await;
    
    let collection_id = "test_collection".to_string();
    let vector_id = "test_vector_1".to_string();
    let record = create_test_vector_record(&collection_id, &vector_id);
    
    // Insert some data
    let _insert_result = manager.insert(collection_id, vector_id, record).await;
    
    // Force flush to ensure data is on disk
    let _flush_result = manager.flush(None).await;
    
    // Test recovery
    let recover_result = manager.recover().await;
    assert!(recover_result.is_ok());
    
    let entries_recovered = recover_result.unwrap();
    assert!(entries_recovered >= 0);
}

#[tokio::test]
async fn test_wal_manager_close() {
    let (manager, _temp_dir) = create_test_wal_manager().await;
    
    let collection_id = "test_collection".to_string();
    let vector_id = "test_vector_1".to_string();
    let record = create_test_vector_record(&collection_id, &vector_id);
    
    // Insert some data
    let _insert_result = manager.insert(collection_id, vector_id, record).await;
    
    // Close the manager
    let close_result = manager.close().await;
    assert!(close_result.is_ok());
}

#[tokio::test]
async fn test_wal_manager_search_operations() {
    let (manager, _temp_dir) = create_test_wal_manager().await;
    
    let collection_id = "test_collection".to_string();
    let vector_id = "test_vector_1".to_string();
    let record = create_test_vector_record(&collection_id, &vector_id);
    
    // Insert a record
    let _insert_result = manager.insert(collection_id.clone(), vector_id.clone(), record).await;
    
    // Search for the record
    let search_result = manager.search(&collection_id, &vector_id).await;
    assert!(search_result.is_ok());
    
    let found_entry = search_result.unwrap();
    // Note: The actual search functionality depends on the strategy implementation
    // This test ensures the API is callable
}

#[tokio::test]
async fn test_wal_manager_read_entries() {
    let (manager, _temp_dir) = create_test_wal_manager().await;
    
    let collection_id = "test_collection".to_string();
    let vector_id = "test_vector_1".to_string();
    let record = create_test_vector_record(&collection_id, &vector_id);
    
    // Insert a record
    let _insert_result = manager.insert(collection_id.clone(), vector_id, record).await;
    
    // Read entries from the beginning
    let read_result = manager.read_entries(&collection_id, 0, Some(10)).await;
    assert!(read_result.is_ok());
    
    let entries = read_result.unwrap();
    // Note: The actual read functionality depends on the strategy implementation
    // This test ensures the API is callable
}

#[tokio::test]
async fn test_wal_manager_get_collection_entries() {
    let (manager, _temp_dir) = create_test_wal_manager().await;
    
    let collection_id = "test_collection".to_string();
    let vector_id = "test_vector_1".to_string();
    let record = create_test_vector_record(&collection_id, &vector_id);
    
    // Insert a record
    let _insert_result = manager.insert(collection_id.clone(), vector_id, record).await;
    
    // Get all entries for the collection
    let entries_result = manager.get_collection_entries(&collection_id).await;
    assert!(entries_result.is_ok());
    
    let entries = entries_result.unwrap();
    // Note: The actual functionality depends on the strategy implementation
    // This test ensures the API is callable
}