//! Unit tests for WAL configuration and related components
//! 
//! This module contains unit tests for WAL configuration structures,
//! memtable types, compression settings, and performance configurations.

use proximadb::storage::persistence::wal::config::{
    WalConfig, WalStrategyType, CompressionConfig, PerformanceConfig,
    MemtableConfig, MemtableType, MultiDiskConfig, DistributionStrategy
};
use proximadb::core::CompressionAlgorithm;
use std::path::PathBuf;
use tempfile::TempDir;

#[tokio::test]
async fn test_wal_strategy_type_variants() {
    // Test Avro strategy type
    let avro_strategy = WalStrategyType::Avro;
    assert_eq!(format!("{:?}", avro_strategy), "Avro");
    
    // Test Bincode strategy type
    let bincode_strategy = WalStrategyType::Bincode;
    assert_eq!(format!("{:?}", bincode_strategy), "Bincode");
    
    // Test cloning
    let cloned_avro = avro_strategy.clone();
    assert_eq!(avro_strategy, cloned_avro);
    
    let cloned_bincode = bincode_strategy.clone();
    assert_eq!(bincode_strategy, cloned_bincode);
}

#[tokio::test]
async fn test_memtable_type_variants() {
    // Test all memtable types
    let btree_type = MemtableType::BTree;
    let hashmap_type = MemtableType::HashMap;
    let skiplist_type = MemtableType::SkipList;
    let art_type = MemtableType::AdaptiveRadixTree;
    
    // Test debug formatting
    assert_eq!(format!("{:?}", btree_type), "BTree");
    assert_eq!(format!("{:?}", hashmap_type), "HashMap");
    assert_eq!(format!("{:?}", skiplist_type), "SkipList");
    assert_eq!(format!("{:?}", art_type), "AdaptiveRadixTree");
    
    // Test cloning
    assert_eq!(btree_type, btree_type.clone());
    assert_eq!(hashmap_type, hashmap_type.clone());
    assert_eq!(skiplist_type, skiplist_type.clone());
    assert_eq!(art_type, art_type.clone());
}

#[tokio::test]
async fn test_distribution_strategy_variants() {
    // Test all distribution strategies
    let round_robin = DistributionStrategy::RoundRobin;
    let collection_based = DistributionStrategy::CollectionBased;
    let size_based = DistributionStrategy::SizeBased;
    
    // Test debug formatting
    assert_eq!(format!("{:?}", round_robin), "RoundRobin");
    assert_eq!(format!("{:?}", collection_based), "CollectionBased");
    assert_eq!(format!("{:?}", size_based), "SizeBased");
    
    // Test cloning and equality
    assert_eq!(round_robin, round_robin.clone());
    assert_eq!(collection_based, collection_based.clone());
    assert_eq!(size_based, size_based.clone());
}

#[tokio::test]
async fn test_compression_config_default() {
    let config = CompressionConfig::default();
    
    // Test default values
    assert_eq!(config.algorithm, CompressionAlgorithm::default());
    assert!(!config.compress_memory);
    assert!(config.compress_disk);
    assert_eq!(config.min_compress_size, 1024);
    
    // Test debug formatting
    let debug_str = format!("{:?}", config);
    assert!(debug_str.contains("CompressionConfig"));
    assert!(debug_str.contains("compress_memory: false"));
    assert!(debug_str.contains("compress_disk: true"));
}

#[tokio::test]
async fn test_compression_config_custom() {
    let config = CompressionConfig {
        algorithm: CompressionAlgorithm::Zstd,
        compress_memory: true,
        compress_disk: false,
        min_compress_size: 512,
    };
    
    assert_eq!(config.algorithm, CompressionAlgorithm::Zstd);
    assert!(config.compress_memory);
    assert!(!config.compress_disk);
    assert_eq!(config.min_compress_size, 512);
    
    // Test cloning
    let cloned_config = config.clone();
    assert_eq!(config.algorithm, cloned_config.algorithm);
    assert_eq!(config.compress_memory, cloned_config.compress_memory);
    assert_eq!(config.compress_disk, cloned_config.compress_disk);
    assert_eq!(config.min_compress_size, cloned_config.min_compress_size);
}

