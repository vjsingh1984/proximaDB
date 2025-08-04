//! Unit tests for flush management logic
//!
//! Tests collection-level flush triggers, global flush coordination,
//! and intelligent collection selection for flush operations.

use anyhow::Result;
use std::sync::Arc;
use std::time::SystemTime;
use tempfile::TempDir;

use proximadb::core::VectorRecord;
use proximadb::storage::memtable::implementations::global_partitioned::GlobalPartitionedMemtable;
use proximadb::storage::memtable::specialized::write_buffer_behavior::WriteBufferVectorBatch;
use proximadb::storage::persistence::write_buffer::background_manager::{BackgroundMaintenanceManager, BackgroundTaskStatus};
use proximadb::storage::persistence::write_buffer::config::WriteBufferConfig;
use proximadb::storage::BatchId;

/// Helper function to create test vector records
fn create_test_vector_records(collection_id: &str, count: usize, size_per_vector: usize) -> Vec<VectorRecord> {
    let now = chrono::Utc::now().timestamp_millis();
    let vector_data = vec![1.0f32; size_per_vector]; // Each float is 4 bytes
    
    (0..count)
        .map(|i| VectorRecord {
            id: Some(format!("vector_{}", i)),
            vector: vector_data.clone(),
            metadata: vec![],
            timestamp: now as u32,
            created_at: now,
            updated_at: Some(now as u32),
            expires_at: None,
            version: Some(1),
            rank: None,
            score: None,
            distance: None,
        })
        .collect()
}

/// Helper function to create test WAL batch
fn create_test_wal_batch(collection_id: &str, vectors: Vec<VectorRecord>) -> WriteBufferVectorBatch {
    let total_size_bytes = vectors.iter().map(|v| v.actual_size_bytes()).sum();
    let vector_count = vectors.len() as u64;
    let end_sequence = if vector_count > 0 { vector_count } else { 1 };
    let batch_id = BatchId::new(collection_id.to_string(), 1, end_sequence);
    
    WriteBufferVectorBatch {
        batch_id,
        vector_records: vectors,
        created_at: SystemTime::now(),
        total_size_bytes,
        is_flushed: false,
            metadata_bloom_filter: None,
    }
}

#[tokio::test]
async fn test_collection_flush_threshold_trigger() -> Result<()> {
    let memtable = GlobalPartitionedMemtable::new();
    let collection_id = "test_collection";
    
    // Test 1: Below threshold - should not need flush
    let small_vectors = create_test_vector_records(collection_id, 10, 100); // ~4KB total
    let small_batch = create_test_wal_batch(collection_id, small_vectors);
    memtable.add_wal_batch("test_collection", small_batch).await?;
    
    let (vector_count, total_size) = memtable.get_collection_stats(collection_id).await;
    assert_eq!(vector_count, 10);
    assert!(total_size < 10 * 1024 * 1024); // Less than 10MB
    
    // Test 2: Above threshold - should need flush
    let large_vectors = create_test_vector_records(collection_id, 1000, 3000); // ~12MB total
    let large_batch = create_test_wal_batch(collection_id, large_vectors);
    memtable.add_wal_batch("test_collection", large_batch).await?;
    
    let (vector_count, total_size) = memtable.get_collection_stats(collection_id).await;
    assert_eq!(vector_count, 1010); // 10 + 1000
    assert!(total_size > 10 * 1024 * 1024); // Greater than 10MB
    
    // Test collections needing flush with 10MB threshold
    let collections_to_flush = memtable.collections_needing_flush(10 * 1024 * 1024).await?;
    assert!(collections_to_flush.contains(&collection_id.to_string()));
    
    println!("✅ Collection flush threshold trigger test passed");
    Ok(())
}

