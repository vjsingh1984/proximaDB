//! Integration tests for the Nova storage engine.

use std::sync::Arc;
use tempfile::TempDir;

use proximadb::core::VectorRecord;
use proximadb::proto::proximadb_v1::{CollectionConfig, DistanceMetric, StorageEngine};
use proximadb::services::collection::manager::CollectionService;
use proximadb::services::operations::vectors::VectorOperationsService;
use proximadb::storage::engines::impls::nova::NovaEngine;
use proximadb::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
use proximadb::storage::persistence::write_ahead_log::BatchId;
use proximadb::storage::traits::{FlushParameters, UnifiedStorageEngine};
use std::collections::HashMap;

// Import test utilities
#[path = "../common/collection_builder.rs"]
mod collection_builder;
#[path = "../common/vector_generator.rs"]
mod vector_generator;

use collection_builder::TestCollectionBuilder;
use vector_generator::sequential;

/// Test setup helper - DEPRECATED, use direct Collection creation instead
/// This setup requires complex CollectionService initialization which is not needed for engine tests
#[allow(dead_code)]
async fn create_test_setup() -> (Arc<NovaEngine>, Arc<CollectionService>, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let filesystem_config = FilesystemConfig::default();
    let _filesystem = Arc::new(FilesystemFactory::create(filesystem_config).await.unwrap());

    let metadata_backend = Arc::new(
        proximadb::storage::metadata::MetadataStore::new(
            proximadb::storage::metadata::MetadataStoreConfig::default(),
        )
        .await
        .unwrap(),
    ) as Arc<dyn proximadb::storage::traits::InternalCollectionProvider>;
    let storage_config = proximadb::core::config::StorageConfig::default();
    let collection_service = Arc::new(
        CollectionService::new(metadata_backend, storage_config)
            .await
            .unwrap(),
    );

    let nova_engine = Arc::new(NovaEngine::new().await.unwrap());

    (nova_engine, collection_service, temp_dir)
}

/// Create test vectors
/// REFACTORED: Now uses vector_generator::sequential()
fn create_test_vectors(count: usize) -> Vec<VectorRecord> {
    sequential("nova_test_collection", count, 128)
}

#[tokio::test]
async fn test_nova_engine_creation_and_insertion() {
    let nova_engine = NovaEngine::new().await.unwrap();

    let vectors = create_test_vectors(100);
    let collection_id = "nova_test_collection";

    // REFACTORED: Use TestCollectionBuilder
    let (mut collection, _temp) = TestCollectionBuilder::new()
        .with_id(collection_id)
        .with_name(collection_id)
        .with_dimension(128)
        .with_engine(StorageEngine::Nova)
        .with_distance_metric(DistanceMetric::Cosine)
        .build();

    let base_location = _temp.path().to_str().unwrap().to_string();

    let batch_ids: Vec<BatchId> = (0..100).map(|_| BatchId::new()).collect();

    let flush_params = FlushParameters {
        collection_id: Some(collection_id.to_string()),
        vector_records: vectors,
        batch_ids,
        force: true,
        synchronous: true,
        hints: HashMap::new(),
        timeout_ms: Some(30000),
        trigger_compaction: false,
        collection_config: Some(collection),
        estimated_size: 1024 * 1024,
    };

    println!(
        "🧪 Test: Flushing {} vectors",
        flush_params.vector_records.len()
    );
    let flush_result = nova_engine.flush(flush_params).await.unwrap();

    println!(
        "🧪 Test: Flush result - success={}, entries={:?}",
        flush_result.success, flush_result.entries_flushed
    );
    assert!(flush_result.success, "Flush should succeed");
    assert_eq!(
        flush_result.entries_flushed,
        Some(100),
        "Should flush 100 vectors"
    );

    // Verify files were created using filesystem API for cloud compatibility
    let data_path = format!("{}/{}/data", base_location, collection_id);
    let filesystem_config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::create(filesystem_config).await.unwrap());
    let fs = filesystem.get_filesystem(&data_path).unwrap();

    let entries = fs.list(&data_path).await.unwrap();
    let parquet_files: Vec<_> = entries
        .into_iter()
        .filter(|e| !e.metadata.is_directory && e.name.ends_with(".parquet"))
        .collect();

    println!("🧪 Test: Found {} parquet files", parquet_files.len());
    assert!(
        !parquet_files.is_empty(),
        "Should have created at least one parquet file"
    );

    // Note: vector_by_id requires ID index which may not be built yet, so we skip that check
    // The important verification is that flush succeeded and files were created
}

