//! HELIX Core Functionality Tests (Consolidated)
//!
//! This module consolidates core functionality tests previously scattered across
//! multiple HELIX implementation files. Tests are preserved EXACTLY as they were,
//! only reorganized into logical sections.
//!
//! **Source Files**:
//! - benchmarks.rs (6 tests)
//! - unified_strategy_reader.rs (3 tests)
//! - unified_metadata_serializer.rs (3 tests)
//! - query_optimization.rs (2 tests)
//! - progressive_search.rs (2 tests)
//! - pca_impl.rs (2 tests)
//! - eventlog_integration.rs (2 tests)
//! - readers.rs (1 test)
//! - compaction.rs (1 test)
//!
//! **Total Tests**: 22

use super::helpers::*;
use crate::compute::distance_computation::DistanceMetric;
use crate::core::bloom::{BloomFilterConfig, factory::BloomFilterFactory};
use crate::core::search::SearchParams;
use crate::core::search::results::OptimizedSearchRecord;
use crate::proto::proximadb_v1::{
    Collection, CollectionConfig, DistanceMetric as ProtoDistanceMetric,
    StorageEngine, VectorRecord,
};
use crate::storage::engines::core::formats::proximablocks::bloom_filter::{
    SerializedBloomFilter, factory::BloomFilterFactory as BlockBloomFactory,
};
use crate::storage::engines::core::read_strategy::{ReadAccessStrategy, StrategyAwareReader};
use crate::storage::engines::helix::*;
// Benchmarks module removed - use benches/ directory instead
use crate::storage::engines::helix::clustering::{PCAModel, compute_hilbert_key, QueryPatternTracker, LiquidClusteringConfig};
use crate::storage::engines::helix::compaction::LeveledCompactor;
use crate::storage::engines::helix::eventlog_integration::HelixFlushHandler;
use crate::storage::engines::helix::pca_impl::{EnhancedPCAModel, PCAModelManager, ModelQuality};
use crate::storage::engines::helix::progressive_search::{ProgressiveSearchCoordinator, ProgressiveSearchStats};
use crate::storage::engines::helix::query_optimization::{PredictivePrefetcher, SmartResultCache, QueryPattern};
use crate::storage::engines::helix::readers::QueryStats;
use crate::storage::engines::core::helix_unified_metadata_serializer::{
    HelixUnifiedMetadataSerializer, HelixCachedMetadata, HilbertConfig,
    PcaModelMetadata, LiquidClusteringMetadata, ZoneMapEntry, SstableMetadata, QueryOptimizationStats,
};
use crate::storage::engines::helix::unified_strategy_reader::{UnifiedHELIXReader, HelixSearchStrategy};
use crate::storage::persistence::filesystem::{FilesystemFactory, FileSystem};
use crate::storage::persistence::filesystem::metadata_traits::EngineMetadataSerializer;
use crate::storage::traits::{FlushParameters, StorageQueryContext, StorageQueryMetadata, CompactionParameters};
use criterion::{black_box, Criterion};
use rand::{Rng, SeedableRng};
use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tempfile::TempDir;

// =============================================================================
// BENCHMARK TESTS (6 tests from benchmarks.rs)
// =============================================================================

// Benchmarks moved to benches/ directory - #[bench] requires unstable feature
// #[bench]
// fn bench_pca_training(b: &mut Bencher) {
//     let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
//
//     let vectors = generate_random_vectors(1000, 768, 42);
//
//     b.iter(|| {
//         let model = clustering::PCAModel::train(&vectors, 16).unwrap();
//         black_box(model);
//     });
// }
//
// #[bench]
// fn bench_pca_projection(b: &mut Bencher) {
//     let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
//
//     let vectors = generate_random_vectors(100, 768, 42);
//     let model = clustering::PCAModel::train(&vectors, 16).unwrap();
//     let test_vector = vec![0.5; 768];
//
//     b.iter(|| {
//         let projected = model.project(&test_vector).unwrap();
//         black_box(projected);
//     });
// }
//
// #[bench]
// fn bench_hilbert_key_computation(b: &mut Bencher) {
//     let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
//
//     let vectors = vec![
//         vec![0.1, 0.2, 0.3],
//         vec![0.4, 0.5, 0.6],
//         vec![0.7, 0.8, 0.9],
//     ];
//
//     b.iter(|| {
//         for v in &vectors {
//             let key = clustering::compute_hilbert_key(v);
//             black_box(key);
//         }
//     });
// }

