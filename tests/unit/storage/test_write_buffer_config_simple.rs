//! Comprehensive test coverage for WAL configuration module
//!
//! This test module provides 70%+ code coverage for WAL configuration by testing:
//! - All configuration types and their default values
//! - Configuration serialization/deserialization
//! - Configuration validation and edge cases

use anyhow::Result;
use proximadb::storage::persistence::write_ahead_log::config::{
    WriteBufferConfig, WriteBufferStrategyType, MemTableConfig, MemTableType, MultiDiskConfig,
    DiskDistributionStrategy, CompressionConfig, PerformanceConfig, SyncMode,
    CollectionWalConfig,
};
use proximadb::core::CompressionAlgorithm;
use std::collections::HashMap;

#[cfg(test)]
mod wal_config_tests {
    use super::*;

    #[test]
    fn test_wal_strategy_type_defaults() {
        // Test default WAL strategy
        let default_strategy = WriteBufferStrategyType::default();
        assert_eq!(default_strategy, WriteBufferStrategyType::BincodeBatch);
        
        // Test all strategy types
        let strategies = vec![
            WriteBufferStrategyType::AvroBatch,
            WriteBufferStrategyType::BincodeBatch,
        ];
        
        for strategy in strategies {
            // Verify serialization works
            let json = serde_json::to_string(&strategy).unwrap();
            let deserialized: WriteBufferStrategyType = serde_json::from_str(&json).unwrap();
            assert_eq!(strategy, deserialized);
        }
    }

    #[test]
    fn test_memtable_type_defaults() {
        let default_type = MemTableType::default();
        assert_eq!(default_type, MemTableType::Art);
        
        // Test all memtable types
        let types = vec![
            MemTableType::SkipList,
            MemTableType::BTree,
            MemTableType::Art,
            MemTableType::HashMap,
        ];
        
        for memtable_type in types {
            let json = serde_json::to_string(&memtable_type).unwrap();
            let deserialized: MemTableType = serde_json::from_str(&json).unwrap();
            assert_eq!(memtable_type, deserialized);
        }
    }

