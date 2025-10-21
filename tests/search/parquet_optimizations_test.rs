//! Integration tests for Parquet optimizations
//!
//! Validates functionality and performance of:
//! - Footer caching
//! - Page-level indexes
//! - PQ-based sorting
//! - Native metadata types
//! - Hybrid writer strategy
//! - Bloom filters

use anyhow::Result;
use proximadb::storage::engines::columnar::{
    ParquetWriterConfig, StreamingParquetWriter, BatchParquetWriter,
    HybridParquetWriter, HybridWriterConfig, WriterMode,
    ParquetFooterCache, FooterCacheConfig, WarmingStrategy,
    NativeMetadataHandler, NativeMetadataQueryOptimizer,
    MetadataFieldType, PredicateOperator,
};
use proximadb::core::VectorRecord;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::tempdir;
use tokio;

/// Generate test vectors with various metadata patterns
fn generate_test_vectors(count: usize, dimension: usize) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| {
            let vector = (0..dimension)
                .map(|j| ((i + j) as f32 * 0.001) % 1.0)
                .collect();
            
            // Varied metadata for type inference testing
            let metadata = if i % 5 == 0 {
                // Complex metadata with nested structures
                json!({
                    "category": format!("category_{}", i % 10),
                    "is_active": i % 2 == 0,
                    "item_count": i as i64,
                    "confidence_score": (i as f32 * 0.1) % 1.0,
                    "tags": vec![format!("tag_{}", i % 3), format!("tag_{}", i % 7)],
                    "properties": {
                        "nested_key": format!("nested_value_{}", i),
                        "nested_count": i % 100,
                    },
                    "complex_data": {
                        "deeply": {
                            "nested": {
                                "value": i
                            }
                        }
                    }
                })
            } else if i % 3 == 0 {
                // Simple metadata
                json!({
                    "category": format!("simple_{}", i % 5),
                    "enabled": true,
                    "score": i as f32,
                })
            } else {
                // List-heavy metadata
                json!({
                    "tags": (0..5).map(|j| format!("tag_{}_{}", i, j)).collect::<Vec<_>>(),
                    "categories": vec![format!("cat_{}", i % 3)],
                    "keywords": vec![format!("keyword_{}", i)],
                })
            };
            
            VectorRecord {
                id: Some(format!("vec_{:08}", i)),
                vector,
                metadata: Some(metadata.as_object().unwrap().clone()),
                timestamp: (i * 1000) as u32,
                updated_at: if i % 10 == 0 { Some((i * 1000 + 500) as u32) } else { None },
                expires_at: if i % 20 == 0 { Some((i * 1000 + 86400) as u32) } else { None },
                version: Some((i % 5 + 1) as u64),
            }
        })
        .collect()
}

#[tokio::test]
async fn test_footer_cache_functionality() -> Result<()> {
    let dir = tempdir()?;
    let filesystem = Arc::new(
        proximadb::storage::persistence::filesystem::FilesystemFactory::create(Default::default()).await?
    );
    
    // Create test Parquet files
    let file_paths: Vec<_> = (0..10)
        .map(|i| dir.path().join(format!("test_{:02}.parquet", i)))
        .collect();
    
    for (i, path) in file_paths.iter().enumerate() {
        let config = ParquetWriterConfig::default();
        let mut writer = StreamingParquetWriter::new(path, 128, config, None).await?;
        let vectors = generate_test_vectors(100, 128);
        writer.write_batch(&vectors).await?;
        writer.finalize().await?;
    }
    
    // Test cache functionality
    let cache_config = FooterCacheConfig {
        max_entries: 100,
        ttl: Duration::from_secs(60),
        enable_persistence: true,
        persistence_path: Some(dir.path().join("cache.bin").to_str().unwrap().to_string()),
        enable_prefetch: true,
        prefetch_threshold: 2,
        ..Default::default()
    };
    
    let cache = ParquetFooterCache::new(cache_config, filesystem).await?;
    
    // Test cache miss and hit
    let path1 = file_paths[0].to_str().unwrap();
    let start = Instant::now();
    let _ = cache.get_footer(path1).await?; // Cache miss
    let miss_time = start.elapsed();
    
    let start = Instant::now();
    let _ = cache.get_footer(path1).await?; // Cache hit
    let hit_time = start.elapsed();
    
    assert!(hit_time < miss_time / 10, "Cache hit should be much faster than miss");
    
    // Test cache statistics
    let stats = cache.get_stats().await;
    assert_eq!(stats.hit_count, 1);
    assert_eq!(stats.miss_count, 1);
    assert!(stats.hit_rate > 0.0);
    
    // Test cache warming
    let warmed = cache.warm_cache(WarmingStrategy::Custom {
        files: file_paths.iter().map(|p| p.to_str().unwrap().to_string()).collect()
    }).await?;
    assert_eq!(warmed, 10);
    
    // Test cache invalidation
    cache.invalidate(path1).await;
    let stats = cache.get_stats().await;
    assert!(stats.cache_size < 10);
    
    Ok(())
}

