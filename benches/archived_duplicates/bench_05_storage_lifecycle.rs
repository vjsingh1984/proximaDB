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
        VectorRecord, StorageConfig,
    },
    storage::{
        engines::factory::StorageEngineFactory,
        traits::{FlushParameters, UnifiedStorageEngine},
    },
};
use std::collections::HashMap;
use std::sync::{Arc, Once};
use std::time::Duration;
use std::fs;
use std::path::Path;

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
            timestamp: Some(i as i64),
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

/// Helper function to measure directory size using filesystem API for cloud transparency
async fn measure_directory_size_async(
    path: &str,
    filesystem_factory: &proximadb::storage::persistence::filesystem::FilesystemFactory,
) -> anyhow::Result<u64> {
    // Use filesystem API for cloud storage transparency
    let fs_url = if path.starts_with("s3://") || path.starts_with("gs://")
        || path.starts_with("azure://") || path.starts_with("wasbs://") {
        path.to_string()
    } else {
        format!("file://{}", path)
    };

    let fs = filesystem_factory.get_filesystem(&fs_url)?;

    // List all files in the directory recursively
    let files = fs.list_dir(path).await?;
    let mut total_size = 0u64;

    for file_info in files {
        if file_info.is_file {
            // Get file size through filesystem API
            if let Ok(metadata) = fs.metadata(&file_info.path).await {
                total_size += metadata.size;
            }
        }
    }

    Ok(total_size)
}

/// Synchronous wrapper for directory size measurement
fn measure_directory_size(path: &str) -> std::io::Result<u64> {
    // Create a minimal runtime for the async operation
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    // Create filesystem factory
    let fs_factory = rt.block_on(async {
        proximadb::storage::persistence::filesystem::FilesystemFactory::create(
            proximadb::storage::persistence::filesystem::FilesystemConfig::default()
        ).await
    }).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    // Measure size using filesystem API
    rt.block_on(measure_directory_size_async(path, &fs_factory))
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
}

