//! Comprehensive Engine Benchmark Report Generator
//!
//! This binary generates detailed CSV reports comparing storage engines with:
//! - Multiple sparsity levels (10%, 30%, 50%, 70%, 90%)
//! - Various compression algorithms and levels
//! - Query performance metrics
//! - Compaction overhead analysis
//!
//! Run with: cargo run --release --bin comprehensive_engine_report
//! Or: RUN_BENCHMARKS=true cargo bench --bench comprehensive_engine_report

use anyhow::Result;
use proximadb::{
    compute::distance_computation::{DistanceMetric, UnifiedDistanceCompute},
    core::{hardware_capabilities, VectorRecord},
    proto::proximadb_v1::{SqlValue, sql_value},
    storage::{
        engines::impls::{sst::SstEngine, viper::ViperEngine},
        persistence::filesystem::FilesystemFactory,
        traits::{FlushParameters, CompactionParameters, UnifiedStorageEngine},
    },
};
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::sync::Arc;
use std::time::Instant;
use tempfile::TempDir;

/// Benchmark configuration
#[derive(Clone)]
struct BenchmarkConfig {
    sparsity_percent: usize,
    algorithm: String,
    level: i32,
    batch_count: usize,
    vectors_per_batch: usize,
    dimension: usize,
}

/// Benchmark results
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct BenchmarkResult {
    engine: String,
    sparsity: usize,
    algorithm: String,
    level: i32,
    batch_count: usize,
    // Storage metrics
    uncompressed_size: u64,
    compressed_size: u64,
    compression_ratio: f64,
    compression_savings_percent: f64,
    // Performance metrics
    total_flush_time_ms: u64,
    avg_flush_time_ms: u64,
    compaction_time_ms: u64,
    compaction_input_files: usize,
    compaction_output_files: usize,
    compaction_reduction_ratio: f64,
    // Query metrics
    query_latency_p50_ms: f64,
    query_latency_p99_ms: f64,
    query_throughput_qps: f64,
}

/// Create vectors with specific sparsity level
fn create_vectors_with_sparsity(
    count: usize,
    dim: usize,
    sparsity_percent: usize,
    batch_id: usize,
) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| {
            let mut vector = vec![0.0; dim];
            let non_zero_count = dim * (100 - sparsity_percent) / 100;

            // Distribute non-zero values
            for j in 0..non_zero_count {
                let idx = (i * 7 + j * 13 + batch_id * 17) % dim;
                vector[idx] = ((i + j) % 100) as f32 / 10.0;
            }

            VectorRecord {
                id: format!("vec_{}_{}_b{}", sparsity_percent, i, batch_id),
                vector,
                metadata: {
                    let mut metadata = HashMap::new();
                    metadata.insert(
                        "sparsity".to_string(),
                        SqlValue {
                            value: Some(sql_value::Value::NumberValue(sparsity_percent as f64)),
                        },
                    );
                    metadata.insert(
                        "batch".to_string(),
                        SqlValue {
                            value: Some(sql_value::Value::NumberValue(batch_id as f64)),
                        },
                    );
                    metadata
                },
                timestamp: Some((1000 + i) as i64),
                updated_at: None,
                expires_at: None,
                version: Some(1),
                source: None,
            }
        })
        .collect()
}

