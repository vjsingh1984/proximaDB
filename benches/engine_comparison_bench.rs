// Benchmarks comparing all 4 storage engines
// Measures performance across different workloads

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use proximadb::{
    compute::distance_computation::DistanceMetric,
    core::{VectorRecord, hardware_capabilities},
    storage::{
        engines::{
            StorageEngineFactory, WorkloadType, dsst::DsstEngine, dviper::DviperEngine,
            sst::SstStorage, viper::ViperEngine,
        },
        traits::{CompactionParameters, FlushParameters, UnifiedStorageEngine},
    },
};
use std::sync::Arc;
use tokio::runtime::Runtime;

/// Generate test vectors
fn generate_vectors(count: usize, dimension: usize) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| VectorRecord {
            id: Some(format!("vec_{:08}", i)),
            vector: vec![i as f32 / count as f32; dimension],
            metadata: None,
            timestamp: i as i64,
            updated_at: None,
            expires_at: None,
            version: Some(1),
        })
        .collect()
}

/// Benchmark flush operation
fn bench_flush_operation(c: &mut Criterion) {
    let _ = hardware_capabilities::initialize_hardware_capabilities_default();
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("flush_operation");

    // Test different vector counts
    for count in [100, 1000, 10000].iter() {
        let vectors = generate_vectors(*count, 768);
        let params = FlushParameters {
            collection_id: "bench_collection".to_string(),
            num_vectors: *count,
            estimated_size: *count * 768 * 4, // Approximate size
            dimension: Some(768),
            distance_metric: Some(DistanceMetric::Euclidean),
            metadata: None,
            collection_config: None,
        };

        // Benchmark SST
        group.bench_with_input(BenchmarkId::new("SST", count), count, |b, _| {
            let engine = rt.block_on(async { SstStorage::new().unwrap() });

            b.to_async(&rt)
                .iter(|| async { engine.do_flush(&params).await.unwrap() });
        });

        // Benchmark VIPER
        group.bench_with_input(BenchmarkId::new("VIPER", count), count, |b, _| {
            let engine = rt.block_on(async { ViperEngine::new().unwrap() });

            b.to_async(&rt)
                .iter(|| async { engine.do_flush(&params).await.unwrap() });
        });

        // Benchmark DSST
        group.bench_with_input(BenchmarkId::new("DSST", count), count, |b, _| {
            let engine = rt.block_on(async { DsstEngine::new().unwrap() });

            b.to_async(&rt)
                .iter(|| async { engine.do_flush(&params).await.unwrap() });
        });

        // Benchmark DVIPER
        group.bench_with_input(BenchmarkId::new("DVIPER", count), count, |b, _| {
            let engine = rt.block_on(async { DviperEngine::new().unwrap() });

            b.to_async(&rt)
                .iter(|| async { engine.do_flush(&params).await.unwrap() });
        });
    }

    group.finish();
}

/// Benchmark search operation
fn bench_search_operation(c: &mut Criterion) {
    let _ = hardware_capabilities::initialize_hardware_capabilities_default();
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("search_operation");

    let query = vec![0.5; 768];
    let top_k = 10;

    // Test different collection sizes
    for size in [1000, 10000].iter() {
        // Benchmark SST
        group.bench_with_input(BenchmarkId::new("SST", size), size, |b, _| {
            let engine = rt.block_on(async { SstStorage::new().unwrap() });

            b.to_async(&rt).iter(|| async {
                engine
                    .search_vectors_unified(
                        "bench_collection",
                        "memory://test",
                        &query,
                        top_k,
                        DistanceMetric::Euclidean,
                        None,
                        None,
                        None,
                    )
                    .await
                    .unwrap()
            });
        });

        // Benchmark VIPER
        group.bench_with_input(BenchmarkId::new("VIPER", size), size, |b, _| {
            let engine = rt.block_on(async { ViperEngine::new().unwrap() });

            b.to_async(&rt).iter(|| async {
                engine
                    .search_vectors_unified(
                        "bench_collection",
                        "memory://test",
                        &query,
                        top_k,
                        DistanceMetric::Euclidean,
                        None,
                        None,
                        None,
                    )
                    .await
                    .unwrap()
            });
        });

        // Benchmark DSST (with progressive search)
        group.bench_with_input(BenchmarkId::new("DSST", size), size, |b, _| {
            let engine = rt.block_on(async { DsstEngine::new().unwrap() });

            b.to_async(&rt).iter(|| async {
                engine
                    .search_vectors_unified(
                        "bench_collection",
                        "memory://test",
                        &query,
                        top_k,
                        DistanceMetric::Euclidean,
                        None,
                        None,
                        None,
                    )
                    .await
                    .unwrap()
            });
        });

        // Benchmark DVIPER (with columnar optimization)
        group.bench_with_input(BenchmarkId::new("DVIPER", size), size, |b, _| {
            let engine = rt.block_on(async { DviperEngine::new().unwrap() });

            b.to_async(&rt).iter(|| async {
                engine
                    .search_vectors_unified(
                        "bench_collection",
                        "memory://test",
                        &query,
                        top_k,
                        DistanceMetric::Euclidean,
                        None,
                        None,
                        Some(serde_json::json!({
                            "enable_projection": true,
                            "enable_pushdown": true,
                        })),
                    )
                    .await
                    .unwrap()
            });
        });
    }

    group.finish();
}