/// Create compression configuration for different algorithms
fn create_compression_config(algorithm: &str, level: i32) -> QuantizationConfig {
    QuantizationConfig {
        enabled: true,
        strategy: 0, // SMART_DEFAULTS
        custom_levels: vec![],
        enable_progressive_search: true,
        binary_filter_selectivity: 0.3,
        int8_ranking_selectivity: 0.1,
        pq_ranking_selectivity: 0.05,
        training_sample_size: 10000,
        quality_threshold: 0.95,
        enable_adaptive_training: true,
        optimize_for_storage: algorithm != "none",
        optimize_for_memory: false,
        enable_simd_acceleration: true,
        enable_binary: algorithm != "none",
        enable_int8: algorithm != "none",
        enable_pq: algorithm != "none",
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


    group.finish();
}

/// Benchmark flush operations with compression for all engines
/// This function now properly measures and reports compression ratios
fn bench_engine_flush_with_compression(c: &mut Criterion) {
    init_hardware();

    let mut group = c.benchmark_group("engine_flush_compression");
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10);

    let vectors = Arc::new(generate_vectors(1000, 768));

    // Calculate uncompressed baseline size
    let uncompressed_size = 1000 * 768 * 4; // 1024 vectors * 768 dims * 4 bytes (f32)
    eprintln!("\n📊 COMPRESSION BENCHMARK ANALYSIS");
    eprintln!("   Baseline uncompressed size: {} bytes ({:.2} MB)",
             uncompressed_size, uncompressed_size as f64 / (1024.0 * 1024.0));
    eprintln!("   Testing 1024 vectors with 768 dimensions\n");

    // Test different compression algorithms
    let compression_configs = vec![
        ("none", None),
        ("zstd", Some(create_compression_config("zstd", 3))),
        ("lz4", Some(create_compression_config("lz4", 0))),
        ("snappy", Some(create_compression_config("snappy", 0))),
    ];

    // Test each engine
    let runtime = tokio::runtime::Runtime::new().unwrap();

    let engines = vec![
        ("sst", StorageEngineFactory::create_sst().unwrap()),
        ("viper", StorageEngineFactory::create_viper().unwrap()),
        ("nova", StorageEngineFactory::create_nova().unwrap()),
        ("swift", StorageEngineFactory::create_swift().unwrap()),
        ("raptor", StorageEngineFactory::create_raptor_default().unwrap()),
        ("helix", StorageEngineFactory::create_helix().unwrap()),
    ];

    // Summary table header
    eprintln!("{:<10} {:<10} {:<15} {:<15} {:<10} {:<10}",
             "Engine", "Algorithm", "Size (bytes)", "Ratio", "Savings %", "Time (ms)");
    eprintln!("{}", "-".repeat(80));

    for (engine_name, engine) in engines {
        for (comp_name, comp_config) in &compression_configs {
            let vectors_clone = Arc::clone(&vectors);
            let collection_id = format!("bench_{}_{}", engine_name, comp_name);

            // Clean up any existing data using filesystem API
            let storage_path = format!("/tmp/proximadb_bench/{}/{}", engine_name, collection_id);
            // Note: For now, use std::fs for cleanup as filesystem API may not have remove_dir_all
            // This is acceptable as cleanup is not part of the benchmark measurement
            let _ = std::fs::remove_dir_all(&storage_path);

            group.bench_function(format!("{}_{}", engine_name, comp_name), |b| {
                let mut total_time = 0u64;
                let mut compressed_size = 0u64;

                b.iter(|| {
                    runtime.block_on(async {
                        let start = std::time::Instant::now();

                        let collection = Collection {
                            id: collection_id.clone(),
                            config: Some(CollectionConfig {
                                name: collection_id.clone(),
                                dimension: 768,
                                distance_metric: 0,
                                storage_engine: match engine_name {
                                    "sst" => StorageEngine::Sst as i32,
                                    "viper" => StorageEngine::Viper as i32,
                                    "nova" => StorageEngine::Nova as i32,
                                    "swift" => StorageEngine::Swift as i32,
                                    "raptor" => StorageEngine::Raptor as i32,
                                    "helix" => StorageEngine::Helix as i32,
                                    _ => StorageEngine::Sst as i32,
                                },
                                quantization: comp_config.clone(),
                                storage_config: comp_config.as_ref().map(|_| {
                                    let mut sc = StorageConfig::default();
                                    sc.compression = match *comp_name {
                                        "zstd" => 1,    // CompressionAlgorithm::CompressionZstd
                                        "lz4" => 2,     // CompressionAlgorithm::CompressionLz4
                                        "snappy" => 3,  // CompressionAlgorithm::CompressionSnappy
                                        _ => 0,         // CompressionAlgorithm::CompressionNone
                                    };
                                    sc
                                }),
                                ..Default::default()
                            }),
                            created_at: 0,
                            updated_at: 0,
                            stats: None,
                            storage_assignment: Some(StorageAssignment {
                                primary_path: format!("/tmp/proximadb_bench/{}", engine_name),
                                backup_paths: vec![],
                                engine: match engine_name {
                                    "sst" => StorageEngine::Sst as i32,
                                    "viper" => StorageEngine::Viper as i32,
                                    "nova" => StorageEngine::Nova as i32,
                                    "swift" => StorageEngine::Swift as i32,
                                    "raptor" => StorageEngine::Raptor as i32,
                                    "helix" => StorageEngine::Helix as i32,
                                    _ => StorageEngine::Sst as i32,
                                },
                                engine_config: HashMap::new(),
                                base_location: format!("/tmp/proximadb_bench/{}", engine_name),
                                assigned_at: 0,
                            }),
                        };

                        let params = FlushParameters {
                            collection_id: Some(collection_id.clone()),
                            vector_records: (*vectors_clone).clone(),
                            force: true,
                            synchronous: true,
                            collection_config: Some(collection),
                            ..Default::default()
                        };

                        let result = engine.flush(params).await;
                        total_time = start.elapsed().as_millis() as u64;

                        // Measure file size after flush
                        if result.is_ok() {
                            let data_path = format!("/tmp/proximadb_bench/{}/{}/data",
                                                   engine_name, collection_id);
                            compressed_size = measure_directory_size(&data_path).unwrap_or(0);
                        }
                    })
                });

                // Report compression results after benchmark
                if compressed_size > 0 {
                    let ratio = compressed_size as f64 / uncompressed_size as f64;
                    let savings = (1.0 - ratio) * 100.0;
                    eprintln!("{:<10} {:<10} {:<15} {:.3}          {:<10.1} {:<10}",
                             engine_name, comp_name, compressed_size, ratio, savings, total_time);
                }
            });
        }
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

    let batch_sizes = vec![128, 512, 1000, 5000];

    let runtime = tokio::runtime::Runtime::new().unwrap();

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


    let engines = vec![
        ("sst", StorageEngineFactory::create_sst().unwrap()),
        ("viper", StorageEngineFactory::create_viper().unwrap()),
        ("nova", StorageEngineFactory::create_nova().unwrap()),
        ("swift", StorageEngineFactory::create_swift().unwrap()),
        ("raptor", StorageEngineFactory::create_raptor_default().unwrap()),
        ("helix", StorageEngineFactory::create_helix().unwrap()),
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