//! Comprehensive test suite for HELIX engine

use super::*;
use crate::compute::distance_computation::DistanceMetric;
use crate::proto::proximadb_v1::VectorRecord;
use crate::core::search::SearchParams;
use crate::proto::proximadb_v1::{
    Collection, CollectionConfig, CollectionStats, DistanceMetric as ProtoDistanceMetric,
    StorageEngine, StorageAssignment,
};
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::traits::{
    CompactionParameters, FlushParameters, OperationPriority, StorageQueryContext, StorageQueryMetadata,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::tempdir;

/// Test helper to create sample vector records
fn create_test_records(count: usize, dims: usize) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| VectorRecord {
            id: format!("vec_{}", i),
            vector: (0..dims).map(|d| (i * dims + d) as f32 / 100.0).collect(),
            metadata: HashMap::from([
                ("type".to_string(), crate::proto::proximadb_v1::SqlValue { value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue("test".to_string())) }),
                ("index".to_string(), crate::proto::proximadb_v1::SqlValue { value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(i.to_string())) }),
            ]),
            timestamp: i as i64,
            expires_at: None,
            source: None,
            updated_at: None,
            version: Some(1),
        })
        .collect()
}

#[tokio::test]
async fn test_helix_engine_creation() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let engine = HelixEngine::new()
    .await
    .unwrap();

    assert_eq!(engine.engine_name(), "helix");
    assert_eq!(engine.engine_version(), "1.0.0");
}

#[tokio::test]
async fn test_flush_operation() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let temp_dir = tempdir().unwrap();
    let path = temp_dir.path().to_str().unwrap().to_string();

    // Create filesystem factory with proper config
    let mut fs_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
    fs_config.default_fs = Some(format!("file://{}", path));
    let filesystem_factory = Arc::new(
        crate::storage::persistence::filesystem::FilesystemFactory::new(fs_config)
            .await
            .unwrap()
    );
    let distance_compute = Arc::new(
        crate::compute::distance_computation::engine::UnifiedDistanceCompute::default()
    );

    let config = HelixConfig::default();
    let engine = HelixEngine::new_with_config(config, filesystem_factory, distance_compute)
        .await
        .unwrap();

    let records = create_test_records(100, 128);

    // Create collection for the flush
    let collection_config = CollectionConfig {
        name: "test_collection".to_string(),
        dimension: 128,
        distance_metric: ProtoDistanceMetric::Euclidean as i32,
        storage_engine: StorageEngine::Helix as i32,
        ..Default::default()
    };

    let collection = Collection {
        id: "test_collection".to_string(),
        config: Some(collection_config),
        stats: Some(CollectionStats {
            vector_count: 0,
            index_size_bytes: 0,
            data_size_bytes: 0,
        }),
        storage_assignment: Some(StorageAssignment {
            primary_path: "/tmp/proximadb-data/helix".to_string(),
            backup_paths: vec![],
            engine: StorageEngine::Helix as i32,
            engine_config: HashMap::new(),
            base_location: "/tmp/proximadb-data".to_string(),
            assigned_at: 0,
        }),
        ..Default::default()
    };

    let params = FlushParameters {
        collection_id: Some("test_collection".to_string()),
        force: false,
        synchronous: true,
        hints: HashMap::new(),
        timeout_ms: None,
        vector_records: records,
        trigger_compaction: false,
        batch_ids: vec![],
        collection_config: Some(collection),
        estimated_size: 0,
    };

    let result = engine.do_flush(&params).await.unwrap();

    assert_eq!(result.entries_flushed, Some(100));
    assert!(result.bytes_written.unwrap_or(0) > 0);
    assert_eq!(result.files_created, Some(1));
}