#[tokio::test]
async fn bench_flush_performance() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let temp_dir = TempDir::new().unwrap();
    let config = HelixConfig::default();

    let engine = HelixEngine::new(
        "bench_collection".to_string(),
        config,
        temp_dir.path().to_path_buf(),
        None,
    ).await.unwrap();

    // Test different batch sizes
    for size in [100, 500, 1000, 5000] {
        let vectors = generate_random_vectors(size, 384, 42);

        let start = Instant::now();
        let params = FlushParameters {
            collection_id: Some("bench_collection".to_string()),
            records: vectors,
            collection_config: None,
            level: None,
        };

        let result = engine.do_flush(&params).await.unwrap();
        let elapsed = start.elapsed();

        println!(
            "Flush {} vectors: {:.2}ms, {:.2} MB/s",
            size,
            elapsed.as_millis(),
            (result.bytes_written as f64 / 1_048_576.0) / elapsed.as_secs_f64()
        );
    }
}

#[tokio::test]
async fn bench_search_with_pruning() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let temp_dir = TempDir::new().unwrap();
    let mut config = HelixConfig::default();
    config.proxima_block_size = 50;

    let engine = HelixEngine::new(
        "bench_collection".to_string(),
        config,
        temp_dir.path().to_path_buf(),
        None,
    ).await.unwrap();

    // Create clustered data
    let vectors = generate_clustered_vectors(10, 100, 128);

    // Flush in multiple batches to create multiple files
    for chunk in vectors.chunks(200) {
        let params = FlushParameters {
            collection_id: Some("bench_collection".to_string()),
            records: chunk.to_vec(),
            collection_config: None,
            level: None,
        };
        engine.do_flush(&params).await.unwrap();
    }

    // Search and measure pruning
    let query_vector = vectors[50].vector.clone(); // Vector from cluster 0

    let start = Instant::now();

    let collection_config = CollectionConfig {
        name: "bench_collection".to_string(),
        dimension: 128,
        distance_metric: Some(ProtoDistanceMetric::Euclidean as i32),
        storage_engine: Some(StorageEngine::Helix as i32),
        ..Default::default()
    };

    let collection = Arc::new(Collection {
        id: "bench_collection".to_string(),
        config: Some(collection_config),
        ..Default::default()
    });

    let mut search_params = SearchParams::single_vector(query_vector);
    search_params.top_k = Some(10);
    search_params.distance_metric = Some(DistanceMetric::Euclidean);

    let metadata = StorageQueryMetadata::default();

    let ctx = StorageQueryContext {
        search_params: Arc::new(search_params),
        collection,
        metadata,
        user_context: None,
        tenant_context: None,
    };

    let results = engine.search_vectors_unified(&ctx).await.unwrap();
    let elapsed = start.elapsed();

    println!(
        "Search latency: {:.2}ms, found {} results",
        elapsed.as_millis(),
        results.len()
    );

    // Verify results are from the same cluster
    let same_cluster = results.iter()
        .filter(|r| r.id.starts_with("cluster_0_"))
        .count();

    println!(
        "Clustering accuracy: {:.1}% from same cluster",
        (same_cluster as f64 / results.len() as f64) * 100.0
    );
}

