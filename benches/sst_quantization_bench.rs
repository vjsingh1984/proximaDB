//! SST Quantization Performance Benchmarks
//!
//! Benchmarks for measuring the performance of SST quantization operations including:
//! - Quantization speed for different vector dimensions
//! - Progressive search performance
//! - Memory usage and pooling efficiency
//! - Compression ratios
//! - Distance table precomputation overhead

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use std::sync::Arc;
use std::time::Duration;

use proximadb::compute::distance_computation::engine::{DistanceMetric, UnifiedDistanceCompute};
use proximadb::compute::quantization::unified::{
    InMemoryCodebookStore, ProductQuantization, QuantizationLevelType, UnifiedQuantizationEngine,
    UnifiedQuantizationLevel,
};
use proximadb::compute::quantization::{
    SearchStage, StorageQuantizationConfig, StorageQuantizationEngine, StorageQuantizedData,
};
use proximadb::core::memory::pool::VectorMemoryPool;

/// Generate random vectors for benchmarking
fn generate_vectors(count: usize, dim: usize) -> Vec<Vec<f32>> {
    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};

    let mut rng = StdRng::seed_from_u64(42);
    let mut vectors = Vec::with_capacity(count);

    for _ in 0..count {
        let mut vec = vec![0.0f32; dim];
        for val in &mut vec {
            *val = rng.gen_range(-1.0..1.0);
        }

        // Normalize
        let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for val in &mut vec {
                *val /= norm;
            }
        }

        vectors.push(vec);
    }

    vectors
}

/// Benchmark quantization speed for different dimensions
fn bench_quantization_speed(c: &mut Criterion) {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let mut group = c.benchmark_group("quantization_speed");
    group.measurement_time(Duration::from_secs(10));

    // Test different dimensions
    for dim in &[128, 256, 384, 512, 768, 1536] {
        let vectors = generate_vectors(1000, *dim);

        // Create engine
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let engine = runtime.block_on(async {
            let distance_compute = Arc::new(UnifiedDistanceCompute::default());
            let codebook_store = Arc::new(InMemoryCodebookStore::new());
            let unified_engine = Arc::new(UnifiedQuantizationEngine::new(
                distance_compute.clone(),
                codebook_store,
            ));

            let config = StorageQuantizationConfig::default();
            Arc::new(StorageQuantizationEngine::new(
                unified_engine,
                distance_compute,
                config,
            ))
        });

        group.throughput(Throughput::Elements(vectors.len() as u64));
        group.bench_with_input(BenchmarkId::new("dimension", dim), dim, |b, _| {
            b.to_async(&runtime).iter(|| async {
                let result = engine.quantize_batch(&vectors, None).await.unwrap();
                black_box(result);
            });
        });
    }

    group.finish();
}

/// Benchmark progressive search performance
fn bench_progressive_search(c: &mut Criterion) {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let mut group = c.benchmark_group("progressive_search");
    group.measurement_time(Duration::from_secs(10));

    // Test different dataset sizes
    for size in &[1000, 5000, 10000] {
        let vectors = generate_vectors(*size, 384);

        let runtime = tokio::runtime::Runtime::new().unwrap();

        // Setup: Create engine and quantize vectors
        let (engine, quantized) = runtime.block_on(async {
            let distance_compute = Arc::new(UnifiedDistanceCompute::default());
            let codebook_store = Arc::new(InMemoryCodebookStore::new());
            let unified_engine = Arc::new(UnifiedQuantizationEngine::new(
                distance_compute.clone(),
                codebook_store,
            ));

            let config = StorageQuantizationConfig::default();
            let engine = Arc::new(StorageQuantizationEngine::new(
                unified_engine,
                distance_compute,
                config,
            ));

            let quantized = engine.quantize_batch(&vectors, None).await.unwrap();
            (engine, quantized)
        });

        let query = vectors[0].clone();

        group.throughput(Throughput::Elements(1));
        group.bench_with_input(BenchmarkId::new("dataset_size", size), size, |b, _| {
            b.to_async(&runtime).iter(|| async {
                let stages = engine
                    .progressive_search(&query, &quantized, 10, &DistanceMetric::Cosine)
                    .await
                    .unwrap();
                black_box(stages);
            });
        });
    }

    group.finish();
}

/// Benchmark memory pool efficiency
fn bench_memory_pool(c: &mut Criterion) {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let mut group = c.benchmark_group("memory_pool");
    group.measurement_time(Duration::from_secs(5));

    let memory_pool = Arc::new(VectorMemoryPool::new());
    let vectors = generate_vectors(100, 384);

    group.bench_function("with_pooling", |b| {
        b.iter(|| {
            let mut buffer = memory_pool.serialization_buffers/* TODO: Fix VectorMemoryPool::acquire() method */;
            // Simulate serialization
            for vec in &vectors {
                for val in vec {
                    buffer.extend_from_slice(&val.to_le_bytes());
                }
            }
            black_box(buffer.len());
            // Buffer returns to pool when dropped
        });
    });

    group.bench_function("without_pooling", |b| {
        b.iter(|| {
            let mut buffer = Vec::with_capacity(100 * 384 * 4);

            // Simulate serialization
            for vec in &vectors {
                for val in vec {
                    buffer.extend_from_slice(&val.to_le_bytes());
                }
            }

            black_box(buffer.len());
        });
    });

    group.finish();
}

