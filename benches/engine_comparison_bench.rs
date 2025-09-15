// Benchmarks comparing all 7 storage engines in specified order
// SST, VIPER, HELIX, RAPTOR, SWIFT, NOVA, PRISM

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use proximadb::{
    compute::distance_computation::DistanceMetric,
    core::hardware_capabilities,
    proto::proximadb_v1::{VectorRecord, SqlValue},
    storage::{
        engines::{
            factory::StorageEngineFactory,
            impls::{
                sst::SstEngine,
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

/// Generate test vectors with correct protobuf structure
fn generate_vectors(count: usize, dimension: usize) -> Vec<VectorRecord> {
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
                                StorageEngineFactory::create_engine(
                                    engine_type,
                                    &format!("/tmp/bench_{}", engine_type),
                                    None,
                                ).await.unwrap()
                            })
                        },
                        |engine| {
                            // Benchmark: Insert vectors
                            rt.block_on(async {
                                for vector in &vectors {
                                    let _ = engine.insert_vector("bench_collection", vector.clone()).await;
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
                        let engine = StorageEngineFactory::create_engine(
                            engine_type,
                            &format!("/tmp/bench_{}", engine_type),
                            None,
                        ).await.unwrap();

                        // Insert test vectors
                        for vector in &test_vectors {
                            let _ = engine.insert_vector("bench_collection", vector.clone()).await;
                        }

                        engine
                    })
                },
                |engine| {
                    // Benchmark: Search operation
                    rt.block_on(async {
                        let results = engine.search_vectors_unified(
                            "bench_collection",
                            &format!("/tmp/bench_{}/bench_collection", engine_type),
                            &query,
                            top_k,
                            DistanceMetric::Euclidean,
                            None,
                            None,
                            None,
                        ).await.unwrap();
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