#[tokio::test]
async fn test_vector_search() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let temp_dir = tempdir().unwrap();
    let path = temp_dir.path().to_str().unwrap().to_string();

    // Create filesystem factory with proper config
    let mut fs_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
    fs_config.default_fs = Some(format!("file://{}", path));
    let filesystem_factory = Arc::new(
        crate::storage::persistence::filesystem::FilesystemFactory::new(fs_config)
            .await
            .unwrap()
    );
    let distance_compute = Arc::new(
        crate::compute::distance_computation::engine::UnifiedDistanceCompute::default()
    );

    let config = HelixConfig::default();
    let engine = HelixEngine::new_with_config(config, filesystem_factory, distance_compute)
        .await
        .unwrap();

    // Create collection
    let collection_config = CollectionConfig {
        name: "test_collection".to_string(),
        dimension: 128,
        distance_metric: ProtoDistanceMetric::Euclidean as i32,
        storage_engine: StorageEngine::Helix as i32,
        ..Default::default()
    };

    let collection = Collection {
        id: "test_collection".to_string(),
        config: Some(collection_config.clone()),
        stats: Some(CollectionStats {
            vector_count: 0,
            index_size_bytes: 0,
            data_size_bytes: 0,
        }),
        storage_assignment: Some(StorageAssignment {
            primary_path: "/tmp/proximadb-data/helix".to_string(),
            backup_paths: vec![],
            engine: StorageEngine::Helix as i32,
            engine_config: HashMap::new(),
            base_location: "/tmp/proximadb-data".to_string(),
            assigned_at: 0,
        }),
        ..Default::default()
    };

    // Flush some vectors
    let records = create_test_records(50, 128);
    let params = FlushParameters {
        collection_id: Some("test_collection".to_string()),
        vector_records: records,
        force: false,
        synchronous: true,
        collection_config: Some(collection.clone()),
        ..Default::default()
    };

    engine.do_flush(&params).await.unwrap();

    // Search for nearest neighbors
    let query_vector = vec![0.5; 128];

    let collection = Arc::new(Collection {
        id: "test_collection".to_string(),
        config: Some(collection_config),
        ..Default::default()
    });

    let mut search_params = SearchParams::single_vector(query_vector);
    search_params.top_k = Some(5);
    search_params.distance_metric = Some(DistanceMetric::Euclidean);

    let metadata = StorageQueryMetadata::default();

    let ctx = StorageQueryContext {
        search_params: Arc::new(search_params),
        collection,
        metadata,
    };

    let results = engine.search_vectors_unified(&ctx).await.unwrap();

    assert!(!results.is_empty());
    assert!(results.len() <= 5);
}

#[tokio::test]
async fn test_vector_by_id() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let temp_dir = tempdir().unwrap();
    let path = temp_dir.path().to_str().unwrap().to_string();

    // Create filesystem factory with proper config
    let mut fs_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
    fs_config.default_fs = Some(format!("file://{}", path));
    let filesystem_factory = Arc::new(
        crate::storage::persistence::filesystem::FilesystemFactory::new(fs_config)
            .await
            .unwrap()
    );
    let distance_compute = Arc::new(
        crate::compute::distance_computation::engine::UnifiedDistanceCompute::default()
    );

    let config = HelixConfig::default();
    let engine = HelixEngine::new_with_config(config, filesystem_factory, distance_compute)
        .await
        .unwrap();

    // Create collection
    let collection_config = CollectionConfig {
        name: "test_collection".to_string(),
        dimension: 128,
        distance_metric: ProtoDistanceMetric::Euclidean as i32,
        storage_engine: StorageEngine::Helix as i32,
        ..Default::default()
    };

    let collection = Collection {
        id: "test_collection".to_string(),
        config: Some(collection_config),
        stats: Some(CollectionStats {
            vector_count: 0,
            index_size_bytes: 0,
            data_size_bytes: 0,
        }),
        storage_assignment: Some(StorageAssignment {
            primary_path: format!("{}/helix", path),
            backup_paths: vec![],
            engine: StorageEngine::Helix as i32,
            engine_config: HashMap::new(),
            base_location: path.clone(),
            assigned_at: 0,
        }),
        ..Default::default()
    };

    // Flush some vectors - use 1100+ to ensure PCA training happens (min is 1000)
    let records = create_test_records(1100, 128);
    let params = FlushParameters {
        collection_id: Some("test_collection".to_string()),
        vector_records: records,
        force: true,  // Force flush to ensure data is written
        synchronous: true,
        collection_config: Some(collection.clone()),
        ..Default::default()
    };

    let flush_result = engine.do_flush(&params).await.unwrap();
    assert!(flush_result.success, "Flush should succeed");

    // Find specific vector - use same base_location as in flush
    let result = engine
        .vector_by_id("test_collection", &path, "vec_5")
        .await
        .expect("Failed to search for vector by ID");

    assert!(result.is_some(), "Vector vec_5 should be found");
    assert_eq!(result.unwrap().id, "vec_5");
}

