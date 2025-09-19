//! Comprehensive benchmark for storage engine compression across different sparsity levels
//!
//! Benchmarks both SST and VIPER engines with:
//! - Sparsity levels: 10%, 25%, 50%, 75%, 90%
//! - Compression algorithms: none, lz4, snappy, zstd
//! - Compression levels: 1, 3, 6, 9 (where supported)
//! - Query performance measurement

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use proximadb::{
    compute::distance_computation::{DistanceMetric, UnifiedDistanceCompute},
    core::{hardware_capabilities, VectorRecord},
    proto::proximadb_v1::StorageEngine,
    storage::{
        engines::impls::{sst::SstStorage, viper::ViperEngine},
        persistence::filesystem::FilesystemFactory,
        traits::{FlushParameters, UnifiedStorageEngine},
    },
};
use std::sync::Arc;
use std::time::Instant;
use tempfile::TempDir;
use tokio::runtime::Runtime;

/// Create vectors with specific sparsity level
fn create_vectors_with_sparsity(
    count: usize,
    dim: usize,
    sparsity_percent: usize,
) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| {
            let mut vector = vec![0.0; dim];
            let non_zero_count = dim * (100 - sparsity_percent) / 100;

            // Distribute non-zero values
            for j in 0..non_zero_count {
                let idx = (i * 7 + j * 13) % dim;
                vector[idx] = ((i + j) % 100) as f32 / 10.0;
            }

            VectorRecord {
                id: format!("sparse_{}_{}", sparsity_percent, i),
                vector,
                metadata: std::collections::HashMap::new(),
                timestamp: (1000 + i) as i64,
                updated_at: None,
                expires_at: None,
                version: Some(1),
                quantized_vector: vec![],
                source: None,
            }
        })
        .collect()
}

/// Benchmark SST engine with different sparsity levels and compression
fn bench_sst_sparsity_compression(c: &mut Criterion) {
    let _ = hardware_capabilities::initialize_hardware_capabilities_default();
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("sst_sparsity_compression");
    group.sample_size(10);

    let sparsity_levels = vec![10, 50, 90]; // 10%, 50%, 90% sparse
    let compression_configs = vec![
        ("none", 0),
        ("zstd", 1),
        ("zstd", 3),
        ("lz4", 0),
        ("snappy", 0),
    ];

    for sparsity in &sparsity_levels {
        for (algo, level) in &compression_configs {
            let bench_id = BenchmarkId::new(
                format!("sparsity_{}_algo_{}_level_{}", sparsity, algo, level),
                sparsity,
            );

            group.bench_with_input(bench_id, sparsity, |b, &sparsity| {
                b.to_async(&rt).iter(|| async move {
                    let temp_dir = TempDir::new().unwrap();
                    let vectors = create_vectors_with_sparsity(100, 256, sparsity);

                    // Create filesystem factory
                    let filesystem_factory = Arc::new(
                        FilesystemFactory::default()
                    );

                    // Create distance compute
                    let distance_compute = Arc::new(UnifiedDistanceCompute::new(
                        DistanceMetric::Cosine,
                    ));

                    // Create SST config with compression
                    let mut sst_config = proximadb::core::config::SstConfig::default();
                    sst_config.compression = algo.to_string();
                    sst_config.compression_level = *level;
                    sst_config.data_directory = temp_dir.path().to_str().unwrap().to_string();

                    // Create SST engine
                    let engine = SstStorage::new(
                        sst_config,
                        filesystem_factory,
                        distance_compute,
                    )
                    .await
                    .unwrap();

                    // Create flush parameters
                    let flush_params = FlushParameters {
                        collection_id: Some("bench_collection".to_string()),
                        vector_records: vectors.clone(),
                        force: true,
                        ..Default::default()
                    };

                    // Measure flush time
                    let start = Instant::now();
                    let result = engine.do_flush(&flush_params).await.unwrap();
                    let flush_time = start.elapsed();

                    black_box((result.success, flush_time, vectors.len()))
                });
            });
        }
    }

    group.finish();
}