/// Benchmark PQ distance table precomputation
fn bench_pq_distance_tables(c: &mut Criterion) {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let mut group = c.benchmark_group("pq_distance_tables");
    group.measurement_time(Duration::from_secs(10));

    // Test different PQ configurations
    for (subvectors, bits) in &[(8, 8), (16, 8), (32, 8), (16, 4)] {
        let vectors = generate_vectors(1000, 512);

        let runtime = tokio::runtime::Runtime::new().unwrap();

        // Setup: Create engine with PQ configuration
        let (engine, quantized) = runtime.block_on(async {
            let mut config = StorageQuantizationConfig::default();
            config.primary_level = Some(UnifiedQuantizationLevel {
                level_type: Some(QuantizationLevelType::Pq(ProductQuantization {
                    num_subvectors: *subvectors,
                    bits_per_code: *bits,
                    codebook_id: None,
                    adaptive_subvectors: false,
                })),
            });

            let distance_compute = Arc::new(UnifiedDistanceCompute::default());
            let codebook_store = Arc::new(InMemoryCodebookStore::new());
            let unified_engine = Arc::new(UnifiedQuantizationEngine::new(
                distance_compute.clone(),
                codebook_store,
            ));

            let mut engine =
                StorageQuantizationEngine::new(unified_engine, distance_compute, config);

            // Train PQ
            engine.train(&vectors).await.unwrap();
            let quantized = engine.quantize_batch(&vectors, None).await.unwrap();

            (Arc::new(engine), quantized)
        });

        let query = vectors[0].clone();
        let config_name = format!("pq_{}_{}", subvectors, bits);

        group.bench_with_input(
            BenchmarkId::new("config", &config_name),
            &config_name,
            |b, _| {
                b.to_async(&runtime).iter(|| async {
                    let stages = engine
                        .progressive_search(&query, &quantized, 10, &DistanceMetric::L2)
                        .await
                        .unwrap();

                    // Find PQ ranking stage
                    let pq_stage = stages.iter().find(|s| s.stage == SearchStage::PQRanking);

                    black_box(pq_stage);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark compression ratios
fn bench_compression_ratios(c: &mut Criterion) {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let mut group = c.benchmark_group("compression_ratios");
    group.measurement_time(Duration::from_secs(5));

    let runtime = tokio::runtime::Runtime::new().unwrap();

    for dim in &[256, 512, 768, 1536] {
        let vectors = generate_vectors(1000, *dim);
        let original_size = vectors.len() * *dim * 4; // f32 = 4 bytes

        let engine = runtime.block_on(async {
            let distance_compute = Arc::new(UnifiedDistanceCompute::default());
            let codebook_store = Arc::new(InMemoryCodebookStore::new());
            let unified_engine = Arc::new(UnifiedQuantizationEngine::new(
                distance_compute.clone(),
                codebook_store,
            ));

            let config = StorageQuantizationConfig::default();
            Arc::new(StorageQuantizationEngine::new(
                unified_engine,
                distance_compute,
                config,
            ))
        });

        group.bench_with_input(BenchmarkId::new("dimension", dim), dim, |b, _| {
            b.to_async(&runtime).iter(|| async {
                let quantized = engine.quantize_batch(&vectors, None).await.unwrap();
                let savings = engine.calculate_savings(original_size, &quantized);
                black_box(savings);
            });
        });
    }

    group.finish();
}

/// Benchmark binary filtering efficiency
fn bench_binary_filtering(c: &mut Criterion) {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

    let mut group = c.benchmark_group("binary_filtering");
    group.measurement_time(Duration::from_secs(10));

    let runtime = tokio::runtime::Runtime::new().unwrap();

    for size in &[1000, 5000, 10000] {
        let vectors = generate_vectors(*size, 384);

        let (engine, quantized) = runtime.block_on(async {
            let distance_compute = Arc::new(UnifiedDistanceCompute::default());
            let codebook_store = Arc::new(InMemoryCodebookStore::new());
            let unified_engine = Arc::new(UnifiedQuantizationEngine::new(
                distance_compute.clone(),
                codebook_store,
            ));

            let config = StorageQuantizationConfig {
                enable_progressive: true,
                filter_threshold: 0.3,
                ..Default::default()
            };

            let engine = Arc::new(StorageQuantizationEngine::new(
                unified_engine,
                distance_compute,
                config,
            ));

            let quantized = engine.quantize_batch(&vectors, None).await.unwrap();
            (engine, quantized)
        });

        let query = vectors[0].clone();

        group.bench_with_input(BenchmarkId::new("candidates", size), size, |b, _| {
            b.to_async(&runtime).iter(|| async {
                let stages = engine
                    .progressive_search(&query, &quantized, 10, &DistanceMetric::Cosine)
                    .await
                    .unwrap();

                let binary_stage = stages.iter().find(|s| s.stage == SearchStage::BinaryFilter);

                black_box(binary_stage);
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_quantization_speed,
    bench_progressive_search,
    bench_memory_pool,
    bench_pq_distance_tables,
    bench_compression_ratios,
    bench_binary_filtering
);

criterion_main!(benches);
