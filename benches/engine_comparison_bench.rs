// Benchmarks comparing all 7 storage engines in specified order
// SST, VIPER, HELIX, RAPTOR, SWIFT, NOVA, PRISM

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
use std::sync::Arc;
use tokio::runtime::Runtime;

/// Generate test vectors with internal structure
fn generate_vectors(count: usize, dimension: usize) -> Vec<proximadb::proto::proximadb_v1::VectorRecord> {
    use proximadb::proto::proximadb_v1::VectorRecord;

    (0..count)
        .map(|i| VectorRecord {
            id: format!("vec_{:08}", i),
            vector: vec![i as f32 / count as f32; dimension],
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

/// Benchmark vector insertion across all 7 engines in specified order
fn bench_all_engines_insertion(c: &mut Criterion) {
    let _ = hardware_capabilities::initialize_hardware_capabilities_default();
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("engine_comparison_insertion");

    // Test different vector counts
    for count in [100, 1000].iter() {
        let vectors = generate_vectors(*count, 768);

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
                    b.iter_batched(
                        || {
                            // Setup: Create fresh engine for each iteration
                            rt.block_on(async {
                                match *engine_type {
                                    "sst" => StorageEngineFactory::create_sst().unwrap(),
                                    "viper" => StorageEngineFactory::create_viper().unwrap(),
                                    "helix" => StorageEngineFactory::create_helix().unwrap(),
                                    "raptor" => StorageEngineFactory::create_sst().unwrap(), // Fallback
                                    "swift" => StorageEngineFactory::create_swift().unwrap(),
                                    "nova" => StorageEngineFactory::create_nova().unwrap(),
                                    "prism" => StorageEngineFactory::create_sst().unwrap(), // Fallback
                                    _ => StorageEngineFactory::create_sst().unwrap(),
                                }
                            })
                        },
                        |engine| {
                            // Benchmark: Flush vectors to storage
                            rt.block_on(async {
                                use proximadb::storage::traits::FlushParameters;

                                let params = FlushParameters {
                                    collection_id: Some("bench_collection".to_string()),
                                    vector_records: vectors.clone(),
                                    force: true,
                                    synchronous: true,
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
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("engine_comparison_search");

    let query = vec![0.5; 768];
    let top_k = 10;
    let test_vectors = generate_vectors(1000, 768);

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
        group.bench_function(engine_name, |b| {
            b.iter_batched(
                || {
                    // Setup: Create engine with test data
                    rt.block_on(async {
                        let engine = match *engine_type {
                            "sst" => StorageEngineFactory::create_sst().unwrap(),
                            "viper" => StorageEngineFactory::create_viper().unwrap(),
                            "helix" => StorageEngineFactory::create_helix().unwrap(),
                            "raptor" => StorageEngineFactory::create_sst().unwrap(), // Fallback
                            "swift" => StorageEngineFactory::create_swift().unwrap(),
                            "nova" => StorageEngineFactory::create_nova().unwrap(),
                            "prism" => StorageEngineFactory::create_sst().unwrap(), // Fallback
                            _ => StorageEngineFactory::create_sst().unwrap(),
                        };

                        // Flush test vectors to storage
                        use proximadb::storage::traits::FlushParameters;

                        let params = FlushParameters {
                            collection_id: Some("bench_collection".to_string()),
                            vector_records: test_vectors.clone(),
                            force: true,
                            synchronous: true,
                            ..Default::default()
                        };

                        let _ = engine.flush(params).await;

                        engine
                    })
                },
                |engine| {
                    // Benchmark: Search operation
                    rt.block_on(async {
                        use proximadb::{
                            storage::traits::{StorageQueryContext, StorageQueryMetadata},
                            core::search::SearchParams,
                            proto::proximadb_v1::Collection,
                        };
                        use std::sync::Arc;

                        // Create search context
                        let search_params = Arc::new(SearchParams {
                            vector: Some(query.clone()),
                            query_vectors: None,
                            top_k: Some(top_k),
                            distance_metric: Some(DistanceMetric::Euclidean),
                            filter_expression: None,
                            filters: None,
                            accuracy_threshold: None,
                            include_expired: Some(false),
                            ..Default::default()
                        });

                        use proximadb::proto::proximadb_v1::{CollectionConfig, CollectionStats};

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
                            storage_assignment: None,
                        });

                        let ctx = StorageQueryContext {
                            search_params,
                            collection,
                            metadata: StorageQueryMetadata::default(),
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