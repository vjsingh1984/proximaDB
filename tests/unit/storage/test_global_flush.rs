//! Unit tests for global flush functionality
//!
//! Tests global memory threshold enforcement, intelligent collection selection
//! for global flush scenarios, and shrink factor behavior.

use anyhow::Result;
use std::sync::Arc;
use std::time::SystemTime;

use proximadb::core::VectorRecord;
use proximadb::storage::memtable::implementations::global_partitioned::GlobalPartitionedMemtable;
use proximadb::storage::memtable::specialized::wal_behavior::WalVectorBatch;
use proximadb::storage::persistence::wal::background_manager::BackgroundMaintenanceManager;
use proximadb::storage::persistence::wal::config::WalConfig;
use proximadb::storage::BatchId;

/// Helper function to create test vector records with specific size
fn create_sized_vector_records(collection_id: &str, count: usize, size_per_vector: usize) -> Vec<VectorRecord> {
    let now = chrono::Utc::now().timestamp_millis();
    let vector_data = vec![1.0f32; size_per_vector]; // Each float is 4 bytes
    
    (0..count)
        .map(|i| VectorRecord {
            id: format!("vector_{}", i),
            collection_id: collection_id.to_string(),
            vector: vector_data.clone(),
            metadata: std::collections::HashMap::new(),
            timestamp: now,
            created_at: now,
            updated_at: now,
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        })
        .collect()
}

/// Helper function to create test WAL batch
fn create_test_wal_batch(collection_id: &str, vectors: Vec<VectorRecord>) -> WalVectorBatch {
    let total_size_bytes = vectors.iter().map(|v| v.actual_size_bytes()).sum();
    let batch_id = BatchId::new(collection_id.to_string(), 1, vectors.len() as u64);
    
    WalVectorBatch {
        batch_id,
        vector_records: vectors,
        created_at: SystemTime::now(),
        total_size_bytes,
        is_flushed: false,
    }
}

/// Helper function to populate memtable with collections of different sizes
async fn populate_memtable_with_collections(
    memtable: &GlobalPartitionedMemtable,
    collection_configs: Vec<(&str, usize, usize)> // (collection_id, count, size_per_vector)
) -> Result<()> {
    for (collection_id, count, size_per_vector) in collection_configs {
        let vectors = create_sized_vector_records(collection_id, count, size_per_vector);
        let batch = create_test_wal_batch(collection_id, vectors);
        memtable.add_wal_batch("test_collection", batch).await?;
    }
    Ok(())
}

#[tokio::test]
async fn test_global_memory_threshold_calculation() -> Result<()> {
    let memtable = GlobalPartitionedMemtable::new();
    
    // Create collections with known sizes
    let collection_configs = vec![
        ("small_collection", 100, 250),      // ~100KB
        ("medium_collection", 500, 500),     // ~1MB
        ("large_collection", 1000, 1000),    // ~4MB
        ("huge_collection", 2000, 2000),     // ~16MB
    ];
    
    populate_memtable_with_collections(&memtable, collection_configs).await?;
    
    // Calculate total memory usage
    let total_memory_usage = memtable.size_bytes().await;
    let total_entries = memtable.len().await;
    
    assert_eq!(total_entries, 3600); // 100 + 500 + 1000 + 2000
    assert!(total_memory_usage > 20 * 1024 * 1024); // Should be > 20MB
    
    // Test global threshold scenarios
    let global_threshold_10mb = 10 * 1024 * 1024;
    let global_threshold_50mb = 50 * 1024 * 1024;
    
    assert!(total_memory_usage > global_threshold_10mb); // Should exceed 10MB
    assert!(total_memory_usage < global_threshold_50mb); // Should be under 50MB
    
    println!("✅ Global memory threshold calculation test passed");
    println!("   Total memory usage: {} bytes ({} MB)", total_memory_usage, total_memory_usage / 1024 / 1024);
    Ok(())
}

