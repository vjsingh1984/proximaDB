//! Tests for Avro WAL serialization strategy
//!
//! These tests ensure the Avro serialization strategy correctly handles:
//! - Writing and reading batches
//! - Similarity search operations
//! - Flush operations
//! - Recovery operations
//! - Stats tracking

use crate::compute::distance_computation::DistanceMetric;
use crate::core::VectorRecord;
use crate::proto::proximadb_v1::MetadataItem;
use crate::storage::memtable::specialized::wal_behavior::WALVectorBatch;
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::persistence::write_ahead_log::{
    AvroSerializationStrategy, BatchId, WALBatchStrategy, WALConfig,
};
use anyhow::Result;
use std::sync::Arc;

/// Create test configuration
fn create_test_config() -> WALConfig {
    WALConfig {
        memtable: crate::storage::persistence::write_ahead_log::config::MemTableConfig {
            memtable_type:
                crate::storage::persistence::write_ahead_log::config::MemTableType::default(),
            global_memory_limit: 10 * 1024 * 1024, // 10MB
            mvcc_versions_retained: 5,
            enable_concurrency: true,
        },
        multi_disk: crate::storage::persistence::write_ahead_log::config::MultiDiskConfig {
            data_directories: vec!["/tmp/proximadb-wal-test".to_string()],
            ..Default::default()
        },
        performance: crate::storage::persistence::write_ahead_log::config::PerformanceConfig {
            memory_flush_size_bytes: 5 * 1024 * 1024, // 5MB
            sync_mode: crate::storage::persistence::write_ahead_log::config::SyncMode::Always,
            ..Default::default()
        },
        enable_mvcc: true,
        ..Default::default()
    }
}

/// Create test vector
fn create_test_vector(id: &str, dimension: usize) -> VectorRecord {
    VectorRecord {
        id: id.to_string(),
        vector: vec![0.1; dimension],
        metadata: std::collections::HashMap::from([
            ("category".to_string(), crate::proto::proximadb_v1::SqlValue {
                value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue("test".to_string())),
            }),
            ("priority".to_string(), crate::proto::proximadb_v1::SqlValue {
                value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue("1".to_string())),
            }),
        ]),
        timestamp: 1234567890,
        updated_at: Some(1234567890),
        expires_at: None,
        version: Some(1),
        quantized_vector: vec![],
        source: None,
        ..Default::default()
    }
}

/// Create test batch
fn create_test_batch(vectors: Vec<VectorRecord>) -> WALVectorBatch {
    let vector_count = vectors.len();
    WALVectorBatch {
        batch_id: BatchId::new(),
        vector_records: Arc::new(vectors),
        timestamp: std::time::SystemTime::now(),
        total_size_bytes: vector_count * 256, // Approximate
        is_flushed: false,
        metadata_bloom_filter: None,
    }
}

/// Create WriteBuffer directory for collection
async fn create_collection_write_buffer_dir(collection_id: &str) {
    let write_buffer_dir = std::path::Path::new("/tmp/proximadb-wal-test")
        .join(collection_id)
        .join("write_buffer");
    tokio::fs::create_dir_all(&write_buffer_dir)
        .await
        .expect("Failed to create WriteBuffer directory");
}

#[tokio::test]
async fn test_avro_strategy_initialization() {
    let config = create_test_config();
    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());

    let strategy = AvroSerializationStrategy::new(&config, filesystem_factory.clone())
        .await
        .expect("Failed to create Avro strategy");

    assert_eq!(strategy.strategy_name(), "AvroBatch");
}

#[tokio::test]
async fn test_avro_write_and_read_batch() {
    let config = create_test_config();
    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
    let strategy = AvroSerializationStrategy::new(&config, filesystem_factory.clone())
        .await
        .expect("Failed to create strategy");

    let collection_id = "test_collection";
    create_collection_write_buffer_dir(collection_id).await;

    let vectors = vec![
        create_test_vector("vec1", 128),
        create_test_vector("vec2", 128),
        create_test_vector("vec3", 128),
    ];
    let batch = create_test_batch(vectors);

    // Write batch
    let sequences = strategy
        .write_native_batch(batch.clone(), collection_id)
        .await
        .expect("Failed to write batch");

    assert_eq!(sequences.len(), 3);
    assert!(sequences.iter().all(|&seq| seq > 0));

    // Read vectors back
    let retrieved = strategy
        .get_collection_vectors(collection_id)
        .await
        .expect("Failed to get vectors");

    assert_eq!(retrieved.len(), 3);

    // Collect IDs and verify all are present (order not guaranteed)
    let mut ids: Vec<String> = retrieved
        .iter()
        .map(|v| v.id.as_ref().unwrap().clone())
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["vec1", "vec2", "vec3"]);
}