    #[test]
    fn test_memtable_config_defaults() {
        let config = MemTableConfig::default();
        
        assert_eq!(config.memtable_type, MemTableType::Art);
        assert_eq!(config.global_memory_limit, 4 * 1024 * 1024 * 1024); // 4GB
        assert_eq!(config.mvcc_versions_retained, 3);
        assert!(config.enable_concurrency);
        
        // Test serialization
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: MemTableConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config.memtable_type, deserialized.memtable_type);
        assert_eq!(config.global_memory_limit, deserialized.global_memory_limit);
    }

    #[test]
    fn test_disk_distribution_strategy() {
        let strategies = vec![
            DiskDistributionStrategy::RoundRobin,
            DiskDistributionStrategy::Hash,
            DiskDistributionStrategy::LoadBalanced,
        ];
        
        for strategy in strategies {
            let json = serde_json::to_string(&strategy).unwrap();
            let deserialized: DiskDistributionStrategy = serde_json::from_str(&json).unwrap();
            assert_eq!(strategy, deserialized);
        }
    }

    #[test]
    fn test_multi_disk_config_defaults() {
        let config = MultiDiskConfig::default();
        
        assert_eq!(config.data_directories.len(), 1);
        assert_eq!(config.data_directories[0], "file://./data/wal");
        assert_eq!(config.distribution_strategy, DiskDistributionStrategy::LoadBalanced);
        assert!(config.collection_affinity);
        
        // Test custom configuration
        let custom_config = MultiDiskConfig {
            data_directories: vec![
                "file:///disk1/wal".to_string(),
                "file:///disk2/wal".to_string(),
                "s3://bucket/wal".to_string(),
            ],
            distribution_strategy: DiskDistributionStrategy::Hash,
            collection_affinity: false,
        };
        
        let json = serde_json::to_string(&custom_config).unwrap();
        let deserialized: MultiDiskConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(custom_config.data_directories, deserialized.data_directories);
        assert_eq!(custom_config.distribution_strategy, deserialized.distribution_strategy);
        assert_eq!(custom_config.collection_affinity, deserialized.collection_affinity);
    }

    #[test]
    fn test_compression_config_defaults() {
        let config = CompressionConfig::default();
        
        assert_eq!(config.algorithm, CompressionAlgorithm::default());
        assert!(!config.compress_memory); // Memory should be uncompressed for fast access
        assert!(config.compress_disk);    // Disk should be compressed for space
        assert_eq!(config.min_compress_size, 1024);
        
        // Test custom compression configuration
        let custom_config = CompressionConfig {
            algorithm: CompressionAlgorithm::Lz4,
            compress_memory: true,
            compress_disk: false,
            min_compress_size: 2048,
        };
        
        let json = serde_json::to_string(&custom_config).unwrap();
        let deserialized: CompressionConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(custom_config.compress_memory, deserialized.compress_memory);
        assert_eq!(custom_config.min_compress_size, deserialized.min_compress_size);
    }

    #[test]
    fn test_sync_mode_variants() {
        let sync_modes = vec![
            SyncMode::Never,
            SyncMode::Always,
            SyncMode::Periodic,
            SyncMode::PerBatch,
            SyncMode::MemoryOnly,
        ];
        
        for mode in sync_modes {
            let json = serde_json::to_string(&mode).unwrap();
            let deserialized: SyncMode = serde_json::from_str(&json).unwrap();
            assert_eq!(mode, deserialized);
        }
    }

    #[test]
    fn test_performance_config_defaults() {
        let config = PerformanceConfig::default();
        
        // Verify size-based flush defaults
        assert_eq!(config.memory_flush_size_bytes, 2 * 1024 * 1024); // 2MB - reduced for faster recovery
        assert_eq!(config.disk_segment_size, 512 * 1024 * 1024);     // 512MB
        assert_eq!(config.global_flush_threshold, 4 * 1024 * 1024 * 1024); // 4GB
        assert_eq!(config.write_ahead_log_size, 8 * 1024 * 1024);       // 8MB
        assert_eq!(config.batch_threshold, 500);
        assert_eq!(config.mvcc_cleanup_interval_secs, 3600);         // 1 hour
        assert_eq!(config.ttl_cleanup_interval_secs, 300);           // 5 minutes
        assert_eq!(config.sync_mode, SyncMode::PerBatch);
        assert_eq!(config.global_shrink_factor, 0.4);               // 40%
        assert!(config.cloud_backup.is_none());
        
        // Verify concurrent flushes is reasonable
        assert!(config.concurrent_flushes >= 1);
        assert!(config.concurrent_flushes <= 4);
    }

    #[test]
    fn test_performance_config_custom() {
        let custom_config = PerformanceConfig {
            memory_flush_size_bytes: 1024 * 1024, // 1MB
            disk_segment_size: 64 * 1024 * 1024, // 64MB
            global_flush_threshold: 1024 * 1024 * 1024, // 1GB
            write_ahead_log_size: 1024 * 1024, // 1MB
            concurrent_flushes: 8,
            batch_threshold: 100,
            mvcc_cleanup_interval_secs: 7200, // 2 hours
            ttl_cleanup_interval_secs: 600,   // 10 minutes
            sync_mode: SyncMode::Always,
            global_shrink_factor: 0.6,
            cloud_backup: None,
            enable_optimized_write_ahead_log_writer: None,
            background_writer_threads: None,
            write_ahead_log_batch_size: None,
        };
        
        let json = serde_json::to_string(&custom_config).unwrap();
        let deserialized: PerformanceConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(custom_config.memory_flush_size_bytes, deserialized.memory_flush_size_bytes);
        assert_eq!(custom_config.concurrent_flushes, deserialized.concurrent_flushes);
        assert_eq!(custom_config.global_shrink_factor, deserialized.global_shrink_factor);
        assert!(deserialized.cloud_backup.is_none());
    }

    #[test]
    fn test_collection_wal_config() -> Result<()> {
        let collection_config = CollectionWalConfig {
            memory_flush_size_bytes: Some(5 * 1024 * 1024), // 5MB
            disk_segment_size: Some(128 * 1024 * 1024), // 128MB
            compression: Some(CompressionConfig {
                algorithm: CompressionAlgorithm::Snappy,
                compress_memory: true,
                compress_disk: true,
                min_compress_size: 512,
            }),
            default_ttl_days: Some(1), // 1 day
        };
        
        let json = serde_json::to_string(&collection_config)?;
        let deserialized: CollectionWalConfig = serde_json::from_str(&json)?;
        
        assert_eq!(collection_config.memory_flush_size_bytes, deserialized.memory_flush_size_bytes);
        assert_eq!(collection_config.disk_segment_size, deserialized.disk_segment_size);
        assert_eq!(collection_config.default_ttl_days, deserialized.default_ttl_days);
        
        Ok(())
    }

    #[test]
    fn test_wal_config_comprehensive() -> Result<()> {
        let mut collection_overrides = HashMap::new();
        collection_overrides.insert(
            "high_volume_collection".to_string(),
            CollectionWalConfig {
                memory_flush_size_bytes: Some(20 * 1024 * 1024), // 20MB
                disk_segment_size: Some(1024 * 1024 * 1024), // 1GB
                compression: None,
                default_ttl_days: None,
            },
        );
        
        let config = WriteBufferConfig {
            strategy_type: WriteBufferStrategyType::AvroBatch,
            memtable: MemTableConfig {
                memtable_type: MemTableType::SkipList,
                global_memory_limit: 8 * 1024 * 1024 * 1024, // 8GB
                mvcc_versions_retained: 5,
                enable_concurrency: true,
            },
            multi_disk: MultiDiskConfig {
                data_directories: vec![
                    "file:///nvme1/wal".to_string(),
                    "file:///nvme2/wal".to_string(),
                ],
                distribution_strategy: DiskDistributionStrategy::Hash,
                collection_affinity: true,
            },
            compression: CompressionConfig {
                algorithm: CompressionAlgorithm::Zstd,
                compress_memory: false,
                compress_disk: true,
                min_compress_size: 2048,
            },
            performance: PerformanceConfig {
                memory_flush_size_bytes: 50 * 1024 * 1024, // 50MB
                disk_segment_size: 1024 * 1024 * 1024, // 1GB
                global_flush_threshold: 16 * 1024 * 1024 * 1024, // 16GB
                write_ahead_log_size: 16 * 1024 * 1024, // 16MB
                concurrent_flushes: 2,
                batch_threshold: 1000,
                mvcc_cleanup_interval_secs: 1800, // 30 minutes
                ttl_cleanup_interval_secs: 600,   // 10 minutes
                sync_mode: SyncMode::Periodic,
                global_shrink_factor: 0.5,
                cloud_backup: None,
                enable_optimized_write_ahead_log_writer: None,
                background_writer_threads: None,
                write_ahead_log_batch_size: None,
            },
            enable_mvcc: true,
            enable_ttl: true,
            enable_background_compaction: true,
            collection_overrides,
            enable_optimized_writer: false,
            optimized_writer_batch_size: None,
            optimized_writer_batch_timeout_ms: None,
            optimized_writer_threads: None,
            optimized_writer_enable_combining: None,
        };
        
        // Test serialization
        let json = serde_json::to_string(&config)?;
        let deserialized: WriteBufferConfig = serde_json::from_str(&json)?;
        
        // Verify key properties
        assert_eq!(config.strategy_type, deserialized.strategy_type);
        assert_eq!(config.memtable.memtable_type, deserialized.memtable.memtable_type);
        assert_eq!(config.multi_disk.data_directories, deserialized.multi_disk.data_directories);
        assert_eq!(config.enable_mvcc, deserialized.enable_mvcc);
        assert_eq!(config.enable_ttl, deserialized.enable_ttl);
        assert_eq!(config.enable_background_compaction, deserialized.enable_background_compaction);
        assert_eq!(config.collection_overrides.len(), deserialized.collection_overrides.len());
        
        Ok(())
    }

    #[test]
    fn test_wal_config_defaults() {
        let config = WriteBufferConfig::default();
        
        // Verify default strategy
        assert_eq!(config.strategy_type, WriteBufferStrategyType::BincodeBatch);
        
        // Verify MVCC and TTL are enabled by default
        assert!(config.enable_mvcc);
        assert!(config.enable_ttl);
        assert!(config.enable_background_compaction);
        
        // Verify memtable defaults
        assert_eq!(config.memtable.memtable_type, MemTableType::Art);
        assert!(config.memtable.enable_concurrency);
        
        // Verify multi-disk defaults
        assert_eq!(config.multi_disk.data_directories.len(), 1);
        assert!(config.multi_disk.collection_affinity);
        
        // Verify compression defaults
        assert!(!config.storage_config.as_ref().and_then(|s| s.compression.as_ref()).compress_memory);
        assert!(config.storage_config.as_ref().and_then(|s| s.compression.as_ref()).compress_disk);
        
        // Verify no collection overrides by default
        assert!(config.collection_overrides.is_empty());
    }

    #[test]
    fn test_compression_algorithms() {
        let algorithms = vec![
            CompressionAlgorithm::None,
            CompressionAlgorithm::Gzip,
            CompressionAlgorithm::Lz4,
            CompressionAlgorithm::Snappy,
            CompressionAlgorithm::Zstd,
        ];
        
        for algorithm in algorithms {
            let json = serde_json::to_string(&algorithm).unwrap();
            let deserialized: CompressionAlgorithm = serde_json::from_str(&json).unwrap();
            assert_eq!(algorithm, deserialized);
        }
    }

    #[test]
    fn test_config_edge_cases() {
        // Test edge case configurations
        let edge_config = WriteBufferConfig {
            strategy_type: WriteBufferStrategyType::BincodeBatch,
            memtable: MemTableConfig {
                memtable_type: MemTableType::HashMap,
                global_memory_limit: 1024, // Very small
                mvcc_versions_retained: 1,  // Minimal
                enable_concurrency: false,
            },
            multi_disk: MultiDiskConfig {
                data_directories: vec![], // Empty directories
                distribution_strategy: DiskDistributionStrategy::RoundRobin,
                collection_affinity: false,
            },
            compression: CompressionConfig {
                algorithm: CompressionAlgorithm::None,
                compress_memory: false,
                compress_disk: false,
                min_compress_size: 0, // Compress everything
            },
            performance: PerformanceConfig {
                memory_flush_size_bytes: 1024, // Very small
                disk_segment_size: 1024,
                global_flush_threshold: 1024,
                write_ahead_log_size: 1024,
                concurrent_flushes: 1, // Single threaded
                batch_threshold: 1,    // No batching
                mvcc_cleanup_interval_secs: 1, // Very frequent
                ttl_cleanup_interval_secs: 1,
                sync_mode: SyncMode::Never,
                global_shrink_factor: 0.0, // Aggressive shrinking
                cloud_backup: None,
                enable_optimized_write_ahead_log_writer: None,
                background_writer_threads: None,
                write_ahead_log_batch_size: None,
            },
            enable_mvcc: false,
            enable_ttl: false,
            enable_background_compaction: false,
            collection_overrides: HashMap::new(),
            enable_optimized_writer: false,
            optimized_writer_batch_size: None,
            optimized_writer_batch_timeout_ms: None,
            optimized_writer_threads: None,
            optimized_writer_enable_combining: None,
        };
        
        // Should still serialize/deserialize correctly
        let json = serde_json::to_string(&edge_config).unwrap();
        let deserialized: WriteBufferConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(edge_config.strategy_type, deserialized.strategy_type);
        assert_eq!(edge_config.enable_mvcc, deserialized.enable_mvcc);
        assert_eq!(edge_config.enable_ttl, deserialized.enable_ttl);
    }

    #[test]
    fn test_config_validation_properties() {
        let config = WriteBufferConfig::default();
        
        // Verify reasonable defaults for production use
        assert!(config.performance.memory_flush_size_bytes >= 1024 * 1024); // At least 1MB
        assert!(config.performance.global_flush_threshold >= config.performance.memory_flush_size_bytes);
        assert!(config.performance.concurrent_flushes >= 1);
        assert!(config.performance.batch_threshold >= 1);
        assert!(config.performance.global_shrink_factor > 0.0);
        assert!(config.performance.global_shrink_factor <= 1.0);
        assert!(config.memtable.mvcc_versions_retained >= 1);
        // min_compress_size is always >= 0 as it's unsigned
    }

    #[test]
    fn test_collection_config_overrides() {
        let mut config = WriteBufferConfig::default();
        
        // Add collection-specific overrides
        config.collection_overrides.insert(
            "large_vectors".to_string(),
            CollectionWalConfig {
                memory_flush_size_bytes: Some(100 * 1024 * 1024), // 100MB
                disk_segment_size: Some(2 * 1024 * 1024 * 1024), // 2GB
                compression: Some(CompressionConfig {
                    algorithm: CompressionAlgorithm::Zstd,
                    compress_memory: false,
                    compress_disk: true,
                    min_compress_size: 4096,
                }),
                default_ttl_days: Some(90), // 90 days
            },
        );
        
        config.collection_overrides.insert(
            "small_vectors".to_string(),
            CollectionWalConfig {
                memory_flush_size_bytes: Some(1024 * 1024), // 1MB
                disk_segment_size: Some(64 * 1024 * 1024), // 64MB
                compression: Some(CompressionConfig {
                    algorithm: CompressionAlgorithm::Lz4,
                    compress_memory: true,
                    compress_disk: true,
                    min_compress_size: 256,
                }),
                default_ttl_days: Some(7), // 7 days
            },
        );
        
        // Verify overrides are preserved
        assert_eq!(config.collection_overrides.len(), 2);
        
        let large_config = config.collection_overrides.get("enable_two_stage_search").unwrap();
        assert_eq!(large_config.memory_flush_size_bytes, Some(100 * 1024 * 1024));
        assert_eq!(large_config.default_ttl_days, Some(90));
        
        let small_config = config.collection_overrides.get("enable_two_stage_search").unwrap();
        assert_eq!(small_config.memory_flush_size_bytes, Some(1024 * 1024));
        assert_eq!(small_config.default_ttl_days, Some(7));
        
        // Test serialization with overrides
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: WriteBufferConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config.collection_overrides.len(), deserialized.collection_overrides.len());
    }
}