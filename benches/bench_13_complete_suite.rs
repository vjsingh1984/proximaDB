//! ProximaDB Unified Benchmark Suite using Criterion
//!
//! A comprehensive benchmark suite for ProximaDB that tests:
//! - Distance computation performance
//! - Vector operations
//! - Index operations (HNSW, LSH)
//! - Storage engines and concurrent operations
//!
//! Run with: cargo bench --bench proximadb_consolidated_bench

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use proximadb::{
    compute::distance_computation::{engine::UnifiedDistanceCompute, DistanceMetric},
    index::axis::{
        AxisVectorIndex,
        indexes::{
            hnsw_index::{create_hnsw_index, AxisHnswConfig},
            lsh_index::{AxisLshConfig, AxisLshIndex},
        },
    },
    proto::proximadb_v1::VectorRecord,
};
use std::collections::HashMap;
use std::sync::Once;
use std::time::Duration;

/// Global initialization for hardware capabilities
static INIT: Once = Once::new();

/// Initialize hardware capabilities once for all benchmarks
fn init_hardware() {
    INIT.call_once(|| {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    });
}

/// Generate test vectors
fn generate_test_vectors(count: usize, dimension: usize) -> Vec<Vec<f32>> {
    (0..count)
        .map(|i| {
            (0..dimension)
                .map(|j| ((i + j) as f32 * 0.1).sin())
                .collect()
        })
        .collect()
}

/// Generate VectorRecord instances for benchmarking
fn generate_vector_records(count: usize, dimension: usize) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| VectorRecord {
            id: format!("vec_{:08}", i),
            vector: (0..dimension)
                .map(|j| ((i + j) as f32 * 0.1).sin())
                .collect(),
            metadata: HashMap::new(),
            timestamp: i as i64,
            updated_at: Some(chrono::Utc::now().timestamp()),
            expires_at: None,
            version: Some(1),
            quantized_vector: vec![],
            source: None,
        })
        .collect()
}

/// Benchmark distance computation algorithms
fn bench_distance_metrics(c: &mut Criterion) {
    init_hardware();

    let mut group = c.benchmark_group("distance_computation");
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(5));

    let dimensions = vec![128, 256, 512, 768, 1536];
    let metrics = vec![
        DistanceMetric::Cosine,
        DistanceMetric::Euclidean,
        DistanceMetric::DotProduct,
        DistanceMetric::Manhattan,
    ];

    for dimension in dimensions {
        // Generate test vectors
        let a: Vec<f32> = (0..dimension).map(|i| (i as f32).sin()).collect();
        let b: Vec<f32> = (0..dimension).map(|i| (i as f32).cos()).collect();

        for metric in &metrics {
            let compute = UnifiedDistanceCompute::new(*metric);

            group.bench_with_input(
                BenchmarkId::new(format!("{:?}", metric), dimension),
                &dimension,
                |bencher, _| {
                    bencher.iter(|| {
                        let result = compute.calculate_distance(&a, &b, metric);
                        black_box(result)
                    });
                },
            );
        }
    }

    group.finish();
}

/// Benchmark vector operations
fn bench_vector_operations(c: &mut Criterion) {
    init_hardware();

    let mut group = c.benchmark_group("vector_operations");
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(3));

    let dimensions = vec![128, 512, 1536];
    let vector_counts = vec![100, 1000, 10000];

    for dimension in dimensions {
        for &count in &vector_counts {
            group.throughput(Throughput::Elements(count as u64));

            // Benchmark VectorRecord creation
            group.bench_with_input(
                BenchmarkId::new("create", format!("dim_{}_count_{}", dimension, count)),
                &(dimension, count),
                |bencher, &(dim, cnt)| {
                    bencher.iter(|| {
                        let records = generate_vector_records(cnt, dim);
                        black_box(records)
                    });
                },
            );

            // Benchmark vector normalization
            let vectors = generate_test_vectors(count, dimension);
            group.bench_with_input(
                BenchmarkId::new("normalize", format!("dim_{}_count_{}", dimension, count)),
                &vectors,
                |bencher, vecs| {
                    bencher.iter(|| {
                        let normalized: Vec<Vec<f32>> = vecs
                            .iter()
                            .map(|v| {
                                let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                                if norm > 0.0 {
                                    v.iter().map(|x| x / norm).collect()
                                } else {
                                    v.clone()
                                }
                            })
                            .collect();
                        black_box(normalized)
                    });
                },
            );

            // Benchmark batch distance computation
            if count <= 1000 {  // Limit for larger operations
                let query = vec![0.5f32; dimension];
                group.bench_with_input(
                    BenchmarkId::new("batch_distance", format!("dim_{}_count_{}", dimension, count)),
                    &(&query, &vectors),
                    |bencher, (q, vecs)| {
                        bencher.iter(|| {
                            let compute = UnifiedDistanceCompute::new(DistanceMetric::Cosine);
                            let distances: Vec<_> = vecs
                                .iter()
                                .map(|v| compute.calculate_distance(q, v, &DistanceMetric::Cosine))
                                .collect();
                            black_box(distances)
                        });
                    },
                );
            }
        }
    }

    group.finish();
}

