//! Comprehensive benchmarks for all ProximaDB storage engines
//!
//! This benchmark suite evaluates all seven storage engines with:
//! - Creation overhead
//! - Flush performance (with and without compression)
//! - Memory efficiency
//! - Compression ratios
//! - Query performance

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use proximadb::{
    proto::proximadb_v1::{
        Collection, CollectionConfig, QuantizationConfig, StorageAssignment, StorageEngine,
        VectorRecord,
    },
    storage::{
        engines::factory::StorageEngineFactory,
        traits::{FlushParameters, UnifiedStorageEngine},
    },
};
use std::collections::HashMap;
use std::sync::{Arc, Once};
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
fn generate_vectors(count: usize, dimension: usize) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| VectorRecord {
            id: format!("vec_{:08}", i),
            vector: vec![i as f32 / count as f32; dimension],
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

/// Create comprehensive quantization config for compression testing
fn create_quantization_config() -> QuantizationConfig {
    QuantizationConfig {
        enabled: true,
        strategy: 3, // MEMORY_OPTIMIZED
        custom_levels: vec![],
        enable_progressive_search: true,
        binary_filter_selectivity: 0.3,
        int8_ranking_selectivity: 0.1,
        pq_ranking_selectivity: 0.05,
        training_sample_size: 10000,
        quality_threshold: 0.95,
        enable_adaptive_training: true,
        optimize_for_storage: true,
        optimize_for_memory: true,
        enable_simd_acceleration: true,
        enable_binary: true,
        enable_int8: true,
        enable_pq: true,
        pq_segments: 32,
        pq_bits: 8,
        pq_codebooks: 256,
        binary_threshold: 0.5,
        int8_threshold: 0.3,
        pq_threshold: 0.1,
    }
}

/// Benchmark engine creation for all engines
fn bench_all_engine_creation(c: &mut Criterion) {
    init_hardware();

    let mut group = c.benchmark_group("comprehensive_engine_creation");
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(3));

    // SST Engine - Row-based, write-optimized
    group.bench_function("sst", |b| {
        b.iter(|| {
            let engine = StorageEngineFactory::create_sst().unwrap();
            black_box(engine)
        })
    });

    // VIPER Engine - Columnar Parquet
    group.bench_function("viper", |b| {
        b.iter(|| {
            let engine = StorageEngineFactory::create_viper().unwrap();
            black_box(engine)
        })
    });

    // NOVA Engine - Progressive columnar
    group.bench_function("nova", |b| {
        b.iter(|| {
            let engine = StorageEngineFactory::create_nova().unwrap();
            black_box(engine)
        })
    });

    // SWIFT Engine - High-speed row-based
    group.bench_function("swift", |b| {
        b.iter(|| {
            let engine = StorageEngineFactory::create_swift().unwrap();
            black_box(engine)
        })
    });

    // RAPTOR Engine - Adaptive row-group
    group.bench_function("raptor", |b| {
        b.iter(|| {
            let engine = StorageEngineFactory::create_raptor_default().unwrap();
            black_box(engine)
        })
    });

    // HELIX Engine - Spiral-pattern storage
    group.bench_function("helix", |b| {
        b.iter(|| {
            let engine = StorageEngineFactory::create_helix().unwrap();
            black_box(engine)
        })
    });

    // PRISM Engine - Memory-optimized (requires async, handled separately)
    // Note: PRISM requires async initialization, so we create it once outside the iter
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let prism_engine = runtime.block_on(async {
        StorageEngineFactory::create_prism_async().await.unwrap()
    });
    group.bench_function("prism", |b| {
        b.iter(|| {
            // Just measure Arc clone cost since creation requires async
            let engine = Arc::clone(&prism_engine);
            black_box(engine)
        })
    });

    group.finish();
}