#[tokio::test]
async fn test_avro_search_by_id() {
    let config = create_test_config();
    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
    let strategy = AvroSerializationStrategy::new(&config, filesystem_factory.clone())
        .await
        .expect("Failed to create strategy");

    let collection_id = "test_collection";
    create_collection_write_buffer_dir(collection_id).await;
    let vector = create_test_vector("search_test", 64);
    let batch = create_test_batch(vec![vector.clone()]);

    strategy
        .write_native_batch(batch, collection_id)
        .await
        .expect("Failed to write batch");

    // Search for existing vector
    let found = strategy
        .search_vector_by_id(collection_id, &"search_test".to_string())
        .await
        .expect("Failed to search");

    assert!(found.is_some());
    assert_eq!(found.unwrap().id.as_ref().unwrap(), "search_test");

    // Search for non-existing vector
    let not_found = strategy
        .search_vector_by_id(collection_id, &"non_existent".to_string())
        .await
        .expect("Failed to search");

    assert!(not_found.is_none());
}

#[tokio::test]
async fn test_avro_similarity_search() {
    let config = create_test_config();
    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
    let strategy = AvroSerializationStrategy::new(&config, filesystem_factory.clone())
        .await
        .expect("Failed to create strategy");

    let collection_id = "similarity_test";
    create_collection_write_buffer_dir(collection_id).await;

    // Create vectors with different values
    let mut vectors = Vec::new();
    for i in 0..10 {
        let mut vector = create_test_vector(&format!("vec_{}", i), 128);
        // Make each vector slightly different
        vector.vector = vec![i as f32 * 0.1; 128];
        vectors.push(vector);
    }

    let batch = create_test_batch(vectors);
    strategy
        .write_native_batch(batch, collection_id)
        .await
        .expect("Failed to write batch");

    // Search for similar vectors
    let query = vec![0.25; 128]; // Should be closest to vec_2 or vec_3
    let results = strategy
        .search_vectors_similarity(collection_id, &query, 5, Some(DistanceMetric::Cosine))
        .await
        .expect("Failed to search");

    assert_eq!(results.len(), 5);
    // Results should be sorted by distance
    for i in 1..results.len() {
        assert!(
            results[i - 1].1 <= results[i].1,
            "Results not sorted by distance"
        );
    }
}

#[tokio::test]
async fn test_avro_stats_tracking() {
    let config = create_test_config();
    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
    let strategy = AvroSerializationStrategy::new(&config, filesystem_factory.clone())
        .await
        .expect("Failed to create strategy");

    let collection_id = "stats_test";
    create_collection_write_buffer_dir(collection_id).await;

    // Get initial stats
    let initial_stats = strategy.stats().await.expect("Failed to get stats");

    assert_eq!(initial_stats.total_entries, 0);
    assert_eq!(initial_stats.memory_entries, 0);

    // Write some vectors
    let vectors = vec![
        create_test_vector("stat1", 64),
        create_test_vector("stat2", 64),
    ];
    let batch = create_test_batch(vectors);

    strategy
        .write_native_batch(batch, collection_id)
        .await
        .expect("Failed to write batch");

    // Check updated stats
    let updated_stats = strategy.stats().await.expect("Failed to get stats");

    assert_eq!(updated_stats.total_entries, 2);
    assert_eq!(updated_stats.memory_entries, 2);
    assert!(updated_stats.memory_size_bytes > 0);
}