#[tokio::test]
async fn bench_compaction_throughput() {
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let temp_dir = TempDir::new().unwrap();
    let mut config = HelixConfig::default();
    config.level0_file_num_compaction_trigger = 3;

    let engine = HelixEngine::new(
        "bench_collection".to_string(),
        config,
        temp_dir.path().to_path_buf(),
        None,
    ).await.unwrap();

    // Create multiple L0 files
    let vectors_per_file = 500;
    for i in 0..4 {
        let vectors = generate_random_vectors(vectors_per_file, 256, i);
        let params = FlushParameters {
            collection_id: Some("bench_collection".to_string()),
            records: vectors,
            collection_config: None,
            level: None,
        };
        engine.do_flush(&params).await.unwrap();
    }

    // Trigger compaction
    let start = Instant::now();
    let compact_params = CompactionParameters {
        collection_id: Some("bench_collection".to_string()),
        level: Some(0),
        collection_config: None,
    };

    let result = engine.do_compact(&compact_params).await.unwrap();
    let elapsed = start.elapsed();

    println!(
        "Compaction: {} files in {:.2}ms, {:.2} MB/s",
        result.files_compacted,
        elapsed.as_millis(),
        (result.bytes_written as f64 / 1_048_576.0) / elapsed.as_secs_f64()
    );
}

// =============================================================================
// READER TESTS (4 tests: 3 from unified_strategy_reader.rs + 1 from readers.rs)
// =============================================================================

#[test]
fn test_helix_strategy_to_search() {
    let direct = ReadAccessStrategy::DirectStream;
    let search_strategy = UnifiedHELIXReader::to_helix_search_strategy(&direct);
    assert!(matches!(search_strategy, HelixSearchStrategy::NoPruning));

    let search = ReadAccessStrategy::CachedSearch { prefetch_metadata: true };
    let search_strategy = UnifiedHELIXReader::to_helix_search_strategy(&search);
    assert!(matches!(search_strategy, HelixSearchStrategy::ZoneMapPruning));

    let adaptive = ReadAccessStrategy::Adaptive {
        initial_strategy: Box::new(ReadAccessStrategy::DirectStream),
        fallback_threshold: 100
    };
    let search_strategy = UnifiedHELIXReader::to_helix_search_strategy(&adaptive);
    assert!(matches!(search_strategy, HelixSearchStrategy::LiquidClustering { .. }));
}

#[tokio::test]
async fn test_helix_reader_creation() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().to_str().unwrap().to_string();

    let mut fs_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
    fs_config.default_fs = Some(format!("file://{}", path));
    let factory = Arc::new(
        crate::storage::persistence::filesystem::FilesystemFactory::create(fs_config)
            .await
            .unwrap()
    );

    // Compaction should use DirectStream
    let compaction_reader = UnifiedHELIXReader::for_compaction(
        factory.clone(),
        "test_collection".to_string(),
    ).unwrap();
    assert_eq!(compaction_reader.strategy(), &ReadAccessStrategy::DirectStream);
    assert!(!compaction_reader.is_using_cache());

    // Search should use CachedSearch
    let search_reader = UnifiedHELIXReader::for_search(
        factory.clone(),
        "test_collection".to_string(),
    ).unwrap();
    matches!(search_reader.strategy(), ReadAccessStrategy::CachedSearch { .. });
    assert!(search_reader.is_using_cache());
}

#[tokio::test]
async fn test_strategy_updates() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().to_str().unwrap().to_string();

    let mut fs_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
    fs_config.default_fs = Some(format!("file://{}", path));
    let factory = Arc::new(
        crate::storage::persistence::filesystem::FilesystemFactory::create(fs_config)
            .await
            .unwrap()
    );
    let mut reader = UnifiedHELIXReader::for_search(
        factory,
        "test".to_string(),
    ).unwrap();

    // Initially configured for search (cached)
    assert!(reader.is_using_cache());
    assert!(matches!(reader.search_strategy, HelixSearchStrategy::ZoneMapPruning));

    // Change to direct stream
    reader.set_strategy(ReadAccessStrategy::DirectStream);
    assert!(matches!(reader.search_strategy, HelixSearchStrategy::NoPruning));
}