#[tokio::test]
async fn test_performance_config_default() {
    let config = PerformanceConfig::default();
    
    // Test default values
    assert_eq!(config.memory_flush_size_bytes, 64 * 1024 * 1024);
    assert_eq!(config.disk_segment_size, 256 * 1024 * 1024);
    assert_eq!(config.write_buffer_size, 4096);
    assert_eq!(config.read_buffer_size, 4096);
    assert!(!config.fsync_after_write);
    
    // Test debug formatting
    let debug_str = format!("{:?}", config);
    assert!(debug_str.contains("PerformanceConfig"));
    assert!(debug_str.contains("fsync_after_write: false"));
}

#[tokio::test]
async fn test_performance_config_custom() {
    let config = PerformanceConfig {
        memory_flush_size_bytes: 32 * 1024 * 1024,
        disk_segment_size: 128 * 1024 * 1024,
        write_buffer_size: 8192,
        read_buffer_size: 16384,
        fsync_after_write: true,
    };
    
    assert_eq!(config.memory_flush_size_bytes, 32 * 1024 * 1024);
    assert_eq!(config.disk_segment_size, 128 * 1024 * 1024);
    assert_eq!(config.write_buffer_size, 8192);
    assert_eq!(config.read_buffer_size, 16384);
    assert!(config.fsync_after_write);
    
    // Test cloning
    let cloned_config = config.clone();
    assert_eq!(config.memory_flush_size_bytes, cloned_config.memory_flush_size_bytes);
    assert_eq!(config.disk_segment_size, cloned_config.disk_segment_size);
    assert_eq!(config.write_buffer_size, cloned_config.write_buffer_size);
    assert_eq!(config.read_buffer_size, cloned_config.read_buffer_size);
    assert_eq!(config.fsync_after_write, cloned_config.fsync_after_write);
}

#[tokio::test]
async fn test_memtable_config_default() {
    let config = MemtableConfig::default();
    
    // Test default memtable type
    assert_eq!(config.memtable_type, MemtableType::BTree);
    
    // Test debug formatting
    let debug_str = format!("{:?}", config);
    assert!(debug_str.contains("MemtableConfig"));
    assert!(debug_str.contains("BTree"));
}

#[tokio::test]
async fn test_memtable_config_custom() {
    let config = MemtableConfig {
        memtable_type: MemtableType::HashMap,
    };
    
    assert_eq!(config.memtable_type, MemtableType::HashMap);
    
    // Test cloning
    let cloned_config = config.clone();
    assert_eq!(config.memtable_type, cloned_config.memtable_type);
}

#[tokio::test]
async fn test_multi_disk_config_default() {
    let config = MultiDiskConfig::default();
    
    // Test default values
    assert!(config.data_directories.is_empty());
    assert_eq!(config.distribution_strategy, DistributionStrategy::RoundRobin);
    
    // Test debug formatting
    let debug_str = format!("{:?}", config);
    assert!(debug_str.contains("MultiDiskConfig"));
    assert!(debug_str.contains("RoundRobin"));
}

#[tokio::test]
async fn test_multi_disk_config_custom() {
    let temp_dir1 = TempDir::new().expect("Failed to create temp dir 1");
    let temp_dir2 = TempDir::new().expect("Failed to create temp dir 2");
    
    let config = MultiDiskConfig {
        data_directories: vec![
            temp_dir1.path().to_path_buf(),
            temp_dir2.path().to_path_buf(),
        ],
        distribution_strategy: DistributionStrategy::CollectionBased,
    };
    
    assert_eq!(config.data_directories.len(), 2);
    assert_eq!(config.distribution_strategy, DistributionStrategy::CollectionBased);
    
    // Test cloning
    let cloned_config = config.clone();
    assert_eq!(config.data_directories.len(), cloned_config.data_directories.len());
    assert_eq!(config.distribution_strategy, cloned_config.distribution_strategy);
}

