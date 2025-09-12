//! Benchmarks for flush optimization strategies

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use proximadb::proto::proximadb::VectorRecord;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::runtime::Runtime;

/// Create test vectors for benchmarking
fn create_test_vectors(count: usize, dimension: usize) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| VectorRecord {
            id: Some(format!("vec_{}", i)),
            vector: vec![i as f32; dimension],
            metadata: vec![],
            timestamp: 0,
            updated_at: Some(0),
            expires_at: None,
            version: Some(1),
            distance: None,
            rank: None,
            score: None,
        })
        .collect()
}

/// Benchmark standard vector processing
fn bench_standard_processing(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    c.bench_function("standard_processing_1000", |b| {
        let vectors = create_test_vectors(1000, 128);

        b.iter(|| {
            let vectors_clone = vectors.clone();
            rt.block_on(async {
                // Simulate processing
                for vector in vectors_clone {
                    black_box(vector);
                }
            })
        });
    });
}

/// Benchmark batched vector processing
fn bench_batched_processing(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    c.bench_function("batched_processing_1000", |b| {
        let vectors = create_test_vectors(1000, 128);

        b.iter(|| {
            let vectors_clone = vectors.clone();

            rt.block_on(async {
                // Process in batches of 100
                for chunk in vectors_clone.chunks(100) {
                    let batch = chunk.to_vec();
                    black_box(batch);
                }
            })
        });
    });
}

/// Benchmark memory allocation patterns
fn bench_memory_patterns(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("memory_patterns");

    for pool_size_mb in [64, 128, 256].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(pool_size_mb),
            pool_size_mb,
            |b, &pool_size_mb| {
                let vectors = create_test_vectors(1000, 128);
                let pool_size = pool_size_mb * 1024 * 1024;

                b.iter(|| {
                    let vecs = vectors.clone();

                    rt.block_on(async move {
                        let mut allocated = 0usize;
                        for v in vecs {
                            allocated += v.vector.len() * 4;
                            if allocated > pool_size {
                                allocated = 0;
                            }
                            black_box(allocated);
                        }
                    })
                });
            },
        );
    }

    group.finish();
}

/// Benchmark parallel processing with different worker counts
fn bench_parallel_processing(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("parallel_processing");

    let worker_counts = vec![1, 2, 4, 8];

    for worker_count in worker_counts.iter() {
        let bench_id = BenchmarkId::from_parameter(worker_count);

        group.bench_with_input(bench_id, worker_count, |b, &worker_count| {
            let vectors = create_test_vectors(1000, 128);

            b.iter(|| {
                let vecs = vectors.clone();

                rt.block_on(async move {
                    // Simulate parallel processing
                    let chunk_size = vecs.len() / worker_count;
                    let mut handles = vec![];

                    for i in 0..worker_count {
                        let start = i * chunk_size;
                        let end = if i == worker_count - 1 {
                            vecs.len()
                        } else {
                            (i + 1) * chunk_size
                        };
                        let chunk = vecs[start..end].to_vec();

                        handles.push(tokio::spawn(async move {
                            for v in chunk {
                                black_box(v);
                            }
                        }));
                    }

                    for handle in handles {
                        let _ = handle.await;
                    }
                })
            });
        });
    }

    group.finish();
}

// Define the benchmark group
criterion_group!(
    benches,
    bench_standard_processing,
    bench_batched_processing,
    bench_memory_patterns,
    bench_parallel_processing
);

// Define the main entry point
criterion_main!(benches);
