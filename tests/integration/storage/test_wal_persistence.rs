//! Integration tests for WAL (Write-Ahead Log) persistence layer
//! 
//! This module contains comprehensive integration tests for the WAL implementation,
//! covering configuration, factory creation, serialization strategies, and persistence operations.

use proximadb::storage::persistence::wal::{
    WalConfig, WalFactory, WalManager, WalStrategy, WalStrategyType, WalOperation, WalEntry,
    PerformanceConfig, CompressionConfig, FlushResult, WalStats
};
use proximadb::storage::persistence::wal::config::MemtableType;
use proximadb::storage::persistence::filesystem::{
    FilesystemFactory, local::{LocalFileSystem, LocalConfig}
};
use proximadb::core::{VectorRecord, CompressionAlgorithm};
use chrono::{DateTime, Utc};
use serde_json::json;
use std::sync::Arc;
use std::collections::HashMap;
use tempfile::TempDir;

/// Create a test WAL configuration with temporary directory
async fn create_test_wal_config() -> (WalConfig, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    
    let mut config = WalConfig::default();
    config.multi_disk.data_directories = vec![temp_dir.path().to_path_buf()];
    
    (config, temp_dir)
}

/// Create a test filesystem factory
async fn create_test_filesystem() -> Arc<FilesystemFactory> {
    let factory = FilesystemFactory::new();
    Arc::new(factory)
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

/// Create a test WAL entry
fn create_test_wal_entry(collection_id: &str, vector_id: &str, sequence: u64) -> WalEntry {
    let record = create_test_vector_record(collection_id, vector_id);
    
    WalEntry {
        entry_id: vector_id.to_string(),
        collection_id: collection_id.to_string(),
        operation: WalOperation::Insert {
            vector_id: vector_id.to_string(),
            record,
            expires_at: None,
        },
        timestamp: Utc::now(),
        sequence,
        global_sequence: sequence,
        expires_at: None,
        version: 1,
    }
}

#[tokio::test]
async fn test_wal_config_creation() {
    let (config, _temp_dir) = create_test_wal_config().await;
    
    // Test default configuration values
    assert_eq!(config.strategy_type, WalStrategyType::Avro);
    assert_eq!(config.memtable.memtable_type, MemtableType::BTree);
    assert!(config.compression.compress_disk);
    assert!(!config.compression.compress_memory);
    assert_eq!(config.compression.algorithm, CompressionAlgorithm::Lz4);
    assert_eq!(config.performance.memory_flush_size_bytes, 64 * 1024 * 1024);
}

#[tokio::test]
async fn test_wal_config_custom_values() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    
    let config = WalConfig {
        strategy_type: WalStrategyType::Bincode,
        multi_disk: proximadb::storage::persistence::wal::config::MultiDiskConfig {
            data_directories: vec![temp_dir.path().to_path_buf()],
            distribution_strategy: proximadb::storage::persistence::wal::config::DistributionStrategy::RoundRobin,
        },
        memtable: proximadb::storage::persistence::wal::config::MemtableConfig {
            memtable_type: MemtableType::HashMap,
        },
        compression: CompressionConfig {
            algorithm: CompressionAlgorithm::Zstd,
            compress_memory: true,
            compress_disk: true,
            min_compress_size: 512,
        },
        performance: PerformanceConfig {
            memory_flush_size_bytes: 32 * 1024 * 1024,
            disk_segment_size: 128 * 1024 * 1024,
            write_buffer_size: 8192,
            read_buffer_size: 8192,
            fsync_after_write: true,
        },
    };
    
    // Test custom configuration values
    assert_eq!(config.strategy_type, WalStrategyType::Bincode);
    assert_eq!(config.memtable.memtable_type, MemtableType::HashMap);
    assert_eq!(config.compression.algorithm, CompressionAlgorithm::Zstd);
    assert!(config.compression.compress_memory);
    assert_eq!(config.compression.min_compress_size, 512);
    assert_eq!(config.performance.memory_flush_size_bytes, 32 * 1024 * 1024);
    assert_eq!(config.performance.disk_segment_size, 128 * 1024 * 1024);
    assert!(config.performance.fsync_after_write);
}

#[tokio::test]
async fn test_wal_factory_avro_strategy() {
    let (config, _temp_dir) = create_test_wal_config().await;
    let filesystem = create_test_filesystem().await;
    
    let strategy = WalFactory::create_strategy(
        WalStrategyType::Avro,
        &config,
        filesystem
    ).await;
    
    assert!(strategy.is_ok());
    let strategy = strategy.unwrap();
    assert_eq!(strategy.strategy_name(), "avro");
}