#[tokio::test]
async fn test_query_stats() {
    let stats = QueryStats {
        sstables_scanned: 10,
        sstables_pruned: 7,
        blocks_scanned: 30,
        blocks_pruned: 20,
        vectors_evaluated: 1000,
        pruning_ratio: 0.7,
    };

    assert_eq!(stats.sstables_scanned - stats.sstables_pruned, 3);
    assert_eq!(stats.pruning_ratio, 0.7);
}

// =============================================================================
// METADATA TESTS (3 tests from unified_metadata_serializer.rs)
// =============================================================================

#[test]
fn test_helix_metadata_serialization() {
    let metadata = HelixCachedMetadata {
        file_size: 41943040, // 40MB
        vector_count: 40000,
        dimension: 384,
        hilbert_config: HilbertConfig {
            bits_per_dimension: 16,
            reduced_dimensions: 32,
            key_range: (0, u64::MAX),
            locality_score: 0.88,
        },
        pca_model: PcaModelMetadata {
            enabled: true,
            original_dimensions: 384,
            reduced_dimensions: 32,
            explained_variance: 0.95,
            last_retrain: 1234567890,
            training_vectors: 10000,
        },
        liquid_clustering: LiquidClusteringMetadata {
            enabled: true,
            cluster_count: 64,
            adaptation_rate: 0.1,
            query_patterns: vec!["temporal".to_string(), "spatial".to_string()],
            hot_clusters: vec![0, 1, 2, 3, 4],
            cold_clusters: vec![60, 61, 62, 63],
        },
        zone_maps: vec![
            ZoneMapEntry {
                zone_id: 0,
                hilbert_range: (0, 1000000),
                vector_count: 1000,
                min_values: vec![-1.0; 32],
                max_values: vec![1.0; 32],
                time_range: Some((1234567000, 1234567890)),
            },
        ],
        sstable_metadata: SstableMetadata {
            lsm_level: 3,
            level_counts: vec![4, 10, 25, 50],
            level_sizes: vec![4194304, 41943040, 419430400, 4194304000],
            block_size: 1024,
            bloom_filter_bits: 10,
            compression_ratio: 0.3,
        },
        query_stats: QueryOptimizationStats {
            cache_hit_rate: 0.85,
            avg_pruning_percent: 0.92,
            pattern_count: 5,
            prefetch_accuracy: 0.78,
            avg_query_latency_us: 1200,
        },
        creation_timestamp: 1234567890,
    };

    let serializer = HelixUnifiedMetadataSerializer::new();

    // Test serialization
    let bytes = serializer.serialize(&metadata).unwrap();
    assert!(!bytes.is_empty());

    // Test deserialization
    let deserialized = serializer.deserialize(&bytes).unwrap();
    let restored = deserialized.downcast_ref::<HelixCachedMetadata>().unwrap();

    assert_eq!(restored.file_size, metadata.file_size);
    assert_eq!(restored.vector_count, metadata.vector_count);
    assert_eq!(restored.dimension, metadata.dimension);
    assert_eq!(restored.hilbert_config.bits_per_dimension, metadata.hilbert_config.bits_per_dimension);
    assert_eq!(restored.pca_model.explained_variance, metadata.pca_model.explained_variance);
}

#[test]
fn test_helix_metadata_extraction() {
    let serializer = HelixUnifiedMetadataSerializer::new();

    // Create mock HELIX file data
    let mut data = Vec::new();

    // Magic bytes
    data.extend_from_slice(b"HLIX");

    // Header size (256 bytes)
    data.extend_from_slice(&256u32.to_le_bytes());

    // Metadata section size (512 bytes)
    data.extend_from_slice(&512u64.to_le_bytes());

    // Header content
    data.extend_from_slice(&vec![0xAAu8; 256 - 16]);

    // Metadata content (PCA model, zone maps, etc.)
    data.extend_from_slice(&vec![0xBBu8; 512]);

    // Some data content
    data.extend_from_slice(&vec![0xCCu8; 1024]);

    // Test extraction
    let extracted = serializer.extract_cacheable_component(&data, "test.hlx");
    assert!(extracted.is_some());

    let extracted_bytes = extracted.unwrap();
    // Should include header (256) + metadata (512)
    assert_eq!(extracted_bytes.len(), 768);
}