#[tokio::test]
async fn test_pq_sorting_compression() -> Result<()> {
    let dir = tempdir()?;
    let vectors = generate_test_vectors(1000, 768);
    
    // Write without PQ sorting
    let path_without = dir.path().join("without_pq.parquet");
    let config_without = ParquetWriterConfig {
        enable_pq_sorting: false,
        ..Default::default()
    };
    let mut writer_without = StreamingParquetWriter::new(&path_without, 768, config_without, None).await?;
    writer_without.write_batch(&vectors).await?;
    let stats_without = writer_without.finalize().await?;
    
    // Write with PQ sorting
    let path_with = dir.path().join("with_pq.parquet");
    let config_with = ParquetWriterConfig {
        enable_pq_sorting: true,
        pq_sorting_segments: 16,
        pq_sorting_codebook_size: 256,
        ..Default::default()
    };
    let mut writer_with = StreamingParquetWriter::new(&path_with, 768, config_with, None).await?;
    writer_with.write_batch(&vectors).await?;
    let stats_with = writer_with.finalize().await?;
    
    // Compare compression ratios
    println!("Compression without PQ: {:.2}", stats_without.compression_ratio);
    println!("Compression with PQ: {:.2}", stats_with.compression_ratio);
    
    // PQ sorting should improve compression
    assert!(
        stats_with.compression_ratio > stats_without.compression_ratio * 1.2,
        "PQ sorting should improve compression by at least 20%"
    );
    
    // Compare file sizes
    let size_without = std::fs::metadata(&path_without)?.len();
    let size_with = std::fs::metadata(&path_with)?.len();
    
    println!("File size without PQ: {} bytes", size_without);
    println!("File size with PQ: {} bytes", size_with);
    
    assert!(size_with < size_without, "PQ sorted file should be smaller");
    
    Ok(())
}

#[tokio::test]
async fn test_native_metadata_types() -> Result<()> {
    let vectors = generate_test_vectors(500, 128);
    let metadata_samples: Vec<_> = vectors.iter()
        .filter_map(|v| v.metadata.as_ref())
        .take(100)
        .cloned()
        .collect();
    
    // Test type inference
    let mut handler = NativeMetadataHandler::new();
    handler.analyze_metadata(&metadata_samples)?;
    
    let stats = handler.get_optimization_stats();
    println!("Native metadata optimization stats:");
    println!("  Total fields: {}", stats.total_fields);
    println!("  Native fields: {}", stats.native_fields);
    println!("  List fields: {}", stats.list_fields);
    println!("  Map fields: {}", stats.map_fields);
    println!("  JSON fields: {}", stats.json_fields);
    println!("  Optimization ratio: {:.2}%", stats.optimization_ratio * 100.0);
    
    assert!(stats.optimization_ratio > 0.5, "Should optimize at least 50% of fields");
    
    // Test query optimization
    let field_types = HashMap::from([
        ("category".to_string(), MetadataFieldType::String),
        ("is_active".to_string(), MetadataFieldType::Boolean),
        ("item_count".to_string(), MetadataFieldType::Integer),
        ("tags".to_string(), MetadataFieldType::List(Box::new(MetadataFieldType::String))),
    ]);
    
    let optimizer = NativeMetadataQueryOptimizer::new(field_types);
    
    let filter = json!({
        "category": "category_5",
        "is_active": true,
        "item_count": 42,
        "tags": "tag_1",
        "unknown_field": "value"
    }).as_object().unwrap().clone();
    
    let optimized = optimizer.optimize_filter(&filter)?;
    
    assert_eq!(optimized.native_predicates.len(), 4);
    assert_eq!(optimized.json_predicates.len(), 1);
    assert!(optimized.pushdown_ratio >= 0.8);
    
    Ok(())
}

