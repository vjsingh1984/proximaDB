//! Tests for WAL Flush Coordinator
//!
//! These tests ensure the flush coordinator correctly handles:
//! - Coordinating flush operations between WAL and storage engines
//! - Managing flush state for collections
//! - Handling memory vs disk WAL modes
//! - Cleanup instructions after successful flushes

use crate::core::VectorRecord;
use crate::proto::proximadb_v1::MetadataItem;
use crate::storage::persistence::write_ahead_log::{
    FlushDataSource, WALFlushCoordinator, config::SyncMode,
};
use crate::storage::traits::{FlushParameters, FlushResult, UnifiedStorageEngine};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

/// Mock storage engine for testing
struct MockStorageEngine {
    flush_count: Arc<tokio::sync::Mutex<usize>>,
    should_fail: bool,
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
        crate::storage::traits::StorageEngineStrategy::Lsm
    }

    async fn do_flush(&self, params: &FlushParameters) -> Result<FlushResult> {
        if self.should_fail {
            return Err(anyhow::anyhow!("Mock flush failure"));
        }

        let mut count = self.flush_count.lock().await;
        *count += 1;

        Ok(FlushResult {
            success: true,
            entries_flushed: params.vector_records.len() as u64,
            bytes_written: (params.vector_records.len() * 100) as u64,
            files_created: 1,
            duration_ms: 10,
            completed_at: chrono::Utc::now(),
            engine_metrics: std::collections::HashMap::new(),
            compaction_triggered: false,
            collections_affected: vec![],
            flushed_batch_ids: params.batch_ids.clone(),
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

    async fn search_vectors_unified(
        &self,
        _collection_id: &str,
        _storage_url: &str,
        _query_vector: &[f32],
        _k: usize,
        _distance_metric: &crate::compute::distance_computation::DistanceMetric,
        _metadata_filters: Option<&crate::core::search::FilterExpression>,
        _include_vectors: bool,
        _include_metadata: bool,
    ) -> Result<Vec<crate::core::search::SearchResult>> {
        Ok(vec![])
    }

    fn get_filesystem_factory(
        &self,
    ) -> &crate::storage::persistence::filesystem::FilesystemFactory {
        panic!("Mock engine doesn't have filesystem factory")
    }

}

/// Create test vector
fn create_test_vector(id: &str) -> VectorRecord {
    VectorRecord {
        id: Some(id.to_string()),
        vector: vec![0.1; 128],
        metadata: vec![MetadataItem {
            key: "test".to_string(),
            value: Some(crate::proto::proximadb_v1::metadata_item::Value::StringValue(
                "true".to_string(),
            )),
        }],
        timestamp: 1234567890,
        updated_at: Some(1234567890),
        expires_at: None,
        version: Some(1),
        // rank removed -  None,
        similarity: None,
        similarity: None,
    }
}

#[tokio::test]
async fn test_flush_coordinator_creation() {
    let coordinator = WALFlushCoordinator::new();

    // Should be able to create and use immediately
    let result = coordinator.initialize_flush_state("test_collection").await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_register_storage_engine() {
    let coordinator = WALFlushCoordinator::new();

    let mock_engine = Arc::new(MockStorageEngine {
        flush_count: Arc::new(tokio::sync::Mutex::new(0)),
        should_fail: false,
    });

    // Register engine
    coordinator
        .register_storage_engine("VIPER", mock_engine)
        .await;

    // Engine should now be available for flush operations
    let flush_data = FlushDataSource::VectorRecords(vec![create_test_vector("test1")]);

    let result = coordinator
        .execute_coordinated_flush("test_collection", flush_data, Some("VIPER"), None)
        .await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap().base.entries_flushed, 1);
}

#[tokio::test]
async fn test_initialize_flush_state() {
    let coordinator = WALFlushCoordinator::new();

    // Initialize state for collection
    let result = coordinator.initialize_flush_state("test_collection").await;
    assert!(result.is_ok());

    // Should be idempotent
    let result2 = coordinator.initialize_flush_state("test_collection").await;
    assert!(result2.is_ok());

    // State should exist
    let state = coordinator.get_flush_state("test_collection").await;
    assert!(state.is_some());
}

#[tokio::test]
async fn test_initiate_flush_memory_mode() {
    let coordinator = WALFlushCoordinator::new();
    coordinator
        .initialize_flush_state("memory_test")
        .await
        .unwrap();

    let sequences = vec![1, 2, 3, 4, 5];
    let data_source = coordinator
        .initiate_flush("memory_test", sequences.clone(), &SyncMode::MemoryOnly)
        .await
        .unwrap();

    // Should use memory source for MemoryOnly mode
    match data_source {
        FlushDataSource::Memory => {
            // Expected
        }
        _ => panic!("Expected Memory data source for MemoryOnly sync mode"),
    }

    // Should not use disk WAL
    assert!(!coordinator.uses_disk_wal("memory_test").await);
}

#[tokio::test]
async fn test_initiate_flush_disk_mode() {
    let coordinator = WALFlushCoordinator::new();
    coordinator
        .initialize_flush_state("disk_test")
        .await
        .unwrap();

    let sequences = vec![10, 20, 30];
    let data_source = coordinator
        .initiate_flush("disk_test", sequences.clone(), &SyncMode::Always)
        .await
        .unwrap();

    // Should use disk files for non-MemoryOnly modes
    match data_source {
        FlushDataSource::DiskWalFiles(_) => {
            // Expected
        }
        _ => panic!("Expected DiskWalFiles data source for Always sync mode"),
    }

    // Should use disk WAL
    assert!(coordinator.uses_disk_wal("disk_test").await);
}

#[tokio::test]
async fn test_acknowledge_flush() {
    let coordinator = WALFlushCoordinator::new();
    coordinator
        .initialize_flush_state("ack_test")
        .await
        .unwrap();

    // Initiate a flush
    let sequences = vec![1, 2, 3];
    let _data_source = coordinator
        .initiate_flush("ack_test", sequences.clone(), &SyncMode::MemoryOnly)
        .await
        .unwrap();

    // Get the pending flush
    let pending = coordinator.get_pending_flushes("ack_test").await;
    assert_eq!(pending.len(), 1);
    let flush_id = pending[0].flush_id;

    // Acknowledge the flush
    let cleanup = coordinator
        .acknowledge_flush("ack_test", flush_id, sequences.clone())
        .await
        .unwrap();

    // Should get cleanup instructions
    assert!(cleanup.cleanup_memory);
    assert_eq!(cleanup.sequences_to_cleanup, sequences);

    // Pending flush should be removed
    let pending_after = coordinator.get_pending_flushes("ack_test").await;
    assert_eq!(pending_after.len(), 0);
}

#[tokio::test]
async fn test_execute_coordinated_flush_empty() {
    let coordinator = WALFlushCoordinator::new();

    // Register mock engine
    let mock_engine = Arc::new(MockStorageEngine {
        flush_count: Arc::new(tokio::sync::Mutex::new(0)),
        should_fail: false,
    });
    coordinator
        .register_storage_engine("VIPER", mock_engine)
        .await;

    // Execute flush with empty data
    let flush_data = FlushDataSource::VectorRecords(vec![]);
    let result = coordinator
        .execute_coordinated_flush("empty_test", flush_data, Some("VIPER"), None)
        .await
        .unwrap();

    // Should succeed with 0 entries
    assert!(result.base.success);
    assert_eq!(result.base.entries_flushed, 0);
}

#[tokio::test]
async fn test_execute_coordinated_flush_with_data() {
    let coordinator = WALFlushCoordinator::new();

    // Register mock engine
    let mock_engine = Arc::new(MockStorageEngine {
        flush_count: Arc::new(tokio::sync::Mutex::new(0)),
        should_fail: false,
    });
    coordinator
        .register_storage_engine("VIPER", mock_engine.clone())
        .await;

    // Execute flush with actual data
    let vectors = vec![
        create_test_vector("vec1"),
        create_test_vector("vec2"),
        create_test_vector("vec3"),
    ];
    let flush_data = FlushDataSource::VectorRecords(vectors);

    let result = coordinator
        .execute_coordinated_flush("data_test", flush_data, Some("VIPER"), None)
        .await
        .unwrap();

    // Should succeed with 3 entries
    assert!(result.base.success);
    assert_eq!(result.base.entries_flushed, 3);
    assert_eq!(result.base.bytes_written, 300); // 3 * 100 from mock

    // Verify flush was called
    let count = mock_engine.flush_count.lock().await;
    assert_eq!(*count, 1);
}

#[tokio::test]
async fn test_execute_coordinated_flush_engine_not_found() {
    let coordinator = WALFlushCoordinator::new();

    // Try to flush without registering engine
    let flush_data = FlushDataSource::VectorRecords(vec![create_test_vector("test")]);
    let result = coordinator
        .execute_coordinated_flush("no_engine_test", flush_data, Some("NonExistent"), None)
        .await;

    // Should fail
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains_hash("not registered")
    );
}

#[tokio::test]
async fn test_cancel_flush() {
    let coordinator = WALFlushCoordinator::new();
    coordinator
        .initialize_flush_state("cancel_test")
        .await
        .unwrap();

    // Initiate a flush
    let sequences = vec![1, 2, 3];
    let _data_source = coordinator
        .initiate_flush("cancel_test", sequences, &SyncMode::Always)
        .await
        .unwrap();

    // Get the pending flush
    let pending = coordinator.get_pending_flushes("cancel_test").await;
    assert_eq!(pending.len(), 1);
    let flush_id = pending[0].flush_id;

    // Cancel the flush
    let result = coordinator.cancel_flush("cancel_test", flush_id).await;
    assert!(result.is_ok());

    // Should no longer have pending flush
    let pending_after = coordinator.get_pending_flushes("cancel_test").await;
    assert_eq!(pending_after.len(), 0);
}

#[tokio::test]
async fn test_cleanup_collection() {
    let coordinator = WALFlushCoordinator::new();

    // Initialize and use collection
    coordinator
        .initialize_flush_state("cleanup_test")
        .await
        .unwrap();
    let _data_source = coordinator
        .initiate_flush("cleanup_test", vec![1, 2, 3], &SyncMode::Always)
        .await
        .unwrap();

    // Verify state exists
    assert!(coordinator.get_flush_state("cleanup_test").await.is_some());

    // Cleanup collection
    coordinator.cleanup_collection("cleanup_test").await;

    // State should be gone
    assert!(coordinator.get_flush_state("cleanup_test").await.is_none());
}

#[tokio::test]
async fn test_drop_collection() {
    let coordinator = WALFlushCoordinator::new();

    // Initialize collection
    coordinator
        .initialize_flush_state("drop_test")
        .await
        .unwrap();

    // Drop collection
    let result = coordinator.drop_collection("drop_test").await;
    assert!(result.is_ok());

    // State should be gone
    assert!(coordinator.get_flush_state("drop_test").await.is_none());
}

#[tokio::test]
async fn test_multiple_pending_flushes() {
    let coordinator = WALFlushCoordinator::new();
    coordinator
        .initialize_flush_state("multi_flush")
        .await
        .unwrap();

    // Initiate multiple flushes
    for i in 0..3 {
        let sequences = vec![i * 10, i * 10 + 1, i * 10 + 2];
        let _data_source = coordinator
            .initiate_flush("multi_flush", sequences, &SyncMode::Always)
            .await
            .unwrap();
    }

    // Should have 3 pending flushes
    let pending = coordinator.get_pending_flushes("multi_flush").await;
    assert_eq!(pending.len(), 3);

    // Each should have different flush IDs
    let mut flush_ids: Vec<_> = pending.iter().map(|p| p.flush_id).collect();
    flush_ids.sort();
    flush_ids.dedup();
    assert_eq!(flush_ids.len(), 3);
}
