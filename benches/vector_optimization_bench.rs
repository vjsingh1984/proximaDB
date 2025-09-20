//! Benchmark to measure the impact of vector optimization
//!
//! This benchmark compares:
//! 1. Original approach with Vec<f32> cloning
//! 2. Optimized approach with Arc<Vec<f32>> sharing

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use std::collections::HashMap;
use std::sync::Arc;

/// Original search result (with cloning)
#[derive(Clone)]
struct OriginalSearchResult {
    id: String,
    score: f32,
    vector: Option<Vec<f32>>,
    metadata: HashMap<String, serde_json::Value>,
}

/// Optimized search result (with Arc)
#[derive(Clone)]
struct OptimizedSearchResult {
    id: String,
    score: f32,
    vector: Option<Arc<Vec<f32>>>,
    metadata: HashMap<String, serde_json::Value>,
}

/// Simulate creating search results from a batch of vectors
fn bench_original_result_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("result_creation");

    for dimension in [128, 512, 1536].iter() {
        let vector = vec![0.1_f32; *dimension];
        let metadata: HashMap<String, serde_json::Value> = HashMap::new();

        group.bench_with_input(
            BenchmarkId::new("original", dimension),
            dimension,
            |b, _| {
                b.iter(|| {
                    let result = OriginalSearchResult {
                        id: "test_id".to_string(),
                        score: 0.95,
                        vector: Some(black_box(vector.clone())), // Expensive clone!
                        metadata: metadata.clone(),
                    };
                    black_box(result);
                });
            },
        );

        let arc_vector = Arc::new(vector);
        group.bench_with_input(
            BenchmarkId::new("optimized", dimension),
            dimension,
            |b, _| {
                b.iter(|| {
                    let result = OptimizedSearchResult {
                        id: "test_id".to_string(),
                        score: 0.95,
                        vector: Some(black_box(Arc::clone(&arc_vector))), // Cheap Arc clone!
                        metadata: metadata.clone(),
                    };
                    black_box(result);
                });
            },
        );
    }

    group.finish();
}

/// Simulate batch processing of search results
fn bench_batch_processing(c: &mut Criterion) {
    let mut group = c.benchmark_group("batch_processing");

    for batch_size in [100, 1000, 10000].iter() {
        let dimension = 512;
        let vectors: Vec<Vec<f32>> = (0..*batch_size)
            .map(|i| vec![i as f32 / 100.0; dimension])
            .collect();

        group.bench_with_input(
            BenchmarkId::new("original", batch_size),
            batch_size,
            |b, _| {
                b.iter(|| {
                    let results: Vec<OriginalSearchResult> = vectors
                        .iter()
                        .enumerate()
                        .map(|(i, v)| OriginalSearchResult {
                            id: format!("id_{}", i),
                            score: 0.9,
                            vector: Some(v.clone()), // Clone for each result
                            metadata: HashMap::new(),
                        })
                        .collect();
                    black_box(results);
                });
            },
        );

        let arc_vectors: Vec<Arc<Vec<f32>>> = vectors.iter().map(|v| Arc::new(v.clone())).collect();

        group.bench_with_input(
            BenchmarkId::new("optimized", batch_size),
            batch_size,
            |b, _| {
                b.iter(|| {
                    let results: Vec<OptimizedSearchResult> = arc_vectors
                        .iter()
                        .enumerate()
                        .map(|(i, v)| OptimizedSearchResult {
                            id: format!("id_{}", i),
                            score: 0.9,
                            vector: Some(Arc::clone(v)), // Cheap Arc clone
                            metadata: HashMap::new(),
                        })
                        .collect();
                    black_box(results);
                });
            },
        );
    }

    group.finish();
}

/// Simulate result sharing across threads
fn bench_result_sharing(c: &mut Criterion) {
    let mut group = c.benchmark_group("result_sharing");

    let dimension = 1536; // OpenAI embedding size
    let vector = vec![0.1_f32; dimension];

    group.bench_function("original_10_clones", |b| {
        b.iter(|| {
            let original = OriginalSearchResult {
                id: "test".to_string(),
                score: 0.95,
                vector: Some(vector.clone()),
                metadata: HashMap::new(),
            };

            // Simulate sharing across 10 threads/contexts
            let clones: Vec<_> = (0..10).map(|_| original.clone()).collect();
            black_box(clones);
        });
    });

    let arc_vector = Arc::new(vector);
    group.bench_function("optimized_10_clones", |b| {
        b.iter(|| {
            let optimized = OptimizedSearchResult {
                id: "test".to_string(),
                score: 0.95,
                vector: Some(Arc::clone(&arc_vector)),
                metadata: HashMap::new(),
            };

            // Simulate sharing across 10 threads/contexts
            let clones: Vec<_> = (0..10).map(|_| optimized.clone()).collect();
            black_box(clones);
        });
    });

    group.finish();
}

/// Measure memory allocation impact
fn bench_memory_pressure(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_pressure");

    // Simulate a large result set
    let num_results = 10000;
    let dimension = 512;

    group.bench_function("original_memory", |b| {
        b.iter(|| {
            let mut results = Vec::with_capacity(num_results);
            for i in 0..num_results {
                let vector = vec![i as f32 / 1000.0; dimension];
                results.push(OriginalSearchResult {
                    id: format!("id_{}", i),
                    score: 0.9,
                    vector: Some(vector), // Each result owns its vector
                    metadata: HashMap::new(),
                });
            }
            black_box(results);
        });
    });

    group.bench_function("optimized_memory", |b| {
        b.iter(|| {
            let mut results = Vec::with_capacity(num_results);
            // Pre-create Arc vectors (simulating a shared pool)
            let vectors: Vec<Arc<Vec<f32>>> =
                (0..100) // Reuse 100 vectors
                    .map(|i| Arc::new(vec![i as f32 / 100.0; dimension]))
                    .collect();

            for i in 0..num_results {
                results.push(OptimizedSearchResult {
                    id: format!("id_{}", i),
                    score: 0.9,
                    vector: Some(Arc::clone(&vectors[i % 100])), // Share vectors
                    metadata: HashMap::new(),
                });
            }
            black_box(results);
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_original_result_creation,
    bench_batch_processing,
    bench_result_sharing,
    bench_memory_pressure
);
criterion_main!(benches);