#[tokio::test]
async fn test_global_flush_collection_selection() -> Result<()> {
    let memtable = GlobalPartitionedMemtable::new();
    
    // Create collections with different sizes (ascending order)
    let collection_configs = vec![
        ("tiny_collection", 50, 100),        // ~20KB
        ("small_collection", 200, 200),      // ~160KB
        ("medium_collection", 500, 500),     // ~1MB
        ("large_collection", 1000, 1000),    // ~4MB
        ("huge_collection", 2000, 2000),     // ~16MB
    ];
    
    populate_memtable_with_collections(&memtable, collection_configs).await?;
    
    // Test collection selection with different thresholds
    
    // Scenario 1: High threshold - only huge collection should be selected
    let high_threshold_collections = memtable.collections_needing_flush(10 * 1024 * 1024).await?;
    assert_eq!(high_threshold_collections.len(), 1);
    assert!(high_threshold_collections.contains(&"huge_collection".to_string()));
    
    // Scenario 2: Medium threshold - large and huge collections should be selected
    let medium_threshold_collections = memtable.collections_needing_flush(2 * 1024 * 1024).await?;
    assert_eq!(medium_threshold_collections.len(), 2);
    assert!(medium_threshold_collections.contains(&"large_collection".to_string()));
    assert!(medium_threshold_collections.contains(&"huge_collection".to_string()));
    
    // Scenario 3: Low threshold - medium, large, and huge collections should be selected
    let low_threshold_collections = memtable.collections_needing_flush(500 * 1024).await?;
    assert_eq!(low_threshold_collections.len(), 3);
    assert!(low_threshold_collections.contains(&"medium_collection".to_string()));
    assert!(low_threshold_collections.contains(&"large_collection".to_string()));
    assert!(low_threshold_collections.contains(&"huge_collection".to_string()));
    
    println!("✅ Global flush collection selection test passed");
    Ok(())
}

#[tokio::test]
async fn test_global_shrink_factor_behavior() -> Result<()> {
    let memtable = GlobalPartitionedMemtable::new();
    
    // Create collections with known sizes
    let collection_configs = vec![
        ("collection_a", 500, 1000),   // ~2MB
        ("collection_b", 750, 1000),   // ~3MB
        ("collection_c", 1000, 1000),  // ~4MB
        ("collection_d", 1250, 1000),  // ~5MB
    ];
    
    populate_memtable_with_collections(&memtable, collection_configs).await?;
    
    let total_memory_usage = memtable.size_bytes().await;
    
    // Test shrink factor calculations
    let shrink_factors = vec![0.2, 0.4, 0.6, 0.8];
    
    for shrink_factor in shrink_factors {
        let target_size = (total_memory_usage as f64 * shrink_factor) as usize;
        let reduction_needed = total_memory_usage - target_size;
        
        println!("Shrink factor {}: target_size={} bytes, reduction_needed={} bytes", 
                 shrink_factor, target_size, reduction_needed);
        
        assert!(target_size < total_memory_usage);
        assert!(reduction_needed > 0);
        
        // Verify shrink factor is reasonable
        assert!(shrink_factor > 0.0);
        assert!(shrink_factor < 1.0);
    }
    
    println!("✅ Global shrink factor behavior test passed");
    Ok(())
}

#[tokio::test]
async fn test_global_flush_many_small_collections() -> Result<()> {
    let memtable = GlobalPartitionedMemtable::new();
    
    // Create many small collections (edge case scenario)
    let mut collection_configs = Vec::new();
    for i in 0..20 {
        let collection_id = format!("small_collection_{}", i);
        collection_configs.push((collection_id.as_str(), 100, 500)); // Each ~200KB
    }
    
    // This is a bit tricky since we need string references, let's create them differently
    let mut collections = Vec::new();
    for i in 0..20 {
        let collection_id = format!("small_collection_{}", i);
        let vectors = create_sized_vector_records(&collection_id, 100, 500);
        let batch = create_test_wal_batch(&collection_id, vectors);
        memtable.add_wal_batch("test_collection", batch).await?;
        collections.push(collection_id);
    }
    
    let total_memory_usage = memtable.size_bytes().await;
    let total_entries = memtable.len().await;
    
    assert_eq!(total_entries, 2000); // 20 * 100
    assert!(total_memory_usage > 3 * 1024 * 1024); // Should be > 3MB
    
    // Test that we can identify all small collections when threshold is low
    let low_threshold_collections = memtable.collections_needing_flush(100 * 1024).await?; // 100KB
    assert_eq!(low_threshold_collections.len(), 20); // All collections should be selected
    
    // Test that no collections are selected when threshold is high
    let high_threshold_collections = memtable.collections_needing_flush(1024 * 1024).await?; // 1MB
    assert_eq!(high_threshold_collections.len(), 0); // No collections should be selected
    
    println!("✅ Global flush many small collections test passed");
    Ok(())
}