#[tokio::test]
async fn test_multiple_collections_flush_selection() -> Result<()> {
    let memtable = GlobalPartitionedMemtable::new();
    
    // Create collections with different sizes
    let collections = vec![
        ("small_collection", 100, 100),    // ~40KB
        ("medium_collection", 500, 500),   // ~1MB
        ("large_collection", 1000, 3000),  // ~12MB
        ("huge_collection", 2000, 4000),   // ~32MB
    ];
    
    for (collection_id, count, size_per_vector) in collections {
        let vectors = create_test_vector_records(collection_id, count, size_per_vector);
        let batch = create_test_wal_batch(collection_id, vectors);
        memtable.add_wal_batch("test_collection", batch).await?;
    }
    
    // Test with 10MB threshold - should only trigger large and huge collections
    let collections_to_flush = memtable.collections_needing_flush(10 * 1024 * 1024).await?;
    assert_eq!(collections_to_flush.len(), 2);
    assert!(collections_to_flush.contains(&"large_collection".to_string()));
    assert!(collections_to_flush.contains(&"huge_collection".to_string()));
    
    // Test with 1MB threshold - should trigger medium, large, and huge collections
    let collections_to_flush = memtable.collections_needing_flush(1 * 1024 * 1024).await?;
    assert_eq!(collections_to_flush.len(), 3);
    assert!(collections_to_flush.contains(&"medium_collection".to_string()));
    assert!(collections_to_flush.contains(&"large_collection".to_string()));
    assert!(collections_to_flush.contains(&"huge_collection".to_string()));
    
    // Test with 100KB threshold - should trigger all collections
    let collections_to_flush = memtable.collections_needing_flush(100 * 1024).await?;
    assert_eq!(collections_to_flush.len(), 4);
    
    println!("✅ Multiple collections flush selection test passed");
    Ok(())
}

#[tokio::test]
async fn test_global_memory_threshold_calculation() -> Result<()> {
    let memtable = GlobalPartitionedMemtable::new();
    
    // Create multiple collections that together exceed global threshold
    let collections = vec![
        ("collection_1", 1000, 1000),  // ~4MB
        ("collection_2", 1000, 1000),  // ~4MB
        ("collection_3", 1000, 1000),  // ~4MB
        ("collection_4", 1000, 1000),  // ~4MB
    ];
    
    let mut total_expected_size = 0;
    for (collection_id, count, size_per_vector) in collections {
        let vectors = create_test_vector_records(collection_id, count, size_per_vector);
        let batch = create_test_wal_batch(collection_id, vectors);
        total_expected_size += batch.total_size_bytes;
        memtable.add_wal_batch("test_collection", batch).await?;
    }
    
    // Test total memory usage
    let total_memory_usage = memtable.size_bytes().await;
    assert!(total_memory_usage > 0);
    assert!(total_memory_usage > 15 * 1024 * 1024); // Should be > 15MB
    
    // Test that we can get statistics for all collections
    let all_collection_stats = memtable.get_all_collection_stats().await;
    assert_eq!(all_collection_stats.len(), 4);
    
    for (collection_id, (count, size)) in all_collection_stats {
        assert_eq!(count, 1000);
        assert!(size > 1024 * 1024); // Each collection > 1MB
        println!("Collection {}: {} vectors, {} bytes", collection_id, count, size);
    }
    
    println!("✅ Global memory threshold calculation test passed");
    Ok(())
}

#[tokio::test]
async fn test_background_manager_flush_trigger() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let mut config = WriteBufferConfig::default();
    config.performance.memory_flush_size_bytes = 1024 * 1024; // 1MB for testing
    
    let manager = BackgroundMaintenanceManager::new(Arc::new(config));
    let collection_id = "test_collection".to_string();
    
    // Test 1: Below threshold - should not trigger flush
    let small_memory_size = 500 * 1024; // 500KB
    let should_flush = manager.trigger_flush_if_needed(&collection_id, small_memory_size).await?;
    assert!(!should_flush);
    
    // Verify status is still idle
    let status = manager.get_collection_status(&collection_id).await;
    assert_eq!(status, BackgroundTaskStatus::Idle);
    
    // Test 2: Above threshold - should trigger flush (but will fail due to missing dependencies)
    let large_memory_size = 2 * 1024 * 1024; // 2MB
    let should_flush = manager.trigger_flush_if_needed(&collection_id, large_memory_size).await?;
    assert!(should_flush);
    
    // Wait a bit for async task to start
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    
    // Verify status changed to flushing
    let status = manager.get_collection_status(&collection_id).await;
    assert_eq!(status, BackgroundTaskStatus::Flushing);
    
    println!("✅ Background manager flush trigger test passed");
    Ok(())
}