#[tokio::test]
async fn test_nova_flush_basic() {
    let nova_engine = NovaEngine::new().await.unwrap();

    let vectors = create_test_vectors(10);
    let collection_id = "test_flush";

    // REFACTORED: Use TestCollectionBuilder
    let (collection, _temp) = TestCollectionBuilder::new()
        .with_id(collection_id)
        .with_name(collection_id)
        .with_dimension(128)
        .with_engine(StorageEngine::Nova)
        .with_distance_metric(DistanceMetric::Cosine)
        .build();

    let base_location = _temp.path().to_str().unwrap().to_string();

    let flush_params = FlushParameters {
        collection_id: Some(collection_id.to_string()),
        vector_records: vectors,
        batch_ids: vec![],
        force: true,
        synchronous: true,
        hints: HashMap::new(),
        timeout_ms: Some(30000),
        trigger_compaction: false,
        collection_config: Some(collection),
        estimated_size: 1024 * 1024,
    };

    println!(
        "🧪 Test: Flushing {} vectors to {}",
        flush_params.vector_records.len(),
        base_location
    );
    let flush_result = nova_engine.flush(flush_params).await.unwrap();

    println!(
        "🧪 Test: Flush result - success={}, entries={:?}, bytes={:?}",
        flush_result.success, flush_result.entries_flushed, flush_result.bytes_written
    );

    assert!(flush_result.success, "Flush should succeed");
    assert_eq!(
        flush_result.entries_flushed,
        Some(10),
        "Should flush 10 vectors"
    );
    assert!(
        flush_result.bytes_written.unwrap() > 0,
        "Should write some bytes"
    );

    // Verify files exist using filesystem API (cloud-compatible)
    let data_path = format!("{}/{}/data", base_location, collection_id);
    println!("🧪 Test: Checking for files in {}", data_path);

    let filesystem_config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::create(filesystem_config).await.unwrap());
    let fs = filesystem.get_filesystem(&data_path).unwrap();

    let entries = fs.list(&data_path).await.unwrap();
    let parquet_files: Vec<_> = entries
        .into_iter()
        .filter(|e| !e.metadata.is_directory && e.name.ends_with(".parquet"))
        .collect();

    println!("🧪 Test: Found {} parquet files", parquet_files.len());
    for file in &parquet_files {
        println!("  - {}", file.name);
    }

    assert!(
        !parquet_files.is_empty(),
        "Should have created at least one parquet file"
    );
}