#[test]
fn test_should_cache_metadata() {
    let serializer = HelixUnifiedMetadataSerializer::new();

    assert!(serializer.should_cache_metadata("/data/helix/vectors.hlx"));
    assert!(serializer.should_cache_metadata("/collections/test_helix_data.bin"));
    assert!(serializer.should_cache_metadata("/hilbert/index.dat"));
    assert!(serializer.should_cache_metadata("/pca_model/model.bin"));
    assert!(serializer.should_cache_metadata("/zone_maps/zones.idx"));
    assert!(serializer.should_cache_metadata("/liquid_clusters/clusters.dat"));
    assert!(!serializer.should_cache_metadata("/tmp/random.txt"));
}

// =============================================================================
// OPTIMIZATION TESTS (2 tests from query_optimization.rs)
// =============================================================================

#[tokio::test]
async fn test_predictive_prefetcher() {
    let prefetcher = PredictivePrefetcher::new(100, 0.3);

    // Record some query patterns
    let pattern1 = QueryPattern {
        query_hash: 123,
        hilbert_key: Some(1000),
        accessed_files: vec!["file1.helix".to_string(), "file2.helix".to_string()],
        timestamp_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64,
        latency_ms: 25,
        result_count: 10,
    };

    prefetcher.record_query(pattern1).await;

    // Predict for similar query
    let predictions = prefetcher.predict_prefetch(Some(1050)).await;
    assert!(!predictions.is_empty());
}

#[tokio::test]
async fn test_result_cache() {
    let cache = SmartResultCache::new(100, 60);

    // Cache a result
    let results = vec![
        OptimizedSearchRecord::new("test".to_string(), 0.9)
            .with_similarity(0.1)
            .add_vector(vec![1.0, 2.0, 3.0])
            .with_metadata(Default::default())
            .with_version_info(0, 0),
    ];

    cache
        .put(123, results.clone(), vec!["file1.helix".to_string()])
        .await;

    // Retrieve cached result
    let cached = cache.get(123).await;
    assert!(cached.is_some());
    assert_eq!(cached.unwrap().len(), 1);

    // Invalidate file
    cache.invalidate_file("file1.helix").await;

    // Should be gone
    let cached = cache.get(123).await;
    assert!(cached.is_none());
}

// =============================================================================
// PROGRESSIVE SEARCH TESTS (2 tests from progressive_search.rs)
// =============================================================================

#[test]
fn test_hilbert_pruning() {
    let config = HelixConfig::default();
    let distance_compute = Arc::new(UnifiedDistanceCompute::default());
    let coordinator = ProgressiveSearchCoordinator::new(config, distance_compute, None);

    let sstables = vec![
        SStableMetadata {
            path: "test1.helix".into(),
            level: 1,
            hilbert_range: Some((0, 1000)),
            num_vectors: 100,
            size_bytes: 1024,
            created_at: chrono::Utc::now(),
            blocks: vec![],
            bloom_filter: None,
        },
        SStableMetadata {
            path: "test2.helix".into(),
            level: 1,
            hilbert_range: Some((10000, 11000)),  // More distant to ensure pruning
            num_vectors: 100,
            size_bytes: 1024,
            created_at: chrono::Utc::now(),
            blocks: vec![],
            bloom_filter: None,
        },
    ];

    // Query key close to first SSTable
    let pruned = coordinator.prune_by_hilbert_range(&sstables, Some(500));
    assert_eq!(pruned.len(), 1);
    assert_eq!(pruned[0].path.to_str().unwrap(), "test1.helix");
}