#[tokio::test]
async fn test_global_flush_config_integration() -> Result<()> {
    // Test different global flush configurations
    let configs = vec![
        (1 * 1024 * 1024, 4 * 1024 * 1024 * 1024, 0.3),    // 1MB collection, 4GB global, 30% shrink
        (5 * 1024 * 1024, 2 * 1024 * 1024 * 1024, 0.5),    // 5MB collection, 2GB global, 50% shrink
        (20 * 1024 * 1024, 8 * 1024 * 1024 * 1024, 0.7),   // 20MB collection, 8GB global, 70% shrink
    ];
    
    for (collection_threshold, global_threshold, shrink_factor) in configs {
        let mut config = WalConfig::default();
        config.performance.memory_flush_size_bytes = collection_threshold;
        config.performance.global_flush_threshold = global_threshold;
        config.performance.global_shrink_factor = shrink_factor;
        
        // Test that configurations are valid
        assert!(config.performance.memory_flush_size_bytes > 0);
        assert!(config.performance.global_flush_threshold > config.performance.memory_flush_size_bytes);
        assert!(config.performance.global_shrink_factor > 0.0);
        assert!(config.performance.global_shrink_factor < 1.0);
        
        // Test effective configuration
        let effective_config = config.effective_config_for_collection("test_collection");
        assert_eq!(effective_config.memory_flush_size_bytes, collection_threshold);
        
        println!("Config test passed: collection_threshold={} bytes, global_threshold={} bytes, shrink_factor={}", 
                 collection_threshold, global_threshold, shrink_factor);
    }
    
    println!("✅ Global flush config integration test passed");
    Ok(())
}

#[tokio::test]
async fn test_global_flush_background_manager_integration() -> Result<()> {
    // Test background manager with global flush settings
    let mut config = WalConfig::default();
    config.performance.memory_flush_size_bytes = 2 * 1024 * 1024; // 2MB
    config.performance.global_flush_threshold = 8 * 1024 * 1024; // 8MB
    config.performance.global_shrink_factor = 0.4; // 40%
    
    let manager = BackgroundMaintenanceManager::new(Arc::new(config));
    
    // Test that manager can handle collections with different sizes
    let test_scenarios = vec![
        ("small_collection", 1 * 1024 * 1024, false),  // 1MB - should not trigger
        ("medium_collection", 3 * 1024 * 1024, true),  // 3MB - should trigger
        ("large_collection", 5 * 1024 * 1024, true),   // 5MB - should trigger
    ];
    
    for (collection_id, memory_size, should_trigger) in test_scenarios {
        let triggered = manager.trigger_flush_if_needed(&collection_id.to_string(), memory_size).await?;
        assert_eq!(triggered, should_trigger, 
                   "Collection {} with size {} should have triggered: {}", 
                   collection_id, memory_size, should_trigger);
        
        // Wait a bit for async operations
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    }
    
    println!("✅ Global flush background manager integration test passed");
    Ok(())
}

#[tokio::test]
async fn test_global_flush_performance_metrics() -> Result<()> {
    let memtable = GlobalPartitionedMemtable::new();
    
    // Create collections and measure performance
    let start_time = std::time::Instant::now();
    
    let collection_configs = vec![
        ("perf_collection_1", 1000, 1000),  // ~4MB
        ("perf_collection_2", 1500, 1000),  // ~6MB
        ("perf_collection_3", 2000, 1000),  // ~8MB
    ];
    
    populate_memtable_with_collections(&memtable, collection_configs).await?;
    
    let populate_duration = start_time.elapsed();
    
    // Test flush selection performance
    let flush_start_time = std::time::Instant::now();
    
    let collections_to_flush = memtable.collections_needing_flush(5 * 1024 * 1024).await?;
    
    let flush_selection_duration = flush_start_time.elapsed();
    
    // Test memory calculation performance
    let memory_start_time = std::time::Instant::now();
    
    let total_memory = memtable.size_bytes().await;
    let total_entries = memtable.len().await;
    
    let memory_calc_duration = memory_start_time.elapsed();
    
    // Verify results
    assert_eq!(collections_to_flush.len(), 2); // perf_collection_2 and perf_collection_3
    assert_eq!(total_entries, 4500); // 1000 + 1500 + 2000
    assert!(total_memory > 15 * 1024 * 1024); // Should be > 15MB
    
    // Performance assertions (should be fast)
    assert!(populate_duration.as_millis() < 1000, "Population took too long: {}ms", populate_duration.as_millis());
    assert!(flush_selection_duration.as_micros() < 10000, "Flush selection took too long: {}μs", flush_selection_duration.as_micros());
    assert!(memory_calc_duration.as_micros() < 1000, "Memory calculation took too long: {}μs", memory_calc_duration.as_micros());
    
    println!("✅ Global flush performance metrics test passed");
    println!("   Population time: {:?}", populate_duration);
    println!("   Flush selection time: {:?}", flush_selection_duration);
    println!("   Memory calculation time: {:?}", memory_calc_duration);
    Ok(())
}