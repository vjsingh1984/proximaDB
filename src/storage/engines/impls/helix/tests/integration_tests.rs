//! Comprehensive integration tests for HELIX engine
//!
//! Tests all major features including PCA, Hilbert clustering,
//! liquid clustering, progressive search, and zone maps.

use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;
use tokio;

use crate::compute::distance_computation::engine::{DistanceMetric, UnifiedDistanceCompute};
use crate::compute::quantization::storage_engine::StorageQuantizationEngine;
use crate::core::VectorRecord;
use crate::proto::proximadb_v1::Collection;
use crate::storage::engines::impls::helix::*;
use crate::storage::persistence::filesystem::{FileSystem, FilesystemFactory};
use crate::storage::traits::{FlushParameters, StorageQueryContext, UnifiedStorageEngine};

/// Create test vector records with known patterns
fn create_test_vectors(count: usize, dimensions: usize) -> Vec<VectorRecord> {
    let mut records = Vec::new();

    for i in 0..count {
        // Create vectors with patterns for clustering
        let mut vector = Vec::with_capacity(dimensions);
        for d in 0..dimensions {
            // Create clusters in the data
            let cluster = i / 100; // Every 100 vectors form a cluster
            let base = cluster as f32 * 10.0;
            let noise = (i as f32 * 0.1).sin() * 0.5;
            vector.push(base + (d as f32) + noise);
        }

        records.push(VectorRecord {
            id: format!("vec_{:06}", i),
            vector,
            metadata: None,
            timestamp: i as i64,
            expires_at: None,
        });
    }

    records
}

/// Test basic HELIX engine initialization
#[tokio::test]
async fn test_helix_engine_initialization() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let temp_dir = TempDir::new().unwrap();
    let config = HelixConfig::default();
    let filesystem_factory = Arc::new(FilesystemFactory::new());
    let filesystem = filesystem_factory.get_filesystem("file://").unwrap();

    let engine = HelixEngine::new(
        config,
        "test_collection".to_string(),
        temp_dir.path().to_path_buf(),
        filesystem_factory,
        filesystem,
        None, // No EventLog for testing
    );

    assert_eq!(engine.engine_name(), "helix");
    assert_eq!(engine.engine_version(), "1.0.0");
}

/// Test PCA model training and projection
#[tokio::test]
async fn test_pca_model_training() {
    use crate::storage::engines::impls::helix::pca_impl::EnhancedPCAModel;

    let vectors = create_test_vectors(1000, 128);

    // Train PCA model
    let model = EnhancedPCAModel::train(&vectors, 16).unwrap();

    assert_eq!(model.n_components, 16);
    assert_eq!(model.original_dim, 128);

    // Test projection
    let test_vector = &vectors[0].vector;
    let projected = model.project(test_vector).unwrap();
    assert_eq!(projected.len(), 16);

    // Test reconstruction error
    let error = model.reconstruction_error(test_vector).unwrap();
    assert!(error >= 0.0);

    // Test variance explained
    let variance_ratio = model.explained_variance_ratio();
    assert_eq!(variance_ratio.len(), 16);
    assert!(variance_ratio[0] >= variance_ratio[1]); // First component explains most
}

/// Test Hilbert curve encoding
#[tokio::test]
async fn test_hilbert_curve_encoding() {
    use crate::storage::engines::impls::helix::hilbert_curve::{HilbertCurve, HilbertUtils};

    // Test 2D Hilbert curve
    let curve_2d = HilbertCurve::new(2, 8);
    let key1 = curve_2d.encode(&[0, 0]);
    let key2 = curve_2d.encode(&[255, 255]);
    assert!(key1 < key2);

    // Test 3D Hilbert curve
    let curve_3d = HilbertCurve::new(3, 8);
    let key3 = curve_3d.encode(&[128, 128, 128]);
    assert!(key3 > 0);

    // Test vector to Hilbert key conversion
    let vector = vec![0.1, 0.5, 0.9, 0.3];
    let hilbert_key = HilbertUtils::vector_to_hilbert_key(&vector, 16);
    assert!(hilbert_key > 0);

    // Test locality preservation
    let nearby_vector = vec![0.11, 0.51, 0.89, 0.31];
    let nearby_key = HilbertUtils::vector_to_hilbert_key(&nearby_vector, 16);
    let distance = HilbertUtils::hilbert_distance(hilbert_key, nearby_key);

    let far_vector = vec![0.9, 0.1, 0.2, 0.8];
    let far_key = HilbertUtils::vector_to_hilbert_key(&far_vector, 16);
    let far_distance = HilbertUtils::hilbert_distance(hilbert_key, far_key);

    // Nearby vectors should have smaller Hilbert distance
    assert!(distance < far_distance);
}

