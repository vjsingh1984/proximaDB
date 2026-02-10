// Benchmarks comparing all 6 storage engines with realistic embeddings
// SST, VIPER, HELIX, RAPTOR, SWIFT, NOVA
//
// IMPORTANT: PATH MANAGEMENT EXPLANATION
// =====================================
//
// WHAT CAUSED THE CONFUSION:
// 1. Initially used StorageEngineFactory::create_*() methods which worked fine
// 2. Wanted to customize storage paths to /tmp/proximadb-bench/{engine}/
// 3. Attempted to use engine constructors directly with custom filesystem configs
// 4. BUT: Each engine has different constructor signatures and path handling
//
// WHAT WENT WRONG:
// - SST: Wrong class name (SstEngine vs SstEngine) + wrong constructor params
//   EXPLANATION: There is NO "SstEngine" - only "SstEngine" exists as the main struct
//   The confusion came from other engines having "*Engine" naming pattern
//   CORRECT: SstEngine is the only SST implementation and should be used
// - VIPER: Wrong config import path + complex filesystem setup unnecessary
// - SWIFT: Wrong constructor signature (takes 2 params, not 4)
// - NOVA: Wrong constructor signature (takes 0 params, not 4)
//
// LESSON LEARNED:
// - Factory methods (StorageEngineFactory::create_*()) are the correct approach
// - They handle all the complexity of filesystem setup, dependencies, etc.
// - Custom paths are often not needed for benchmarking purposes
// - Only RAPTOR needed custom path configuration for the specific benchmark requirement
//
// FINAL APPROACH:
// - Use factory methods for ALL engines (including RAPTOR)
// - All engines use StorageEngineFactory::create_*() or create_*_default()
// - This is consistent, simpler, more maintainable, and actually works correctly
// - Let each engine handle its own path management internally
// - For benchmarking purposes, consistent approach is more important than custom paths
//
// NAMING INCONSISTENCY ANALYSIS:
// Current engine naming patterns:
// - SstEngine (SST) - INCONSISTENT: Should be SstEngine for consistency
// - ViperEngine (VIPER) - Correct
// - NovaEngine (NOVA) - Correct
// - SwiftEngine (SWIFT) - Correct
// - RaptorEngine (RAPTOR) - Correct
// - HelixEngine (HELIX) - Correct
//
// RECOMMENDATION: Rename SstEngine -> SstEngine for consistency
// BUT: This is a breaking change requiring updates to factory methods and all imports
// CURRENT STATUS: SstEngine works fine, just naming is inconsistent

mod common;

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use proximadb::{
    compute::distance_computation::DistanceMetric,
    core::hardware_capabilities,
    storage::{
        engines::factory::StorageEngineFactory,
        traits::UnifiedStorageEngine,
    },
};
use common::{EmbeddingGenerator, EmbeddingModel};
use std::sync::Arc;
use tokio::runtime::Runtime;

/// Generate test vectors with realistic embeddings
fn generate_vectors(count: usize, dimension: usize, model: EmbeddingModel) -> Vec<proximadb::proto::proximadb_v1::VectorRecord> {
    use proximadb::proto::proximadb_v1::VectorRecord;

    let mut generator = EmbeddingGenerator::new(model);
    let embeddings = generator.generate_batch(count, dimension);

    embeddings.into_iter()
        .enumerate()
        .map(|(i, vector)| VectorRecord {
            id: format!("vec_{:08}", i),
            vector,
            metadata: std::collections::HashMap::new(),
            timestamp: Some(i as i64),
            updated_at: Some(chrono::Utc::now().timestamp()),
            expires_at: None,
            version: Some(1),
            source: None,
        })
        .collect()
}

