//! Integration tests for storage operations
//!
//! Tests the complete storage layer functionality:
//! - WAL operations and batching
//! - Flush operations
//! - Compaction operations
//! - Cross-engine consistency
//! - Atomic operations

// Import the common test helpers
#[path = "../../common/mod.rs"]
mod common;

use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::time::{Duration, sleep};

use proximadb::core::VectorRecord;
use proximadb::proto::proximadb_v1::{
    CollectionConfig, DistanceMetric, IndexingAlgorithm, MetadataItem, StorageEngine,
};
use proximadb::services::VectorOperationsService;
use proximadb::services::collection_service::CollectionService;
use proximadb::storage::engines::impls::sst::SstEngine as LsmEngine;
use proximadb::storage::engines::impls::viper::ViperEngine;
use proximadb::storage::memtable::implementations::global_partitioned::GlobalPartitionedMemtable;
use proximadb::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
use proximadb::storage::persistence::write_ahead_log::batch_strategy::WriteBufferStrategyType;
use proximadb::storage::persistence::write_ahead_log::{WriteBufferConfig, WriteBufferManager};

/// Test setup helper
async fn create_test_setup() -> (
    VectorOperationsService,
    CollectionService,
    Arc<FilesystemFactory>,
    TempDir,
) {
    let temp_dir = TempDir::new().unwrap();

    // Create filesystem
    let filesystem_config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::create(filesystem_config).await.unwrap());

    // Create memtable
    let memtable = Arc::new(GlobalPartitionedMemtable::new(
        16 * 1024 * 1024, // 16MB
        1000,             // 1000 partitions
        2 * 1024 * 1024,  // 2MB flush threshold
    ));

    // Create services using test utilities
    let direct_vector_service =
        tests::common::integration_test_helpers::create_test_vector_operations_service()
            .await
            .expect("Failed to create VectorOperationsService");

    let collection_service =
        CollectionService::new(filesystem.clone(), temp_dir.path().to_path_buf());

    (
        direct_vector_service,
        collection_service,
        filesystem,
        temp_dir,
    )
}

/// Create test vectors
fn create_test_vectors(collection_id: &str, count: usize) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| {
            let vector = (0..128)
                .map(|j| (i * 128 + j) as f32 / (count * 128) as f32)
                .collect();

            VectorRecord {
                id: Some(format!(
                    "vec_{,
            timestamp: 0,
            updated_at: None,
            expires_at: None,
            distance: None,
            rank: None,
            score: None,
        }",
                    i
                )),
                vector,
                metadata: vec![
                    MetadataItem {
                        key: "category".to_string(),
                        value: format!("category_{}", i % 3),
                    },
                    MetadataItem {
                        key: "batch_id".to_string(),
                        value: format!("batch_{}", i / 10),
                    },
                ],
                timestamp: chrono::Utc::now().timestamp() as u32,
                created_at: chrono::Utc::now().timestamp_millis(),
                updated_at: Some(chrono::Utc::now().timestamp() as u32),
                expires_at: None,
                distance: 0.0,
                rank: 0,
                score: 0.0,
            }
        })
        .collect()
}

/// Test WAL operations and batching
#[tokio::test]
async fn test_wal_operations_and_batching() {
    setup_hardware_capabilities();
    let (direct_service, collection_service, filesystem, _temp_dir) = create_test_setup().await;

    // Create test collection
    let config = CollectionConfig {
        name: "wal_test_collection".to_string(),
        dimension: 128,
        distance_metric: DistanceMetric::Cosine as i32,
        storage_engine: StorageEngine::Viper as i32,
        primary_indexing_algorithm: IndexingAlgorithm::Hnsw as i32,
        compression: None,
        optimization_hints: None,
        ..Default::default()
    };

    collection_service.create_collection(&config).await.unwrap();

    // Create WAL manager
    let wal_config = WriteBufferConfig {
        wal_dir: _temp_dir.path().join("wal"),
        max_batch_size: 1000,
        flush_interval_ms: 5000,
        enable_compression: true,
        ..Default::default()
    };

    let write_ahead_log_manager = WriteBufferManager::new(
        "wal_test_collection",
        WriteBufferStrategyType::ProtoBatch,
        &wal_config,
        filesystem.clone(),
    )
    .await
    .unwrap();

    // Test batch insertion
    let batch_size = 100;
    let vectors = create_test_vectors("wal_test_collection", batch_size);

    let sequences = write_ahead_log_manager
        .insert_vectors("wal_test_collection".to_string(), vectors.clone())
        .await
        .unwrap();

    assert_eq!(sequences.len(), batch_size);

    // Verify sequences are sequential
    for i in 1..sequences.len() {
        assert!(sequences[i] > sequences[i - 1]);
    }

    // Test reading from WAL
    let read_vectors = write_ahead_log_manager
        .read_vectors_by_sequence_range(sequences[0], sequences[batch_size - 1])
        .await
        .unwrap();

    assert_eq!(read_vectors.len(), batch_size);

    // Test WAL recovery
    let recovery_vectors = write_ahead_log_manager
        .recover_vectors_from_sequence(0)
        .await
        .unwrap();

    assert!(recovery_vectors.len() >= batch_size);

    // Test WAL compaction
    write_ahead_log_manager.compact_wal().await.unwrap();

    // Verify data is still readable after compaction
    let post_compaction_vectors = write_ahead_log_manager
        .read_vectors_by_sequence_range(sequences[0], sequences[batch_size - 1])
        .await
        .unwrap();

    assert_eq!(post_compaction_vectors.len(), batch_size);
}