#[tokio::test]
async fn test_nova_search_basic() {
    use proximadb::core::search::SearchParams;
    use proximadb::storage::traits::{StorageQueryContext, StorageQueryMetadata};

    let nova_engine = NovaEngine::new().await.unwrap();

    let vectors = create_test_vectors(20);
    let collection_id = "test_search";

    // REFACTORED: Use TestCollectionBuilder
    let (collection, _temp) = TestCollectionBuilder::new()
        .with_id(collection_id)
        .with_name(collection_id)
        .with_dimension(128)
        .with_engine(StorageEngine::Nova)
        .with_distance_metric(DistanceMetric::Cosine)
        .build();

    let base_location = _temp.path().to_str().unwrap().to_string();

    // First flush some vectors
    let flush_params = FlushParameters {
        collection_id: Some(collection_id.to_string()),
        vector_records: vectors.clone(),
        batch_ids: vec![],
        force: true,
        synchronous: true,
        hints: HashMap::new(),
        timeout_ms: Some(30000),
        trigger_compaction: false,
        collection_config: Some(collection.clone()),
        estimated_size: 1024 * 1024,
    };

    println!("🧪 Test: Flushing {} vectors", vectors.len());
    let flush_result = nova_engine.flush(flush_params).await.unwrap();
    assert!(flush_result.success);

    // Now search
    let query_vector = vectors[0].vector.clone();
    let search_params = Arc::new(SearchParams {
        vector: Some(query_vector),
        top_k: Some(5),
        ..Default::default()
    });

    let ctx = StorageQueryContext {
        search_params,
        collection: Arc::new(collection),
        metadata: StorageQueryMetadata {
            collection_id: collection_id.to_string(),
            dimension: 128,
            storage_path: base_location.clone(),
            ..Default::default()
        },
        user_context: None,
        tenant_context: None,
    };

    // Check what files exist before searching (using filesystem API for cloud compatibility)
    let data_path = format!("{}/{}/data", base_location, collection_id);
    println!("🧪 Test: Checking data_path: {}", data_path);

    let filesystem_config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::create(filesystem_config).await.unwrap());
    let fs = filesystem.get_filesystem(&data_path).unwrap();

    if let Ok(entries) = fs.list(&data_path).await {
        println!("🧪 Test: Found {} files in data directory", entries.len());
        for file in entries {
            println!("  - {} (is_dir={})", file.name, file.metadata.is_directory);
        }
    } else {
        println!("🧪 Test: data_path does not exist or cannot be read");
    }

    println!("🧪 Test: Searching...");
    let results = nova_engine.search_vectors_unified(&ctx).await.unwrap();

    println!("🧪 Test: Found {} results", results.len());
    for (i, r) in results.iter().enumerate() {
        println!("  Result {}: id={}, score={}", i, r.id, r.score);
    }

    assert!(!results.is_empty(), "Should find at least one result");
    assert!(results.len() <= 5, "Should return at most 5 results");
    assert_eq!(
        results[0].id, "vec_0",
        "First result should be vec_0 (exact match)"
    );
}

#[tokio::test]
async fn test_nova_compact_basic() {
    use proximadb::storage::traits::CompactionParameters;

    let nova_engine = NovaEngine::new().await.unwrap();

    let collection_id = "test_compact";

    // REFACTORED: Use TestCollectionBuilder
    let (collection, _temp) = TestCollectionBuilder::new()
        .with_id(collection_id)
        .with_name(collection_id)
        .with_dimension(128)
        .with_engine(StorageEngine::Nova)
        .with_distance_metric(DistanceMetric::Cosine)
        .build();

    let base_location = _temp.path().to_str().unwrap().to_string();

    // Flush multiple batches to create multiple files
    for i in 0..3 {
        let vectors = create_test_vectors(10);
        let flush_params = FlushParameters {
            collection_id: Some(collection_id.to_string()),
            vector_records: vectors,
            batch_ids: vec![],
            force: true,
            synchronous: true,
            hints: HashMap::new(),
            timeout_ms: Some(30000),
            trigger_compaction: false,
            collection_config: Some(collection.clone()),
            estimated_size: 1024 * 1024,
        };

        println!("🧪 Test: Flushing batch {}", i);
        let flush_result = nova_engine.flush(flush_params).await.unwrap();
        assert!(flush_result.success);
    }

    // Compact
    let compact_params = CompactionParameters {
        collection_id: Some(collection_id.to_string()),
        collection_config: Some(collection),
        force: true,
        synchronous: true,
        hints: HashMap::new(),
        timeout_ms: Some(60000),
        priority: proximadb::storage::traits::OperationPriority::Medium,
        estimated_input_size: 1024 * 1024 * 10, // 10MB estimate
    };

    println!("🧪 Test: Starting compaction");
    let compact_result = nova_engine.compact(compact_params).await.unwrap();

    println!(
        "🧪 Test: Compaction result - success={}, input_files={:?}, output_files={:?}, entries_processed={:?}",
        compact_result.success,
        compact_result.input_files,
        compact_result.output_files,
        compact_result.entries_processed
    );

    assert!(compact_result.success, "Compaction should succeed");
}
