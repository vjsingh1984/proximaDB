//! WAL Manager infrastructure tests
//!
//! Tests for WriteAheadLogManager and WriteAheadLogManagerRegistry creation,
//! configuration, and strategy naming — no actual I/O.

use crate::storage::persistence::write_ahead_log::config::{WALConfig, WriteBufferStrategyType};
use crate::storage::persistence::write_ahead_log::{
    WriteAheadLogManagerPoolConfig, WriteAheadLogManagerRegistry,
};

// ---------------------------------------------------------------------------
// WriteAheadLogManager creation & stats
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_wal_manager_creation() {
    // Creating a WriteAheadLogManager via the per-collection constructor should
    // succeed without panic and return Ok.
    let config = WALConfig::default();
    let result =
        crate::storage::persistence::write_ahead_log::WriteAheadLogManager::new_for_collection(
            config,
            "test_infra_collection".to_string(),
        )
        .await;

    assert!(
        result.is_ok(),
        "WriteAheadLogManager::new_for_collection should initialize without error"
    );
}

#[tokio::test]
async fn test_wal_manager_stats() {
    // A freshly created manager should report zeroed-out stats.
    let config = WALConfig::default();
    let manager =
        crate::storage::persistence::write_ahead_log::WriteAheadLogManager::new_for_collection(
            config,
            "test_stats_collection".to_string(),
        )
        .await
        .expect("manager creation should succeed");

    let stats = manager.stats().await.expect("stats() should succeed");

    assert_eq!(
        stats.total_entries, 0,
        "total_entries should be 0 initially"
    );
    assert_eq!(
        stats.memory_entries, 0,
        "memory_entries should be 0 initially"
    );
    assert_eq!(
        stats.disk_segments, 0,
        "disk_segments should be 0 initially"
    );
    assert_eq!(stats.total_disk_size_bytes, 0);
    assert_eq!(stats.memory_size_bytes, 0);
    assert_eq!(stats.collections_count, 0, "no collections assigned yet");
    assert!(stats.last_flush_time.is_none());
    assert!((stats.compression_ratio - 1.0).abs() < f64::EPSILON);
}

#[tokio::test]
async fn test_wal_manager_with_config() {
    // Verify that custom WALConfig values are respected.
    let mut config = WALConfig::default();
    config.strategy_type = WriteBufferStrategyType::ProtoBatch;
    config.enable_mvcc = false;
    config.enable_ttl = false;
    config.enable_background_compaction = false;

    let manager =
        crate::storage::persistence::write_ahead_log::WriteAheadLogManager::new_for_collection(
            config.clone(),
            "test_config_collection".to_string(),
        )
        .await
        .expect("manager creation with custom config should succeed");

    // The manager should be operational — verify via stats (no panic).
    let stats = manager.stats().await.expect("stats should succeed");
    assert_eq!(stats.total_entries, 0);
}

// ---------------------------------------------------------------------------
// WriteAheadLogManagerRegistry creation & configuration
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_wal_registry_creation() {
    // A default registry should start with an empty pool.
    let registry = WriteAheadLogManagerRegistry::new();
    let managers = registry.get_all_managers().await;
    assert_eq!(
        managers.len(),
        0,
        "registry pool should be empty before any collection assignment"
    );
}

#[tokio::test]
async fn test_wal_registry_with_config() {
    // Verify custom pool configuration is accepted without panic.
    let pool_config = WriteAheadLogManagerPoolConfig::builder()
        .initial_pool_size(5)
        .soft_thread_limit(10)
        .target_collections_per_manager(200)
        .rebalance_load_threshold(0.6)
        .rebalance_cooldown_secs(15)
        .enable_dynamic_scaling(false)
        .build();

    assert_eq!(pool_config.initial_pool_size, 5);
    assert_eq!(pool_config.soft_thread_limit, 10);
    assert_eq!(pool_config.target_collections_per_manager, 200);
    assert!((pool_config.rebalance_load_threshold - 0.6).abs() < f64::EPSILON);
    assert_eq!(pool_config.rebalance_cooldown_secs, 15);
    assert!(!pool_config.enable_dynamic_scaling);

    let registry = WriteAheadLogManagerRegistry::with_config(pool_config);
    let managers = registry.get_all_managers().await;
    assert_eq!(
        managers.len(),
        0,
        "custom-config registry pool should start empty"
    );
}

// ---------------------------------------------------------------------------
// Strategy names
// ---------------------------------------------------------------------------

#[test]
fn test_strategy_name() {
    // WriteBufferStrategyType Display should produce the expected strategy names
    // that match the WALBatchStrategy::strategy_name() implementations:
    //   ProtoSerializationStrategy  -> "ProtoBatch"
    //   AvroSerializationStrategy   -> "AvroBatch"
    //   BincodeSerializationStrategy -> "BincodeBatch"

    assert_eq!(
        WriteBufferStrategyType::ProtoBatch.to_string(),
        "ProtoBatch"
    );
    assert_eq!(WriteBufferStrategyType::AvroBatch.to_string(), "AvroBatch");
    assert_eq!(
        WriteBufferStrategyType::BincodeBatch.to_string(),
        "BincodeBatch"
    );

    // Verify default is BincodeBatch (maximum vector ingestion performance)
    assert_eq!(
        WriteBufferStrategyType::default(),
        WriteBufferStrategyType::BincodeBatch
    );
}