#[tokio::test]
async fn test_flush_coordination_prevents_concurrent_flushes() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let mut config = WriteBufferConfig::default();
    config.performance.memory_flush_size_bytes = 1024 * 1024; // 1MB for testing
    
    let manager = BackgroundMaintenanceManager::new(Arc::new(config));
    let collection_id = "test_collection".to_string();
    
    let large_memory_size = 2 * 1024 * 1024; // 2MB
    
    // Trigger first flush
    let should_flush_1 = manager.trigger_flush_if_needed(&collection_id, large_memory_size).await?;
    assert!(should_flush_1);
    
    // Wait for flush to start
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    
    // Try to trigger second flush while first is running
    let should_flush_2 = manager.trigger_flush_if_needed(&collection_id, large_memory_size).await?;
    assert!(!should_flush_2); // Should be prevented
    
    // Verify stats show skipped operation
    let stats = manager.get_stats().await;
    assert_eq!(stats.flush_operations_skipped, 1);
    
    println!("✅ Flush coordination prevents concurrent flushes test passed");
    Ok(())
}

#[tokio::test]
async fn test_collection_isolation_in_flush_decisions() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let mut config = WriteBufferConfig::default();
    config.performance.memory_flush_size_bytes = 1024 * 1024; // 1MB for testing
    
    let manager = BackgroundMaintenanceManager::new(Arc::new(config));
    
    // Two different collections
    let collection_1 = "collection_1".to_string();
    let collection_2 = "collection_2".to_string();
    
    let large_memory_size = 2 * 1024 * 1024; // 2MB
    
    // Trigger flush for collection_1
    let should_flush_1 = manager.trigger_flush_if_needed(&collection_1, large_memory_size).await?;
    assert!(should_flush_1);
    
    // Wait for flush to start
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    
    // Trigger flush for collection_2 - should work independently
    let should_flush_2 = manager.trigger_flush_if_needed(&collection_2, large_memory_size).await?;
    assert!(should_flush_2);
    
    // Verify both collections have independent status
    let status_1 = manager.get_collection_status(&collection_1).await;
    let status_2 = manager.get_collection_status(&collection_2).await;
    
    assert_eq!(status_1, BackgroundTaskStatus::Flushing);
    assert_eq!(status_2, BackgroundTaskStatus::Flushing);
    
    println!("✅ Collection isolation in flush decisions test passed");
    Ok(())
}

#[tokio::test]
async fn test_memtable_clear_functionality() -> Result<()> {
    let memtable = GlobalPartitionedMemtable::new();
    let collection_id = "test_collection";
    
    // Add some data
    let vectors = create_test_vector_records(collection_id, 100, 1000);
    let batch = create_test_wal_batch(collection_id, vectors);
    memtable.add_wal_batch("test_collection", batch).await?;
    
    // Verify data is there
    let (vector_count, total_size) = memtable.get_collection_stats(collection_id).await;
    assert_eq!(vector_count, 100);
    assert!(total_size > 0);
    
    // Clear up to sequence 50
    let cleared_count = memtable.clear_collection_up_to(collection_id, 50).await?;
    assert!(cleared_count > 0);
    
    // Verify global clear functionality
    let total_cleared = memtable.clear_up_to(100).await?;
    assert!(total_cleared > 0);
    
    // Test complete clear
    memtable.clear().await?;
    
    let (vector_count, total_size) = memtable.get_collection_stats(collection_id).await;
    assert_eq!(vector_count, 0);
    assert_eq!(total_size, 0);
    
    println!("✅ Memtable clear functionality test passed");
    Ok(())
}

#[tokio::test]
async fn test_flush_threshold_edge_cases() -> Result<()> {
    let memtable = GlobalPartitionedMemtable::new();
    
    // Test 1: Empty collection - should not need flush
    let empty_collections = memtable.collections_needing_flush(1024).await?;
    assert!(empty_collections.is_empty());
    
    // Test 2: Zero threshold - all collections should need flush
    let collection_id = "test_collection";
    let vectors = create_test_vector_records(collection_id, 1, 100);
    let batch = create_test_wal_batch(collection_id, vectors);
    memtable.add_wal_batch("test_collection", batch).await?;
    
    let zero_threshold_collections = memtable.collections_needing_flush(0).await?;
    assert_eq!(zero_threshold_collections.len(), 1);
    
    // Test 3: Very high threshold - no collections should need flush
    let high_threshold_collections = memtable.collections_needing_flush(1024 * 1024 * 1024).await?; // 1GB
    assert!(high_threshold_collections.is_empty());
    
    println!("✅ Flush threshold edge cases test passed");
    Ok(())
}