/// Test flush operations
#[tokio::test]
async fn test_flush_operations() {
    setup_hardware_capabilities();
    let (direct_service, collection_service, filesystem, _temp_dir) = create_test_setup().await;

    // Create test collection
    let config = CollectionConfig {
        name: "flush_test_collection".to_string(),
        dimension: 128,
        distance_metric: DistanceMetric::Euclidean as i32,
        storage_engine: StorageEngine::Viper as i32,
        primary_indexing_algorithm: IndexingAlgorithm::Hnsw as i32,
        compression: None,
        optimization_hints: None,
        ..Default::default()
    };

    collection_service.create_collection(&config).await.unwrap();

    // Insert vectors to trigger flush
    let vectors = create_test_vectors("flush_test_collection", 500);
    let vectors_arc = Arc::new(vectors);

    let sequences = direct_service
        .insert_vectors_direct("flush_test_collection", vectors_arc.clone())
        .await
        .unwrap();

    assert_eq!(sequences.len(), 500);

    // Test manual flush
    direct_service
        .force_flush_collection("flush_test_collection")
        .await
        .unwrap();

    // Verify data is searchable after flush
    let query_vector = vec![0.5; 128];
    let search_results = direct_service
        .search_vectors_unified(
            "flush_test_collection",
            &query_vector,
            20,
            DistanceMetric::Euclidean,
            None,
            None,
            true,
            true,
        )
        .await
        .unwrap();

    assert!(search_results.vectors.len() > 0);
    assert!(search_results.vectors.len() <= 20);

    // Test flush all
    direct_service.force_flush_all().await.unwrap();

    // Verify data is still searchable after flush all
    let search_results_after_flush_all = direct_service
        .search_vectors_unified(
            "flush_test_collection",
            &query_vector,
            20,
            DistanceMetric::Euclidean,
            None,
            None,
            true,
            true,
        )
        .await
        .unwrap();

    assert!(search_results_after_flush_all.vectors.len() > 0);

    // Test automatic flush threshold
    let large_vectors = create_test_vectors("flush_test_collection", 1000);
    let large_vectors_arc = Arc::new(large_vectors);

    // Insert large batch to trigger automatic flush
    let large_sequences = direct_service
        .insert_vectors_direct("flush_test_collection", large_vectors_arc)
        .await
        .unwrap();

    assert_eq!(large_sequences.len(), 1000);

    // Give some time for automatic flush to trigger
    sleep(Duration::from_millis(100)).await;

    // Verify data is still searchable
    let final_search_results = direct_service
        .search_vectors_unified(
            "flush_test_collection",
            &query_vector,
            30,
            DistanceMetric::Euclidean,
            None,
            None,
            true,
            true,
        )
        .await
        .unwrap();

    assert!(final_search_results.vectors.len() > 0);
}