/// Benchmark VIPER engine with different sparsity levels and compression
fn bench_viper_sparsity_compression(c: &mut Criterion) {
    let _ = hardware_capabilities::initialize_hardware_capabilities_default();
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("viper_sparsity_compression");
    group.sample_size(10);

    let sparsity_levels = vec![10, 50, 90]; // 10%, 50%, 90% sparse
    let compression_configs = vec![
        ("none", 0),
        ("zstd", 1),
        ("zstd", 3),
        ("snappy", 0),
        ("gzip", 1),
    ];

    for sparsity in &sparsity_levels {
        for (algo, level) in &compression_configs {
            let bench_id = BenchmarkId::new(
                format!("sparsity_{}_algo_{}_level_{}", sparsity, algo, level),
                sparsity,
            );

            group.bench_with_input(bench_id, sparsity, |b, &sparsity| {
                b.to_async(&rt).iter(|| async move {
                    let temp_dir = TempDir::new().unwrap();
                    let vectors = create_vectors_with_sparsity(100, 256, sparsity);

                    // Create filesystem factory
                    let filesystem_factory = Arc::new(
                        FilesystemFactory::default()
                    );

                    // Create VIPER config with compression
                    let mut viper_config = proximadb::core::config::ViperConfig::default();
                    viper_config.compression = algo.to_string();
                    viper_config.compression_level = *level;
                    viper_config.data_directory = temp_dir.path().to_str().unwrap().to_string();
                    viper_config.row_group_size = 50_000;

                    // Create VIPER engine
                    let engine = ViperEngine::from_core_config(
                        viper_config,
                        filesystem_factory,
                    )
                    .await
                    .unwrap();

                    // Create flush parameters
                    let flush_params = FlushParameters {
                        collection_id: Some("bench_collection".to_string()),
                        vector_records: vectors.clone(),
                        force: true,
                        ..Default::default()
                    };

                    // Measure flush time
                    let start = Instant::now();
                    let result = engine.flush(flush_params).await.unwrap();
                    let flush_time = start.elapsed();

                    black_box((result.success, flush_time, vectors.len()))
                });
            });
        }
    }

    group.finish();
}

/// Compare compression effectiveness across sparsity levels
fn bench_compression_ratio_by_sparsity(c: &mut Criterion) {
    let _ = hardware_capabilities::initialize_hardware_capabilities_default();
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("compression_ratio_by_sparsity");
    group.sample_size(10);

    // Test extreme sparsity levels to see compression benefits
    let sparsity_levels = vec![0, 25, 50, 75, 95, 99]; // 0% to 99% sparse

    for sparsity in &sparsity_levels {
        group.bench_with_input(
            BenchmarkId::new("sst_zstd3", sparsity),
            sparsity,
            |b, &sparsity| {
                b.to_async(&rt).iter(|| async move {
                    let temp_dir = TempDir::new().unwrap();
                    let vectors = create_vectors_with_sparsity(500, 512, sparsity);

                    // Calculate theoretical size
                    let vector_size = vectors.len() * 512 * 4; // 4 bytes per f32

                    // Create filesystem factory
                    let filesystem_factory = Arc::new(
                        FilesystemFactory::default()
                    );

                    // Create distance compute
                    let distance_compute = Arc::new(UnifiedDistanceCompute::new(
                        DistanceMetric::Cosine,
                    ));

                    // Test with ZSTD level 3 (good balance)
                    let mut sst_config = proximadb::core::config::SstConfig::default();
                    sst_config.compression = "zstd".to_string();
                    sst_config.compression_level = 3;
                    sst_config.data_directory = temp_dir.path().to_str().unwrap().to_string();

                    let engine = SstStorage::new(
                        sst_config,
                        filesystem_factory,
                        distance_compute,
                    )
                    .await
                    .unwrap();

                    let flush_params = FlushParameters {
                        collection_id: Some("compression_test".to_string()),
                        vector_records: vectors,
                        force: true,
                        ..Default::default()
                    };

                    let result = engine.do_flush(&flush_params).await.unwrap();

                    // Measure actual file size
                    let data_dir = temp_dir.path().join("compression_test");
                    let compressed_size = if data_dir.exists() {
                        std::fs::read_dir(data_dir)
                            .unwrap()
                            .filter_map(|entry| entry.ok())
                            .filter_map(|entry| {
                                entry.metadata().ok().map(|m| m.len())
                            })
                            .sum::<u64>()
                    } else {
                        0
                    };

                    let ratio = if vector_size > 0 {
                        compressed_size as f64 / vector_size as f64
                    } else {
                        1.0
                    };

                    black_box((result.success, ratio, sparsity))
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_sst_sparsity_compression,
    bench_viper_sparsity_compression,
    bench_compression_ratio_by_sparsity
);
criterion_main!(benches);