#[tokio::test]
async fn test_compaction() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().to_str().unwrap().to_string();

    // Create filesystem factory with proper config (like SST)
    let mut fs_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
    fs_config.default_fs = Some(format!("file://{}", path));
    let filesystem_factory = Arc::new(
        crate::storage::persistence::filesystem::FilesystemFactory::new(fs_config)
            .await
            .unwrap()
    );
    let distance_compute = Arc::new(
        crate::compute::distance_computation::engine::UnifiedDistanceCompute::default()
    );

    let mut config = HelixConfig::default();
    config.level0_file_num_compaction_trigger = 2;

    let engine = HelixEngine::new_with_config(config, filesystem_factory, distance_compute)
        .await
        .unwrap();

    // Create collection for compaction test
    let collection_config = CollectionConfig {
        name: "test_collection".to_string(),
        dimension: 128,
        distance_metric: ProtoDistanceMetric::Euclidean as i32,
        storage_engine: StorageEngine::Helix as i32,
        ..Default::default()
    };

    let collection = Collection {
        id: "test_collection".to_string(),
        config: Some(collection_config),
        stats: Some(CollectionStats {
            vector_count: 0,
            index_size_bytes: 0,
            data_size_bytes: 0,
        }),
        storage_assignment: Some(StorageAssignment {
            primary_path: "/tmp/proximadb-data/helix".to_string(),
            backup_paths: vec![],
            engine: StorageEngine::Helix as i32,
            engine_config: HashMap::new(),
            base_location: "/tmp/proximadb-data".to_string(),
            assigned_at: 0,
        }),
        ..Default::default()
    };

    // Flush multiple L0 files to trigger compaction
    for i in 0..3 {
        let records = create_test_records(50, 128);
        let params = FlushParameters {
            collection_id: Some("test_collection".to_string()),
            vector_records: records,
            collection_config: Some(collection.clone()),
            force: true,  // Force flush to ensure files are created
            synchronous: true,  // Wait for completion
            hints: HashMap::new(),
            timeout_ms: Some(5000),
            trigger_compaction: false,  // Don't trigger compaction yet
            batch_ids: vec![],
            estimated_size: 50 * 128 * 4,  // 50 vectors * 128 dims * 4 bytes
        };

        let result = engine.do_flush(&params).await.unwrap();
        println!("Flush {} completed: {:?}", i, result);
    }

    // Wait a bit for background compaction
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Trigger manual compaction
    let compact_params = CompactionParameters {
        collection_id: Some("test_collection".to_string()),
        collection_config: Some(collection),
        force: true,  // Force compaction
        synchronous: true,  // Wait for completion
        hints: HashMap::new(),
        timeout_ms: Some(5000),
        priority: OperationPriority::Medium,
        estimated_input_size: 3 * 50 * 128 * 4,  // 3 flushes * 50 vectors * 128 dims * 4 bytes
    };

    let result = engine.do_compact(&compact_params).await.unwrap();

    assert!(result.input_files.unwrap_or(0) > 0);
    assert!(result.bytes_written.unwrap_or(0) > 0);
}

#[tokio::test]
async fn test_pca_model_training() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let records = create_test_records(100, 128);
    let model = clustering::PCAModel::train(&records, 16).unwrap();

    assert_eq!(model.n_components, 16);
    assert_eq!(model.original_dim, 128);

    // Test projection
    let projected = model.project(&records[0].vector).unwrap();
    assert_eq!(projected.len(), 16);
}

#[tokio::test]
async fn test_hilbert_key_computation() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    // Use vectors with different patterns (not uniform values)
    // Uniform vectors like [0,0,0] or [1,1,1] normalize to [0.5,0.5,0.5] and produce same key
    let vector1 = vec![0.0, 0.5, 1.0];
    let vector2 = vec![1.0, 0.5, 0.0];
    let vector3 = vec![0.25, 0.75, 0.5];

    let key1 = clustering::compute_hilbert_key(&vector1);
    let key2 = clustering::compute_hilbert_key(&vector2);
    let key3 = clustering::compute_hilbert_key(&vector3);

    // Different vectors should have different keys
    assert_ne!(key1, key2, "key1 {} should differ from key2 {}", key1, key2);
    assert_ne!(key2, key3, "key2 {} should differ from key3 {}", key2, key3);
    assert_ne!(key1, key3, "key1 {} should differ from key3 {}", key1, key3);
}

#[tokio::test]
async fn test_liquid_clustering() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let mut tracker = clustering::QueryPatternTracker::default();

    // Record some access patterns
    tracker.record_access("vec_1", 100);
    tracker.record_access("vec_1", 100);
    tracker.record_access("vec_2", 200);
    tracker.record_access("vec_1", 100);

    assert_eq!(tracker.access_counts["vec_1"], 3);
    assert_eq!(tracker.access_counts["vec_2"], 1);

    // Get clustering hints
    let config = clustering::LiquidClusteringConfig::default();
    let hints = tracker.get_clustering_hints(&["vec_1".to_string(), "vec_2".to_string()], &config);

    // vec_1 should have higher score due to more accesses
    assert!(hints["vec_1"] > hints["vec_2"]);
}