/// Test compaction operations
#[tokio::test]
async fn test_compaction_operations() {
    setup_hardware_capabilities();
    let (direct_service, collection_service, filesystem, _temp_dir) = create_test_setup().await;

    // Create test collection with SST engine for compaction testing
    let config = CollectionConfig {
        name: "compaction_test_collection".to_string(),
        dimension: 128,
        distance_metric: DistanceMetric::Manhattan as i32,
        storage_engine: StorageEngine::Sst as i32,
        primary_indexing_algorithm: IndexingAlgorithm::Ivf as i32,
        compression: None,
        optimization_hints: None,
        ..Default::default()
    };

    collection_service.create_collection(&config).await.unwrap();

    // Insert multiple batches to create multiple levels
    let batch_count = 5;
    let batch_size = 100;

    for batch_idx in 0..batch_count {
        let vectors = create_test_vectors("compaction_test_collection", batch_size);
        let vectors_arc = Arc::new(vectors);

        direct_service
            .insert_vectors_direct("compaction_test_collection", vectors_arc)
            .await
            .unwrap();

        // Flush each batch to create separate levels
        direct_service
            .force_flush_collection("compaction_test_collection")
            .await
            .unwrap();

        // Small delay to ensure timestamps are different
        sleep(Duration::from_millis(10)).await;
    }

    // Verify all data is searchable before compaction
    let query_vector = vec![0.3; 128];
    let pre_compaction_results = direct_service
        .search_vectors_unified(
            "compaction_test_collection",
            &query_vector,
            50,
            DistanceMetric::Manhattan,
            None,
            None,
            true,
            true,
        )
        .await
        .unwrap();

    assert!(pre_compaction_results.vectors.len() > 0);

    // Create SST engine for compaction testing
    let sst_config = proximadb::core::SstConfig {
        memtable_size_mb: 1,
        level_count: 7,
        compaction_threshold: 2,
        block_size_kb: 3072,
        memory_flush_size_bytes: 1024 * 1024,
        write_ahead_log_size_mb: 1,
        max_levels: 7,
        compaction_strategy: "leveled".to_string(),
        compression: "lz4".to_string(),
        bloom_filter_config: Some(proximadb::core::bloom::BloomFilterConfig {
            bits_per_key: 10,
            enabled: true,
            ..Default::default()
        }),
        decompression_cache_config: None,
        cache_size_mb: 128,
        level_size_multiplier: 10.0,
        enable_write_ahead_log: true,
        sync_mode: "sync".to_string(),
        ..Default::default()
    };

    let sst_engine = LsmEngine::new(sst_config, filesystem.clone())
        .await
        .unwrap();

    // Trigger compaction
    sst_engine
        .compact_collection("compaction_test_collection")
        .await
        .unwrap();

    // Verify all data is still searchable after compaction
    let post_compaction_results = direct_service
        .search_vectors_unified(
            "compaction_test_collection",
            &query_vector,
            50,
            DistanceMetric::Manhattan,
            None,
            None,
            true,
            true,
        )
        .await
        .unwrap();

    assert!(post_compaction_results.vectors.len() > 0);
    assert_eq!(
        pre_compaction_results.vectors.len(),
        post_compaction_results.vectors.len()
    );
}

/// Test cross-engine consistency
#[tokio::test]
async fn test_cross_engine_consistency() {
    setup_hardware_capabilities();
    let (direct_service, collection_service, filesystem, _temp_dir) = create_test_setup().await;

    // Create identical collections with different engines
    let viper_config = CollectionConfig {
        name: "viper_consistency_test".to_string(),
        dimension: 128,
        distance_metric: DistanceMetric::Cosine as i32,
        storage_engine: StorageEngine::Viper as i32,
        primary_indexing_algorithm: IndexingAlgorithm::Hnsw as i32,
        compression: None,
        optimization_hints: None,
        ..Default::default()
    };

    let sst_config = CollectionConfig {
        name: "lsm_consistency_test".to_string(),
        dimension: 128,
        distance_metric: DistanceMetric::Cosine as i32,
        storage_engine: StorageEngine::Sst as i32,
        primary_indexing_algorithm: IndexingAlgorithm::Hnsw as i32,
        compression: None,
        optimization_hints: None,
        ..Default::default()
    };

    collection_service
        .create_collection(&viper_config)
        .await
        .unwrap();
    collection_service
        .create_collection(&sst_config)
        .await
        .unwrap();

    // Insert identical data to both engines
    let vectors = create_test_vectors("consistency_test", 200);
    let vectors_arc = Arc::new(vectors);

    let viper_sequences = direct_service
        .insert_vectors_direct("viper_consistency_test", vectors_arc.clone())
        .await
        .unwrap();

    let lsm_sequences = direct_service
        .insert_vectors_direct("lsm_consistency_test", vectors_arc.clone())
        .await
        .unwrap();

    assert_eq!(viper_sequences.len(), lsm_sequences.len());

    // Flush both engines
    direct_service
        .force_flush_collection("viper_consistency_test")
        .await
        .unwrap();
    direct_service
        .force_flush_collection("lsm_consistency_test")
        .await
        .unwrap();

    // Test search consistency
    let query_vector = vec![0.4; 128];
    let k = 15;

    let viper_results = direct_service
        .search_vectors_unified(
            "viper_consistency_test",
            &query_vector,
            k,
            DistanceMetric::Cosine,
            None,
            None,
            true,
            true,
        )
        .await
        .unwrap();

    let lsm_results = direct_service
        .search_vectors_unified(
            "lsm_consistency_test",
            &query_vector,
            k,
            DistanceMetric::Cosine,
            None,
            None,
            true,
            true,
        )
        .await
        .unwrap();

    // Both should return same number of results
    assert_eq!(viper_results.vectors.len(), lsm_results.vectors.len());

    // Results should be very similar (allowing for small floating point differences)
    let tolerance = 1e-6;
    for i in 0..std::cmp::min(viper_results.vectors.len(), lsm_results.vectors.len()) {
        let viper_distance = viper_results.vectors[i].distance;
        let lsm_distance = lsm_results.vectors[i].distance;

        let difference = (viper_distance - lsm_distance).abs();
        assert!(
            difference < tolerance,
            "Distance mismatch at position {}: VIPER = {}, LSM = {}, difference = {}",
            i,
            viper_distance,
            lsm_distance,
            difference
        );
    }
}

