//! Integration tests for the Nova storage engine.

use std::sync::Arc;
use tempfile::TempDir;

use std::collections::HashMap;
use proximadb::core::VectorRecord;
use proximadb::proto::proximadb_v1::{
    CollectionConfig, DistanceMetric, StorageEngine,
};
use proximadb::services::operations::vectors::VectorOperationsService;
use proximadb::services::collection::manager::CollectionService;
use proximadb::storage::engines::impls::nova::NovaEngine;
use proximadb::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
use proximadb::storage::persistence::write_ahead_log::BatchId;
use proximadb::storage::traits::{FlushParameters, UnifiedStorageEngine};

/// Test setup helper
async fn create_test_setup() -> (Arc<NovaEngine>, Arc<CollectionService>, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let filesystem_config = FilesystemConfig::default();
    let _filesystem = Arc::new(FilesystemFactory::new(filesystem_config).await.unwrap());

    // TODO: Fix CollectionService constructor - needs metadata backend and storage config
    // Placeholder for now
    let metadata_backend = Arc::new(
        proximadb::storage::metadata::MetadataStore::new(
            proximadb::storage::metadata::MetadataStoreConfig::default()
        ).await.unwrap()
    ) as Arc<dyn proximadb::storage::traits::InternalCollectionProvider>;
    let storage_config = proximadb::core::config::StorageConfig::default();
    let collection_service = Arc::new(
        CollectionService::new(metadata_backend, storage_config)
            .await
            .unwrap(),
    );

    let nova_engine = Arc::new(
        NovaEngine::new()
        .await
        .unwrap(),
    );

    (nova_engine, collection_service, temp_dir)
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
                source: None,
            }
        })
        .collect()
}

#[tokio::test]
async fn test_nova_engine_creation_and_insertion() {
    let (nova_engine, collection_service, _temp_dir) = create_test_setup().await;

    let config = CollectionConfig {
        name: "nova_test_collection".to_string(),
        dimension: 128,
        distance_metric: DistanceMetric::Cosine as i32,
        storage_engine: StorageEngine::Nova as i32,
        auto_index_selection: true,
        owner: Some("test_user".to_string()),
        embedding_models: vec!["test_model".to_string()],
        ..Default::default()
    };
    collection_service.create_collection(&config).await.unwrap();

    let vectors = create_test_vectors(100);
    let batch_ids: Vec<BatchId> = (0..100).map(|_i| {
        BatchId::new()
    }).collect();

    let collection = collection_service
        .get_collection_with_tenant_context("nova_test_collection", None)
        .await
        .unwrap()
        .unwrap();

    let flush_params = FlushParameters {
        collection_id: Some("nova_test_collection".to_string()),
        vector_records: vectors,
        batch_ids,
        force: true,
        synchronous: true,
        hints: HashMap::new(),
        timeout_ms: Some(30000),
        trigger_compaction: false,
        collection_config: Some(collection),
        estimated_size: 1024 * 1024, // 1MB estimate
    };

    let flush_result = nova_engine.flush(flush_params).await.unwrap();
    assert!(flush_result.success);
    assert_eq!(flush_result.entries_flushed, Some(100));

    let vector = nova_engine
        .vector_by_id("nova_test_collection", "/tmp/proximadb-test/", "vec_10")
        .await
        .unwrap();
    assert!(vector.is_some());
    assert_eq!(vector.unwrap().id, "vec_10");
}
