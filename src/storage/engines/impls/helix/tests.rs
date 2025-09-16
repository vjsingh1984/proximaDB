//! Comprehensive test suite for HELIX engine

use super::*;
use crate::compute::distance_computation::DistanceMetric;
use crate::core::VectorRecord;
use crate::core::search::SearchParams;
use crate::proto::proximadb_v1::{
    Collection, CollectionConfig, DistanceMetric as ProtoDistanceMetric, StorageEngine,
};
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::traits::{
    CompactionParameters, FlushParameters, StorageQueryContext, StorageQueryMetadata,
};
use std::collections::HashMap;
use std::path::PathBuf;
use tempfile::TempDir;

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
            quantized_vector: vec![],
            source: None,
            updated_at: None,
            version: Some(1),
        })
        .collect()
}

#[tokio::test]
async fn test_helix_engine_creation() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let temp_dir = TempDir::new().unwrap();
    let config = HelixConfig::default();

    let engine = HelixEngine::new(
        "test_collection".to_string(),
        config,
        temp_dir.path().to_path_buf(),
        None,
    )
    .await
    .unwrap();

    assert_eq!(engine.engine_name(), "helix");
    assert_eq!(engine.engine_version(), "1.0.0");
}

#[tokio::test]
async fn test_flush_operation() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let temp_dir = TempDir::new().unwrap();
    let config = HelixConfig::default();

    let engine = HelixEngine::new(
        "test_collection".to_string(),
        config,
        temp_dir.path().to_path_buf(),
        None,
    )
    .await
    .unwrap();

    let records = create_test_records(100, 128);

    let params = FlushParameters {
        collection_id: Some("test_collection".to_string()),
        force: false,
        synchronous: true,
        hints: HashMap::new(),
        timeout_ms: None,
        vector_records: records,
        trigger_compaction: false,
        batch_ids: vec![],
        collection_config: None,
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

    let temp_dir = TempDir::new().unwrap();
    let config = HelixConfig::default();

    let engine = HelixEngine::new(
        "test_collection".to_string(),
        config,
        temp_dir.path().to_path_buf(),
        None,
    )
    .await
    .unwrap();

    // Flush some vectors
    let records = create_test_records(50, 128);
    let params = FlushParameters {
        collection_id: Some("test_collection".to_string()),
        vector_records: records,
        force: false,
        synchronous: true,
        collection_config: None,
        ..Default::default()
    };

    engine.do_flush(&params).await.unwrap();

    // Search for nearest neighbors
    let query_vector = vec![0.5; 128];

    let collection_config = CollectionConfig {
        name: "test_collection".to_string(),
        dimension: 128,
        distance_metric: ProtoDistanceMetric::Euclidean as i32,
        storage_engine: StorageEngine::Helix as i32,
        ..Default::default()
    };

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

    let temp_dir = TempDir::new().unwrap();
    let config = HelixConfig::default();

    let engine = HelixEngine::new(
        "test_collection".to_string(),
        config,
        temp_dir.path().to_path_buf(),
        None,
    )
    .await
    .unwrap();

    // Flush some vectors
    let records = create_test_records(10, 128);
    let params = FlushParameters {
        collection_id: Some("test_collection".to_string()),
        vector_records: records,
        force: false,
        synchronous: true,
        collection_config: None,
        ..Default::default()
    };

    engine.do_flush(&params).await.unwrap();

    // Find specific vector
    let result = engine
        .vector_by_id("test_collection", "vec_5")
        .await
        .unwrap();

    assert!(result.is_some());
    assert_eq!(result.unwrap().id, "vec_5");
}

#[tokio::test]
async fn test_compaction() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let temp_dir = TempDir::new().unwrap();
    let mut config = HelixConfig::default();
    config.level0_file_num_compaction_trigger = 2;

    let engine = HelixEngine::new(
        "test_collection".to_string(),
        config,
        temp_dir.path().to_path_buf(),
        None,
    )
    .await
    .unwrap();

    // Flush multiple L0 files to trigger compaction
    for i in 0..3 {
        let records = create_test_records(50, 128);
        let params = FlushParameters {
            collection_id: Some("test_collection".to_string()),
            vector_records: records,
            collection_config: None,
            ..Default::default()
        };

        engine.do_flush(&params).await.unwrap();
    }

    // Wait a bit for background compaction
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Trigger manual compaction
    let compact_params = CompactionParameters {
        collection_id: Some("test_collection".to_string()),
        level: Some(0),
        collection_config: None,
    };

    let result = engine.do_compact(&compact_params).await.unwrap();

    assert!(result.files_compacted > 0);
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

    let vector1 = vec![0.0, 0.0, 0.0];
    let vector2 = vec![1.0, 1.0, 1.0];

    let key1 = clustering::compute_hilbert_key(&vector1);
    let key2 = clustering::compute_hilbert_key(&vector2);

    // Different vectors should have different keys
    assert_ne!(key1, key2);
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
async fn test_fastlanes_integration() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let temp_dir = TempDir::new().unwrap();
    let filesystem = FilesystemFactory::create_local().unwrap();

    let records = create_test_records(100, 128);
    let path = temp_dir.path().join("test.helix");

    // Write SSTable
    let bytes_written = fastlane::write_helix_sstable(
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
    let results = fastlane::search_helix_sstable(
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

    let temp_dir = TempDir::new().unwrap();
    let config = HelixConfig::default();

    let engine = HelixEngine::new(
        "test_collection".to_string(),
        config,
        temp_dir.path().to_path_buf(),
        None,
    )
    .await
    .unwrap();

    // Perform some operations
    let records = create_test_records(50, 128);
    let params = FlushParameters {
        collection_id: Some("test_collection".to_string()),
        records,
        collection_config: None,
        level: None,
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