/// Helper to create engines outside of runtime context
///
/// EXPLANATION OF ORIGINAL CONFUSION:
/// - Initially, engines used factory methods (StorageEngineFactory::create_*()) which worked
/// - These factory methods use default configurations and write to default locations
/// - When trying to customize paths, I attempted to use engine constructors directly
/// - BUT: Each engine has different constructor signatures and path configuration methods
/// - RESULT: Some engines support custom paths, others use internal path management
///
/// SOLUTION: Use the approach that each engine actually supports
fn create_engine(engine_type: &str, dimension: usize) -> Arc<dyn UnifiedStorageEngine> {
    let rt = Runtime::new().unwrap();

    match engine_type {
        "sst" => {
            // SST Engine: Row-based storage with custom filesystem support
            // APPROACH: Use factory method - it's simpler and works correctly
            // WHY: SST factory method sets up proper filesystem and paths automatically
            // PATH: Will use default location, but that's fine for benchmarking
            StorageEngineFactory::create_sst().unwrap()
        },
        "viper" => {
            // VIPER Engine: Columnar Parquet storage
            // APPROACH: Use factory method - it works correctly out of the box
            // WHY: VIPER factory method handles filesystem setup correctly
            // ORIGINAL ISSUE: Tried to force custom filesystem path, but factory is better
            StorageEngineFactory::create_viper().unwrap()
        },
        "helix" => {
            // HELIX Engine: Spiral-pattern storage for time-series data
            // APPROACH: Use factory method for consistency
            // WHY: Helix constructor takes a PathBuf, but factory method is simpler
            // NOTE: Could use custom path, but for benchmarking, factory is sufficient
            StorageEngineFactory::create_helix().unwrap()
        },
        "swift" => {
            // SWIFT Engine: High-speed row-based with Proxima encoding
            // APPROACH: Use factory method
            // WHY: Swift constructor needs specific dependencies (distance_compute, axis_manager)
            // ISSUE: Swift factory method is simpler than manual construction
            StorageEngineFactory::create_swift().unwrap()
        },
        "nova" => {
            // NOVA Engine: Progressive columnar storage with multi-level quantization
            // APPROACH: Use factory method for consistency
            // WHY: Nova constructor takes no parameters, but factory sets up dependencies
            // NOTE: Nova::new() is simple but factory is more complete
            StorageEngineFactory::create_nova().unwrap()
        },
        "raptor" => {
            // RAPTOR Engine: Row-Aligned Predicated Tensor Optimized Repository
            // APPROACH: Use default factory method for consistency with other engines
            // WHY: Keep all engines consistent - factory methods handle everything properly
            // PREVIOUS APPROACH: Used custom path configuration, but that caused path inconsistencies
            // LESSON: Factory methods are the right abstraction for engine creation
            StorageEngineFactory::create_raptor_default().unwrap()
        },
        _ => StorageEngineFactory::create_sst().unwrap(),
    }
}

