//! Unit tests for WAL flush configuration management
//!
//! Tests collection-level and global flush configurations

use anyhow::Result;
use proximadb::storage::persistence::write_ahead_log::config::{
    CollectionWalConfig, MemTableConfig, PerformanceConfig, WALConfig,
};
use std::collections::HashMap;

#[test]
fn test_default_flush_configuration() {
    let config = WALConfig::default();

    // Test default performance settings
    let perf = config.performance;
    assert!(
        perf.memory_flush_size_bytes > 0,
        "Should have positive memory flush size"
    );
    assert!(
        perf.disk_segment_size > 0,
        "Should have positive disk segment size"
    );

    // Test default memtable settings
    let memtable = config.memtable;
    assert!(
        memtable.global_memory_limit > 0,
        "Should have positive global memory limit"
    );

    println!("✅ Default flush configuration test passed");
}

#[test]
fn test_collection_specific_overrides() {
    // Create collection-specific configurations
    let mut collection_configs = HashMap::new();

    // Large collection needs higher threshold
    collection_configs.insert(
        "embeddings".to_string(),
        CollectionWalConfig {
            memory_flush_size_bytes: Some(50 * 1024 * 1024), // 50MB
            disk_segment_size: Some(1024 * 1024 * 1024),     // 1GB
            compression: None,
            default_ttl_days: Some(30),
        },
    );

    // Small collection can use lower threshold
    collection_configs.insert(
        "metadata".to_string(),
        CollectionWalConfig {
            memory_flush_size_bytes: Some(5 * 1024 * 1024), // 5MB
            disk_segment_size: Some(100 * 1024 * 1024),     // 100MB
            compression: None,
            default_ttl_days: Some(7),
        },
    );

    // Verify overrides
    let embeddings_config = collection_configs.get("embeddings").unwrap();
    assert_eq!(
        embeddings_config.memory_flush_size_bytes,
        Some(50 * 1024 * 1024)
    );
    assert_eq!(embeddings_config.default_ttl_days, Some(30));

    let metadata_config = collection_configs.get("metadata").unwrap();
    assert_eq!(
        metadata_config.memory_flush_size_bytes,
        Some(5 * 1024 * 1024)
    );
    assert_eq!(metadata_config.default_ttl_days, Some(7));

    println!("✅ Collection-specific overrides test passed");
}

#[test]
fn test_performance_config_limits() {
    let mut perf_config = PerformanceConfig::default();

    // Test setting custom limits
    perf_config.memory_flush_size_bytes = 1000 * 1024 * 1024; // 1000MB
    perf_config.disk_segment_size = 2048 * 1024 * 1024; // 2048MB
    perf_config.batch_threshold = 5000;

    assert_eq!(perf_config.memory_flush_size_bytes, 1000 * 1024 * 1024);
    assert_eq!(perf_config.disk_segment_size, 2048 * 1024 * 1024);
    assert_eq!(perf_config.batch_threshold, 5000);

    println!("✅ Performance config limits test passed");
}

#[test]
fn test_memtable_config() {
    let mut memtable_config = MemTableConfig::default();

    // Test setting memtable parameters
    memtable_config.global_memory_limit = 4096 * 1024 * 1024; // 4GB
    memtable_config.mvcc_versions_retained = 10;

    assert_eq!(memtable_config.global_memory_limit, 4096 * 1024 * 1024);
    assert_eq!(memtable_config.mvcc_versions_retained, 10);

    println!("✅ Memtable config test passed");
}

#[test]
fn test_effective_config_resolution() {
    // Test how collection-specific configs override defaults
    let default_config = CollectionWalConfig {
        memory_flush_size_bytes: Some(10 * 1024 * 1024), // 10MB default
        disk_segment_size: Some(256 * 1024 * 1024),      // 256MB default
        compression: None,
        default_ttl_days: None,
    };

    let override_config = CollectionWalConfig {
        memory_flush_size_bytes: Some(20 * 1024 * 1024), // Override to 20MB
        disk_segment_size: None,                         // Keep default
        compression: None,
        default_ttl_days: Some(14), // Add TTL
    };

    // Simulate resolving effective config
    let effective_memory = override_config
        .memory_flush_size_bytes
        .or(default_config.memory_flush_size_bytes)
        .unwrap();
    let effective_disk = override_config
        .disk_segment_size
        .or(default_config.disk_segment_size)
        .unwrap();
    let effective_ttl = override_config
        .default_ttl_days
        .or(default_config.default_ttl_days);

    assert_eq!(effective_memory, 20 * 1024 * 1024, "Should use override");
    assert_eq!(effective_disk, 256 * 1024 * 1024, "Should use default");
    assert_eq!(effective_ttl, Some(14), "Should use override");

    println!("✅ Effective config resolution test passed");
}
