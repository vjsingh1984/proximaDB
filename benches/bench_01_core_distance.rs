//! Real distance computation benchmarks

mod common;
use common::benchmark_utils::{print_system_info, STANDARD_DIMENSIONS, STANDARD_BATCH_SIZES};

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use proximadb::compute::distance_computation::{
    DistanceMetric,
    SimilarityResult,
    engine::UnifiedDistanceCompute,
};

fn init_hardware() {
    print_system_info("Core Distance Computation");
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
}

fn benchmark_distance_computation(c: &mut Criterion) {
    init_hardware();
    // Use standard dimensions from common module
    let dimensions = STANDARD_DIMENSIONS.to_vec();

    for dim in dimensions {
        let mut group = c.benchmark_group(format!("distance_dim_{}", dim));

        // Create test vectors
        let a: Vec<f32> = (0..dim).map(|i| (i as f32).sin()).collect();
        let b: Vec<f32> = (0..dim).map(|i| (i as f32).cos()).collect();

        // Benchmark each metric
        for metric in [
            DistanceMetric::Cosine,
            DistanceMetric::Euclidean,
            DistanceMetric::DotProduct,
        ] {
            let compute = UnifiedDistanceCompute::default();

            group.bench_with_input(
                BenchmarkId::new(format!("{:?}", metric), dim),
                &(&a, &b),
                |bencher, (a, b)| {
                    bencher.iter(|| {
                        let result =
                            compute.calculate_distance(a, b, &metric);
                        black_box(result)
                    });
                },
            );
        }

        group.finish();
    }
}

fn benchmark_batch_operations(c: &mut Criterion) {
    init_hardware();
    let mut group = c.benchmark_group("batch_operations");

    // Use standard BERT dimension for batch tests
    let query: Vec<f32> = (0..768).map(|i| (i as f32).sin()).collect();
    // Use standard batch sizes from common module
    let batch_sizes = STANDARD_BATCH_SIZES.to_vec();

    for batch_size in batch_sizes {
        let vectors: Vec<Vec<f32>> = (0..batch_size)
            .map(|j| (0..768).map(|i| ((i + j) as f32).cos()).collect())
            .collect();
        let vector_refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();

        let compute = UnifiedDistanceCompute::default();

        // Benchmark OLD method (for comparison)
        group.bench_with_input(
            BenchmarkId::new("cosine_batch_old", batch_size),
            &(&query, &vector_refs),
            |bencher, (query, vectors)| {
                bencher.iter(|| {
                    let results: Vec<f32> = vectors
                        .iter()
                        .map(|v| {
                            compute
                                .calculate_distance(
                                    query,
                                    v,
                                    &DistanceMetric::Cosine,
                                ).distance
                        })
                        .collect();
                    black_box(results)
                });
            },
        );

        // Benchmark NEW optimized batch method
        group.bench_with_input(
            BenchmarkId::new("cosine_batch_optimized", batch_size),
            &(&query, &vector_refs),
            |bencher, (query, vectors)| {
                bencher.iter(|| {
                    let results = compute.batch_distance_pooled_simd(
                        query,
                        vectors,
                        &DistanceMetric::Cosine,
                    );
                    black_box(results)
                });
            },
        );

        // Benchmark NEW pooled batch method
        group.bench_with_input(
            BenchmarkId::new("cosine_batch_pooled", batch_size),
            &(&query, &vector_refs),
            |bencher, (query, vectors)| {
                bencher.iter(|| {
                    let results = compute.batch_distance_pooled_simd(
                        query,
                        vectors,
                        &DistanceMetric::Cosine,
                    );
                    black_box(results)
                });
            },
        );

        // Benchmark NEW lazy batch method
        group.bench_with_input(
            BenchmarkId::new("cosine_batch_lazy", batch_size),
            &(&query, &vector_refs),
            |bencher, (query, vectors)| {
                bencher.iter(|| {
                    let results = compute.batch_distance_pooled_lazy(
                        query,
                        vectors,
                        &DistanceMetric::Cosine,
                    );
                    // Access first few results to trigger computation
                    let distances = results.distances.get();
                    let _ = distances.get(0);
                    let _ = distances.get(1);
                    black_box(results)
                });
            },
        );
    }

    group.finish();
}

fn benchmark_memory_pool_effectiveness(c: &mut Criterion) {
    init_hardware();
    let mut group = c.benchmark_group("memory_pool_effectiveness");

    // Test with 1024 vectors of dimension 768
    let query: Vec<f32> = (0..768).map(|i| (i as f32).sin()).collect();
    let vectors: Vec<Vec<f32>> = (0..1024)
        .map(|j| (0..768).map(|i| ((i + j) as f32).cos()).collect())
        .collect();
    let vector_refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();

    // Create compute engines with and without pooling
    let compute_default = UnifiedDistanceCompute::default();
    let compute_pooled = UnifiedDistanceCompute::default(); // Memory pool is integrated

    // Benchmark without memory pool
    group.bench_function("batch_without_pool", |b| {
        b.iter(|| {
            // Use basic loop for non-pooled version
            let results: Vec<SimilarityResult> = vector_refs.iter()
                .map(|v| compute_default.calculate_distance(&query, v, &DistanceMetric::Cosine))
                .collect();
            black_box(results)
        });
    });

    // Benchmark with memory pool
    group.bench_function("batch_with_pool", |b| {
        b.iter(|| {
            let results = compute_pooled.batch_distance_pooled_simd(
                &query,
                &vector_refs,
                &DistanceMetric::Cosine,
            );
            black_box(results)
        });
    });

    group.finish();
}

// Configure with consistent settings across all benchmarks
criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(40)
        .measurement_time(std::time::Duration::from_secs(5))
        .warm_up_time(std::time::Duration::from_secs(1));
    targets = benchmark_distance_computation,
              benchmark_batch_operations,
              benchmark_memory_pool_effectiveness
}
criterion_main!(benches);
