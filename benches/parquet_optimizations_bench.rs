//! Comprehensive benchmarks for Parquet optimizations
//!
//! Measures performance improvements from:
//! - Footer caching
//! - Page-level indexes
//! - PQ-based sorting
//! - Native metadata types
//! - Hybrid writer strategy

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use proximadb::core::VectorRecord;
use proximadb::storage::engines::columnar::{
    BatchParquetWriter, FooterCacheConfig, HybridParquetWriter, HybridWriterConfig,
    NativeMetadataHandler, NativeMetadataQueryOptimizer, ParquetFooterCache, ParquetWriterConfig,
    StreamingParquetWriter, WarmingStrategy, WriterMode,
};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;
use tokio::runtime::Runtime;

/// Generate test vectors
fn generate_vectors(count: usize, dimension: usize) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| {
            let vector = (0..dimension)
                .map(|j| ((i + j) as f32 * 0.001) % 1.0)
                .collect();

            let metadata = json!({
                "category": format!("cat_{}", i % 10),
                "is_active": i % 2 == 0,
                "count": i,
                "score": (i as f32 * 0.1) % 100.0,
                "tags": vec![format!("tag_{}", i % 5), format!("tag_{}", i % 7)],
                "properties": {
                    "key1": format!("value_{}", i),
                    "key2": i.to_string(),
                }
            });

            VectorRecord {
                id: Some(format!("vec_{:08}", i)),
                vector,
                metadata: Some(metadata.as_object().unwrap().clone()),
                timestamp: i as u32,
                updated_at: None,
                expires_at: None,
                version: Some(1),
            }
        })
        .collect()
}

/// Benchmark footer caching
fn bench_footer_cache(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("footer_cache");

    // Setup
    let dir = tempdir().unwrap();
    let file_paths: Vec<_> = (0..100)
        .map(|i| dir.path().join(format!("test_{:03}.parquet", i)))
        .collect();

    // Create test files
    rt.block_on(async {
        for (i, path) in file_paths.iter().enumerate().take(10) {
            let config = ParquetWriterConfig::default();
            let mut writer = StreamingParquetWriter::new(path, 128, config).unwrap();
            let vectors = generate_vectors(100, 128);
            writer.write_batch(&vectors).await.unwrap();
            writer.finalize().await.unwrap();
        }
    });

    let filesystem = Arc::new(
        rt.block_on(
            proximadb::storage::persistence::filesystem::FilesystemFactory::new(Default::default()),
        )
        .unwrap(),
    );

    // Benchmark without cache
    group.bench_function("without_cache", |b| {
        b.iter(|| {
            rt.block_on(async {
                for path in file_paths.iter().take(10) {
                    // Simulate metadata read
                    let _ = filesystem.metadata(path.to_str().unwrap()).await;
                }
            });
        });
    });

    // Benchmark with cache
    let cache_config = FooterCacheConfig {
        max_entries: 1000,
        ttl: Duration::from_secs(3600),
        enable_persistence: false,
        ..Default::default()
    };

    let cache = rt
        .block_on(ParquetFooterCache::new(cache_config, filesystem.clone()))
        .unwrap();

    group.bench_function("with_cache", |b| {
        b.iter(|| {
            rt.block_on(async {
                for path in file_paths.iter().take(10) {
                    let _ = cache.get_footer(path.to_str().unwrap()).await;
                }
            });
        });
    });

    // Benchmark cache warming
    group.bench_function("cache_warming", |b| {
        b.iter(|| {
            rt.block_on(async {
                cache
                    .warm_cache(WarmingStrategy::FrequentlyAccessed { count: 50 })
                    .await
            });
        });
    });

    group.finish();
}

/// Benchmark PQ-based sorting
fn bench_pq_sorting(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("pq_sorting");

    for size in [100, 1000, 10000].iter() {
        let vectors = generate_vectors(*size, 768);
        let dir = tempdir().unwrap();

        // Without PQ sorting
        group.bench_with_input(BenchmarkId::new("without_pq", size), size, |b, _| {
            b.iter(|| {
                rt.block_on(async {
                    let path = dir.path().join("without_pq.parquet");
                    let config = ParquetWriterConfig {
                        enable_pq_sorting: false,
                        ..Default::default()
                    };
                    let mut writer = StreamingParquetWriter::new(&path, 768, config).unwrap();
                    writer.write_batch(&vectors).await.unwrap();
                    writer.finalize().await.unwrap();
                });
            });
        });

        // With PQ sorting
        group.bench_with_input(BenchmarkId::new("with_pq", size), size, |b, _| {
            b.iter(|| {
                rt.block_on(async {
                    let path = dir.path().join("with_pq.parquet");
                    let config = ParquetWriterConfig {
                        enable_pq_sorting: true,
                        pq_sorting_segments: 16,
                        pq_sorting_codebook_size: 256,
                        ..Default::default()
                    };
                    let mut writer = StreamingParquetWriter::new(&path, 768, config).unwrap();
                    writer.write_batch(&vectors).await.unwrap();
                    let stats = writer.finalize().await.unwrap();
                    black_box(stats.compression_ratio);
                });
            });
        });
    }

    group.finish();
}