/// Test flush and compaction with clustering
#[tokio::test]
async fn test_flush_and_compaction() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let temp_dir = TempDir::new().unwrap();
    let mut config = HelixConfig::default();
    config.level0_file_num_compaction_trigger = 2; // Trigger compaction after 2 files

    let filesystem_factory = Arc::new(FilesystemFactory::new());
    let filesystem = filesystem_factory.get_filesystem("file://").unwrap();

    let engine = HelixEngine::new(
        config,
        "test_collection".to_string(),
        temp_dir.path().to_path_buf(),
        filesystem_factory,
        filesystem,
        None,
    );

    // Create and flush test vectors
    let vectors = create_test_vectors(500, 64);

    // Flush batch 1
    let flush_params1 = FlushParameters {
        collection_id: Some("test_collection".to_string()),
        vector_records: vectors[..250].to_vec(),
        collection_config: None,
        force: false,
    };

    let result1 = engine.do_flush(&flush_params1).await.unwrap();
    assert!(result1.success);
    assert_eq!(result1.entries_flushed, Some(250));

    // Flush batch 2 (should trigger compaction)
    let flush_params2 = FlushParameters {
        collection_id: Some("test_collection".to_string()),
        vector_records: vectors[250..].to_vec(),
        collection_config: None,
        force: false,
    };

    let result2 = engine.do_flush(&flush_params2).await.unwrap();
    assert!(result2.success);
    assert_eq!(result2.entries_flushed, Some(250));

    // Wait a bit for background compaction
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Verify metrics
    let metrics = engine.get_metrics().await;
    assert!(metrics.total_vectors > 0);
    assert!(metrics.total_sstables > 0);
}

/// Test liquid clustering with query patterns
#[tokio::test]
async fn test_liquid_clustering() {
    use crate::storage::engines::impls::helix::clustering::QueryPatternTracker;
    use crate::storage::engines::impls::helix::liquid_clustering::LiquidClusteringCoordinator;

    let config = LiquidClusteringConfig::default();
    let query_tracker = Arc::new(RwLock::new(QueryPatternTracker::default()));

    // Simulate query patterns
    {
        let mut tracker = query_tracker.write().await;
        // Hot vectors (frequently accessed)
        for _ in 0..10 {
            tracker.record_access("vec_000001", 100);
            tracker.record_access("vec_000002", 101);
            tracker.record_access("vec_000003", 102);
        }
        // Cold vectors (rarely accessed)
        tracker.record_access("vec_000100", 200);
        tracker.record_access("vec_000200", 300);
    }

    let coordinator = LiquidClusteringCoordinator::new(config, query_tracker);

    // Create test vectors
    let vectors = create_test_vectors(300, 32);
    let hilbert_keys: Vec<u64> = (0..300).map(|i| i as u64 * 100).collect();

    // Apply liquid clustering
    let (reorganized, new_keys) = coordinator
        .apply_liquid_clustering(vectors.clone(), &hilbert_keys)
        .await
        .unwrap();

    assert_eq!(reorganized.len(), vectors.len());
    assert_eq!(new_keys.len(), hilbert_keys.len());

    // Check that hot vectors are prioritized (should be near the beginning)
    let hot_positions: Vec<usize> = reorganized
        .iter()
        .enumerate()
        .filter(|(_, r)| r.id == "vec_000001" || r.id == "vec_000002" || r.id == "vec_000003")
        .map(|(i, _)| i)
        .collect();

    // Hot vectors should be in the first third of the reorganized list
    for pos in hot_positions {
        assert!(
            pos < 100,
            "Hot vector at position {} should be near beginning",
            pos
        );
    }
}