/// Benchmark HNSW index operations
fn bench_hnsw_index(c: &mut Criterion) {
    init_hardware();

    let mut group = c.benchmark_group("hnsw_index");
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10);

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let dimensions = vec![128, 256, 512];
    let vector_counts = vec![100, 500, 1000];

    for dimension in dimensions {
        for &count in &vector_counts {
            let vectors = generate_test_vectors(count, dimension);

            // Benchmark index creation and insertion
            group.bench_with_input(
                BenchmarkId::new("build", format!("dim_{}_count_{}", dimension, count)),
                &(dimension, &vectors),
                |bencher, (dim, vecs)| {
                    bencher.iter(|| {
                        runtime.block_on(async {
                            let config = AxisHnswConfig {
                                m: 16,
                                ef_construction: 200,
                                ef: 100,
                                max_layers: 16,
                                distance_metric: DistanceMetric::Cosine,
                            };

                            let index = create_hnsw_index(config, *dim).unwrap();

                            for (i, vec) in vecs.iter().enumerate() {
                                index.add(format!("vec_{}", i), vec.clone()).await.unwrap();
                            }

                            black_box(index)
                        })
                    });
                },
            );

            // Benchmark search operations
            let config = AxisHnswConfig {
                m: 16,
                ef_construction: 200,
                ef: 100,
                max_layers: 16,
                distance_metric: DistanceMetric::Cosine,
            };

            let index = runtime.block_on(async {
                let idx = create_hnsw_index(config, dimension).unwrap();
                for (i, vec) in vectors.iter().enumerate() {
                    idx.add(format!("vec_{}", i), vec.clone()).await.unwrap();
                }
                idx
            });

            let query = &vectors[0];
            group.bench_with_input(
                BenchmarkId::new("search", format!("dim_{}_count_{}", dimension, count)),
                query,
                |bencher, q| {
                    bencher.iter(|| {
                        runtime.block_on(async {
                            let results = index.search(q, 10, None).await;
                            black_box(results)
                        })
                    });
                },
            );
        }
    }

    group.finish();
}

/// Benchmark LSH index operations
fn bench_lsh_index(c: &mut Criterion) {
    init_hardware();

    let mut group = c.benchmark_group("lsh_index");
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(10);

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let dimensions = vec![128, 256, 512];
    let vector_counts = vec![100, 500, 1000];

    for dimension in dimensions {
        for &count in &vector_counts {
            let vectors = generate_test_vectors(count, dimension);

            // Benchmark index creation and insertion
            group.bench_with_input(
                BenchmarkId::new("build", format!("dim_{}_count_{}", dimension, count)),
                &(dimension, &vectors),
                |bencher, (dim, vecs)| {
                    bencher.iter(|| {
                        runtime.block_on(async {
                            let config = AxisLshConfig {
                                n_tables: 8,
                                n_hashes: 10,
                                hash_width: 4.0,
                                distance_metric: DistanceMetric::Cosine,
                                binary_mode: false,
                                seed: 42,
                            };

                            let index = AxisLshIndex::new(config, *dim);

                            for (i, vec) in vecs.iter().enumerate() {
                                index.add(format!("vec_{}", i), vec.clone()).await.unwrap();
                            }

                            black_box(index)
                        })
                    });
                },
            );

            // Benchmark search operations
            let config = AxisLshConfig {
                n_tables: 8,
                n_hashes: 10,
                hash_width: 4.0,
                distance_metric: DistanceMetric::Cosine,
                binary_mode: false,
                seed: 42,
            };

            let index = runtime.block_on(async {
                let idx = AxisLshIndex::new(config, dimension);
                for (i, vec) in vectors.iter().enumerate() {
                    idx.add(format!("vec_{}", i), vec.clone()).await.unwrap();
                }
                idx
            });

            let query = &vectors[0];
            group.bench_with_input(
                BenchmarkId::new("search", format!("dim_{}_count_{}", dimension, count)),
                query,
                |bencher, q| {
                    bencher.iter(|| {
                        runtime.block_on(async {
                            let results = index.search(q, 10, None).await;
                            black_box(results)
                        })
                    });
                },
            );
        }
    }

    group.finish();
}

/// Benchmark concurrent operations
fn bench_concurrent_operations(c: &mut Criterion) {
    init_hardware();

    let mut group = c.benchmark_group("concurrent_operations");
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(3));

    let dimension = 256;
    let num_threads = vec![1, 2, 4, 8];
    let operations_per_thread = 100;

    for &threads in &num_threads {
        group.throughput(Throughput::Elements(
            (threads * operations_per_thread) as u64,
        ));

        group.bench_with_input(
            BenchmarkId::new("distance_computation", threads),
            &threads,
            |bencher, &thread_count| {
                bencher.iter(|| {
                    let handles: Vec<_> = (0..thread_count)
                        .map(|_| {
                            std::thread::spawn(move || {
                                let compute = UnifiedDistanceCompute::new(DistanceMetric::Cosine);
                                let a: Vec<f32> = (0..dimension).map(|i| (i as f32).sin()).collect();
                                let b: Vec<f32> = (0..dimension).map(|i| (i as f32).cos()).collect();

                                for _ in 0..operations_per_thread {
                                    let result = compute.calculate_distance(&a, &b, &DistanceMetric::Cosine);
                                    black_box(result);
                                }
                            })
                        })
                        .collect();

                    for handle in handles {
                        handle.join().unwrap();
                    }
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_distance_metrics,
    bench_vector_operations,
    bench_hnsw_index,
    bench_lsh_index,
    bench_concurrent_operations
);
criterion_main!(benches);