#[tokio::test]
async fn test_avro_collection_stats() {
    let config = create_test_config();
    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
    let strategy = AvroSerializationStrategy::new(&config, filesystem_factory.clone())
        .await
        .expect("Failed to create strategy");

    let collection_id = "collection_stats_test";
    create_collection_write_buffer_dir(collection_id).await;

    // Write vectors to collection
    let vectors = vec![
        create_test_vector("col1", 128),
        create_test_vector("col2", 128),
        create_test_vector("col3", 128),
    ];
    let batch = create_test_batch(vectors);

    strategy
        .write_native_batch(batch, collection_id)
        .await
        .expect("Failed to write batch");

    // Get collection-specific stats
    let col_stats = strategy
        .collection_stats(collection_id)
        .await
        .expect("Failed to get collection stats");

    assert_eq!(col_stats.total_entries, 3);
    assert_eq!(col_stats.memory_entries, 3);
    assert!(col_stats.memory_size_bytes > 0);
}

#[tokio::test]
async fn test_avro_write_with_sync() {
    let config = create_test_config();
    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
    let strategy = AvroSerializationStrategy::new(&config, filesystem_factory.clone())
        .await
        .expect("Failed to create strategy");

    let collection_id = "sync_test";
    create_collection_write_buffer_dir(collection_id).await;
    let vectors = vec![create_test_vector("sync1", 64)];
    let batch = create_test_batch(vectors);

    // Write with immediate sync
    let sequences = strategy
        .write_vector_batch_with_sync(batch, collection_id, true)
        .await
        .expect("Failed to write with sync");

    assert_eq!(sequences.len(), 1);
}

#[tokio::test]
async fn test_avro_read_all_batches() {
    let config = create_test_config();
    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
    let strategy = AvroSerializationStrategy::new(&config, filesystem_factory.clone())
        .await
        .expect("Failed to create strategy");

    let collection_id = "batch_read_test";

    create_collection_write_buffer_dir(collection_id).await;
    // Write multiple batches
    for i in 0..3 {
        let vectors = vec![
            create_test_vector(&format!("batch{}_vec1", i), 64),
            create_test_vector(&format!("batch{}_vec2", i), 64),
        ];
        let batch = create_test_batch(vectors);

        strategy
            .write_native_batch(batch, collection_id)
            .await
            .expect("Failed to write batch");
    }

    // Read all batches
    let all_batches = strategy
        .read_all_batches(collection_id, None)
        .await
        .expect("Failed to read batches");

    assert_eq!(all_batches.len(), 3);

    // Read with limit
    let limited_batches = strategy
        .read_all_batches(collection_id, Some(2))
        .await
        .expect("Failed to read batches with limit");

    assert_eq!(limited_batches.len(), 2);
}

#[tokio::test]
async fn test_avro_empty_collection_operations() {
    let config = create_test_config();
    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
    let strategy = AvroSerializationStrategy::new(&config, filesystem_factory.clone())
        .await
        .expect("Failed to create strategy");

    let collection_id = "empty_collection";

    // Operations on empty collection should not fail
    let vectors = strategy
        .get_collection_vectors(collection_id)
        .await
        .expect("Failed to get vectors from empty collection");
    assert_eq!(vectors.len(), 0);

    let search_results = strategy
        .search_vectors_similarity(collection_id, &vec![0.1; 64], 10, None)
        .await
        .expect("Failed to search empty collection");
    assert_eq!(search_results.len(), 0);

    let stats = strategy
        .collection_stats(collection_id)
        .await
        .expect("Failed to get stats for empty collection");
    assert_eq!(stats.total_entries, 0);
}

#[tokio::test]
async fn test_avro_multiple_collections() {
    let config = create_test_config();
    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
    let strategy = AvroSerializationStrategy::new(&config, filesystem_factory.clone())
        .await
        .expect("Failed to create strategy");

    // Write to multiple collections
    for i in 0..3 {
        let collection_id = format!("collection_{}", i);
        create_collection_write_buffer_dir(&collection_id).await;
        let vectors = vec![
            create_test_vector(&format!("col{}_vec1", i), 64),
            create_test_vector(&format!("col{}_vec2", i), 64),
        ];
        let batch = create_test_batch(vectors);

        strategy
            .write_native_batch(batch, &collection_id)
            .await
            .expect("Failed to write batch");
    }

    // Verify isolation between collections
    for i in 0..3 {
        let collection_id = format!("collection_{}", i);
        let vectors = strategy
            .get_collection_vectors(&collection_id)
            .await
            .expect("Failed to get vectors");

        assert_eq!(vectors.len(), 2);
        assert!(
            vectors[0]
                .id
                .as_ref()
                .unwrap()
                .contains_hash(&format!("col{}_", i))
        );
        assert!(
            vectors[1]
                .id
                .as_ref()
                .unwrap()
                .contains_hash(&format!("col{}_", i))
        );
    }
}