/// Test progressive search with quantization
#[tokio::test]
async fn test_progressive_search() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    use crate::storage::engines::impls::helix::progressive_search::ProgressiveSearchCoordinator;

    let config = HelixConfig::default();
    let distance_compute = Arc::new(UnifiedDistanceCompute::default());

    // Create mock quantization engine
    let codebook_store =
        Arc::new(crate::compute::quantization::unified::InMemoryCodebookStore::new());
    let unified_engine = Arc::new(
        crate::compute::quantization::unified::UnifiedQuantizationEngine::new(
            distance_compute.clone(),
            codebook_store,
        ),
    );
    let storage_config = crate::compute::quantization::storage_engine::StorageQuantizationConfig {
        primary_level: Some(
            crate::compute::quantization::unified::UnifiedQuantizationLevel::pq8(32),
        ),
        filter_level: Some(
            crate::compute::quantization::unified::UnifiedQuantizationLevel::binary(),
        ),
        fast_level: Some(crate::compute::quantization::unified::UnifiedQuantizationLevel::int8()),
        distance_metric: DistanceMetric::Euclidean,
        enable_progressive: true,
        filter_threshold: 100.0,
        candidate_multiplier: 10,
        training_sample_size: 100,
        memory_budget_mb: 256,
        enable_hardware_acceleration: true,
    };
    let quantization_engine = Some(Arc::new(StorageQuantizationEngine::new(
        unified_engine,
        distance_compute.clone(),
        storage_config,
    )));

    let coordinator =
        ProgressiveSearchCoordinator::new(config, distance_compute, quantization_engine);

    // Create test SSTables metadata
    let sstables = vec![
        SStableMetadata {
            path: PathBuf::from("test1.helix"),
            level: 1,
            hilbert_range: Some((0, 1000)),
            num_vectors: 100,
            size_bytes: 10240,
            created_at: chrono::Utc::now(),
            blocks: vec![],
            bloom_filter: None,
        },
        SStableMetadata {
            path: PathBuf::from("test2.helix"),
            level: 1,
            hilbert_range: Some((2000, 3000)),
            num_vectors: 100,
            size_bytes: 10240,
            created_at: chrono::Utc::now(),
            blocks: vec![],
            bloom_filter: None,
        },
        SStableMetadata {
            path: PathBuf::from("test3.helix"),
            level: 1,
            hilbert_range: Some((5000, 6000)),
            num_vectors: 100,
            size_bytes: 10240,
            created_at: chrono::Utc::now(),
            blocks: vec![],
            bloom_filter: None,
        },
    ];

    // Test progressive search with Hilbert pruning
    let query_vector = vec![1.0; 128];
    let query_hilbert = Some(500u64); // Close to first SSTable

    let temp_dir = TempDir::new().unwrap();
    let filesystem_factory = Arc::new(FilesystemFactory::new());
    let filesystem = filesystem_factory.get_filesystem("file://").unwrap();

    // Note: This would fail in real execution as files don't exist,
    // but we're testing the pruning logic
    let pruned_count = coordinator
        .prune_by_hilbert_range(&sstables, query_hilbert)
        .len();

    // Should prune to only nearby SSTables
    assert!(pruned_count < sstables.len());
    assert_eq!(pruned_count, 1); // Only first SSTable should be selected
}

/// Test zone maps for dimension-level pruning
#[tokio::test]
async fn test_zone_maps() {
    use crate::storage::engines::impls::helix::zone_maps::{ZoneMap, ZoneMapBuilder, ZoneMapIndex};

    // Create test vectors with known patterns
    let vectors = create_test_vectors(500, 32);

    // Build zone maps
    let mut builder = ZoneMapBuilder::new(100); // 100 vectors per block

    for vector in &vectors {
        builder.add_vector(vector.clone()).unwrap();
    }

    let index = builder.build().unwrap();

    // Should have 5 zone maps (500 vectors / 100 per block)
    assert_eq!(index.maps.len(), 5);
    assert_eq!(index.total_vectors, 500);

    // Test pruning with query vector
    let query_vector = vec![5.0; 32]; // Middle range query
    let selected_blocks = index.prune_blocks(&query_vector, 10);

    // Should select some but not all blocks
    assert!(!selected_blocks.is_empty());
    assert!(selected_blocks.len() < 5);

    // Test selectivity estimation
    let selectivity = index.estimate_query_selectivity(&query_vector, 10.0);
    assert!(selectivity >= 0.0 && selectivity <= 1.0);

    // Test dimension statistics
    let dim_stats = index.get_dimension_stats();
    assert_eq!(dim_stats.dimensions, 32);
    assert_eq!(dim_stats.range_per_dim.len(), 32);

    // Test individual zone map
    let zone_map = &index.maps[&0];
    let pruning_score = zone_map.pruning_score(&query_vector, 10.0);
    assert!(pruning_score >= 0.0);
}

