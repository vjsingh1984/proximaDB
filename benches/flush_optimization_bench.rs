//! Benchmarks for flush optimization strategies
use proximadb::core::hardware_capabilities;

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use proximadb::proto::proximadb_v1::{VectorRecord, SqlValue};
use std::collections::HashMap;
use std::sync::{Arc, Once};

/// Global initialization for hardware capabilities
static INIT: Once = Once::new();

/// Initialize hardware capabilities once for all benchmarks
fn init_hardware() {
    INIT.call_once(|| {
        let _ = hardware_capabilities::initialize_hardware_capabilities_default();
    });
}

/// Create test vectors for benchmarking
fn create_test_vectors(count: usize, dimension: usize) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| VectorRecord {
            id: format!("vec_{}", i),
            vector: vec![i as f32; dimension],
            metadata: std::collections::HashMap::new(),
            timestamp: 0,
            updated_at: Some(0),
            expires_at: None,
            version: Some(1),
            quantized_vector: vec![],
            source: None,
        })
        .collect()
}

/// Benchmark standard vector processing
fn bench_standard_processing(c: &mut Criterion) {
    init_hardware();

    c.bench_function("standard_processing_1000", |b| {
        let vectors = create_test_vectors(1000, 128);

        b.iter(|| {
            let vectors_clone = vectors.clone();
            // Simulate processing synchronously to avoid async runtime overhead
            for vector in vectors_clone {
                black_box(vector);
            }
        });
    });
}

/// Benchmark batched vector processing
fn bench_batched_processing(c: &mut Criterion) {
    init_hardware();

    c.bench_function("batched_processing_1000", |b| {
        let vectors = create_test_vectors(1000, 128);

        b.iter(|| {
            let vectors_clone = vectors.clone();

            // Process in batches of 100 synchronously
            for chunk in vectors_clone.chunks(100) {
                let batch = chunk.to_vec();
                black_box(batch);
            }
        });
    });
}

/// Benchmark memory allocation patterns
fn bench_memory_patterns(c: &mut Criterion) {
    init_hardware();

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

                    // Simulate memory allocation patterns synchronously
                    let mut allocated = 0usize;
                    for v in vecs {
                        allocated += v.vector.len() * 4;
                        if allocated > pool_size {
                            allocated = 0;
                        }
                        black_box(allocated);
                    }
                });
            },
        );
    }

    group.finish();
}

/// Benchmark parallel processing with different worker counts
fn bench_parallel_processing(c: &mut Criterion) {
    init_hardware();

    let mut group = c.benchmark_group("parallel_processing");

    let worker_counts = vec![1, 2, 4, 8];

    for worker_count in worker_counts.iter() {
        let bench_id = BenchmarkId::from_parameter(worker_count);

        group.bench_with_input(bench_id, worker_count, |b, &worker_count| {
            let vectors = create_test_vectors(1000, 128);

            b.iter(|| {
                let vecs = vectors.clone();

                // Use rayon for parallel processing instead of tokio
                use rayon::prelude::*;

                let chunk_size = vecs.len() / worker_count;
                vecs.par_chunks(chunk_size)
                    .for_each(|chunk| {
                        for v in chunk {
                            black_box(v);
                        }
                    });
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
