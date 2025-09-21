// Benchmarks comparing all 7 storage engines with realistic embeddings
// SST, VIPER, HELIX, RAPTOR, SWIFT, NOVA, PRISM

mod common;

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use proximadb::{
    compute::distance_computation::DistanceMetric,
    core::hardware_capabilities,
    storage::{
        engines::{
            factory::StorageEngineFactory,
            impls::{
                sst::SstStorage,
                viper::ViperEngine,
                nova::NovaEngine,
                swift::SwiftEngine,
                raptor::RaptorEngine,
                prism::PrismEngine,
                helix::HelixEngine,
            },
        },
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
            timestamp: i as i64,
            updated_at: Some(chrono::Utc::now().timestamp()),
            expires_at: None,
            version: Some(1),
            quantized_vector: vec![],
            source: None,
        })
        .collect()
}

/// Helper to create engines outside of runtime context
fn create_engine(engine_type: &str, dimension: usize) -> Arc<dyn UnifiedStorageEngine> {
    // Create engines synchronously - factory methods handle their own runtime
    match engine_type {
        "sst" => StorageEngineFactory::create_sst().unwrap(),
        "viper" => StorageEngineFactory::create_viper().unwrap(),
        "helix" => StorageEngineFactory::create_helix().unwrap(),
        "swift" => StorageEngineFactory::create_swift().unwrap(),
        "nova" => StorageEngineFactory::create_nova().unwrap(),
        "raptor" => {
            // Raptor needs async creation with config, use a fresh runtime
            let rt = Runtime::new().unwrap();
            rt.block_on(async {
                use proximadb::storage::engines::impls::raptor::RaptorConfig;

                let mut config = RaptorConfig::default();
                config.dimension = dimension;
                config.rowgroup_size = 1000;  // Optimize for benchmark size

                StorageEngineFactory::create_raptor(
                    "bench_collection".to_string(),
                    "/tmp/proximadb_bench_raptor".to_string(),
                    Some(config),
                ).await.unwrap()
            })
        },
        "prism" => {
            // Prism needs async creation, use a fresh runtime
            let rt = Runtime::new().unwrap();
            rt.block_on(async {
                StorageEngineFactory::create_prism_async().await.unwrap()
            })
        },
        _ => StorageEngineFactory::create_sst().unwrap(),
    }
}

/// Benchmark vector insertion across all 7 engines in specified order
fn bench_all_engines_insertion(c: &mut Criterion) {
    let _ = hardware_capabilities::initialize_hardware_capabilities_default();

    let mut group = c.benchmark_group("engine_comparison_insertion");

    // Test different vector counts with BERT embeddings (768D)
    for count in [1000, 5000].iter() {
        let vectors = generate_vectors(*count, 768, EmbeddingModel::Bert);

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
                                use proximadb::proto::proximadb_v1::{Collection, CollectionConfig};

                                // Create collection config with dimension for RAPTOR
                                let collection = Collection {
                                    id: "bench_collection".to_string(),
                                    config: Some(CollectionConfig {
                                        name: "bench_collection".to_string(),
                                        dimension: 768, // BERT dimension
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                };

                                let params = FlushParameters {
                                    collection_id: Some("bench_collection".to_string()),
                                    vector_records: vectors.clone(),
                                    force: true,
                                    synchronous: true,
                                    collection_config: Some(collection),
                                    ..Default::default()
                                };

                                let _ = engine.flush(params).await;
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
                            id: "bench_collection".to_string(),
                            config: Some(CollectionConfig {
                                name: "bench_collection".to_string(),
                                dimension: 768, // BERT dimension
                                ..Default::default()
                            }),
                            ..Default::default()
                        };

                        let params = FlushParameters {
                            collection_id: Some("bench_collection".to_string()),
                            vector_records: test_vectors.clone(),
                            force: true,
                            synchronous: true,
                            collection_config: Some(collection),
                            ..Default::default()
                        };

                        let _ = engine.flush(params).await;
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
                            id: "bench_collection".to_string(),
                            config: Some(CollectionConfig {
                                name: "bench_collection".to_string(),
                                dimension: 768,
                                distance_metric: proximadb::proto::proximadb_v1::DistanceMetric::Euclidean as i32,
                                storage_engine: proximadb::proto::proximadb_v1::StorageEngine::Sst as i32,
                                ..Default::default()
                            }),
                            stats: Some(CollectionStats::default()),
                            created_at: 0,
                            updated_at: 0,
                            storage_assignment: Some(StorageAssignment {
                                primary_path: "/tmp/proximadb_bench".to_string(),
                                engine: proximadb::proto::proximadb_v1::StorageEngine::Sst as i32,
                                ..Default::default()
                            }),
                        });

                        // Create metadata with proper configuration
                        use proximadb::storage::traits::StorageEngineStrategy;

                        let metadata = StorageQueryMetadata {
                            collection_id: "bench_collection".to_string(),
                            dimension: 768,
                            distance_metric: DistanceMetric::Euclidean,
                            storage_path: "/tmp/proximadb_bench".to_string(),
                            storage_strategy: match *engine_type {
                                "sst" => StorageEngineStrategy::Sst,
                                "viper" => StorageEngineStrategy::Viper,
                                "helix" => StorageEngineStrategy::Helix,
                                "raptor" => StorageEngineStrategy::Raptor,
                                "swift" => StorageEngineStrategy::Swift,
                                "nova" => StorageEngineStrategy::Nova,
                                "prism" => StorageEngineStrategy::Prism,
                                _ => StorageEngineStrategy::Sst,  // Default fallback
                            },
                            ..Default::default()
                        };

                        let ctx = StorageQueryContext {
                            search_params,
                            collection,
                            metadata,
                        };

                        let results = engine.search_vectors_unified(&ctx).await.unwrap();
                        black_box(results)
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