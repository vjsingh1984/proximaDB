//! Unit tests for WAL configuration components

#[cfg(test)]
mod tests {
    use crate::storage::persistence::wal::config::{
        WalConfig, WalStrategyType, CompressionConfig, PerformanceConfig,
        MemTableConfig, MemTableType, MultiDiskConfig, DiskDistributionStrategy
    };
    use crate::core::CompressionAlgorithm;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_wal_strategy_type_variants() {
        let avro_strategy = WalStrategyType::Avro;
        let bincode_strategy = WalStrategyType::Bincode;
        
        assert_eq!(format!("{:?}", avro_strategy), "Avro");
        assert_eq!(format!("{:?}", bincode_strategy), "Bincode");
        
        let cloned_avro = avro_strategy.clone();
        assert_eq!(avro_strategy, cloned_avro);
    }

    #[tokio::test]
    async fn test_memtable_type_variants() {
        let btree_type = MemTableType::BTree;
        let hashmap_type = MemTableType::HashMap;
        let skiplist_type = MemTableType::SkipList;
        let art_type = MemTableType::Art;
        
        assert_eq!(format!("{:?}", btree_type), "BTree");
        assert_eq!(format!("{:?}", hashmap_type), "HashMap");
        assert_eq!(format!("{:?}", skiplist_type), "SkipList");
        assert_eq!(format!("{:?}", art_type), "Art");
    }

    #[tokio::test]
    async fn test_compression_config_default() {
        let config = CompressionConfig::default();
        
        assert_eq!(config.algorithm, CompressionAlgorithm::default());
        assert!(!config.compress_memory);
        assert!(config.compress_disk);
        assert_eq!(config.min_compress_size, 1024);
    }

    #[tokio::test]
    async fn test_performance_config_default() {
        let config = PerformanceConfig::default();
        
        assert_eq!(config.memory_flush_size_bytes, 1 * 1024 * 1024);
        assert_eq!(config.disk_segment_size, 512 * 1024 * 1024);
        assert_eq!(config.write_buffer_size, 8 * 1024 * 1024);
        assert_eq!(config.sync_mode, crate::storage::persistence::wal::config::SyncMode::PerBatch);
    }

    #[tokio::test]
    async fn test_wal_config_default() {
        let config = WalConfig::default();
        
        assert_eq!(config.strategy_type, WalStrategyType::Avro);
        assert_eq!(config.memtable.memtable_type, MemTableType::Art);
        assert_eq!(config.multi_disk.distribution_strategy, DiskDistributionStrategy::LoadBalanced);
        assert!(!config.compression.compress_memory);
        assert!(config.compression.compress_disk);
    }

    #[tokio::test]
    async fn test_wal_config_custom() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        
        let config = WalConfig {
            strategy_type: WalStrategyType::Bincode,
            multi_disk: MultiDiskConfig {
                data_directories: vec![temp_dir.path().to_string_lossy().to_string()],
                distribution_strategy: DiskDistributionStrategy::Hash,
                collection_affinity: true,
            },
            memtable: MemTableConfig {
                memtable_type: MemTableType::SkipList,
                global_memory_limit: 256 * 1024 * 1024,
                mvcc_versions_retained: 5,
                enable_concurrency: true,
            },
            compression: CompressionConfig {
                algorithm: CompressionAlgorithm::Zstd,
                compress_memory: true,
                compress_disk: true,
                min_compress_size: 2048,
            },
            performance: PerformanceConfig {
                memory_flush_size_bytes: 128 * 1024 * 1024,
                disk_segment_size: 512 * 1024 * 1024,
                global_flush_threshold: 1024 * 1024 * 1024,
                write_buffer_size: 16384,
                concurrent_flushes: 2,
                batch_threshold: 100,
                mvcc_cleanup_interval_secs: 1800,
                ttl_cleanup_interval_secs: 600,
                sync_mode: crate::storage::persistence::wal::config::SyncMode::Always,
            },
            enable_mvcc: true,
            enable_ttl: true,
            enable_background_compaction: true,
            collection_overrides: std::collections::HashMap::new(),
        };
        
        assert_eq!(config.strategy_type, WalStrategyType::Bincode);
        assert_eq!(config.memtable.memtable_type, MemTableType::SkipList);
        assert_eq!(config.multi_disk.distribution_strategy, DiskDistributionStrategy::Hash);
        assert_eq!(config.compression.algorithm, CompressionAlgorithm::Zstd);
        assert!(config.compression.compress_memory);
        assert_eq!(config.performance.memory_flush_size_bytes, 128 * 1024 * 1024);
        assert_eq!(config.performance.sync_mode, crate::storage::persistence::wal::config::SyncMode::Always);
    }

    #[tokio::test]
    async fn test_wal_config_serialization() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        
        let config = WalConfig {
            strategy_type: WalStrategyType::Avro,
            multi_disk: MultiDiskConfig {
                data_directories: vec![temp_dir.path().to_string_lossy().to_string()],
                distribution_strategy: DiskDistributionStrategy::LoadBalanced,
                collection_affinity: true,
            },
            memtable: MemTableConfig {
                memtable_type: MemTableType::Art,
                global_memory_limit: 512 * 1024 * 1024,
                mvcc_versions_retained: 3,
                enable_concurrency: true,
            },
            compression: CompressionConfig::default(),
            performance: PerformanceConfig::default(),
            enable_mvcc: true,
            enable_ttl: true,
            enable_background_compaction: true,
            collection_overrides: std::collections::HashMap::new(),
        };
        
        let serialized = serde_json::to_string(&config);
        assert!(serialized.is_ok());
        
        let json_str = serialized.unwrap();
        assert!(json_str.contains("Avro"));
        assert!(json_str.contains("Art"));
        assert!(json_str.contains("LoadBalanced"));
    }
}