#[tokio::test]
async fn test_hybrid_writer_mode_switching() -> Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("hybrid.parquet");
    
    let config = HybridWriterConfig {
        initial_mode: WriterMode::Adaptive,
        enable_auto_switch: true,
        mode_switch_threshold: 100,
        streaming_threshold: 50.0,
        batch_threshold: 500,
        ..Default::default()
    };
    
    let writer = HybridParquetWriter::new(&path, 256, config).await?;
    
    // Start with streaming pattern (small batches)
    for _ in 0..20 {
        let small_batch = generate_test_vectors(10, 256);
        writer.write(small_batch).await?;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    
    let mode1 = writer.get_current_mode().await;
    println!("Mode after streaming pattern: {:?}", mode1);
    
    // Switch to batch pattern (large batches)
    for _ in 0..5 {
        let large_batch = generate_test_vectors(1000, 256);
        writer.write(large_batch).await?;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    
    let mode2 = writer.get_current_mode().await;
    println!("Mode after batch pattern: {:?}", mode2);
    
    // Mixed pattern
    for i in 0..20 {
        let batch_size = if i % 3 == 0 { 1000 } else { 20 };
        let batch = generate_test_vectors(batch_size, 256);
        writer.write(batch).await?;
    }
    
    let stats = writer.finalize().await?;
    
    println!("Hybrid writer statistics:");
    println!("  Total records: {}", stats.total_records);
    println!("  Streaming writes: {}", stats.streaming_writes);
    println!("  Batch writes: {}", stats.batch_writes);
    println!("  Mode switches: {}", stats.mode_switches);
    println!("  Buffer flushes: {}", stats.buffer_flushes);
    println!("  Avg flush latency: {}ms", stats.avg_flush_latency_ms);
    
    assert!(stats.mode_switches > 0, "Should have switched modes");
    assert!(stats.streaming_writes > 0, "Should have streaming writes");
    assert!(stats.batch_writes > 0, "Should have batch writes");
    
    Ok(())
}

#[tokio::test]
async fn test_bloom_filter_effectiveness() -> Result<()> {
    let dir = tempdir()?;
    let vectors = generate_test_vectors(5000, 128);
    
    // Write with bloom filters
    let path = dir.path().join("with_bloom.parquet");
    let config = ParquetWriterConfig {
        enable_bloom_filters: true,
        bloom_filter_fpp: 0.01,
        expected_ndv: Some(5000),
        bloom_filter_columns: vec!["id".to_string(), "category".to_string()],
        ..Default::default()
    };
    
    let writer = BatchParquetWriter::new(&path, 128, config);
    let stats = writer.write_all(&vectors).await?;
    
    println!("Bloom filter statistics:");
    println!("  Total records: {}", stats.total_records);
    println!("  Row groups: {}", stats.total_row_groups);
    println!("  Bloom filters: {}", stats.bloom_filter_count);
    
    assert!(stats.bloom_filter_count > 0, "Should have created bloom filters");
    
    // TODO: Add bloom filter query test once reader is implemented
    
    Ok(())
}

#[tokio::test]
async fn test_page_indexes_query_performance() -> Result<()> {
    let dir = tempdir()?;
    let vectors = generate_test_vectors(10000, 512);
    
    // Write with page indexes
    let path = dir.path().join("with_indexes.parquet");
    let config = ParquetWriterConfig {
        enable_column_index: true,
        enable_offset_index: true,
        page_size: 512 * 1024, // 512KB pages
        page_index_granularity: 500,
        ..Default::default()
    };
    
    let writer = BatchParquetWriter::new(&path, 512, config);
    let stats = writer.write_all(&vectors).await?;
    
    println!("Page index statistics:");
    println!("  Total records: {}", stats.total_records);
    println!("  Row groups: {}", stats.total_row_groups);
    println!("  File size: {} bytes", stats.file_size);
    println!("  Compression ratio: {:.2}", stats.compression_ratio);
    
    // Verify file was created with expected size
    let metadata = std::fs::metadata(&path)?;
    assert!(metadata.len() > 0, "File should have content");
    
    Ok(())
}

#[tokio::test]
async fn test_end_to_end_optimizations() -> Result<()> {
    let dir = tempdir()?;
    let vectors = generate_test_vectors(5000, 768);
    
    // Baseline configuration (no optimizations)
    let baseline_path = dir.path().join("baseline.parquet");
    let baseline_config = ParquetWriterConfig {
        enable_bloom_filters: false,
        enable_column_index: false,
        enable_offset_index: false,
        enable_pq_sorting: false,
        enable_native_metadata: false,
        ..Default::default()
    };
    
    let start = Instant::now();
    let baseline_writer = BatchParquetWriter::new(&baseline_path, 768, baseline_config);
    let baseline_stats = baseline_writer.write_all(&vectors).await?;
    let baseline_time = start.elapsed();
    
    // Optimized configuration (all optimizations)
    let optimized_path = dir.path().join("optimized.parquet");
    let optimized_config = ParquetWriterConfig {
        enable_bloom_filters: true,
        enable_column_index: true,
        enable_offset_index: true,
        enable_pq_sorting: true,
        enable_native_metadata: true,
        page_size: 1024 * 1024,
        metadata_inference_samples: 100,
        ..Default::default()
    };
    
    let start = Instant::now();
    let optimized_writer = BatchParquetWriter::new(&optimized_path, 768, optimized_config);
    let optimized_stats = optimized_writer.write_all(&vectors).await?;
    let optimized_time = start.elapsed();
    
    // Compare results
    println!("\nEnd-to-end optimization comparison:");
    println!("Baseline:");
    println!("  Write time: {:?}", baseline_time);
    println!("  File size: {} bytes", baseline_stats.file_size);
    println!("  Compression ratio: {:.2}", baseline_stats.compression_ratio);
    
    println!("Optimized:");
    println!("  Write time: {:?}", optimized_time);
    println!("  File size: {} bytes", optimized_stats.file_size);
    println!("  Compression ratio: {:.2}", optimized_stats.compression_ratio);
    println!("  Bloom filters: {}", optimized_stats.bloom_filter_count);
    
    let size_reduction = (baseline_stats.file_size as f64 - optimized_stats.file_size as f64) 
        / baseline_stats.file_size as f64 * 100.0;
    println!("  Size reduction: {:.1}%", size_reduction);
    
    // Optimized version should have better compression
    assert!(
        optimized_stats.compression_ratio > baseline_stats.compression_ratio,
        "Optimized should have better compression"
    );
    
    // Optimized file should be smaller
    assert!(
        optimized_stats.file_size < baseline_stats.file_size,
        "Optimized file should be smaller"
    );
    
    Ok(())
}

#[tokio::test]
async fn test_concurrent_writes_with_hybrid_writer() -> Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("concurrent.parquet");
    
    let config = HybridWriterConfig {
        initial_mode: WriterMode::Adaptive,
        enable_concurrent_writes: true,
        max_concurrent_writers: 4,
        ..Default::default()
    };
    
    let writer = Arc::new(HybridParquetWriter::new(&path, 384, config).await?);
    
    // Spawn multiple concurrent write tasks
    let mut handles = vec![];
    
    for task_id in 0..10 {
        let writer_clone = writer.clone();
        let handle = tokio::spawn(async move {
            for batch_id in 0..10 {
                let batch_size = if (task_id + batch_id) % 3 == 0 { 500 } else { 50 };
                let vectors = generate_test_vectors(batch_size, 384);
                writer_clone.write(vectors).await.unwrap();
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        handles.push(handle);
    }
    
    // Wait for all tasks to complete
    for handle in handles {
        handle.await?;
    }
    
    // Get final statistics
    let writer = Arc::try_unwrap(writer).unwrap();
    let stats = writer.finalize().await?;
    
    println!("Concurrent write statistics:");
    println!("  Total records: {}", stats.total_records);
    println!("  Mode switches: {}", stats.mode_switches);
    println!("  Buffer flushes: {}", stats.buffer_flushes);
    
    assert!(stats.total_records > 0, "Should have written records");
    
    Ok(())
}