// Integration test with storage engine mock
#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::storage::traits::{FlushParameters, FlushResult, UnifiedStorageEngine};
    use async_trait::async_trait;

    /// Mock storage engine for testing
    struct MockStorageEngine {
        flush_called: Arc<tokio::sync::Mutex<bool>>,
    }

    #[async_trait]
    impl UnifiedStorageEngine for MockStorageEngine {
        fn engine_name(&self) -> &'static str {
            "MockEngine"
        }

        fn engine_version(&self) -> &'static str {
            "1.0.0"
        }

        fn strategy(&self) -> crate::storage::traits::StorageEngineStrategy {
            crate::storage::traits::StorageEngineStrategy::Viper
        }

        async fn do_flush(&self, _params: &FlushParameters) -> Result<FlushResult> {
            let mut called = self.flush_called.lock().await;
            *called = true;

            Ok(FlushResult {
                success: true,
                entries_flushed: 10,
                bytes_written: 1024,
                files_created: 1,
                duration_ms: 100,
                completed_at: chrono::Utc::now(),
                engine_metrics: std::collections::HashMap::new(),
                compaction_triggered: false,
                collections_affected: vec![],
                flushed_batch_ids: vec![],
            })
        }

        async fn do_compact(
            &self,
            _params: &crate::storage::traits::CompactionParameters,
        ) -> Result<crate::storage::traits::CompactionResult> {
            Ok(crate::storage::traits::CompactionResult {
                success: true,
                collections_affected: vec![],
                entries_processed: 0,
                entries_removed: 0,
                bytes_read: 0,
                bytes_written: 0,
                input_files: 0,
                output_files: 0,
                duration_ms: 0,
                completed_at: chrono::Utc::now(),
                engine_metrics: std::collections::HashMap::new(),
            })
        }

        async fn collect_engine_metrics(
            &self,
        ) -> Result<std::collections::HashMap<String, serde_json::Value>> {
            Ok(std::collections::HashMap::new())
        }

        async fn vector_by_id(
            &self,
            _collection_id: &str,
            _vector_id: &str,
        ) -> Result<Option<crate::core::VectorRecord>> {
            Ok(None)
        }

        async fn search_vectors(
            &self,
            _query_context: &crate::storage::traits::StorageQueryContext,
            _operation_name: &str,
            _query_vector: &[f32],
            _top_k: usize,
        ) -> Result<Vec<VectorRecord>> {
            Ok(vec![])
        }

        fn get_filesystem_factory(
            &self,
        ) -> &crate::storage::persistence::filesystem::FilesystemFactory {
            panic!("Mock engine doesn't have filesystem factory")
        }
    }

    #[tokio::test]
    #[ignore = "Requires runtime modifications - blocking_write issue"]
    async fn test_avro_flush_with_storage_engine() {
        let config = create_test_config();
        let filesystem_factory =
            Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
        let strategy = AvroSerializationStrategy::new(&config, filesystem_factory.clone())
            .await
            .expect("Failed to create strategy");

        // Set up mock storage engine
        let mock_engine = Arc::new(MockStorageEngine {
            flush_called: Arc::new(tokio::sync::Mutex::new(false)),
        });
        strategy.set_storage_engine(mock_engine.clone());

        let collection_id = "flush_test";

        // Write vectors
        let vectors = vec![
            create_test_vector("flush1", 64),
            create_test_vector("flush2", 64),
        ];
        let batch = create_test_batch(vectors);

        strategy
            .write_native_batch(batch, collection_id)
            .await
            .expect("Failed to write batch");

        // Flush collection
        let flush_result = strategy
            .flush_collection(collection_id)
            .await
            .expect("Failed to flush");

        assert!(flush_result.success);
        assert_eq!(flush_result.entries_flushed, 10); // From mock

        // Verify storage engine was called
        let flush_called = mock_engine.flush_called.lock().await;
        assert!(*flush_called);
    }
}