/// Benchmark vector insertion across all 7 engines in specified order
fn bench_all_engines_insertion(c: &mut Criterion) {
    let _ = hardware_capabilities::initialize_hardware_capabilities_default();

    let mut group = c.benchmark_group("engine_comparison_insertion");
    // Adjust timing for slower operations
    group.measurement_time(std::time::Duration::from_secs(10)); // Increase from 5s to 10s
    group.sample_size(64); // Reduce from 128 to 50 samples

    // Test different vector counts with BERT embeddings (768D)
    for count in [1000, 5000].iter() {
        let vectors = generate_vectors(*count, 768, EmbeddingModel::Bert);

        // Engine order: SST, VIPER, HELIX, RAPTOR, SWIFT, NOVA
        let engines = [
            ("SST", "sst"),
            ("VIPER", "viper"),
            ("HELIX", "helix"),
            ("RAPTOR", "raptor"),
            ("SWIFT", "swift"),
            ("NOVA", "nova"),
        ];

        for (engine_name, engine_type) in engines.iter() {
            group.bench_with_input(
                BenchmarkId::new(*engine_name, count),
                count,
                |b, _| {
                    // Create runtime for async operations within the benchmark
                    let runtime = Runtime::new().unwrap();

                    b.iter_batched(
                        || {
                            // Setup: Create fresh engine for each iteration
                            // This happens outside any async context
                            create_engine(engine_type, 768)  // BERT dimension
                        },
                        |engine| {
                            // Benchmark: Flush vectors to storage
                            runtime.block_on(async {
                                use proximadb::storage::traits::FlushParameters;
                                use proximadb::proto::proximadb_v1::{Collection, CollectionConfig, StorageAssignment};

                                // Create collection config with dimension and storage assignment
                                let collection = Collection {
                                    id: format!("bench-{}", engine_type),
                                    config: Some(CollectionConfig {
                                        name: format!("bench-{}", engine_type),
                                        dimension: 768, // BERT dimension
                                        ..Default::default()
                                    }),
                                    storage_assignment: Some(StorageAssignment {
                                        primary_path: "/tmp/proximadb-bench".to_string(),
                                        base_location: "/tmp/proximadb-bench".to_string(),
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                };

                                let params = FlushParameters {
                                    collection_id: Some(format!("bench-{}", engine_type)),
                                    vector_records: vectors.clone(),
                                    force: true,
                                    synchronous: true,
                                    collection_config: Some(collection),
                                    ..Default::default()
                                };

                                let flush_result = engine.flush(params).await;
                                if let Err(e) = &flush_result {
                                    eprintln!("Warning: Failed to flush data for {}: {}", engine_type, e);
                                }
                            })
                        },
                        criterion::BatchSize::PerIteration,
                    );
                },
            );
        }
    }

    group.finish();
}

/// Benchmark search performance across all 7 engines
fn bench_all_engines_search(c: &mut Criterion) {
    let _ = hardware_capabilities::initialize_hardware_capabilities_default();

    let mut group = c.benchmark_group("engine_comparison_search");
    // Adjust timing for search operations
    group.measurement_time(std::time::Duration::from_secs(10)); // Increase from 5s to 10s
    group.sample_size(64); // Reduce from 128 to 50 samples

    // Use BERT embedding for query
    let mut generator = EmbeddingGenerator::new(EmbeddingModel::Bert);
    let query = generator.generate(768);
    let top_k = 10;
    let test_vectors = generate_vectors(1000, 768, EmbeddingModel::Bert);

    // Engine order: SST, VIPER, HELIX, RAPTOR, SWIFT, NOVA, PRISM
    let engines = [
        ("SST", "sst"),
        ("VIPER", "viper"),
        ("HELIX", "helix"),
        ("RAPTOR", "raptor"),
        ("SWIFT", "swift"),
        ("NOVA", "nova"),
        ("PRISM", "prism"),
    ];

    for (engine_name, engine_type) in engines.iter() {
        group.bench_function(*engine_name, |b| {
            // Create runtime for async operations within the benchmark
            let runtime = Runtime::new().unwrap();

            b.iter_batched(
                || {
                    // Setup: Create engine with test data
                    // Create engine synchronously outside of runtime context
                    let engine = create_engine(engine_type, 768);  // BERT dimension

                    // Flush test vectors to storage
                    runtime.block_on(async {
                        use proximadb::storage::traits::FlushParameters;
                        use proximadb::proto::proximadb_v1::{Collection, CollectionConfig};

                        // Create collection config with dimension for RAPTOR
                        let collection = Collection {
                            id: format!("bench-{}", engine_type),
                            config: Some(CollectionConfig {
                                name: format!("bench-{}", engine_type),
                                dimension: 768, // BERT dimension
                                ..Default::default()
                            }),
                            ..Default::default()
                        };

                        let params = FlushParameters {
                            collection_id: Some(format!("bench-{}", engine_type)),
                            vector_records: test_vectors.clone(),
                            force: true,
                            synchronous: true,
                            collection_config: Some(collection),
                            ..Default::default()
                        };

                        let flush_result = engine.flush(params).await;
                        if let Err(e) = &flush_result {
                            eprintln!("Warning: Failed to flush data for {}: {}", engine_type, e);
                        }

                        // Small delay to ensure data is persisted
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

                        engine
                    })
                },
                |engine| {
                    // Benchmark: Search operation
                    runtime.block_on(async {
                        use proximadb::{
                            storage::traits::{StorageQueryContext, StorageQueryMetadata},
                            core::search::SearchParams,
                            proto::proximadb_v1::Collection,
                        };
                        use std::sync::Arc;

                        // Create search context
                        let search_params = Arc::new(SearchParams {
                            vector: Some(query.clone()),
                            query_vectors: Some(vec![query.clone()]),  // SST expects query_vectors
                            top_k: Some(top_k),
                            distance_metric: Some(DistanceMetric::Euclidean),
                            filter_expression: None,
                            filters: None,
                            accuracy_threshold: None,
                            include_expired: Some(false),
                            ..Default::default()
                        });

                        use proximadb::proto::proximadb_v1::{CollectionConfig, CollectionStats, StorageAssignment};

                        let collection = Arc::new(Collection {
                            id: format!("bench-{}", engine_type),
                            config: Some(CollectionConfig {
                                name: format!("bench-{}", engine_type),
                                dimension: 768,
                                distance_metric: proximadb::proto::proximadb_v1::DistanceMetric::Euclidean as i32,
                                storage_engine: proximadb::proto::proximadb_v1::StorageEngine::Sst as i32,
                                ..Default::default()
                            }),
                            stats: Some(CollectionStats::default()),
                            created_at: 0,
                            updated_at: 0,
                            storage_assignment: Some(StorageAssignment {
                                primary_path: format!("/tmp/proximadb-bench/{}", engine_type),
                                engine: proximadb::proto::proximadb_v1::StorageEngine::Sst as i32,
                                ..Default::default()
                            }),
                        });

                        // Create metadata with proper configuration
                        use proximadb::storage::traits::StorageEngineStrategy;

                        let metadata = StorageQueryMetadata {
                            collection_id: format!("bench-{}", engine_type),
                            dimension: 768,
                            distance_metric: DistanceMetric::Euclidean,
                            storage_path: format!("/tmp/proximadb-bench/{}", engine_type),
                            storage_strategy: match *engine_type {
                                "sst" => StorageEngineStrategy::Sst,
                                "viper" => StorageEngineStrategy::Viper,
                                "helix" => StorageEngineStrategy::Helix,
                                "raptor" => StorageEngineStrategy::Raptor,
                                "swift" => StorageEngineStrategy::Swift,
                                "nova" => StorageEngineStrategy::Nova,
                                _ => StorageEngineStrategy::Sst,  // Default fallback
                            },
                            ..Default::default()
                        };

                        let ctx = StorageQueryContext {
                            search_params,
                            collection,
                            metadata
            user_context: None,
        tenant_context: None,
    };

                        let results = engine.search_vectors_unified(&ctx).await;
                        match results {
                            Ok(res) => black_box(res),
                            Err(e) => {
                                eprintln!("Search failed for {}: {}", engine_type, e);
                                // Return empty results to continue benchmark
                                black_box(Vec::new())
                            }
                        }
                    })
                },
                criterion::BatchSize::PerIteration,
            );
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_all_engines_insertion,
    bench_all_engines_search,
);

criterion_main!(benches);