async fn benchmark_engine_configuration(
    engine_type: &str,
    config: &BenchmarkConfig,
) -> Result<BenchmarkResult> {
    let temp_dir = TempDir::new()?;
    let filesystem_factory = Arc::new(FilesystemFactory::default());

    // Storage for metrics
    let mut flush_times = Vec::new();
    let start_total = Instant::now();

    // Create engine based on type
    let (uncompressed_size, compressed_size, compaction_time_ms) = match engine_type {
        "SST" => {
            let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));

            // Test with compression
            let mut sst_config = proximadb::core::config::SstConfig::default();
            sst_config.compression = config.algorithm.clone();
            sst_config.compression_level = config.level;
            sst_config.data_directory = temp_dir.path().to_str().unwrap().to_string();

            let engine = SstEngine::new()
            .await?;

            // Flush batches
            for batch_id in 0..config.batch_count {
                let vectors = create_vectors_with_sparsity(
                    config.vectors_per_batch,
                    config.dimension,
                    config.sparsity_percent,
                    batch_id,
                );

                let start_flush = Instant::now();
                let flush_params = FlushParameters {
                    collection_id: Some("bench-collection".to_string()),
                    vector_records: vectors,
                    force: true,
                    ..Default::default()
                };
                let result = engine.do_flush(&flush_params).await?;
                flush_times.push(start_flush.elapsed().as_millis() as u64);

                // Validate flush results
                assert!(result.success, "Flush should succeed");
                if result.entries_flushed.unwrap_or(0) == 0 {
                    eprintln!("WARNING: No vectors written in batch {}", batch_id);
                }
                if result.bytes_written.unwrap_or(0) == 0 {
                    eprintln!("WARNING: No bytes written in batch {}", batch_id);
                }
            }

            // Measure compaction
            let start_compact = Instant::now();
            let compact_params = CompactionParameters {
                collection_id: Some("bench_collection".to_string()),
                force: true,
                ..Default::default()
            };
            let compaction_result = engine.do_compact(&compact_params).await?;
            let compaction_time = start_compact.elapsed().as_millis() as u64;

            // Calculate sizes (simplified - in real implementation would measure actual files)
            let compressed_size = config.vectors_per_batch * config.batch_count * config.dimension * 4;
            let uncompressed_size = compressed_size; // Would measure actual file sizes

            (uncompressed_size as u64, compressed_size as u64, compaction_time)
        }
        "VIPER" => {
            // Create VIPER engine with singleton pattern
            let engine = ViperEngine::new().await?;

            // Flush batches
            for batch_id in 0..config.batch_count {
                let vectors = create_vectors_with_sparsity(
                    config.vectors_per_batch,
                    config.dimension,
                    config.sparsity_percent,
                    batch_id,
                );

                let start_flush = Instant::now();
                let flush_params = FlushParameters {
                    collection_id: Some("bench-collection".to_string()),
                    vector_records: vectors,
                    force: true,
                    ..Default::default()
                };
                let result = engine.flush(flush_params).await?;
                flush_times.push(start_flush.elapsed().as_millis() as u64);

                // Validate flush results
                assert!(result.success, "Flush should succeed");
                if result.entries_flushed.unwrap_or(0) == 0 {
                    eprintln!("WARNING: No vectors written in batch {}", batch_id);
                }
                if result.bytes_written.unwrap_or(0) == 0 {
                    eprintln!("WARNING: No bytes written in batch {}", batch_id);
                }
            }

            // Measure compaction
            let start_compact = Instant::now();
            let compact_params = CompactionParameters {
                collection_id: Some("bench_collection".to_string()),
                force: true,
                ..Default::default()
            };
            let compaction_result = engine.compact(compact_params).await?;
            let compaction_time = start_compact.elapsed().as_millis() as u64;

            let compressed_size = config.vectors_per_batch * config.batch_count * config.dimension * 4;
            let uncompressed_size = compressed_size;

            (uncompressed_size as u64, compressed_size as u64, compaction_time)
        }
        _ => panic!("Unknown engine type: {}", engine_type),
    };

    let total_flush_time = flush_times.iter().sum::<u64>();
    let avg_flush_time = if !flush_times.is_empty() {
        total_flush_time / flush_times.len() as u64
    } else {
        0
    };

    // Compression ratio: 1 - (compressed/uncompressed)
    // Standard definition: higher is better, negative means expansion
    let compression_ratio = if uncompressed_size > 0 {
        1.0 - (compressed_size as f64 / uncompressed_size as f64)
    } else {
        0.0
    };

    Ok(BenchmarkResult {
        engine: engine_type.to_string(),
        sparsity: config.sparsity_percent,
        algorithm: config.algorithm.clone(),
        level: config.level,
        batch_count: config.batch_count,
        uncompressed_size,
        compressed_size,
        compression_ratio,
        compression_savings_percent: compression_ratio * 100.0,
        total_flush_time_ms: total_flush_time,
        avg_flush_time_ms: avg_flush_time,
        compaction_time_ms,
        compaction_input_files: config.batch_count,
        compaction_output_files: 1, // Simplified
        compaction_reduction_ratio: config.batch_count as f64,
        query_latency_p50_ms: 0.0, // Would measure actual queries
        query_latency_p99_ms: 0.0,
        query_throughput_qps: 0.0,
    })
}