#[tokio::test]
async fn test_wal_config_default() {
    let config = WalConfig::default();
    
    // Test default values
    assert_eq!(config.strategy_type, WalStrategyType::Avro);
    assert_eq!(config.memtable.memtable_type, MemtableType::BTree);
    assert_eq!(config.multi_disk.distribution_strategy, DistributionStrategy::RoundRobin);
    assert!(!config.compression.compress_memory);
    assert!(config.compression.compress_disk);
    assert_eq!(config.performance.memory_flush_size_bytes, 64 * 1024 * 1024);
    assert!(!config.performance.fsync_after_write);
    
    // Test debug formatting
    let debug_str = format!("{:?}", config);
    assert!(debug_str.contains("WalConfig"));
    assert!(debug_str.contains("Avro"));
    assert!(debug_str.contains("BTree"));
}

#[tokio::test]
async fn test_wal_config_custom() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    
    let config = WalConfig {
        strategy_type: WalStrategyType::Bincode,
        multi_disk: MultiDiskConfig {
            data_directories: vec![temp_dir.path().to_path_buf()],
            distribution_strategy: DistributionStrategy::SizeBased,
        },
        memtable: MemtableConfig {
            memtable_type: MemtableType::SkipList,
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
            write_buffer_size: 16384,
            read_buffer_size: 32768,
            fsync_after_write: true,
        },
    };\n    \n    // Test all custom values\n    assert_eq!(config.strategy_type, WalStrategyType::Bincode);\n    assert_eq!(config.memtable.memtable_type, MemtableType::SkipList);\n    assert_eq!(config.multi_disk.distribution_strategy, DistributionStrategy::SizeBased);\n    assert_eq!(config.multi_disk.data_directories.len(), 1);\n    assert_eq!(config.compression.algorithm, CompressionAlgorithm::Zstd);\n    assert!(config.compression.compress_memory);\n    assert_eq!(config.compression.min_compress_size, 2048);\n    assert_eq!(config.performance.memory_flush_size_bytes, 128 * 1024 * 1024);\n    assert_eq!(config.performance.disk_segment_size, 512 * 1024 * 1024);\n    assert_eq!(config.performance.write_buffer_size, 16384);\n    assert_eq!(config.performance.read_buffer_size, 32768);\n    assert!(config.performance.fsync_after_write);\n    \n    // Test cloning\n    let cloned_config = config.clone();\n    assert_eq!(config.strategy_type, cloned_config.strategy_type);\n    assert_eq!(config.memtable.memtable_type, cloned_config.memtable.memtable_type);\n    assert_eq!(config.multi_disk.distribution_strategy, cloned_config.multi_disk.distribution_strategy);\n}\n\n#[tokio::test]\nasync fn test_compression_algorithm_variants() {\n    // Test various compression algorithms\n    let lz4 = CompressionAlgorithm::Lz4;\n    let zstd = CompressionAlgorithm::Zstd;\n    let snappy = CompressionAlgorithm::Snappy;\n    \n    // Test debug formatting\n    let debug_lz4 = format!(\"{:?}\", lz4);\n    let debug_zstd = format!(\"{:?}\", zstd);\n    let debug_snappy = format!(\"{:?}\", snappy);\n    \n    assert!(debug_lz4.contains(\"Lz4\") || debug_lz4.contains(\"LZ4\"));\n    assert!(debug_zstd.contains(\"Zstd\") || debug_zstd.contains(\"ZSTD\"));\n    assert!(debug_snappy.contains(\"Snappy\"));\n    \n    // Test cloning and equality\n    assert_eq!(lz4, lz4.clone());\n    assert_eq!(zstd, zstd.clone());\n    assert_eq!(snappy, snappy.clone());\n}\n\n#[tokio::test]\nasync fn test_wal_config_serialization() {\n    let temp_dir = TempDir::new().expect(\"Failed to create temp dir\");\n    \n    let config = WalConfig {\n        strategy_type: WalStrategyType::Avro,\n        multi_disk: MultiDiskConfig {\n            data_directories: vec![temp_dir.path().to_path_buf()],\n            distribution_strategy: DistributionStrategy::RoundRobin,\n        },\n        memtable: MemtableConfig {\n            memtable_type: MemtableType::BTree,\n        },\n        compression: CompressionConfig::default(),\n        performance: PerformanceConfig::default(),\n    };\n    \n    // Test serialization to JSON\n    let serialized = serde_json::to_string(&config);\n    assert!(serialized.is_ok());\n    \n    let json_str = serialized.unwrap();\n    assert!(json_str.contains(\"Avro\"));\n    assert!(json_str.contains(\"BTree\"));\n    assert!(json_str.contains(\"RoundRobin\"));\n    \n    // Test deserialization from JSON\n    let deserialized: Result<WalConfig, _> = serde_json::from_str(&json_str);\n    assert!(deserialized.is_ok());\n    \n    let restored_config = deserialized.unwrap();\n    assert_eq!(config.strategy_type, restored_config.strategy_type);\n    assert_eq!(config.memtable.memtable_type, restored_config.memtable.memtable_type);\n    assert_eq!(config.multi_disk.distribution_strategy, restored_config.multi_disk.distribution_strategy);\n}\n\n#[tokio::test]\nasync fn test_performance_config_edge_cases() {\n    // Test very small buffer sizes\n    let small_config = PerformanceConfig {\n        memory_flush_size_bytes: 1024,\n        disk_segment_size: 4096,\n        write_buffer_size: 64,\n        read_buffer_size: 128,\n        fsync_after_write: true,\n    };\n    \n    assert_eq!(small_config.memory_flush_size_bytes, 1024);\n    assert_eq!(small_config.disk_segment_size, 4096);\n    assert_eq!(small_config.write_buffer_size, 64);\n    assert_eq!(small_config.read_buffer_size, 128);\n    assert!(small_config.fsync_after_write);\n    \n    // Test very large buffer sizes\n    let large_config = PerformanceConfig {\n        memory_flush_size_bytes: 1024 * 1024 * 1024, // 1GB\n        disk_segment_size: 2 * 1024 * 1024 * 1024,   // 2GB\n        write_buffer_size: 1024 * 1024,              // 1MB\n        read_buffer_size: 2 * 1024 * 1024,           // 2MB\n        fsync_after_write: false,\n    };\n    \n    assert_eq!(large_config.memory_flush_size_bytes, 1024 * 1024 * 1024);\n    assert_eq!(large_config.disk_segment_size, 2 * 1024 * 1024 * 1024);\n    assert_eq!(large_config.write_buffer_size, 1024 * 1024);\n    assert_eq!(large_config.read_buffer_size, 2 * 1024 * 1024);\n    assert!(!large_config.fsync_after_write);\n}\n\n#[tokio::test]\nasync fn test_multi_disk_config_with_many_directories() {\n    let temp_dirs: Vec<TempDir> = (0..5)\n        .map(|_| TempDir::new().expect(\"Failed to create temp dir\"))\n        .collect();\n    \n    let data_directories: Vec<PathBuf> = temp_dirs\n        .iter()\n        .map(|dir| dir.path().to_path_buf())\n        .collect();\n    \n    let config = MultiDiskConfig {\n        data_directories: data_directories.clone(),\n        distribution_strategy: DistributionStrategy::SizeBased,\n    };\n    \n    assert_eq!(config.data_directories.len(), 5);\n    assert_eq!(config.distribution_strategy, DistributionStrategy::SizeBased);\n    \n    // Verify all directories are included\n    for (i, dir) in config.data_directories.iter().enumerate() {\n        assert_eq!(dir, &data_directories[i]);\n    }\n}"