#[tokio::test]
async fn test_wal_factory_bincode_strategy() {
    let (mut config, _temp_dir) = create_test_wal_config().await;
    config.strategy_type = WalStrategyType::Bincode;
    let filesystem = create_test_filesystem().await;
    
    let strategy = WalFactory::create_strategy(
        WalStrategyType::Bincode,
        &config,
        filesystem
    ).await;
    
    assert!(strategy.is_ok());
    let strategy = strategy.unwrap();
    assert_eq!(strategy.strategy_name(), "bincode");
}

#[tokio::test]
async fn test_wal_factory_from_config() {
    let (config, _temp_dir) = create_test_wal_config().await;
    let filesystem = create_test_filesystem().await;
    
    let strategy = WalFactory::create_from_config(&config, filesystem).await;
    
    assert!(strategy.is_ok());
    let strategy = strategy.unwrap();
    assert_eq!(strategy.strategy_name(), "avro");
}

#[tokio::test]
async fn test_wal_entry_creation() {
    let collection_id = "test_collection";
    let vector_id = "test_vector_1";
    let entry = create_test_wal_entry(collection_id, vector_id, 1);
    
    assert_eq!(entry.entry_id, vector_id);
    assert_eq!(entry.collection_id, collection_id);
    assert_eq!(entry.sequence, 1);
    assert_eq!(entry.global_sequence, 1);
    assert_eq!(entry.version, 1);
    
    match &entry.operation {
        WalOperation::Insert { vector_id: op_id, record, expires_at } => {
            assert_eq!(op_id, vector_id);
            assert_eq!(record.collection_id, collection_id);
            assert_eq!(record.vector, vec![1.0, 2.0, 3.0, 4.0]);
            assert!(expires_at.is_none());
        }
        _ => panic!("Expected Insert operation"),
    }
}

#[tokio::test]
async fn test_wal_operation_variants() {
    let collection_id = "test_collection";
    let vector_id = "test_vector";
    let record = create_test_vector_record(collection_id, vector_id);
    let expires_at = Some(Utc::now() + chrono::Duration::days(30));
    
    // Test Insert operation
    let insert_op = WalOperation::Insert {
        vector_id: vector_id.to_string(),
        record: record.clone(),
        expires_at: expires_at.clone(),
    };
    
    match insert_op {
        WalOperation::Insert { vector_id: op_id, record: op_record, expires_at: op_expires } => {
            assert_eq!(op_id, vector_id);
            assert_eq!(op_record.collection_id, collection_id);
            assert_eq!(op_expires, expires_at);
        }
        _ => panic!("Expected Insert operation"),
    }
    
    // Test Update operation
    let update_op = WalOperation::Update {
        vector_id: vector_id.to_string(),
        record: record.clone(),
        expires_at: expires_at.clone(),
    };
    
    match update_op {
        WalOperation::Update { vector_id: op_id, record: op_record, expires_at: op_expires } => {
            assert_eq!(op_id, vector_id);
            assert_eq!(op_record.collection_id, collection_id);
            assert_eq!(op_expires, expires_at);
        }
        _ => panic!("Expected Update operation"),
    }
    
    // Test Delete operation
    let delete_op = WalOperation::Delete {
        vector_id: vector_id.to_string(),
        expires_at: expires_at.clone(),
    };
    
    match delete_op {
        WalOperation::Delete { vector_id: op_id, expires_at: op_expires } => {
            assert_eq!(op_id, vector_id);
            assert_eq!(op_expires, expires_at);
        }
        _ => panic!("Expected Delete operation"),
    }
    
    // Test CreateCollection operation
    let create_op = WalOperation::CreateCollection {
        collection_id: collection_id.to_string(),
        config: json!({"dimension": 128}),
    };
    
    match create_op {
        WalOperation::CreateCollection { collection_id: op_id, config: op_config } => {
            assert_eq!(op_id, collection_id);
            assert_eq!(op_config["dimension"], 128);
        }
        _ => panic!("Expected CreateCollection operation"),
    }
    
    // Test DropCollection operation
    let drop_op = WalOperation::DropCollection {
        collection_id: collection_id.to_string(),
    };
    
    match drop_op {
        WalOperation::DropCollection { collection_id: op_id } => {
            assert_eq!(op_id, collection_id);
        }
        _ => panic!("Expected DropCollection operation"),
    }
    
    // Test AvroPayload operation
    let avro_data = vec![1, 2, 3, 4, 5];
    let avro_op = WalOperation::AvroPayload {
        operation_type: "test_operation".to_string(),
        avro_data: avro_data.clone(),
    };
    
    match avro_op {
        WalOperation::AvroPayload { operation_type: op_type, avro_data: op_data } => {
            assert_eq!(op_type, "test_operation");
            assert_eq!(op_data, avro_data);
        }
        _ => panic!("Expected AvroPayload operation"),
    }
}