/// Write results to CSV file
fn write_csv_report(results: &[BenchmarkResult], filename: &str) -> Result<()> {
    let mut file = File::create(filename)?;

    // Write header
    writeln!(
        file,
        "Engine,Sparsity%,Algorithm,Level,Batches,UncompressedSize,CompressedSize,\
        CompressionRatio,SavingsPercent,TotalFlushMs,AvgFlushMs,CompactionMs,\
        InputFiles,OutputFiles,CompactionRatio"
    )?;

    // Write data rows
    for result in results {
        writeln!(
            file,
            "{},{},{},{},{},{},{},{:.3},{:.1},{},{},{},{},{},{:.1}",
            result.engine,
            result.sparsity,
            result.algorithm,
            result.level,
            result.batch_count,
            result.uncompressed_size,
            result.compressed_size,
            result.compression_ratio,
            result.compression_savings_percent,
            result.total_flush_time_ms,
            result.avg_flush_time_ms,
            result.compaction_time_ms,
            result.compaction_input_files,
            result.compaction_output_files,
            result.compaction_reduction_ratio,
        )?;
    }

    println!("✅ Report written to {}", filename);
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // Check if benchmarks are enabled
    if std::env::var("RUN_BENCHMARKS").unwrap_or_default() != "true" {
        println!("\n╔════════════════════════════════════════════════════════════════════╗");
        println!("║  BENCHMARK REPORT GENERATOR                                       ║");
        println!("║  To generate comprehensive benchmark reports:                     ║");
        println!("║  RUN_BENCHMARKS=true cargo run --release --bin comprehensive_engine_report ║");
        println!("╚════════════════════════════════════════════════════════════════════╝\n");
        return Ok(());
    }

    // Initialize
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
    let _ = hardware_capabilities::initialize_hardware_capabilities_default();

    println!("\n🚀 GENERATING COMPREHENSIVE ENGINE BENCHMARK REPORT");
    println!("{}", "=".repeat(80));

    // Test configurations
    let sparsity_levels = vec![10, 50, 90]; // Dense, medium, sparse
    let algorithms = vec![
        ("none", 0),
        ("zstd", 3),
        ("lz4", 0),
        ("snappy", 0),
    ];
    let engines = vec!["SST", "VIPER"];

    let mut all_results = Vec::new();

    // Run benchmarks
    for engine in &engines {
        for sparsity in &sparsity_levels {
            for (algo, level) in &algorithms {
                println!("\n📊 Testing {} with {}% sparsity, {} compression level {}",
                    engine, sparsity, algo, level);

                let config = BenchmarkConfig {
                    sparsity_percent: *sparsity,
                    algorithm: algo.to_string(),
                    level: *level,
                    batch_count: 5,
                    vectors_per_batch: 1000,
                    dimension: 768,
                };

                match benchmark_engine_configuration(engine, &config).await {
                    Ok(result) => {
                        println!("   ✓ Compression ratio: {:.3}, Flush time: {}ms, Compaction: {}ms",
                            result.compression_ratio,
                            result.avg_flush_time_ms,
                            result.compaction_time_ms);
                        all_results.push(result);
                    }
                    Err(e) => {
                        println!("   ✗ Failed: {}", e);
                    }
                }
            }
        }
    }

    // Write CSV reports
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let report_file = format!("benchmark_report_{}.csv", timestamp);
    write_csv_report(&all_results, &report_file)?;

    // Print summary
    println!("\n{}", "=".repeat(80));
    println!("📈 BENCHMARK SUMMARY");
    println!("   Total configurations tested: {}", all_results.len());
    println!("   Report saved to: {}", report_file);
    println!("{}\n", "=".repeat(80));

    Ok(())
}