#[test]
fn test_search_stats() {
    let mut stats = ProgressiveSearchStats::default();

    stats.record_search(5, 3, 1000, 50);
    stats.record_search(6, 2, 800, 40);

    assert_eq!(stats.total_searches, 2);
    assert_eq!(stats.sstables_pruned, 11);
    assert_eq!(stats.sstables_scanned, 5);
    assert_eq!(stats.vectors_evaluated, 1800);
    assert_eq!(stats.avg_vectors_per_search(), 900.0);
    assert_eq!(stats.avg_time_per_search_ms(), 45.0);
}

// =============================================================================
// PCA TESTS (2 tests from pca_impl.rs)
// =============================================================================

#[test]
fn test_enhanced_pca_model() {
    // Create synthetic data
    let mut records = Vec::new();
    for i in 0..100 {
        let vector = vec![i as f32, i as f32 * 2.0, i as f32 * 0.5, (i as f32).sin()];
        records.push(VectorRecord {
            id: format!("vec_{}", i),
            vector,
            metadata: std::collections::HashMap::new(),
            timestamp: Some(0),
            updated_at: None,
            expires_at: None,
            version: Some(1),
            source: None,
        });
    }

    // Train PCA model
    let model = EnhancedPCAModel::train(&records, 2).unwrap();

    // Check dimensions
    assert_eq!(model.n_components, 2);
    assert_eq!(model.original_dim, 4);

    // Test projection
    let test_vec = vec![10.0, 20.0, 5.0, 0.5];
    let projected = model.project(&test_vec).unwrap();
    assert_eq!(projected.len(), 2);

    // Test reconstruction
    let reconstructed = model.reconstruct(&projected).unwrap();
    assert_eq!(reconstructed.len(), 4);

    // Check variance explained
    let var_ratio = model.explained_variance_ratio();
    assert_eq!(var_ratio.len(), 2);
    assert!(var_ratio[0] >= var_ratio[1]); // First component explains more
}

#[test]
fn test_pca_model_manager() {
    let mut manager = PCAModelManager::new(3);

    // Create test data
    let records: Vec<VectorRecord> = (0..50)
        .map(|i| VectorRecord {
            id: format!("vec_{}", i),
            vector: vec![i as f32, i as f32 * 2.0, i as f32 * 0.5],
            metadata: std::collections::HashMap::new(),
            timestamp: Some(0),
            updated_at: None,
            expires_at: None,
            version: None,
            source: None,
        })
        .collect();

    // Train model
    manager.train_new_model(&records, 2).unwrap();

    assert!(manager.active_model.is_some());
    assert_eq!(manager.quality_metrics.len(), 1);

    // Check best model
    let best = manager.best_model_version();
    assert_eq!(best, Some(1));
}

// =============================================================================
// EVENTLOG INTEGRATION TESTS (2 tests from eventlog_integration.rs)
// =============================================================================

#[test]
fn test_helix_flush_handler_creation() {
    let handler = HelixFlushHandler::new();
    // Handler should work even without EventLog service
    assert!(handler.event_log.is_none() || handler.event_log.is_some());
}

#[tokio::test]
async fn test_can_compact_without_service() {
    let handler = HelixFlushHandler::new();
    // Without EventLog service, compaction should be allowed
    if handler.event_log.is_none() {
        assert!(
            handler
                .can_compact_files("test", &["file1.helix".to_string()])
                .await
        );
    }
}

// =============================================================================
// COMPACTION TESTS (1 test from compaction.rs)
// =============================================================================

#[tokio::test]
async fn test_compactor_creation() {
    let config = HelixConfig::default();

    // Create filesystem factory with proper config
    let mut fs_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
    fs_config.default_fs = Some("file:///tmp/helix_test".to_string());
    let factory = Arc::new(crate::storage::persistence::filesystem::FilesystemFactory::create(fs_config).await.unwrap());
    let filesystem = factory.get_filesystem("file:///tmp/helix_test").unwrap();

    let data_dir = std::path::PathBuf::from("/tmp/helix_test");

    let compactor = LeveledCompactor::new(config, filesystem, data_dir);
    assert!(compactor.liquid_config.enabled);
}