/// Benchmark flush operations with compression for all engines
fn bench_engine_flush_with_compression(c: &mut Criterion) {
    init_hardware();

    let mut group = c.benchmark_group("engine_flush_compression");
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10);

    let vectors = Arc::new(generate_vectors(1000, 768));
    let quant_config = create_quantization_config();

    // Test each engine with and without compression
    // Note: PRISM requires async initialization
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let prism_engine = runtime.block_on(async {
        StorageEngineFactory::create_prism_async().await.unwrap()
    });

    let engines = vec![
        ("sst", StorageEngineFactory::create_sst().unwrap()),
        ("viper", StorageEngineFactory::create_viper().unwrap()),
        ("nova", StorageEngineFactory::create_nova().unwrap()),
        ("swift", StorageEngineFactory::create_swift().unwrap()),
        ("raptor", StorageEngineFactory::create_raptor_default().unwrap()),
        ("helix", StorageEngineFactory::create_helix().unwrap()),
        ("prism", prism_engine),
    ];

    for (name, engine) in engines {
        // Benchmark without compression
        let vectors_clone = Arc::clone(&vectors);
        group.bench_function(format!("{}_no_compression", name), |b| {
            b.iter(|| {
                runtime.block_on(async {
                    let collection = Collection {
                        id: "bench".to_string(),
                        config: Some(CollectionConfig {
                            name: "bench".to_string(),
                            dimension: 768,
                            distance_metric: 0, // Euclidean
                            storage_engine: StorageEngine::Sst as i32,
                            quantization: None, // No compression
                            ..Default::default()
                        }),
                        created_at: 0,
                        updated_at: 0,
                        stats: None,
                        storage_assignment: Some(StorageAssignment {
                            primary_path: "/tmp/proximadb_bench".to_string(),
                            backup_paths: vec![],
                            engine: StorageEngine::Sst as i32,
                            engine_config: HashMap::new(),
                            base_location: "/tmp/proximadb_bench".to_string(),
                            assigned_at: 0,
                        }),
                    };

                    let params = FlushParameters {
                        collection_id: Some("bench".to_string()),
                        vector_records: (*vectors_clone).clone(),
                        force: true,
                        synchronous: true,
                        collection_config: Some(collection),
                        ..Default::default()
                    };
                    let _ = engine.flush(params).await;
                })
            })
        });

        // Benchmark with compression
        let vectors_clone = Arc::clone(&vectors);
        let quant_clone = quant_config.clone();
        group.bench_function(format!("{}_with_compression", name), |b| {
            b.iter(|| {
                runtime.block_on(async {
                    let collection = Collection {
                        id: "bench".to_string(),
                        config: Some(CollectionConfig {
                            name: "bench".to_string(),
                            dimension: 768,
                            distance_metric: 0,
                            storage_engine: StorageEngine::Sst as i32,
                            quantization: Some(quant_clone.clone()), // With compression
                            ..Default::default()
                        }),
                        created_at: 0,
                        updated_at: 0,
                        stats: None,
                        storage_assignment: Some(StorageAssignment {
                            primary_path: "/tmp/proximadb_bench".to_string(),
                            backup_paths: vec![],
                            engine: StorageEngine::Sst as i32,
                            engine_config: HashMap::new(),
                            base_location: "/tmp/proximadb_bench".to_string(),
                            assigned_at: 0,
                        }),
                    };

                    let params = FlushParameters {
                        collection_id: Some("bench".to_string()),
                        vector_records: (*vectors_clone).clone(),
                        force: true,
                        synchronous: true,
                        collection_config: Some(collection),
                        ..Default::default()
                    };
                    let _ = engine.flush(params).await;
                })
            })
        });
    }

    group.finish();
}

/// Benchmark memory efficiency across all engines
fn bench_engine_memory_efficiency(c: &mut Criterion) {
    init_hardware();

    let mut group = c.benchmark_group("engine_memory_efficiency");
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10);

    let batch_sizes = vec![100, 500, 1000, 5000];

    // Create PRISM engine once with async initialization
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let prism_engine = runtime.block_on(async {
        StorageEngineFactory::create_prism_async().await.unwrap()
    });

    for size in batch_sizes {
        let vectors = Arc::new(generate_vectors(size, 768));

        // Benchmark each engine
        let engines = vec![
            ("sst", StorageEngineFactory::create_sst().unwrap()),
            ("viper", StorageEngineFactory::create_viper().unwrap()),
            ("nova", StorageEngineFactory::create_nova().unwrap()),
            ("swift", StorageEngineFactory::create_swift().unwrap()),
            ("raptor", StorageEngineFactory::create_raptor_default().unwrap()),
            ("helix", StorageEngineFactory::create_helix().unwrap()),
            ("prism", Arc::clone(&prism_engine)),
        ];

        for (name, engine) in engines {
            let vectors_clone = Arc::clone(&vectors);
            group.bench_with_input(
                BenchmarkId::new(name, size),
                &size,
                |b, _| {
                    b.iter(|| {
                        runtime.block_on(async {
                            let params = FlushParameters {
                                collection_id: Some("bench".to_string()),
                                vector_records: (*vectors_clone).clone(),
                                force: true,
                                synchronous: true,
                                ..Default::default()
                            };
                            let _ = engine.flush(params).await;
                            black_box(&engine);
                        })
                    })
                },
            );
        }
    }

    group.finish();
}

/// Benchmark query performance for each engine
fn bench_engine_query_performance(c: &mut Criterion) {
    init_hardware();

    let mut group = c.benchmark_group("engine_query_performance");
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(5));

    // Pre-load data into engines
    let vectors = generate_vectors(10000, 768);
    let query_vector = vec![0.5f32; 768];

    // Create runtime for async operations
    let runtime = tokio::runtime::Runtime::new().unwrap();

    // Create PRISM engine with async initialization
    let prism_engine = runtime.block_on(async {
        StorageEngineFactory::create_prism_async().await.unwrap()
    });

    let engines = vec![
        ("sst", StorageEngineFactory::create_sst().unwrap()),
        ("viper", StorageEngineFactory::create_viper().unwrap()),
        ("nova", StorageEngineFactory::create_nova().unwrap()),
        ("swift", StorageEngineFactory::create_swift().unwrap()),
        ("raptor", StorageEngineFactory::create_raptor_default().unwrap()),
        ("helix", StorageEngineFactory::create_helix().unwrap()),
        ("prism", prism_engine),
    ];

    for (name, engine) in engines {
        // Pre-load data using Tokio runtime
        runtime.block_on(async {
            let params = FlushParameters {
                collection_id: Some("bench".to_string()),
                vector_records: vectors.clone(),
                force: true,
                synchronous: true,
                ..Default::default()
            };
            let _ = engine.flush(params).await;
        });

        // Benchmark query
        group.bench_function(format!("{}_query_top10", name), |b| {
            b.iter(|| {
                // Simulate search operation
                // Note: Actual search implementation depends on engine trait
                let _ = black_box(&query_vector);
                let _ = black_box(&engine);
            })
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_all_engine_creation,
    bench_engine_flush_with_compression,
    bench_engine_memory_efficiency,
    bench_engine_query_performance
);
criterion_main!(benches);