/// Test end-to-end search with all optimizations
#[tokio::test]
async fn test_end_to_end_search() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let temp_dir = TempDir::new().unwrap();
    let config = HelixConfig::default();

    let filesystem_factory = Arc::new(FilesystemFactory::new());
    let filesystem = filesystem_factory.get_filesystem("file://").unwrap();

    let engine = HelixEngine::new(
        config,
        "test_collection".to_string(),
        temp_dir.path().to_path_buf(),
        filesystem_factory,
        filesystem,
        None,
    );

    // Flush test vectors
    let vectors = create_test_vectors(1000, 64);
    let flush_params = FlushParameters {
        collection_id: Some("test_collection".to_string()),
        vector_records: vectors.clone(),
        collection_config: None,
        force: false,
    };

    let flush_result = engine.do_flush(&flush_params).await.unwrap();
    assert!(flush_result.success);

    // Create search context
    let query_vector = vectors[50].vector.clone(); // Search for a known vector
    let search_context = StorageQueryContext::new(
        query_vector.clone(),
        10, // top-k
        DistanceMetric::Euclidean,
        None,           // No filter
        HashMap::new(), // No hints
    );

    // Execute search
    let results = engine
        .search_vectors_unified(&search_context)
        .await
        .unwrap();

    // Should find results
    assert!(!results.is_empty());
    assert!(results.len() <= 10);

    // The exact vector should be the top result (or very close)
    if !results.is_empty() {
        assert!(results[0].score > 0.9); // High similarity score
    }
}

/// Test configuration wiring
#[test]
fn test_configuration() {
    let mut config = HelixConfig::default();

    // Test defaults
    assert_eq!(config.pca_dimensions, 16);
    assert_eq!(config.hilbert_bits_per_dimension, 16);
    assert_eq!(config.fastlane_block_size, 128);
    assert!(config.enable_liquid_clustering);

    // Test configuration changes
    config.pca_dimensions = 32;
    config.hilbert_bits_per_dimension = 20;
    config.enable_liquid_clustering = false;

    assert_eq!(config.pca_dimensions, 32);
    assert_eq!(config.hilbert_bits_per_dimension, 20);
    assert!(!config.enable_liquid_clustering);
}

/// Benchmark PCA performance
#[tokio::test]
#[ignore] // Run with --ignored flag for benchmarks
async fn bench_pca_performance() {
    use crate::storage::engines::impls::helix::pca_impl::EnhancedPCAModel;
    use std::time::Instant;

    println!("\n=== PCA Performance Benchmark ===");

    for size in [100, 500, 1000, 5000] {
        let vectors = create_test_vectors(size, 256);

        let start = Instant::now();
        let model = EnhancedPCAModel::train(&vectors, 32).unwrap();
        let train_time = start.elapsed();

        // Benchmark projection
        let start = Instant::now();
        for _ in 0..100 {
            let _ = model.project(&vectors[0].vector);
        }
        let project_time = start.elapsed() / 100;

        println!(
            "Vectors: {}, Train: {:?}, Project: {:?}/op",
            size, train_time, project_time
        );
    }
}

/// Benchmark Hilbert curve encoding
#[tokio::test]
#[ignore] // Run with --ignored flag for benchmarks
async fn bench_hilbert_encoding() {
    use crate::storage::engines::impls::helix::hilbert_curve::HilbertUtils;
    use std::time::Instant;

    println!("\n=== Hilbert Encoding Benchmark ===");

    for dims in [2, 4, 8, 16, 32] {
        let vector = vec![0.5; dims];

        let start = Instant::now();
        let iterations = 10000;
        for _ in 0..iterations {
            let _ = HilbertUtils::vector_to_hilbert_key(&vector, 16);
        }
        let time_per_op = start.elapsed() / iterations;

        println!("Dimensions: {}, Time: {:?}/op", dims, time_per_op);
    }
}

/// Benchmark liquid clustering
#[tokio::test]
#[ignore] // Run with --ignored flag for benchmarks
async fn bench_liquid_clustering() {
    use crate::storage::engines::impls::helix::clustering::QueryPatternTracker;
    use crate::storage::engines::impls::helix::liquid_clustering::LiquidClusteringCoordinator;
    use std::time::Instant;

    println!("\n=== Liquid Clustering Benchmark ===");

    let config = LiquidClusteringConfig::default();
    let query_tracker = Arc::new(RwLock::new(QueryPatternTracker::default()));

    // Simulate access patterns
    {
        let mut tracker = query_tracker.write().await;
        for i in 0..1000 {
            let freq = if i < 100 { 10 } else { 1 };
            for _ in 0..freq {
                tracker.record_access(&format!("vec_{:06}", i), i as u64);
            }
        }
    }

    let coordinator = LiquidClusteringCoordinator::new(config, query_tracker);

    for size in [100, 500, 1000, 5000] {
        let vectors = create_test_vectors(size, 64);
        let hilbert_keys: Vec<u64> = (0..size).map(|i| i as u64 * 100).collect();

        let start = Instant::now();
        let (reorganized, _) = coordinator
            .apply_liquid_clustering(vectors, &hilbert_keys)
            .await
            .unwrap();
        let time = start.elapsed();

        println!("Vectors: {}, Time: {:?}", size, time);
    }
}
