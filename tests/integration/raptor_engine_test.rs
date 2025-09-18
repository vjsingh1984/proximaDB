//! Integration tests for the Raptor storage engine.

use std::sync::Arc;
use tempfile::TempDir;

use proximadb::core::VectorRecord;
use proximadb::proto::proximadb_v1::{
    CollectionConfig, DistanceMetric, StorageEngine,
};
use proximadb::storage::engines::impls::raptor::RaptorEngine;
use proximadb::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
use proximadb::storage::persistence::write_ahead_log::BatchId;

/// Test setup helper
async fn create_test_setup() -> (Arc<RaptorEngine>, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let filesystem_config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::new(filesystem_config).await.unwrap());

    // Create collection service - not needed for this test since we're testing the engine directly
    // The collection service would normally manage collections at a higher level

    // Create cache orchestrator for RAPTOR engine with memory budget
    let cache_orchestrator = Arc::new(
        proximadb::storage::cache::orchestrator::CrossCacheOrchestrator::new(
            1024 * 1024 * 1024  // 1GB memory budget
        )
    );

    let raptor_engine = Arc::new(
        RaptorEngine::new(
            "raptor_test_collection".to_string(),
            temp_dir.path().to_string_lossy().to_string(), // base_path
            proximadb::storage::engines::impls::raptor::config::RaptorConfig::default(),
            cache_orchestrator,
        )
        .await
        .unwrap(),
    );

    (raptor_engine, temp_dir)
}

/// Create test vectors
fn create_test_vectors(count: usize) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| {
            let vector = (0..128)
                .map(|j| (i * 128 + j) as f32 / (count * 128) as f32)
                .collect();

            VectorRecord {
                id: format!("vec_{}", i),
                vector,
                metadata: std::collections::HashMap::new(),
                timestamp: 0,
                updated_at: Some(0),
                expires_at: None,
                version: Some(1),
                quantized_vector: vec![],
                source: None,
            }
        })
        .collect()
}

#[tokio::test]
async fn test_raptor_engine_creation_and_insertion() {
    let (raptor_engine, _temp_dir) = create_test_setup().await;

    // Collection configuration - used directly with the engine
    let config = CollectionConfig {
        name: "raptor_test_collection".to_string(),
        dimension: 128,
        distance_metric: DistanceMetric::Cosine as i32,
        storage_engine: StorageEngine::Raptor as i32,
        ..Default::default()
    };

    let vectors = create_test_vectors(100);
    let batch_ids: Vec<BatchId> = (0..100).map(|i| BatchId::new(format!("batch_{}", i))).collect();
    let flush_params = proximadb::storage::traits::FlushParameters {
        collection_id: Some("raptor_test_collection".to_string()),
        vector_records: vectors,
        batch_ids,
        force: true,
        synchronous: true,
        collection_config: Some(config),
        base_location: None,
        flush_id: None,
        merge_policy: None,
    };

    let flush_result = raptor_engine.flush(flush_params).await.unwrap();
    assert!(flush_result.success);
    assert_eq!(flush_result.entries_flushed, Some(100));

    // Test would verify vectors are persisted - actual retrieval depends on engine implementation
    // The test verifies the flush was successful which means vectors were written
    // Individual vector retrieval is typically done through search operations
}