/// Benchmark ID lookup operation (simulating AXIS returns)
fn bench_id_lookup(c: &mut Criterion) {
    let _ = hardware_capabilities::initialize_hardware_capabilities_default();
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("id_lookup");

    // Simulate AXIS returning IDs
    let ids = vec![
        "vec_00001000",
        "vec_00002000",
        "vec_00003000",
        "vec_00004000",
        "vec_00005000",
    ];

    for id in ids.iter() {
        // Benchmark SST
        group.bench_with_input(BenchmarkId::new("SST", id), id, |b, id| {
            let engine = rt.block_on(async {
                Arc::new(SstStorage::new().unwrap()) as Arc<dyn UnifiedStorageEngine>
            });

            b.to_async(&rt).iter(|| async {
                engine
                    .get_vector_by_id("bench_collection", id)
                    .await
                    .unwrap()
            });
        });

        // Benchmark DSST (optimized for ID lookups)
        group.bench_with_input(BenchmarkId::new("DSST", id), id, |b, id| {
            let engine = rt.block_on(async {
                Arc::new(DsstEngine::new().unwrap()) as Arc<dyn UnifiedStorageEngine>
            });

            b.to_async(&rt).iter(|| async {
                engine
                    .get_vector_by_id("bench_collection", id)
                    .await
                    .unwrap()
            });
        });

        // Benchmark DVIPER
        group.bench_with_input(BenchmarkId::new("DVIPER", id), id, |b, id| {
            let engine = rt.block_on(async {
                Arc::new(DviperEngine::new().unwrap()) as Arc<dyn UnifiedStorageEngine>
            });

            b.to_async(&rt).iter(|| async {
                engine
                    .get_vector_by_id("bench_collection", id)
                    .await
                    .unwrap()
            });
        });
    }

    group.finish();
}

/// Benchmark compaction operation
fn bench_compaction(c: &mut Criterion) {
    let _ = hardware_capabilities::initialize_hardware_capabilities_default();
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("compaction");

    let params = CompactionParameters {
        collection_id: "bench_collection".to_string(),
        compaction_level: 1,
        estimated_input_size: 100 * 1024 * 1024,  // 100MB
        max_output_file_size: 1024 * 1024 * 1024, // 1GB
        collection_config: None,
    };

    // Benchmark SST compaction
    group.bench_function("SST", |b| {
        let engine = rt.block_on(async { SstStorage::new().unwrap() });

        b.to_async(&rt)
            .iter(|| async { engine.do_compact(&params).await.unwrap() });
    });

    // Benchmark VIPER compaction
    group.bench_function("VIPER", |b| {
        let engine = rt.block_on(async { ViperEngine::new().unwrap() });

        b.to_async(&rt)
            .iter(|| async { engine.do_compact(&params).await.unwrap() });
    });

    // Benchmark DSST compaction
    group.bench_function("DSST", |b| {
        let engine = rt.block_on(async { DsstEngine::new().unwrap() });

        b.to_async(&rt)
            .iter(|| async { engine.do_compact(&params).await.unwrap() });
    });

    // Benchmark DVIPER compaction (columnar merge)
    group.bench_function("DVIPER", |b| {
        let engine = rt.block_on(async { DviperEngine::new().unwrap() });

        b.to_async(&rt)
            .iter(|| async { engine.do_compact(&params).await.unwrap() });
    });

    group.finish();
}

/// Benchmark memory usage
fn bench_memory_usage(c: &mut Criterion) {
    let _ = hardware_capabilities::initialize_hardware_capabilities_default();
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("memory_usage");

    // Measure memory footprint of each engine
    group.bench_function("SST_baseline", |b| {
        b.iter(|| {
            let _engine = SstStorage::new().unwrap();
            black_box(std::mem::size_of::<SstStorage>())
        });
    });

    group.bench_function("VIPER_baseline", |b| {
        b.iter(|| {
            let _engine = ViperEngine::new().unwrap();
            black_box(std::mem::size_of::<ViperEngine>())
        });
    });

    group.bench_function("DSST_baseline", |b| {
        b.iter(|| {
            let _engine = DsstEngine::new().unwrap();
            black_box(std::mem::size_of::<DsstEngine>())
        });
    });

    group.bench_function("DVIPER_baseline", |b| {
        b.iter(|| {
            let _engine = DviperEngine::new().unwrap();
            black_box(std::mem::size_of::<DviperEngine>())
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_flush_operation,
    bench_search_operation,
    bench_id_lookup,
    bench_compaction,
    bench_memory_usage
);

criterion_main!(benches);