#[tokio::test]
async fn test_proxima_integration() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let temp_dir = tempdir().unwrap();
    let temp_path = temp_dir.path().to_str().unwrap().to_string();

    // Create filesystem factory with proper config
    let mut fs_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
    fs_config.default_fs = Some(format!("file://{}", temp_path));
    let factory = Arc::new(crate::storage::persistence::filesystem::FilesystemFactory::new(fs_config).await.unwrap());
    let filesystem = factory.get_filesystem(&format!("file://{}", temp_path)).unwrap();

    let records = create_test_records(100, 128);
    let path = temp_dir.path().join("test.helix");

    // Write SSTable
    let bytes_written = proxima::write_helix_sstable(
        &filesystem,
        &path,
        &records,
        50, // block size
        crate::storage::engines::constants::HELIX_MAGIC,
        None,
    )
    .await
    .unwrap();

    assert!(bytes_written > 0);

    // Search SSTable
    let query = vec![0.5; 128];
    let results = proxima::search_helix_sstable(
        &filesystem,
        &path,
        &query,
        None,
        5,
        &DistanceMetric::Euclidean,
    )
    .await
    .unwrap();

    assert!(!results.is_empty());
    assert!(results.len() <= 5);
}

#[tokio::test]
async fn test_metrics_collection() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let temp_dir = tempdir().unwrap();
    let path = temp_dir.path().to_str().unwrap().to_string();

    // Create filesystem factory with proper config
    let mut fs_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
    fs_config.default_fs = Some(format!("file://{}", path));
    let filesystem_factory = Arc::new(
        crate::storage::persistence::filesystem::FilesystemFactory::new(fs_config)
            .await
            .unwrap()
    );
    let distance_compute = Arc::new(
        crate::compute::distance_computation::engine::UnifiedDistanceCompute::default()
    );

    let config = HelixConfig::default();
    let engine = HelixEngine::new_with_config(config, filesystem_factory, distance_compute)
        .await
        .unwrap();

    // Perform some operations
    let records = create_test_records(50, 128);

    // Create collection config
    let collection_config = CollectionConfig {
        name: "test_collection".to_string(),
        dimension: 128,
        distance_metric: ProtoDistanceMetric::Euclidean as i32,
        storage_engine: StorageEngine::Helix as i32,
        ..Default::default()
    };

    let collection = Collection {
        id: "test_collection".to_string(),
        config: Some(collection_config),
        stats: Some(CollectionStats {
            vector_count: 0,
            index_size_bytes: 0,
            data_size_bytes: 0,
        }),
        storage_assignment: Some(StorageAssignment {
            primary_path: "/tmp/proximadb-data/helix".to_string(),
            backup_paths: vec![],
            engine: StorageEngine::Helix as i32,
            engine_config: HashMap::new(),
            base_location: "/tmp/proximadb-data".to_string(),
            assigned_at: 0,
        }),
        ..Default::default()
    };

    let params = FlushParameters {
        collection_id: Some("test_collection".to_string()),
        vector_records: records,
        collection_config: Some(collection),
        ..Default::default()
    };

    engine.do_flush(&params).await.unwrap();

    // Collect metrics
    let metrics = engine.collect_engine_metrics().await.unwrap();

    assert!(metrics.contains_key("total_vectors"));
    assert!(metrics.contains_key("total_sstables"));
    assert!(metrics.contains_key("total_size_bytes"));
}

#[cfg(test)]
mod clustering_tests {
    use super::*;

    #[test]
    fn test_hilbert_2d_ordering() {
        use crate::storage::engines::impls::helix::hilbert_curve::HilbertCurve;
        let curve = HilbertCurve::new(2, 16);
        let key00 = curve.encode(&[0, 0]);
        let key01 = curve.encode(&[0, u32::MAX >> 16]);
        let key10 = curve.encode(&[u32::MAX >> 16, 0]);
        let key11 = curve.encode(&[u32::MAX >> 16, u32::MAX >> 16]);

        // Basic ordering test
        assert!(key00 < key11);
    }

    #[test]
    fn test_sort_by_hilbert() {
        let mut records = create_test_records(10, 3);
        let keys: Vec<u64> = (0..10).rev().map(|i| i as u64).collect();

        clustering::sort_by_hilbert(&mut records, &keys).unwrap();

        // Records should be reordered based on keys
        assert_eq!(records[0].id, "vec_9");
        assert_eq!(records[9].id, "vec_0");
    }
}