/// Benchmark native metadata types
fn bench_native_metadata(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("native_metadata");

    let vectors = generate_vectors(1000, 128);
    let dir = tempdir().unwrap();

    // JSON metadata (baseline)
    group.bench_function("json_metadata", |b| {
        b.iter(|| {
            rt.block_on(async {
                let path = dir.path().join("json_metadata.parquet");
                let config = ParquetWriterConfig {
                    enable_native_metadata: false,
                    ..Default::default()
                };
                let mut writer = StreamingParquetWriter::new(&path, 128, config).unwrap();
                writer.write_batch(&vectors).await.unwrap();
                writer.finalize().await.unwrap();
            });
        });
    });

    // Native metadata types
    group.bench_function("native_metadata", |b| {
        b.iter(|| {
            rt.block_on(async {
                let path = dir.path().join("native_metadata.parquet");
                let config = ParquetWriterConfig {
                    enable_native_metadata: true,
                    metadata_inference_samples: 100,
                    ..Default::default()
                };
                let mut writer = StreamingParquetWriter::new(&path, 128, config).unwrap();
                writer.write_batch(&vectors).await.unwrap();
                writer.finalize().await.unwrap();
            });
        });
    });

    // Metadata query optimization
    let metadata_samples: Vec<_> = vectors
        .iter()
        .filter_map(|v| v.metadata.as_ref())
        .take(100)
        .cloned()
        .collect();

    let mut handler = NativeMetadataHandler::new();
    handler.analyze_metadata(&metadata_samples).unwrap();

    group.bench_function("metadata_type_inference", |b| {
        b.iter(|| {
            let mut h = NativeMetadataHandler::new();
            h.analyze_metadata(&metadata_samples).unwrap();
            black_box(h.get_optimization_stats());
        });
    });

    group.finish();
}

/// Benchmark hybrid writer
fn bench_hybrid_writer(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("hybrid_writer");

    let dir = tempdir().unwrap();

    // Streaming pattern (small batches)
    let streaming_batches: Vec<_> = (0..100).map(|i| generate_vectors(10, 128)).collect();

    group.bench_function("streaming_pattern", |b| {
        b.iter(|| {
            rt.block_on(async {
                let path = dir.path().join("streaming.parquet");
                let config = HybridWriterConfig {
                    initial_mode: WriterMode::Adaptive,
                    enable_auto_switch: true,
                    ..Default::default()
                };
                let writer = HybridParquetWriter::new(&path, 128, config).await.unwrap();

                for batch in &streaming_batches {
                    writer.write(batch.clone()).await.unwrap();
                }

                let stats = writer.finalize().await.unwrap();
                black_box(stats.mode_switches);
            });
        });
    });

    // Batch pattern (large batches)
    let batch_batches: Vec<_> = (0..10).map(|i| generate_vectors(1000, 128)).collect();

    group.bench_function("batch_pattern", |b| {
        b.iter(|| {
            rt.block_on(async {
                let path = dir.path().join("batch.parquet");
                let config = HybridWriterConfig {
                    initial_mode: WriterMode::Adaptive,
                    enable_auto_switch: true,
                    ..Default::default()
                };
                let writer = HybridParquetWriter::new(&path, 128, config).await.unwrap();

                for batch in &batch_batches {
                    writer.write(batch.clone()).await.unwrap();
                }

                let stats = writer.finalize().await.unwrap();
                black_box(stats.mode_switches);
            });
        });
    });

    // Mixed pattern
    group.bench_function("mixed_pattern", |b| {
        b.iter(|| {
            rt.block_on(async {
                let path = dir.path().join("mixed.parquet");
                let config = HybridWriterConfig {
                    initial_mode: WriterMode::Adaptive,
                    enable_auto_switch: true,
                    ..Default::default()
                };
                let writer = HybridParquetWriter::new(&path, 128, config).await.unwrap();

                // Alternate between small and large batches
                for i in 0..20 {
                    if i % 2 == 0 {
                        writer.write(generate_vectors(10, 128)).await.unwrap();
                    } else {
                        writer.write(generate_vectors(500, 128)).await.unwrap();
                    }
                }

                let stats = writer.finalize().await.unwrap();
                black_box(stats.mode_switches);
            });
        });
    });

    group.finish();
}