/// Test atomic operations
#[tokio::test]
async fn test_atomic_operations() {
    setup_hardware_capabilities();
    let (direct_service, collection_service, _filesystem, _temp_dir) = create_test_setup().await;

    // Create test collection
    let config = CollectionConfig {
        name: "atomic_test_collection".to_string(),
        dimension: 128,
        distance_metric: DistanceMetric::DotProduct as i32,
        storage_engine: StorageEngine::Viper as i32,
        primary_indexing_algorithm: IndexingAlgorithm::Pq as i32,
        compression: None,
        optimization_hints: None,
        ..Default::default()
    };

    collection_service.create_collection(&config).await.unwrap();

    // Test atomic batch insertion
    let vectors = create_test_vectors("atomic_test_collection", 100);
    let vectors_arc = Arc::new(vectors);

    let sequences = direct_service
        .insert_vectors_direct("atomic_test_collection", vectors_arc.clone())
        .await
        .unwrap();

    assert_eq!(sequences.len(), 100);

    // Verify all or nothing semantics - all vectors should be inserted
    let query_vector = vec![0.5; 128];
    let search_results = direct_service
        .search_vectors_unified(
            "atomic_test_collection",
            &query_vector,
            200,
            DistanceMetric::DotProduct,
            None,
            None,
            true,
            true,
        )
        .await
        .unwrap();

    assert_eq!(search_results.vectors.len(), 100);

    // Test atomic flush
    let pre_flush_count = search_results.vectors.len();

    direct_service
        .force_flush_collection("atomic_test_collection")
        .await
        .unwrap();

    // Verify all data is still present after flush
    let post_flush_results = direct_service
        .search_vectors_unified(
            "atomic_test_collection",
            &query_vector,
            200,
            DistanceMetric::DotProduct,
            None,
            None,
            true,
            true,
        )
        .await
        .unwrap();

    assert_eq!(post_flush_results.vectors.len(), pre_flush_count);

    // Test concurrent operations
    let concurrent_tasks = 10;
    let vectors_per_task = 50;

    let mut handles = Vec::new();
    for task_id in 0..concurrent_tasks {
        let service = direct_service.clone();
        let task_vectors = create_test_vectors("atomic_test_collection", vectors_per_task);
        let task_vectors_arc = Arc::new(task_vectors);

        let handle = tokio::spawn(async move {
            service
                .insert_vectors_direct("atomic_test_collection", task_vectors_arc)
                .await
        });

        handles.push(handle);
    }

    // Wait for all tasks to complete
    let mut total_inserted = 0;
    for handle in handles {
        let result = handle.await.unwrap().unwrap();
        total_inserted += result.len();
    }

    assert_eq!(total_inserted, concurrent_tasks * vectors_per_task);

    // Verify all concurrent insertions are atomically committed
    let final_search_results = direct_service
        .search_vectors_unified(
            "atomic_test_collection",
            &query_vector,
            1000,
            DistanceMetric::DotProduct,
            None,
            None,
            true,
            true,
        )
        .await
        .unwrap();

    let expected_total = 100 + (concurrent_tasks * vectors_per_task);
    assert_eq!(final_search_results.vectors.len(), expected_total);
}
