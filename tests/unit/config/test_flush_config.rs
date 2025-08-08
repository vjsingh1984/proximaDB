//! Unit tests for flush configuration management
//!
//! Tests collection-level and global flush configurations, including:
//! - Default threshold values
//! - Collection-specific overrides
//! - TOML configuration parsing
//! - Effective configuration resolution

use anyhow::Result;
use proximadb::core::config::{Config, WalStorageConfig};
use proximadb::storage::persistence::write_ahead_log::config::{WriteBufferConfig, CollectionWalConfig};
use std::collections::HashMap;

#[test]
fn test_default_flush_configuration() {
    let config = WriteBufferConfig::default();
    
    // Test default collection-level threshold (10MB)
    assert_eq!(config.performance.memory_flush_size_bytes, 10 * 1024 * 1024);
    
    // Test default global threshold (4GB)
    assert_eq!(config.performance.global_flush_threshold, 4 * 1024 * 1024 * 1024);
    
    // Test default shrink factor (40%)
    assert_eq!(config.performance.global_shrink_factor, 0.4);
    
    // Test default memtable global limit (4GB)
    assert_eq!(config.memtable.global_memory_limit, 4 * 1024 * 1024 * 1024);
    
    println!("✅ Default flush configuration test passed");
}

#[test]
fn test_collection_specific_overrides() {
    let mut config = WriteBufferConfig::default();
    
    // Add collection-specific overrides
    let mut collection_overrides = HashMap::new();
    
    // Large collection needs higher threshold
    collection_overrides.insert("embeddings".to_string(), CollectionWalConfig {
        memory_flush_size_bytes: Some(50 * 1024 * 1024), // 50MB
        disk_segment_size: Some(1024 * 1024 * 1024), // 1GB
        compression: None,
        default_ttl_days: Some(30),
    });
    
    // Small collection can use lower threshold
    collection_overrides.insert("metadata".to_string(), CollectionWalConfig {
        memory_flush_size_bytes: Some(1 * 1024 * 1024), // 1MB
        disk_segment_size: Some(64 * 1024 * 1024), // 64MB
        compression: None,
        default_ttl_days: Some(7),
    });
    
    config.collection_overrides = collection_overrides;
    
    // Test embeddings collection gets override
    let embeddings_config = config.effective_config_for_collection("embeddings");
    assert_eq!(embeddings_config.memory_flush_size_bytes, 50 * 1024 * 1024);
    assert_eq!(embeddings_config.disk_segment_size, 1024 * 1024 * 1024);
    assert_eq!(embeddings_config.default_ttl_days, Some(30));
    
    // Test metadata collection gets override
    let metadata_config = config.effective_config_for_collection("metadata");
    assert_eq!(metadata_config.memory_flush_size_bytes, 1 * 1024 * 1024);
    assert_eq!(metadata_config.disk_segment_size, 64 * 1024 * 1024);
    assert_eq!(metadata_config.default_ttl_days, Some(7));
    
    // Test unknown collection gets defaults
    let unknown_config = config.effective_config_for_collection("unknown");
    assert_eq!(unknown_config.memory_flush_size_bytes, 10 * 1024 * 1024);
    assert_eq!(unknown_config.disk_segment_size, 512 * 1024 * 1024);
    assert_eq!(unknown_config.default_ttl_days, None);
    
    println!("✅ Collection-specific overrides test passed");
}

#[test]
fn test_core_config_to_wal_config_conversion() {
    // Create core config with custom values
    let core_config = WalStorageConfig {
        write_ahead_log_urls: vec!["file:///test/wal".to_string()],
        distribution_strategy: proximadb::core::config::WalDistributionStrategy::Hash,
        collection_affinity: false,
        memory_flush_size_bytes: 20 * 1024 * 1024, // 20MB
        global_flush_threshold: 8 * 1024 * 1024 * 1024, // 8GB
        strategy_type: Some("Bincode".to_string()),
        memtable_type: Some("HashMap".to_string()),
        sync_mode: Some("Always".to_string()),
        batch_threshold: Some(1000),
        write_ahead_log_size_mb: Some(16),
        concurrent_flushes: Some(8),
        global_shrink_factor: Some(0.6), // 60%
    };
    
    let wal_config = WriteBufferConfig::from(&core_config);
    
    // Test conversion of basic values
    assert_eq!(wal_config.multi_disk.data_directories, vec!["file:///test/wal"]);
    assert_eq!(wal_config.multi_disk.collection_affinity, false);
    assert_eq!(wal_config.performance.memory_flush_size_bytes, 20 * 1024 * 1024);
    assert_eq!(wal_config.performance.global_flush_threshold, 8 * 1024 * 1024 * 1024);
    assert_eq!(wal_config.performance.global_shrink_factor, 0.6);
    
    // Test strategy type conversion
    assert_eq!(wal_config.strategy_type, proximadb::storage::persistence::write_ahead_log::config::WriteBufferStrategyType::Bincode);
    
    // Test memtable type conversion
    assert_eq!(wal_config.memtable.memtable_type, proximadb::storage::persistence::write_ahead_log::config::MemTableType::HashMap);
    
    // Test sync mode conversion
    assert_eq!(wal_config.performance.sync_mode, proximadb::storage::persistence::write_ahead_log::config::SyncMode::Always);
    
    // Test performance settings
    assert_eq!(wal_config.performance.batch_threshold, 1000);
    assert_eq!(wal_config.performance.write_ahead_log_size, 16 * 1024 * 1024);
    assert_eq!(wal_config.performance.concurrent_flushes, 8);
    
    println!("✅ Core config to WAL config conversion test passed");
}

