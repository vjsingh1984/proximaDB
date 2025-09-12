//! Real distance computation benchmarks

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use proximadb::compute::distance_computation::{
    DistanceMetric, DistanceMode, UnifiedDistanceCompute,
};

fn benchmark_distance_computation(c: &mut Criterion) {
    let dimensions = vec![128, 256, 512, 1024, 2048];

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
            let compute = UnifiedDistanceCompute::new(metric);

            group.bench_with_input(
                BenchmarkId::new(format!("{:?}", metric), dim),
                &(&a, &b),
                |bencher, (a, b)| {
                    bencher.iter(|| {
                        let result =
                            compute.calculate_distance_with_mode(a, b, &metric, DistanceMode::Raw);
                        black_box(result.raw_value)
                    });
                },
            );
        }

        group.finish();
    }
}

fn benchmark_batch_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("batch_operations");

    let query: Vec<f32> = (0..256).map(|i| (i as f32).sin()).collect();
    let batch_sizes = vec![100, 1000, 10000];

    for batch_size in batch_sizes {
        let vectors: Vec<Vec<f32>> = (0..batch_size)
            .map(|j| (0..256).map(|i| ((i + j) as f32).cos()).collect())
            .collect();
        let vector_refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();

        let compute = UnifiedDistanceCompute::new(DistanceMetric::Cosine);

        group.bench_with_input(
            BenchmarkId::new("cosine_batch", batch_size),
            &(&query, &vector_refs),
            |bencher, (query, vectors)| {
                bencher.iter(|| {
                    let results: Vec<f32> = vectors
                        .iter()
                        .map(|v| {
                            compute
                                .calculate_distance_with_mode(
                                    query,
                                    v,
                                    &DistanceMetric::Cosine,
                                    DistanceMode::Raw,
                                )
                                .raw_value
                        })
                        .collect();
                    black_box(results)
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    benchmark_distance_computation,
    benchmark_batch_operations
);
criterion_main!(benches);
