//! Comprehensive benchmarks for ProximaDB optimization features
//!
//! Currently testing:
//! - Basic vector operations performance
//! 
//! TODO: Re-enable additional benchmarks when modules are available:
//! - Vector serialization (bytemuck vs bincode)
//! - Compression algorithms (ZSTD, LZ4, raw)
//! - Memory pooling efficiency
//! - Fixed-length vs dynamic vectors
//! - SST vs VIPER engine performance

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use proximadb::proto::proximadb::{VectorRecord, MetadataItem};
use proximadb::core::search::results::OptimizedSearchRecord;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::sync::Arc;

/// Generate test vectors with specified characteristics
fn generate_test_vectors(count: usize, dimension: usize, sparsity: f32) -> Vec<Vec<f32>> {
    let mut rng = ChaCha8Rng::seed_from_u64(42);

    (0..count)
        .map(|_| {
            let mut vector = vec![0.0; dimension];
            let non_zero_count = ((1.0 - sparsity) * dimension as f32) as usize;

            for _ in 0..non_zero_count {
                let idx = rng.gen_range(0..dimension);
                vector[idx] = rng.gen_range(-1.0..1.0);
            }

            vector
        })
        .collect()
}

/// Create test VectorRecord
fn create_vector_record(id: &str, vector: Vec<f32>) -> VectorRecord {
    VectorRecord {
        id: id.to_string(),
        vector,
        metadata: vec![MetadataItem {
            key: "category".to_string(),
            value: Some(
                proximadb::proto::proximadb::metadata_item::Value::StringValue(
                    "benchmark".to_string(),
                ),
            ),
        }],
        timestamp: 1234567890,
        updated_at: Some(1234567890),
        expires_at: None,
        version: Some(1),
        quantized_vector: None,
        source: None,
    }
}

/// Create test OptimizedSearchRecord
fn create_optimized_record(id: &str, vector: Vec<f32>, score: f32) -> OptimizedSearchRecord {
    let mut record = OptimizedSearchRecord::new(id.to_string(), score);
    record.add_vector(Arc::new(vector));
    record
}

/// Benchmark OptimizedSearchRecord vs VectorRecord cloning
fn bench_record_cloning(c: &mut Criterion) {
    let mut group = c.benchmark_group("record_cloning");
    
    let vectors = generate_test_vectors(100, 512, 0.3);
    
    // Create VectorRecords
    let vector_records: Vec<VectorRecord> = vectors
        .iter()
        .enumerate()
        .map(|(i, v)| create_vector_record(&format!("test_{}", i), v.clone()))
        .collect();
    
    // Create OptimizedSearchRecords
    let optimized_records: Vec<OptimizedSearchRecord> = vectors
        .iter()
        .enumerate()
        .map(|(i, v)| create_optimized_record(&format!("test_{}", i), v.clone(), i as f32))
        .collect();
    
    // Benchmark VectorRecord cloning (deep clone)
    group.throughput(Throughput::Elements(vector_records.len() as u64));
    group.bench_function("vector_record_clone", |b| {
        b.iter(|| {
            let mut clones = Vec::with_capacity(vector_records.len());
            for record in &vector_records {
                clones.push(record.clone());
            }
            clones
        });
    });
    
    // Benchmark OptimizedSearchRecord cloning (Arc-based, O(1))
    group.throughput(Throughput::Elements(optimized_records.len() as u64));
    group.bench_function("optimized_record_clone", |b| {
        b.iter(|| {
            let mut clones = Vec::with_capacity(optimized_records.len());
            for record in &optimized_records {
                clones.push(record.clone());
            }
            clones
        });
    });
    
    group.finish();
}

/// Benchmark memory sharing with Arc
fn bench_arc_sharing(c: &mut Criterion) {
    let mut group = c.benchmark_group("arc_sharing");
    
    let dimension = 1536; // OpenAI embedding size
    let vectors = generate_test_vectors(50, dimension, 0.0);
    
    // Without Arc - each clone duplicates data
    let plain_vectors = vectors.clone();
    group.bench_function("without_arc_10_clones", |b| {
        b.iter(|| {
            let mut all_clones = Vec::new();
            for vector in &plain_vectors {
                for _ in 0..10 {
                    all_clones.push(vector.clone());
                }
            }
            all_clones
        });
    });
    
    // With Arc - shared references
    let arc_vectors: Vec<Arc<Vec<f32>>> = vectors
        .into_iter()
        .map(Arc::new)
        .collect();
    
    group.bench_function("with_arc_10_clones", |b| {
        b.iter(|| {
            let mut all_clones = Vec::new();
            for vector in &arc_vectors {
                for _ in 0..10 {
                    all_clones.push(Arc::clone(vector));
                }
            }
            all_clones
        });
    });
    
    group.finish();
}

/// Benchmark result aggregation patterns
fn bench_result_aggregation(c: &mut Criterion) {
    let mut group = c.benchmark_group("result_aggregation");
    
    let vectors = generate_test_vectors(500, 256, 0.2);
    
    // Create search results
    let search_results: Vec<OptimizedSearchRecord> = vectors
        .iter()
        .enumerate()
        .map(|(i, v)| {
            create_optimized_record(&format!("result_{}", i), v.clone(), (500 - i) as f32)
        })
        .collect();
    
    // Benchmark sorting and top-k selection
    group.bench_function("top_100_selection", |b| {
        b.iter(|| {
            let mut results = search_results.clone();
            results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
            results.truncate(100);
            results
        });
    });
    
    // Benchmark with pre-allocated capacity
    group.bench_function("top_100_preallocated", |b| {
        b.iter(|| {
            let mut results = Vec::with_capacity(100);
            let mut sorted = search_results.clone();
            sorted.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
            results.extend_from_slice(&sorted[..100.min(sorted.len())]);
            results
        });
    });
    
    group.finish();
}

criterion_group!(
    benches,
    bench_record_cloning,
    bench_arc_sharing,
    bench_result_aggregation
);
criterion_main!(benches);