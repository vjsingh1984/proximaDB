//! Integration tests for the Prism storage engine.

use std::sync::Arc;
use tempfile::TempDir;

use proximadb::core::VectorRecord;
use proximadb::proto::proximadb_v1::{
    CollectionConfig, DistanceMetric, IndexingAlgorithm, StorageEngine,
};
use proximadb::services::collection::manager::CollectionService;
use proximadb::storage::engines::impls::prism::PrismEngine;
use proximadb::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};

/// Test setup helper
async fn create_test_setup() -> (Arc<PrismEngine>, Arc<CollectionService>, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let filesystem_config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::new(filesystem_config).await.unwrap());

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

    let prism_engine = Arc::new(
        PrismEngine::new(
            "prism_test_collection".to_string(),
            Default::default(),
            filesystem.clone(),
            Arc::new(Default::default()),
        )
        .await
        .unwrap(),
    );

    (prism_engine, collection_service, temp_dir)
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
async fn test_prism_engine_creation_and_insertion() {
    let (prism_engine, collection_service, _temp_dir) = create_test_setup().await;

    let config = CollectionConfig {
        name: "prism_test_collection".to_string(),
        dimension: 128,
        distance_metric: DistanceMetric::Cosine as i32,
        storage_engine: StorageEngine::Prism as i32,
        indexing_algorithm: IndexingAlgorithm::Hnsw as i32,
        ..Default::default()
    };
    collection_service.create_collection(config).await.unwrap();

    let vectors = create_test_vectors(100);
    let flush_params = proximadb::storage::traits::FlushParameters {
        collection_id: Some("prism_test_collection".to_string()),
        vector_records: vectors,
        batch_ids: (0..100).map(|i| i.to_string()).collect(),
        force: true,
        synchronous: true,
        collection_config: Some(
            collection_service
                .get_collection_proto("prism_test_collection")
                .await
                .unwrap()
                .unwrap(),
        ),
    };

    let flush_result = prism_engine.do_flush(&flush_params).await.unwrap();
    assert!(flush_result.success);
    assert_eq!(flush_result.entries_flushed, Some(100));

    let vector = prism_engine
        .vector_by_id("prism_test_collection", "vec_10")
        .await
        .unwrap();
    assert!(vector.is_some());
    assert_eq!(vector.unwrap().id, "vec_10");
}