#[test]
fn test_toml_config_parsing() {
    let toml_content = r#"
[storage.wal_config]
write_ahead_log_urls = ["file:///test/wal1", "file:///test/wal2"]
distribution_strategy = "LoadBalanced"
collection_affinity = true
memory_flush_size_bytes = 15728640  # 15MB
global_flush_threshold = 2147483648  # 2GB
global_shrink_factor = 0.3  # 30%
strategy_type = "Avro"
memtable_type = "BTree"
sync_mode = "PerBatch"
batch_threshold = 500
write_ahead_log_size_mb = 12
concurrent_flushes = 6
"#;
    
    let config: Result<Config, toml::de::Error> = toml::from_str(toml_content);
    
    match config {
        Ok(parsed_config) => {
            let wal_config = &parsed_config.storage.wal_config;
            
            assert_eq!(wal_config.write_ahead_log_urls, vec!["file:///test/wal1", "file:///test/wal2"]);
            assert_eq!(wal_config.memory_flush_size_bytes, 15 * 1024 * 1024);
            assert_eq!(wal_config.global_flush_threshold, 2 * 1024 * 1024 * 1024);
            assert_eq!(wal_config.global_shrink_factor, Some(0.3));
            assert_eq!(wal_config.strategy_type, Some("Avro".to_string()));
            assert_eq!(wal_config.memtable_type, Some("BTree".to_string()));
            assert_eq!(wal_config.sync_mode, Some("PerBatch".to_string()));
            assert_eq!(wal_config.batch_threshold, Some(500));
            assert_eq!(wal_config.write_ahead_log_size_mb, Some(12));
            assert_eq!(wal_config.concurrent_flushes, Some(6));
            
            println!("✅ TOML config parsing test passed");
        }
        Err(e) => {
            panic!("Failed to parse TOML config: {}", e);
        }
    }
}

#[test]
fn test_flush_threshold_edge_cases() {
    let config = WriteBufferConfig::default();
    
    // Test very small threshold (1KB)
    let mut small_config = config.clone();
    small_config.performance.memory_flush_size_bytes = 1024;
    
    let effective_config = small_config.effective_config_for_collection("test");
    assert_eq!(effective_config.memory_flush_size_bytes, 1024);
    
    // Test very large threshold (1GB)
    let mut large_config = config.clone();
    large_config.performance.memory_flush_size_bytes = 1024 * 1024 * 1024;
    
    let effective_config = large_config.effective_config_for_collection("test");
    assert_eq!(effective_config.memory_flush_size_bytes, 1024 * 1024 * 1024);
    
    // Test zero threshold (should work but not be practical)
    let mut zero_config = config.clone();
    zero_config.performance.memory_flush_size_bytes = 0;
    
    let effective_config = zero_config.effective_config_for_collection("test");
    assert_eq!(effective_config.memory_flush_size_bytes, 0);
    
    println!("✅ Flush threshold edge cases test passed");
}

#[test]
fn test_global_shrink_factor_validation() {
    let mut config = WriteBufferConfig::default();
    
    // Test valid shrink factors
    let valid_factors = vec![0.1, 0.25, 0.4, 0.5, 0.75, 0.9];
    
    for factor in valid_factors {
        config.performance.global_shrink_factor = factor;
        assert!(config.performance.global_shrink_factor > 0.0);
        assert!(config.performance.global_shrink_factor < 1.0);
    }
    
    // Test boundary values
    config.performance.global_shrink_factor = 0.01; // 1%
    assert!(config.performance.global_shrink_factor > 0.0);
    
    config.performance.global_shrink_factor = 0.99; // 99%
    assert!(config.performance.global_shrink_factor < 1.0);
    
    println!("✅ Global shrink factor validation test passed");
}

#[test]
fn test_performance_config_presets() {
    // Test high-throughput configuration
    let high_throughput = WriteBufferConfig::high_throughput();
    assert_eq!(high_throughput.strategy_type, proximadb::storage::persistence::write_ahead_log::config::WriteBufferStrategyType::Bincode);
    assert_eq!(high_throughput.memtable.memtable_type, proximadb::storage::persistence::write_ahead_log::config::MemTableType::HashMap);
    assert_eq!(high_throughput.performance.memory_flush_size_bytes, 256 * 1024 * 1024);
    assert_eq!(high_throughput.performance.batch_threshold, 500);
    assert_eq!(high_throughput.performance.sync_mode, proximadb::storage::persistence::write_ahead_log::config::SyncMode::PerBatch);
    
    // Test low-latency configuration
    let low_latency = WriteBufferConfig::low_latency();
    assert_eq!(low_latency.memtable.memtable_type, proximadb::storage::persistence::write_ahead_log::config::MemTableType::HashMap);
    assert_eq!(low_latency.compression.compress_memory, false);
    assert_eq!(low_latency.compression.compress_disk, false);
    assert_eq!(low_latency.performance.memory_flush_size_bytes, 32 * 1024 * 1024);
    assert_eq!(low_latency.performance.sync_mode, proximadb::storage::persistence::write_ahead_log::config::SyncMode::Always);
    
    // Test storage-optimized configuration
    let storage_optimized = WriteBufferConfig::storage_optimized();
    assert_eq!(storage_optimized.memtable.memtable_type, proximadb::storage::persistence::write_ahead_log::config::MemTableType::BTree);
    assert_eq!(storage_optimized.compression.compress_memory, true);
    assert_eq!(storage_optimized.compression.min_compress_size, 64);
    assert_eq!(storage_optimized.performance.disk_segment_size, 512 * 1024 * 1024);
    
    println!("✅ Performance config presets test passed");
}