/// Benchmark page indexes
fn bench_page_indexes(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("page_indexes");

    let vectors = generate_vectors(10000, 256);
    let dir = tempdir().unwrap();

    // Without page indexes
    group.bench_function("without_indexes", |b| {
        b.iter(|| {
            rt.block_on(async {
                let path = dir.path().join("without_indexes.parquet");
                let config = ParquetWriterConfig {
                    enable_column_index: false,
                    enable_offset_index: false,
                    ..Default::default()
                };
                let writer = BatchParquetWriter::new(&path, 256, config);
                writer.write_all(&vectors).await.unwrap();
            });
        });
    });

    // With page indexes
    group.bench_function("with_indexes", |b| {
        b.iter(|| {
            rt.block_on(async {
                let path = dir.path().join("with_indexes.parquet");
                let config = ParquetWriterConfig {
                    enable_column_index: true,
                    enable_offset_index: true,
                    page_size: 1024 * 1024, // 1MB pages
                    page_index_granularity: 1000,
                    ..Default::default()
                };
                let writer = BatchParquetWriter::new(&path, 256, config);
                writer.write_all(&vectors).await.unwrap();
            });
        });
    });

    group.finish();
}

/// Benchmark bloom filters
fn bench_bloom_filters(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("bloom_filters");

    let vectors = generate_vectors(5000, 128);
    let dir = tempdir().unwrap();

    // Without bloom filters
    group.bench_function("without_bloom", |b| {
        b.iter(|| {
            rt.block_on(async {
                let path = dir.path().join("without_bloom.parquet");
                let config = ParquetWriterConfig {
                    enable_bloom_filters: false,
                    ..Default::default()
                };
                let writer = BatchParquetWriter::new(&path, 128, config);
                writer.write_all(&vectors).await.unwrap();
            });
        });
    });

    // With bloom filters
    group.bench_function("with_bloom", |b| {
        b.iter(|| {
            rt.block_on(async {
                let path = dir.path().join("with_bloom.parquet");
                let config = ParquetWriterConfig {
                    enable_bloom_filters: true,
                    bloom_filter_fpp: 0.01,
                    bloom_filter_columns: vec!["id".to_string(), "category".to_string()],
                    ..Default::default()
                };
                let writer = BatchParquetWriter::new(&path, 128, config);
                let stats = writer.write_all(&vectors).await.unwrap();
                black_box(stats.bloom_filter_count);
            });
        });
    });

    group.finish();
}

/// Benchmark end-to-end optimizations
fn bench_end_to_end(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("end_to_end");

    let vectors = generate_vectors(10000, 768);
    let dir = tempdir().unwrap();

    // Baseline (no optimizations)
    group.bench_function("baseline", |b| {
        b.iter(|| {
            rt.block_on(async {
                let path = dir.path().join("baseline.parquet");
                let config = ParquetWriterConfig {
                    enable_bloom_filters: false,
                    enable_column_index: false,
                    enable_offset_index: false,
                    enable_pq_sorting: false,
                    enable_native_metadata: false,
                    ..Default::default()
                };
                let writer = BatchParquetWriter::new(&path, 768, config);
                writer.write_all(&vectors).await.unwrap();
            });
        });
    });

    // All optimizations enabled
    group.bench_function("optimized", |b| {
        b.iter(|| {
            rt.block_on(async {
                let path = dir.path().join("optimized.parquet");
                let config = ParquetWriterConfig {
                    enable_bloom_filters: true,
                    enable_column_index: true,
                    enable_offset_index: true,
                    enable_pq_sorting: true,
                    enable_native_metadata: true,
                    page_size: 1024 * 1024,
                    metadata_inference_samples: 100,
                    ..Default::default()
                };
                let writer = BatchParquetWriter::new(&path, 768, config);
                let stats = writer.write_all(&vectors).await.unwrap();
                black_box(stats);
            });
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_footer_cache,
    bench_pq_sorting,
    bench_native_metadata,
    bench_hybrid_writer,
    bench_page_indexes,
    bench_bloom_filters,
    bench_end_to_end
);

criterion_main!(benches);
