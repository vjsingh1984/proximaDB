
//! Integration tests for the Swift storage engine.

use std::sync::Arc;
use tempfile::TempDir;

use proximadb::core::VectorRecord;
use proximadb::proto::proximadb::{CollectionConfig, DistanceMetric, StorageEngine, IndexingAlgorithm};
use proximadb::services::collection::manager::CollectionService;
use proximadb::storage::engines::impls::swift::SwiftEngine;
use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};

/// Test setup helper
async fn create_test_setup() -> (
    Arc<SwiftEngine>,
    Arc<CollectionService>,
    TempDir,
) {
    let temp_dir = TempDir::new().unwrap();
    let filesystem_config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::new(filesystem_config).await.unwrap());

    let collection_service = Arc::new(CollectionService::new(filesystem.clone(), temp_dir.path().to_path_buf()).await.unwrap());

    let swift_engine = Arc::new(SwiftEngine::new(
        "swift_test_collection".to_string(),
        Default::default(),
        filesystem.clone(),
        Arc::new(Default::default()),
    ).await.unwrap());

    (swift_engine, collection_service, temp_dir)
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
                metadata: Default::default(),
                timestamp: 0,
                updated_at: None,
                expires_at: None,
                quantized_vector: None,
                source: None,
            }
        })
        .collect()
}

#[tokio::test]
async fn test_swift_engine_creation_and_insertion() {
    let (swift_engine, collection_service, _temp_dir) = create_test_setup().await;

    let config = CollectionConfig {
        name: "swift_test_collection".to_string(),
        dimension: 128,
        distance_metric: DistanceMetric::Cosine as i32,
        storage_engine: StorageEngine::Swift as i32,
        indexing_algorithm: IndexingAlgorithm::Hnsw as i32,
        ..Default::default()
    };
    collection_service.create_collection(config).await.unwrap();

    let vectors = create_test_vectors(100);
    let flush_params = proximadb::storage::traits::FlushParameters {
        collection_id: Some("swift_test_collection".to_string()),
        vector_records: vectors,
        batch_ids: (0..100).map(|i| i.to_string()).collect(),
        force: true,
        synchronous: true,
        collection_config: Some(collection_service.get_collection_proto("swift_test_collection").await.unwrap().unwrap()),
    };

    let flush_result = swift_engine.do_flush(&flush_params).await.unwrap();
    assert!(flush_result.success);
    assert_eq!(flush_result.entries_flushed, Some(100));

    let vector = swift_engine.vector_by_id("swift_test_collection", "vec_10").await.unwrap();
    assert!(vector.is_some());
    assert_eq!(vector.unwrap().id, "vec_10");
}