#[tokio::test]
async fn test_compression_config_defaults() {
    let config = CompressionConfig::default();
    
    assert_eq!(config.algorithm, CompressionAlgorithm::default());
    assert!(!config.compress_memory);
    assert!(config.compress_disk);
    assert_eq!(config.min_compress_size, 1024);
}

#[tokio::test]
async fn test_performance_config_values() {
    let config = PerformanceConfig::default();
    
    assert_eq!(config.memory_flush_size_bytes, 64 * 1024 * 1024);
    assert_eq!(config.disk_segment_size, 256 * 1024 * 1024);
    assert_eq!(config.write_buffer_size, 4096);
    assert_eq!(config.read_buffer_size, 4096);
    assert!(!config.fsync_after_write);
}

#[tokio::test]
async fn test_wal_stats_initialization() {
    let stats = proximadb::storage::persistence::wal::WalStats {
        total_entries: 100,
        memory_entries: 50,
        disk_segments: 5,
        total_disk_size_bytes: 10 * 1024 * 1024,
        memory_size_bytes: 5 * 1024 * 1024,
        collections_count: 3,
        last_flush_time: Some(Utc::now()),
        write_throughput_entries_per_sec: 1000.0,
        read_throughput_entries_per_sec: 2000.0,
        compression_ratio: 0.75,
    };
    
    assert_eq!(stats.total_entries, 100);
    assert_eq!(stats.memory_entries, 50);
    assert_eq!(stats.disk_segments, 5);
    assert_eq!(stats.total_disk_size_bytes, 10 * 1024 * 1024);
    assert_eq!(stats.memory_size_bytes, 5 * 1024 * 1024);
    assert_eq!(stats.collections_count, 3);
    assert!(stats.last_flush_time.is_some());
    assert_eq!(stats.write_throughput_entries_per_sec, 1000.0);
    assert_eq!(stats.read_throughput_entries_per_sec, 2000.0);
    assert_eq!(stats.compression_ratio, 0.75);
}

#[tokio::test]
async fn test_flush_result_creation() {
    let collections = vec!["collection1".to_string(), "collection2".to_string()];
    let result = FlushResult {
        entries_flushed: 100,
        bytes_written: 1024 * 1024,
        segments_created: 2,
        collections_affected: collections.clone(),
        flush_duration_ms: 500,
    };
    
    assert_eq!(result.entries_flushed, 100);
    assert_eq!(result.bytes_written, 1024 * 1024);
    assert_eq!(result.segments_created, 2);
    assert_eq!(result.collections_affected, collections);
    assert_eq!(result.flush_duration_ms, 500);
}

#[tokio::test]
async fn test_wal_entry_with_ttl() {
    let collection_id = "test_collection";
    let vector_id = "test_vector";
    let record = create_test_vector_record(collection_id, vector_id);
    let expires_at = Some(Utc::now() + chrono::Duration::days(30));
    
    let entry = WalEntry {
        entry_id: vector_id.to_string(),
        collection_id: collection_id.to_string(),
        operation: WalOperation::Insert {
            vector_id: vector_id.to_string(),
            record,
            expires_at: expires_at.clone(),
        },
        timestamp: Utc::now(),
        sequence: 1,
        global_sequence: 1,
        expires_at: expires_at.clone(),
        version: 1,
    };
    
    assert_eq!(entry.expires_at, expires_at);
    
    match &entry.operation {
        WalOperation::Insert { expires_at: op_expires, .. } => {
            assert_eq!(op_expires, &expires_at);
        }
        _ => panic!("Expected Insert operation"),
    }
}

#[tokio::test]
async fn test_wal_entry_mvcc_versioning() {
    let collection_id = "test_collection";
    let vector_id = "test_vector";
    
    // Create multiple versions of the same entry
    let mut entries = Vec::new();
    for version in 1..=5 {
        let mut entry = create_test_wal_entry(collection_id, vector_id, version);
        entry.version = version;
        entries.push(entry);
    }
    
    // Verify versions are correctly set
    for (i, entry) in entries.iter().enumerate() {
        assert_eq!(entry.version, (i + 1) as u64);
        assert_eq!(entry.sequence, (i + 1) as u64);
        assert_eq!(entry.global_sequence, (i + 1) as u64);
    }
}