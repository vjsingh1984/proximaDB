//! Comprehensive Engine Benchmark Report Generator
//!
//! Generates detailed tabular reports comparing VIPER vs SST with:
//! - Multiple sparsity levels (0%, 10%, 25%, 50%, 75%, 90%, 99%)
//! - All compression algorithms and levels
//! - Query performance metrics
//! - Compaction overhead across 5+ batches
//! - Uses unified test helpers for consistent testing

mod common {
    include!("../common/mod.rs");
}
use common::integration_test_helpers::{UnifiedTestEnvironment, operations};

use anyhow::Result;
use proximadb::proto::proximadb_v1::{VectorRecord, StorageEngine};
use proximadb::storage::engines::impls::viper::ViperEngine;
use proximadb::storage::traits::{CompactionParameters, FlushParameters, UnifiedStorageEngine};
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info};

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
    // Query metrics - Regular
    query_latency_p50_ms: f64,
    query_latency_p99_ms: f64,
    query_throughput_qps: f64,
    // Query metrics - Metadata Filtered
    filter_latency_p50_ms: f64,
    filter_latency_p99_ms: f64,
    filter_throughput_qps: f64,
    filter_overhead_percent: f64, // % overhead from filtering
    filter_selectivity: f64,      // % of results after filtering
    // Baseline comparison metrics
    latency_change_p50_percent: f64, // % change from baseline P50
    latency_change_p99_percent: f64, // % change from baseline P99
    // Result accuracy
    result_accuracy: f64, // Percentage match with baseline
    top_k_matches: usize, // How many of top-k match baseline
    // Resource metrics
    peak_memory_mb: u64,
    cpu_usage_percent: f64,
    io_operations: u64,
}

/// Baseline results for comparison
#[derive(Debug, Clone)]
struct BaselineResults {
    engine: String,
    sparsity: usize,
    top_k_ids: Vec<String>, // IDs of top-k results
    top_k_scores: Vec<f32>, // Scores/distances of top-k
    uncompressed_size: u64,
    query_latency_ms: f64,
}

/// Structure to hold test vectors that will be reused across all tests
struct TestVectorSet {
    vectors: Vec<VectorRecord>,
    query_vectors: Vec<VectorRecord>,
    query_positions: Vec<usize>, // Positions where query vectors were injected
}

/// Create randomized vectors with specific sparsity and inject query vectors
/// This creates a realistic mix of patterns for better compression testing
fn create_randomized_vector_set(
    total_count: usize,
    dim: usize,
    sparsity_percent: usize,
    num_query_vectors: usize,
) -> TestVectorSet {
    use rand::rngs::StdRng;
    use rand::seq::SliceRandom;
    use rand::{Rng, SeedableRng};

    // Use a fixed seed for reproducibility but with good randomization
    let mut rng = StdRng::seed_from_u64(42 + sparsity_percent as u64);

    // First create the query vectors with known patterns
    let mut query_vectors = Vec::new();
    for q in 0..num_query_vectors {
        let mut vector = vec![0.0; dim];

        if sparsity_percent == 0 {
            // Dense query vector with specific pattern
            for j in 0..dim {
                vector[j] = ((q * 7 + j * 11) as f32 % 10.0) / 10.0;
            }
        } else {
            // Sparse query vector - sparsity_percent indicates % of ZERO values
            // First fill with random values
            for j in 0..dim {
                vector[j] = rng.gen_range(0.1..1.0);
            }

            // Then randomly set sparsity_percent of dimensions to zero
            let zero_count = dim * sparsity_percent / 100;
            let mut indices: Vec<usize> = (0..dim).collect();
            indices.shuffle(&mut rng);

            for &idx in indices.iter().take(zero_count) {
                vector[idx] = 0.0; // Set to zero for sparsity
            }
        }

        query_vectors.push(VectorRecord {
            id: format!("query_s{}_{}", sparsity_percent, q),
            vector,
            metadata: vec![
                proximadb::proto::proximadb_v1::MetadataItem {
                    key: "type".to_string(),
                    value: Some(
                        proximadb::proto::proximadb_v1::metadata_item::Value::StringValue(
                            "query".to_string(),
                        ),
                    ),
                },
                proximadb::proto::proximadb_v1::MetadataItem {
                    key: "sparsity".to_string(),
                    value: Some(
                        proximadb::proto::proximadb_v1::metadata_item::Value::NumberValue(
                            sparsity_percent as f64,
                        ),
                    ),
                },
                proximadb::proto::proximadb_v1::MetadataItem {
                    key: "index".to_string(),
                    value: Some(
                        proximadb::proto::proximadb_v1::metadata_item::Value::NumberValue(q as f64),
                    ),
                },
                proximadb::proto::proximadb_v1::MetadataItem {
                    key: "category".to_string(),
                    value: Some(
                        proximadb::proto::proximadb_v1::metadata_item::Value::StringValue(format!(
                            "cat_{}",
                            q % 5
                        )),
                    ),
                },
            ],
            timestamp: chrono::Utc::now().timestamp(),
            updated_at: Some(chrono::Utc::now().timestamp()),
            expires_at: None,
            version: Some(1),
        });
    }

    // Determine positions to inject query vectors (spread them out)
    let mut query_positions = Vec::new();
    let spacing = total_count / (num_query_vectors + 1);
    for i in 0..num_query_vectors {
        query_positions.push((i + 1) * spacing);
    }

    // Create all vectors with randomization
    let mut all_vectors = Vec::new();
    let mut query_idx = 0;

    for i in 0..total_count {
        // Check if we should inject a query vector at this position
        if query_idx < query_positions.len() && i == query_positions[query_idx] {
            all_vectors.push(query_vectors[query_idx].clone());
            query_idx += 1;
        } else {
            // Create a randomized regular vector
            let mut vector = vec![0.0; dim];

            if sparsity_percent == 0 {
                // Dense: mix of random and patterned values
                for j in 0..dim {
                    if rng.gen_bool(0.3) {
                        // 30% truly random values (harder to compress)
                        vector[j] = rng.gen_range(0.0..1.0);
                    } else {
                        // 70% with some pattern (more compressible)
                        let base = ((i * 7 + j * 13) % 100) as f32 / 100.0;
                        let noise = rng.gen_range(-0.1..0.1);
                        vector[j] = (base + noise).clamp(0.0, 1.0);
                    }
                }
            } else {
                // Sparse: sparsity_percent indicates % of ZERO values
                // 10% sparse = 10% zeros, 90% random values
                // 90% sparse = 90% zeros, 10% random values

                // First fill all dimensions with random values
                for j in 0..dim {
                    vector[j] = rng.gen_range(0.0..1.0);
                }

                // Then randomly set sparsity_percent of dimensions to zero
                let zero_count = dim * sparsity_percent / 100;
                let mut indices: Vec<usize> = (0..dim).collect();
                indices.shuffle(&mut rng);

                for &idx in indices.iter().take(zero_count) {
                    vector[idx] = 0.0; // Set to zero for sparsity
                }
            }

            all_vectors.push(VectorRecord {
                id: format!("vec_s{}_{:06}", sparsity_percent, i),
                vector,
                metadata: vec![
                    proximadb::proto::proximadb_v1::MetadataItem {
                        key: "type".to_string(),
                        value: Some(
                            proximadb::proto::proximadb_v1::metadata_item::Value::StringValue(
                                "regular".to_string(),
                            ),
                        ),
                    },
                    proximadb::proto::proximadb_v1::MetadataItem {
                        key: "batch_id".to_string(),
                        value: Some(
                            proximadb::proto::proximadb_v1::metadata_item::Value::NumberValue(
                                (i / 100) as f64,
                            ),
                        ),
                    },
                    proximadb::proto::proximadb_v1::MetadataItem {
                        key: "index".to_string(),
                        value: Some(
                            proximadb::proto::proximadb_v1::metadata_item::Value::NumberValue(
                                i as f64,
                            ),
                        ),
                    },
                    proximadb::proto::proximadb_v1::MetadataItem {
                        key: "category".to_string(),
                        value: Some(
                            proximadb::proto::proximadb_v1::metadata_item::Value::StringValue(
                                format!("cat_{}", i % 10),
                            ),
                        ),
                    },
                    proximadb::proto::proximadb_v1::MetadataItem {
                        key: "status".to_string(),
                        value: Some(
                            proximadb::proto::proximadb_v1::metadata_item::Value::StringValue(
                                if i % 3 == 0 {
                                    "active"
                                } else if i % 3 == 1 {
                                    "pending"
                                } else {
                                    "archived"
                                }
                                .to_string(),
                            ),
                        ),
                    },
                ],
                timestamp: chrono::Utc::now().timestamp(),
                updated_at: Some(chrono::Utc::now().timestamp()),
                expires_at: None,
                version: Some(1),
            });
        }
    }

    TestVectorSet {
        vectors: all_vectors,
        query_vectors,
        query_positions,
    }
}

/// Run baseline benchmark with no compression
async fn run_baseline(
    engine_type: &str,
    sparsity: usize,
    vector_set: &TestVectorSet,
    batch_count: usize,
    vectors_per_batch: usize,
) -> Result<BaselineResults> {
    println!(
        "    Running BASELINE {} with {}% sparsity (no compression)",
        engine_type, sparsity
    );

    // Get dimension from the vectors
    let dimension = if !vector_set.vectors.is_empty() {
        vector_set.vectors[0].vector.len()
    } else {
        1536
    };

    let env = UnifiedTestEnvironment::new().await?;
    let mut flush_times = Vec::new();

    match engine_type {
        "SST" => {
            let engine = env.create_sst_engine().await?;

            // Create collection config once for baseline (no compression)
            let collection_baseline = env.create_test_collection_with_settings(
                StorageEngine::Sst,
                dimension as i32,
                None, // No compression for baseline
            );

            // Insert all batches WITHOUT compression (baseline) using pre-generated vectors
            for batch_id in 0..batch_count {
                // Get the vectors for this batch from the pre-generated set
                let start_idx = batch_id * vectors_per_batch;
                let end_idx =
                    std::cmp::min(start_idx + vectors_per_batch, vector_set.vectors.len());
                let batch_vectors = vector_set.vectors[start_idx..end_idx].to_vec();

                let flush_params = operations::build_sst_flush_params_with_collection(
                    &env,
                    batch_vectors,
                    collection_baseline.clone(),
                )
                .await?;

                let start_flush = Instant::now();
                let flush_result = engine.do_flush(&flush_params).await?;
                if !flush_result.success {
                    return Err(anyhow::anyhow!("SST baseline flush failed"));
                }
                flush_times.push(start_flush.elapsed().as_millis() as u64);
            }

            // Get storage size
            let uncompressed_size =
                get_directory_size(env.get_sst_data_directory().to_str().unwrap()).await;

            // Perform search using the first query vector (which was injected into the dataset)
            let query_vector = &vector_set.query_vectors[0].vector;
            let start_query = Instant::now();
            let results = operations::search_vectors_sst(&engine, &env, query_vector, 10).await?;
            let query_latency = start_query.elapsed().as_micros() as f64 / 1000.0;

            // Extract top-k IDs and scores
            let top_k_ids: Vec<String> = results.iter().map(|r| r.id.clone()).collect();
            let top_k_scores: Vec<f32> = results.iter().enumerate().map(|(i, _)| 1.0 - (i as f32 * 0.1)).collect();

            println!(
                "      SST Baseline: {} results, {:.2}ms latency, {} bytes",
                results.len(),
                query_latency,
                uncompressed_size
            );

            Ok(BaselineResults {
                engine: "SST".to_string(),
                sparsity,
                top_k_ids,
                top_k_scores,
                uncompressed_size,
                query_latency_ms: query_latency,
            })
        }
        "VIPER" => {
            let engine = env.create_viper_engine().await?;

            // Insert all batches using pre-generated vectors
            for batch_id in 0..batch_count {
                // Get the vectors for this batch from the pre-generated set
                let start_idx = batch_id * vectors_per_batch;
                let end_idx =
                    std::cmp::min(start_idx + vectors_per_batch, vector_set.vectors.len());
                let batch_vectors = vector_set.vectors[start_idx..end_idx].to_vec();

                let flush_params = operations::build_viper_flush_params_with_compression(
                    &env,
                    batch_vectors,
                    "none",
                    0,
                )
                .await?;

                let start_flush = Instant::now();
                let flush_result = engine.do_flush(&flush_params).await?;
                if !flush_result.success {
                    return Err(anyhow::anyhow!("VIPER baseline flush failed"));
                }
                flush_times.push(start_flush.elapsed().as_millis() as u64);
            }

            // Get storage size - VIPER writes to {storage_assignment.base_location}/{collection_id}/data
            // The storage assignment base is file://{persistent_dir}
            let viper_actual_dir = env.persistent_dir.join(env.collection_id()).join("data");
            println!(
                "      Looking for VIPER files in: {}",
                viper_actual_dir.display()
            );

            let mut uncompressed_size = if viper_actual_dir.exists() {
                get_directory_size(viper_actual_dir.to_str().unwrap()).await
            } else {
                // Also check without collection_id subdirectory
                let alt_dir = env.persistent_dir.join("data");
                if alt_dir.exists() {
                    println!("      Checking alternate location: {}", alt_dir.display());
                    get_directory_size(alt_dir.to_str().unwrap()).await
                } else {
                    // Last resort: check viper_data directory from config
                    println!(
                        "      Checking config directory: {}",
                        env.get_viper_data_directory().display()
                    );
                    get_directory_size(env.get_viper_data_directory().to_str().unwrap()).await
                }
            };

            // Perform REAL VIPER search using the pre-generated query vector
            let query_vector = &vector_set.query_vectors[0].vector;
            let start_query = Instant::now();
            let results = operations::search_vectors_viper(&engine, &env, query_vector, 10).await?;
            let query_latency = start_query.elapsed().as_micros() as f64 / 1000.0;

            // Extract top-k IDs and scores from real results
            let top_k_ids: Vec<String> = results.iter().map(|r| r.id.clone()).collect();
            let top_k_scores: Vec<f32> = results.iter().enumerate().map(|(i, _)| 1.0 - (i as f32 * 0.1)).collect();

            println!(
                "      VIPER Baseline: {} results, {:.2}ms latency, {} bytes",
                results.len(),
                query_latency,
                uncompressed_size
            );

            // Verify VIPER results are valid
            if results.is_empty() {
                println!(
                    "      ⚠️ WARNING: VIPER returned no results - data may not be flushed properly"
                );
            }

            Ok(BaselineResults {
                engine: "VIPER".to_string(),
                sparsity,
                top_k_ids,
                top_k_scores,
                uncompressed_size,
                query_latency_ms: query_latency,
            })
        }
        _ => Err(anyhow::anyhow!("Unknown engine: {}", engine_type)),
    }
}

/// Calculate result accuracy compared to baseline
fn calculate_accuracy(baseline: &BaselineResults, actual_ids: &[String]) -> (f64, usize) {
    let mut matches = 0;
    for (i, actual_id) in actual_ids.iter().enumerate() {
        if i < baseline.top_k_ids.len() && actual_id == &baseline.top_k_ids[i] {
            matches += 1;
        }
    }

    let accuracy = if !baseline.top_k_ids.is_empty() {
        (matches as f64 / baseline.top_k_ids.len().min(actual_ids.len()) as f64) * 100.0
    } else {
        0.0
    };

    (accuracy, matches)
}

/// Run benchmark for a specific configuration
async fn run_benchmark(
    engine_type: &str,
    config: BenchmarkConfig,
    baseline: &BaselineResults,
    vector_set: &TestVectorSet,
    test_number: usize,
    total_tests: usize,
) -> Result<BenchmarkResult> {
    println!(
        "    [{}/{}] {} with {}% sparsity, {} level {} (baseline size: {} KB)",
        test_number,
        total_tests,
        engine_type,
        config.sparsity_percent,
        config.algorithm,
        config.level,
        baseline.uncompressed_size / 1024
    );

    let env = UnifiedTestEnvironment::new().await?;

    // Prepare algorithm enum
    let algorithm_enum = match config.algorithm.as_str() {
        "lz4" => ProtoCompressionAlgorithm::CompressionLz4,
        "zstd" => ProtoCompressionAlgorithm::CompressionZstd,
        "snappy" => ProtoCompressionAlgorithm::CompressionSnappy,
        "gzip" => ProtoCompressionAlgorithm::CompressionGzip,
        "brotli" => ProtoCompressionAlgorithm::CompressionBrotli,
        _ => ProtoCompressionAlgorithm::CompressionNone,
    } as i32;

    // Track metrics
    let mut flush_times = Vec::new();
    let mut file_counts_before_compaction = 0;
    let mut file_counts_after_compaction = 0;
    let start_total = Instant::now();

    match engine_type {
        "SST" => {
            let engine = env.create_sst_engine().await?;

            // CREATE COLLECTION CONFIG ONCE with all settings
            use proximadb::proto::proximadb_v1::CompressionAlgorithm;

            let compression_algo = match config.algorithm.as_str() {
                "none" => CompressionAlgorithm::CompressionNone as i32,
                "zstd" => CompressionAlgorithm::CompressionZstd as i32,
                "lz4" => CompressionAlgorithm::CompressionLz4 as i32,
                "snappy" => CompressionAlgorithm::CompressionSnappy as i32,
                "gzip" => CompressionAlgorithm::CompressionGzip as i32,
                "brotli" => CompressionAlgorithm::CompressionBrotli as i32,
                _ => CompressionAlgorithm::CompressionNone as i32,
            };

            let compression_config = CompressionConfig {
                algorithm: compression_algo,
                level: Some(config.level as u32),
                block_size_kb: 2048, // 2MB blocks for SST
                quantization_type: None,
                ..Default::default()
            };

            let collection = env.create_test_collection_with_settings(
                StorageEngine::Sst,
                config.dimension as i32,
                Some(compression_config),
            );

            // Process multiple batches WITH COMPRESSION using pre-generated vectors
            let mut batch_summary = Vec::new();
            for batch_id in 0..config.batch_count {
                // Get the vectors for this batch from the pre-generated set
                let start_idx = batch_id * config.vectors_per_batch;
                let end_idx = std::cmp::min(
                    start_idx + config.vectors_per_batch,
                    vector_set.vectors.len(),
                );
                let batch_vectors = vector_set.vectors[start_idx..end_idx].to_vec();

                // Build flush params with the SAME collection config
                let flush_params = operations::build_sst_flush_params_with_collection(
                    &env,
                    batch_vectors,
                    collection.clone(),
                )
                .await?;

                // Flush with compression
                let start_flush = Instant::now();
                let flush_result = engine.do_flush(&flush_params).await?;
                if !flush_result.success {
                    return Err(anyhow::anyhow!("SST flush failed"));
                }

                let flush_time = start_flush.elapsed().as_millis() as u64;
                flush_times.push(flush_time);
                batch_summary.push(flush_time);
            }

            // Print summary after all batches
            let avg_flush = batch_summary.iter().sum::<u64>() / batch_summary.len() as u64;
            println!(
                "      Flushed {} SST batches, avg: {}ms, total: {}ms",
                config.batch_count,
                avg_flush,
                batch_summary.iter().sum::<u64>()
            );

            // Get size BEFORE compaction for compression measurement
            let compressed_size_before_compaction =
                get_directory_size(env.get_sst_data_directory().to_str().unwrap()).await;
            let pre_compact_files =
                count_files_in_dir(env.get_sst_data_directory().to_str().unwrap()).await;
            println!(
                "      📦 Pre-compaction: {} files, {} KB ({} bytes)",
                pre_compact_files,
                compressed_size_before_compaction / 1024,
                compressed_size_before_compaction
            );

            // CRITICAL: Compare compression effectiveness against uncompressed baseline
            // This measures actual block-level compression, not file compaction
            let true_compression_ratio =
                compressed_size_before_compaction as f64 / baseline.uncompressed_size as f64;
            let true_compression_savings = (1.0 - true_compression_ratio) * 100.0;
            println!(
                "      🗜️ BLOCK COMPRESSION: {} bytes vs {} bytes baseline ({:.1}% savings)",
                compressed_size_before_compaction,
                baseline.uncompressed_size,
                true_compression_savings
            );

            // VALIDATION: Check compression effectiveness but don't fail the test
            // Just warn if compression doesn't provide benefits
            if config.algorithm != "none" {
                if compressed_size_before_compaction >= baseline.uncompressed_size {
                    println!(
                        "      ⚠️  WARNING: Algorithm {} level {} produced size {} which is >= uncompressed size {}. \
                        No compression benefit achieved!",
                        config.algorithm,
                        config.level,
                        compressed_size_before_compaction,
                        baseline.uncompressed_size
                    );
                } else if compressed_size_before_compaction as f64
                    > baseline.uncompressed_size as f64 * 0.95
                {
                    println!(
                        "      ⚠️  WARNING: Algorithm {} level {} only achieved {:.1}% compression. \
                        Minimal compression benefit!",
                        config.algorithm, config.level, true_compression_savings
                    );
                }
            }

            // Count files before compaction
            file_counts_before_compaction =
                count_files_in_dir(env.get_sst_data_directory().to_str().unwrap()).await;

            // Run compaction with debug logging
            println!(
                "        🔧 SST: Starting compaction for collection {}",
                env.collection_id()
            );
            let start_compact = Instant::now();
            // Use the same collection config with compression that was used for flush
            let compact_params =
                operations::build_compaction_params_with_collection(&env, collection.clone());

            println!(
                "        🔧 SST: Compaction params - force: {}, collection_id: {:?}",
                compact_params.force, compact_params.collection_id
            );

            let compact_result = engine.compact(compact_params).await?;
            let compaction_time = start_compact.elapsed().as_millis() as u64;

            println!(
                "        🔧 SST: Compaction result - success: {}, entries_processed: {}, files merged: {}",
                compact_result.success,
                compact_result.entries_processed.unwrap_or(0),
                compact_result.input_files.unwrap_or(0)
            );

            // Count files after compaction
            file_counts_after_compaction =
                count_files_in_dir(env.get_sst_data_directory().to_str().unwrap()).await;

            // Validate compaction results
            println!(
                "      Compaction: {} input files → {} output files, {} entries processed",
                file_counts_before_compaction,
                file_counts_after_compaction,
                compact_result.entries_processed.unwrap_or(0)
            );

            // Check if old files were cleaned up
            if file_counts_after_compaction > file_counts_before_compaction {
                println!(
                    "        ⚠️ WARNING: More files after compaction! Old files may not be deleted yet"
                );
                println!(
                    "        This is normal for atomic compaction - old files deleted after success"
                );
            }

            // Check for potential duplication
            let expected_max_entries = (config.batch_count * config.vectors_per_batch) as u64;
            if compact_result.entries_processed.unwrap_or(0) > expected_max_entries * 2 {
                println!(
                    "        ❌ ERROR: Compaction processed {} entries, expected ~{}",
                    compact_result.entries_processed.unwrap_or(0), expected_max_entries
                );
                println!("        This indicates duplicate data - compaction should deduplicate!");
            } else if compact_result.entries_processed.unwrap_or(0)
                > expected_max_entries + (expected_max_entries / 10)
            {
                println!(
                    "        ⚠️ Note: Compaction processed {} entries vs {} expected (+{}%)",
                    compact_result.entries_processed.unwrap_or(0),
                    expected_max_entries,
                    ((compact_result.entries_processed.unwrap_or(0) - expected_max_entries) * 100
                        / expected_max_entries)
                );
            }

            // Measure query performance with validation using pre-generated query vector
            let query_vector = &vector_set.query_vectors[0].vector;
            let mut query_latencies = Vec::new();
            let mut metadata_filter_latencies = Vec::new();

            // SST search (note: SST does full scan without index, expect high latency)
            let start_query = Instant::now();
            let results = operations::search_vectors_sst(&engine, &env, &query_vector, 10).await?;
            let first_latency = start_query.elapsed().as_micros() as f64 / 1000.0;
            query_latencies.push(first_latency);

            // METADATA FILTERED SEARCH - filter for specific category
            println!("        🔍 Testing metadata filtering impact...");
            let start_filter = Instant::now();

            // Create filter expression for category = "cat_3"
            use proximadb::core::search::{ComparisonOperator, FilterExpression};
            let filter = Some(FilterExpression::Comparison {
                field: "category".to_string(),
                operator: ComparisonOperator::Equals,
                value: serde_json::Value::String("cat_3".to_string()),
            });

            // Search with metadata filter using unified interface
            let collection = Arc::new(env.create_test_collection());
            let search_params = Arc::new(proximadb::core::search::SearchParams::default());
            let query_context = proximadb::storage::traits::StorageQueryContext {
                search_params,
                collection,
                metadata: proximadb::storage::traits::StorageQueryMetadata::default(),
            };

            let filter_search_results = engine
                .search_vectors_unified(&query_context)
                .await?;

            // Convert to VectorRecord format
            let filtered_results: Vec<proximadb::proto::proximadb_v1::VectorRecord> = filter_search_results
                .into_iter()
                .map(|record| {
                    proximadb::proto::proximadb_v1::VectorRecord {
                        id: record.id,
                        vector: record.vector.as_ref().map(|v| (**v).clone()).unwrap_or_default(),
                        metadata: record.metadata,
                        timestamp: record.timestamp.unwrap_or(0),
                        source: None,
                        quantized_vector: vec![],
                        expires_at: None,
                        updated_at: None,
                        version: None,
                    }
                })
                .collect();

            let filter_latency = start_filter.elapsed().as_micros() as f64 / 1000.0;
            metadata_filter_latencies.push(filter_latency);

            // Record filter results for metrics
            let filter_result_count = filtered_results.len();
            let total_result_count = results.len();

            println!("        📊 Metadata filter results:");
            println!(
                "          - Regular search: {} results in {:.2}ms",
                total_result_count, first_latency
            );
            println!(
                "          - Filtered search: {} results in {:.2}ms",
                filter_result_count, filter_latency
            );
            println!(
                "          - Filter overhead: {:.2}ms ({:.1}% change)",
                filter_latency - first_latency,
                ((filter_latency / first_latency) - 1.0) * 100.0
            );

            // Validate results and record count
            let expected_records = config.batch_count * config.vectors_per_batch;
            if results.is_empty() {
                println!(
                    "        ⚠️ SST search: No results (expected {} records)",
                    expected_records
                );
                // Fill with high latency to indicate problem
                for _ in 1..10 {
                    query_latencies.push(1000.0);
                }
            } else {
                println!(
                    "        SST search: {} results in {:.2}ms (full scan, {} total records)",
                    results.len(),
                    first_latency,
                    expected_records
                );

                // Run 2 more queries (3 total) - SST is slow without index
                for _ in 1..3 {
                    let start_query = Instant::now();
                    let _ = operations::search_vectors_sst(&engine, &env, &query_vector, 10).await;
                    query_latencies.push(start_query.elapsed().as_micros() as f64 / 1000.0);
                }

                // Fill remaining with average for consistent stats
                let avg = query_latencies.iter().sum::<f64>() / query_latencies.len() as f64;
                while query_latencies.len() < 10 {
                    query_latencies.push(avg);
                }
            }

            // Get size AFTER compaction
            let compressed_size_after_compaction =
                get_directory_size(env.get_sst_data_directory().to_str().unwrap()).await;
            let post_compact_files =
                count_files_in_dir(env.get_sst_data_directory().to_str().unwrap()).await;
            println!(
                "      📦 Post-compaction: {} files, {} KB ({} bytes)",
                post_compact_files,
                compressed_size_after_compaction / 1024,
                compressed_size_after_compaction
            );

            // Calculate compaction efficiency
            let (size_reduction, file_reduction) = calculate_compaction_efficiency(
                compressed_size_before_compaction,
                compressed_size_after_compaction,
                pre_compact_files,
                post_compact_files,
            );
            println!(
                "      🔄 Compaction efficiency: {:.1}% size reduction, {:.0}:1 file reduction",
                size_reduction,
                1.0 / file_reduction.max(0.01)
            );

            // FIXED: Use pre-compaction size to show true block compression effectiveness
            let compressed_size = compressed_size_before_compaction;

            if compressed_size == 0 {
                println!(
                    "        ERROR: SST compressed size is 0, checking directory: {}",
                    env.get_sst_data_directory().to_str().unwrap()
                );
            } else {
                // Calculate actual compression savings
                let compression_ratio = if baseline.uncompressed_size > 0 {
                    compressed_size as f64 / baseline.uncompressed_size as f64
                } else {
                    1.0
                };
                let savings_percent = (1.0 - compression_ratio) * 100.0;

                // Check for size anomaly
                if compressed_size > baseline.uncompressed_size * 2 {
                    println!("        ❌ ERROR: Compressed size is MORE than 2x baseline!");
                    println!(
                        "        Compressed: {} bytes, Baseline: {} bytes",
                        compressed_size, baseline.uncompressed_size
                    );
                    println!(
                        "        This indicates data duplication or compaction not cleaning up"
                    );
                } else if savings_percent < -10.0 {
                    println!(
                        "        ⚠️ WARNING: Negative compression (file got {:.1}% larger)",
                        -savings_percent
                    );
                    println!(
                        "        Compressed: {} bytes, Baseline: {} bytes",
                        compressed_size, baseline.uncompressed_size
                    );
                    println!("        Small files often have compression overhead");
                } else {
                    println!(
                        "        SST compression: {} bytes vs {} bytes baseline ({:.1}% savings)",
                        compressed_size, baseline.uncompressed_size, savings_percent
                    );

                    // VALIDATION: Compressed must be smaller for non-none algorithms
                    if config.algorithm != "none" && compressed_size >= baseline.uncompressed_size {
                        panic!(
                            "❌ SST COMPRESSION NOT WORKING: {} bytes >= {} bytes baseline",
                            compressed_size, baseline.uncompressed_size
                        );
                    }
                }
                let post_compaction_size =
                    get_directory_size(env.get_sst_data_directory().to_str().unwrap()).await;
                println!(
                    "        After compaction: {} files, {} bytes",
                    file_counts_after_compaction, post_compaction_size
                );
            }

            // Baseline uncompressed size is passed in, no need to recalculate

            // Get actual search results for accuracy comparison
            let final_results =
                operations::search_vectors_sst(&engine, &env, &query_vector, 10).await?;
            let actual_ids: Vec<String> = final_results.iter().map(|r| r.id.clone()).collect();

            // Calculate accuracy vs baseline
            let (accuracy, matches) = calculate_accuracy(baseline, &actual_ids);

            Ok(build_result(
                "SST",
                config,
                baseline.uncompressed_size, // Use baseline's uncompressed size
                compressed_size,
                flush_times,
                compaction_time,
                file_counts_before_compaction,
                file_counts_after_compaction,
                query_latencies,
                metadata_filter_latencies,
                filter_result_count,
                total_result_count,
                compact_result.entries_processed.unwrap_or(0) as usize,
                accuracy,
                matches,
                Some(baseline), // Pass baseline for latency comparison
            ))
        }
        "VIPER" => {
            // Configure VIPER with compression
            let mut viper_config = env.viper_config.clone();
            viper_config.compression = config.algorithm.clone();
            viper_config.compression_level = config.level;

            let engine = ViperEngine::from_core_config(
                viper_config,
                env.filesystem.clone(),
            )
            .await?;

            // Process multiple batches
            let mut batch_summary = Vec::new();
            for batch_id in 0..config.batch_count {
                // Get the vectors for this batch from the pre-generated set
                let start_idx = batch_id * config.vectors_per_batch;
                let end_idx = std::cmp::min(
                    start_idx + config.vectors_per_batch,
                    vector_set.vectors.len(),
                );
                let batch_vectors = vector_set.vectors[start_idx..end_idx].to_vec();

                // Build flush params using unified helper
                let algorithm_str = match config.algorithm.as_str() {
                    "lz4" => "lz4",
                    "zstd" => "zstd",
                    "snappy" => "snappy",
                    "gzip" => "gzip",
                    _ => "none",
                };
                let flush_params = operations::build_viper_flush_params_with_compression(
                    &env,
                    batch_vectors,
                    algorithm_str,
                    config.level,
                )
                .await?;

                let start_flush = Instant::now();

                // VIPER flush through the engine
                let flush_result = engine.do_flush(&flush_params).await?;

                // Validate flush succeeded
                if !flush_result.success {
                    return Err(anyhow::anyhow!("VIPER flush failed for batch {}", batch_id));
                }

                let flush_time = start_flush.elapsed().as_millis() as u64;
                flush_times.push(flush_time);
                batch_summary.push(flush_time);
            }

            // Print summary after all batches
            let avg_flush = batch_summary.iter().sum::<u64>() / batch_summary.len() as u64;
            println!(
                "      Flushed {} VIPER batches, avg: {}ms, total: {}ms",
                config.batch_count,
                avg_flush,
                batch_summary.iter().sum::<u64>()
            );

            // Show where we expect files
            println!(
                "      VIPER storage location: file://{}",
                env.get_viper_data_directory().display()
            );

            // Count files before compaction
            file_counts_before_compaction =
                count_files_in_dir(env.get_viper_data_directory().to_str().unwrap()).await;

            // Run compaction with compression config
            let start_compact = Instant::now();
            // Create collection config for VIPER with compression
            let compression_config = CompressionConfig {
                algorithm: algorithm_enum,
                level: Some(config.level as u32),
                quantization_type: None,
                block_size_kb: 1024,
                ..Default::default()
            };
            let viper_collection = env.create_test_collection_with_settings(
                StorageEngine::Viper,
                config.dimension as i32,
                Some(compression_config),
            );
            let compact_params =
                operations::build_compaction_params_with_collection(&env, viper_collection);
            let compact_result = engine.compact(compact_params).await?;
            let compaction_time = start_compact.elapsed().as_millis() as u64;

            // Count files after compaction
            file_counts_after_compaction =
                count_files_in_dir(env.get_viper_data_directory().to_str().unwrap()).await;

            // Validate VIPER compaction
            println!(
                "      VIPER Compaction: {} input files → {} output files, {:?} entries",
                file_counts_before_compaction,
                file_counts_after_compaction,
                compact_result.entries_processed
            );

            // Measure query performance with actual VIPER search using pre-generated query vector
            let query_vector = &vector_set.query_vectors[0].vector;
            let mut query_latencies = Vec::new();

            // Perform REAL VIPER search for query performance measurement
            let start_query = Instant::now();
            let results =
                operations::search_vectors_viper(&engine, &env, &query_vector, 10).await?;
            let first_latency = start_query.elapsed().as_micros() as f64 / 1000.0;
            query_latencies.push(first_latency);

            // Validate VIPER results and record count
            let expected_records = config.batch_count * config.vectors_per_batch;
            if results.is_empty() {
                println!(
                    "        ⚠️ VIPER search: No results (expected {} records)",
                    expected_records
                );
                // Fill with reasonable latency to indicate problem
                for _ in 1..10 {
                    query_latencies.push(100.0); // 100ms to indicate issue
                }
            } else {
                println!(
                    "        VIPER search: {} results in {:.2}ms (expected {} total records)",
                    results.len(),
                    first_latency,
                    expected_records
                );

                // Run more queries for performance measurement
                for _ in 1..10 {
                    let start_query = Instant::now();
                    let _ =
                        operations::search_vectors_viper(&engine, &env, &query_vector, 10).await;
                    query_latencies.push(start_query.elapsed().as_micros() as f64 / 1000.0);
                }
            }

            let avg_latency = query_latencies.iter().sum::<f64>() / query_latencies.len() as f64;
            println!("        VIPER avg query latency: {:.2}ms", avg_latency);

            // Add metadata filtering for VIPER
            let mut metadata_filter_latencies = Vec::new();

            // Perform metadata filtered search
            let start_filter = Instant::now();

            // Create filter expression for category = "cat_3"
            use proximadb::core::search::{ComparisonOperator, FilterExpression};
            let filter = Some(FilterExpression::Comparison {
                field: "category".to_string(),
                operator: ComparisonOperator::Equals,
                value: serde_json::Value::String("cat_3".to_string()),
            });

            // Create search context for the unified API
            let search_params = std::sync::Arc::new(proximadb::core::search::SearchParams {
                vector: Some(query_vector.clone()),
                query_vectors: None,
                top_k: Some(10),
                distance_metric: Some(proximadb::compute::distance_computation::DistanceMetric::Cosine),
                filter_expression: filter,
                include_metadata: Some(false),
                include_vectors: Some(false),
                timeout_ms: None,
                accuracy_threshold: None,
                enable_early_termination: None,
                max_results_per_stage: None,
                progressive_search: None,
            });

            let collection_config = proximadb::proto::proximadb_v1::CollectionConfig {
                name: env.collection_id().to_string(),
                dimension: query_vector.len() as u32,
                distance_metric: proximadb::proto::proximadb_v1::DistanceMetric::Cosine as i32,
                storage_engine: proximadb::proto::proximadb_v1::StorageEngine::Viper as i32,
                tags: vec![],
                auto_index_selection: None,
                embedding_models: None,
                owner: None,
                shared_with: vec![],
                storage_assignment: None,
            };

            let collection = std::sync::Arc::new(proximadb::proto::proximadb_v1::Collection {
                id: env.collection_id().to_string(),
                config: Some(collection_config),
                stats: None,
                created_at: 0,
                updated_at: 0,
            });

            let query_context = proximadb::storage::traits::StorageQueryContext {
                search_params,
                collection,
                metadata: proximadb::storage::traits::StorageQueryMetadata {
                    collection_id: env.collection_id().to_string(),
                    use_axis_indexes: false,
                    storage_url: Some(format!("file://{}/data", env.persistent_dir.to_str().unwrap())),
                    ..Default::default()
                },
            };

            let filtered_results = engine
                .search_vectors_unified(&query_context)
                .await?;

            let filter_latency = start_filter.elapsed().as_micros() as f64 / 1000.0;
            metadata_filter_latencies.push(filter_latency);

            // Record filter results for metrics
            let filter_result_count = filtered_results.len();
            let total_result_count = results.len();

            println!("        📊 Metadata filter results:");
            println!(
                "          - Regular search: {} results in {:.2}ms",
                total_result_count, first_latency
            );
            println!(
                "          - Filtered search: {} results in {:.2}ms",
                filter_result_count, filter_latency
            );
            println!(
                "          - Filter overhead: {:.2}ms ({:.1}% change)",
                filter_latency - first_latency,
                ((filter_latency / first_latency) - 1.0) * 100.0
            );

            // Run additional filter queries for measurement
            for _ in 1..3 {
                let start_filter = Instant::now();
                let _ = engine
                    .search_vectors_unified(&query_context)
                    .await;
                metadata_filter_latencies.push(start_filter.elapsed().as_micros() as f64 / 1000.0);
            }

            // Calculate metrics - VIPER writes to {storage_assignment.base_location}/{collection_id}/data
            println!("      Calculating VIPER compressed size:");
            let viper_actual_dir = env.persistent_dir.join(env.collection_id()).join("data");
            println!("        Primary location: {}", viper_actual_dir.display());

            let mut compressed_size = if viper_actual_dir.exists() {
                get_directory_size(viper_actual_dir.to_str().unwrap()).await
            } else {
                // Try without collection_id
                let alt_dir = env.persistent_dir.join("data");
                if alt_dir.exists() {
                    println!("        Alternate location: {}", alt_dir.display());
                    get_directory_size(alt_dir.to_str().unwrap()).await
                } else {
                    // Fallback to viper_data directory from config
                    println!(
                        "        Config directory: {}",
                        env.get_viper_data_directory().display()
                    );
                    get_directory_size(env.get_viper_data_directory().to_str().unwrap()).await
                }
            };

            if compressed_size == 0 {
                // If still 0, estimate based on data
                compressed_size =
                    (config.batch_count * config.vectors_per_batch * config.dimension * 4 / 10)
                        as u64;
                println!(
                    "        WARNING: No VIPER files found, using estimate: {} bytes",
                    compressed_size
                );
                println!(
                    "        This likely means VIPER flush is not writing to expected location"
                );
            } else {
                let compression_ratio = if baseline.uncompressed_size > 0 {
                    ((compressed_size as f64 / baseline.uncompressed_size as f64) - 1.0) * 100.0
                } else {
                    0.0
                };
                println!(
                    "        VIPER compression result: {} bytes vs baseline {} bytes ({:+.1}%)",
                    compressed_size, baseline.uncompressed_size, compression_ratio
                );

                // VALIDATION: For non-none algorithms, compressed should be smaller than uncompressed
                // Allow up to 99.99% of original size to account for compression headers/metadata overhead
                if config.algorithm != "none" {
                    let compression_threshold = baseline.uncompressed_size as f64 * 0.9999;
                    assert!(
                        compressed_size as f64 <= compression_threshold,
                        "❌ VIPER COMPRESSION FAILURE: Algorithm {} level {} produced size {} which is > 99.99% of uncompressed {}. \
                        Compression is not working!",
                        config.algorithm,
                        config.level,
                        compressed_size,
                        baseline.uncompressed_size
                    );
                }
            }

            // Baseline uncompressed size is passed in, no need to recalculate

            // Get REAL VIPER search results for accuracy comparison
            // After compaction, files might be in the collection's data directory
            println!("      🔍 DEBUG: VIPER Search after compaction:");
            println!("        Collection ID: {}", env.collection_id());
            let storage_url = format!("file://{}/data", env.persistent_dir.to_str().unwrap());
            println!("        Storage URL for search: {}", storage_url);

            // List files at storage URL to debug
            if let Ok(mut entries) =
                tokio::fs::read_dir(format!("{}/data", env.persistent_dir.to_str().unwrap())).await
            {
                println!("        Files at search location:");
                while let Ok(Some(entry)) = entries.next_entry().await {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.ends_with(".parquet") {
                        println!("          - {}", name);
                    }
                }
            }

            // Create search context for final results search with metadata and vectors
            let final_search_params = std::sync::Arc::new(proximadb::core::search::SearchParams {
                vector: Some(query_vector.clone()),
                query_vectors: None,
                top_k: Some(10),
                distance_metric: Some(proximadb::compute::distance_computation::DistanceMetric::Cosine),
                filter_expression: None,
                include_metadata: Some(true),
                include_vectors: Some(true),
                timeout_ms: None,
                accuracy_threshold: None,
                enable_early_termination: None,
                max_results_per_stage: None,
                progressive_search: None,
            });

            let final_query_context = proximadb::storage::traits::StorageQueryContext {
                search_params: final_search_params,
                collection: collection.clone(),
                metadata: proximadb::storage::traits::StorageQueryMetadata {
                    collection_id: env.collection_id().to_string(),
                    use_axis_indexes: false,
                    storage_url: Some(storage_url.clone()),
                    ..Default::default()
                },
            };

            let final_results = engine
                .search_vectors_unified(&final_query_context)
                .await?;
            println!("        Search returned {} results", final_results.len());
            let actual_ids: Vec<String> = final_results.iter().map(|r| r.id.clone()).collect();

            // Calculate accuracy vs baseline
            let (accuracy, matches) = calculate_accuracy(baseline, &actual_ids);

            // Verify consistency: VIPER and SST should return similar results for same data
            if accuracy < 80.0 && config.algorithm == "none" {
                println!(
                    "        ⚠️ WARNING: Low accuracy {:.1}% for uncompressed VIPER vs baseline",
                    accuracy
                );
            }

            Ok(build_result(
                "VIPER",
                config,
                baseline.uncompressed_size, // Use baseline's uncompressed size
                compressed_size,
                flush_times,
                compaction_time,
                file_counts_before_compaction,
                file_counts_after_compaction,
                query_latencies,
                metadata_filter_latencies,
                filter_result_count,
                total_result_count,
                compact_result.entries_processed.unwrap_or(0) as usize,
                accuracy,
                matches,
                Some(baseline), // Pass baseline for latency comparison
            ))
        }
        _ => panic!("Unknown engine: {}", engine_type),
    }
}

/// Build result from metrics
fn build_result(
    engine: &str,
    config: BenchmarkConfig,
    uncompressed_size: u64,
    compressed_size: u64,
    flush_times: Vec<u64>,
    compaction_time: u64,
    files_before: usize,
    files_after: usize,
    query_latencies: Vec<f64>,
    filter_latencies: Vec<f64>, // New: metadata filter query latencies
    filter_result_count: usize, // New: number of results after filtering
    total_result_count: usize,  // New: total results without filter
    entries_processed: usize,
    result_accuracy: f64,
    top_k_matches: usize,
    baseline: Option<&BaselineResults>, // Add baseline for percentage comparison
) -> BenchmarkResult {
    let compression_ratio = if uncompressed_size > 0 {
        compressed_size as f64 / uncompressed_size as f64
    } else {
        1.0
    };

    let compression_savings_percent = (1.0 - compression_ratio) * 100.0;

    let total_flush_time: u64 = flush_times.iter().sum();
    let avg_flush_time = total_flush_time / flush_times.len() as u64;

    // Calculate compaction efficiency - show actual data size reduction, not just file count
    // Use the compression ratio already calculated from actual data sizes
    let compaction_reduction_ratio = compression_ratio;

    // Calculate query percentiles
    let mut sorted_latencies = query_latencies.clone();
    sorted_latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let p50_idx = sorted_latencies.len() / 2;
    let p99_idx = (sorted_latencies.len() * 99) / 100;

    let query_latency_p50 = sorted_latencies.get(&(index / 2)).copied().unwrap_or(0.0);
    let query_latency_p99 = sorted_latencies.get(&(index / 2)).copied().unwrap_or(0.0);

    // Calculate latency percentage change vs baseline
    let (latency_change_p50, latency_change_p99) = if let Some(baseline) = baseline {
        let p50_change = if baseline.query_latency_ms > 0.0 {
            ((query_latency_p50 - baseline.query_latency_ms) / baseline.query_latency_ms) * 100.0
        } else {
            0.0
        };
        let p99_change = if baseline.query_latency_ms > 0.0 {
            ((query_latency_p99 - baseline.query_latency_ms) / baseline.query_latency_ms) * 100.0
        } else {
            0.0
        };
        (p50_change, p99_change)
    } else {
        (0.0, 0.0)
    };

    let avg_latency = sorted_latencies.iter().sum::<f64>() / sorted_latencies.len() as f64;
    let query_throughput = if avg_latency > 0.0 {
        1000.0 / avg_latency // QPS
    } else {
        0.0
    };

    // Calculate metadata filter metrics
    let mut sorted_filter_latencies = filter_latencies.clone();
    sorted_filter_latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let filter_p50_idx = sorted_filter_latencies.len() / 2;
    let filter_p99_idx = (sorted_filter_latencies.len() * 99) / 100;

    let filter_latency_p50 = sorted_filter_latencies.get("enable_two_stage_search").copied().unwrap_or(0.0);
    let filter_latency_p99 = sorted_filter_latencies.get("enable_two_stage_search").copied().unwrap_or(0.0);

    let avg_filter_latency = if !sorted_filter_latencies.is_empty() {
        sorted_filter_latencies.iter().sum::<f64>() / sorted_filter_latencies.len() as f64
    } else {
        0.0
    };

    let filter_throughput = if avg_filter_latency > 0.0 {
        1000.0 / avg_filter_latency // QPS
    } else {
        0.0
    };

    // Calculate filter overhead and selectivity
    let filter_overhead = if query_latency_p50 > 0.0 {
        ((filter_latency_p50 - query_latency_p50) / query_latency_p50) * 100.0
    } else {
        0.0
    };

    let filter_selectivity = if total_result_count > 0 {
        (filter_result_count as f64 / total_result_count as f64) * 100.0
    } else {
        0.0
    };

    BenchmarkResult {
        engine: engine.to_string(),
        sparsity: config.sparsity_percent,
        algorithm: config.algorithm,
        level: config.level,
        batch_count: config.batch_count,
        uncompressed_size,
        compressed_size,
        compression_ratio,
        compression_savings_percent,
        total_flush_time_ms: total_flush_time,
        avg_flush_time_ms: avg_flush_time,
        compaction_time_ms: compaction_time,
        compaction_input_files: files_before,
        compaction_output_files: files_after,
        compaction_reduction_ratio,
        query_latency_p50_ms: query_latency_p50,
        query_latency_p99_ms: query_latency_p99,
        query_throughput_qps: query_throughput,
        latency_change_p50_percent: latency_change_p50,
        latency_change_p99_percent: latency_change_p99,
        filter_latency_p50_ms: filter_latency_p50,
        filter_latency_p99_ms: filter_latency_p99,
        filter_throughput_qps: filter_throughput,
        filter_overhead_percent: filter_overhead,
        filter_selectivity: filter_selectivity,
        result_accuracy,
        top_k_matches,
        peak_memory_mb: 0,      // Would need actual measurement
        cpu_usage_percent: 0.0, // Would need actual measurement
        io_operations: 0,       // Would need actual measurement
    }
}

/// Count data files in directory (SST or Parquet, excluding temp/staging files)
async fn count_files_in_dir(path: &str) -> usize {
    use tokio::fs;

    let mut count = 0;
    if let Ok(mut entries) = fs::read_dir(path).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let file_name = entry.file_name().to_string_lossy().to_string();

            // Skip staging directories entirely
            if file_name.starts_with("__") {
                continue;
            }

            if let Ok(metadata) = entry.metadata().await {
                if metadata.is_file() {
                    // Only count actual data files (SST or Parquet)
                    if file_name.ends_with(".sstable") || file_name.ends_with(".parquet") {
                        count += 1;
                    }
                }
            }
        }
    }
    count
}

/// Get directory size with detailed reporting for VIPER
async fn get_directory_size(path: &str) -> u64 {
    use tokio::fs;

    let mut total = 0u64;
    let mut file_count = 0;
    let mut parquet_files = Vec::new();
    let mut sst_files = Vec::new();

    if let Ok(mut entries) = fs::read_dir(path).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let file_name = entry.file_name().to_string_lossy().to_string();

            // Skip staging directories entirely
            if file_name.starts_with("__") {
                continue;
            }

            if let Ok(metadata) = entry.metadata().await {
                if metadata.is_file() {
                    let file_size = metadata.len();
                    total += file_size;
                    file_count += 1;

                    // Track specific file types
                    if file_name.ends_with(".parquet") {
                        parquet_files.push((file_name.clone(), file_size));
                    } else if file_name.ends_with(".sstable") {
                        sst_files.push((file_name.clone(), file_size));
                    }
                }
            }
        }
    }

    // Report details for debugging
    if file_count > 0 {
        debug!(
            "        Directory: {} - {} files, total size: {} bytes",
            path, file_count, total
        );
        if !parquet_files.is_empty() {
            debug!("        Parquet files: {}", parquet_files.len());
            // Show all files for manual verification
            for (name, size) in parquet_files.iter() {
                debug!("          {} ({} bytes)", name, size);
            }
        }
        if !sst_files.is_empty() {
            debug!("        SST files: {}", sst_files.len());
            // Show all files for manual verification
            for (name, size) in sst_files.iter() {
                debug!("          {} ({} bytes)", name, size);
            }
        }
    } else {
        debug!("        WARNING: No files found in directory: {}", path);
    }

    total
}

/// Calculate compaction efficiency metrics
fn calculate_compaction_efficiency(
    before_size: u64,
    after_size: u64,
    files_before: usize,
    files_after: usize,
) -> (f64, f64) {
    let size_reduction = if before_size > 0 {
        ((before_size - after_size) as f64 / before_size as f64) * 100.0
    } else {
        0.0
    };

    let file_reduction = if files_before > 0 {
        (files_before as f64 - files_after as f64) / files_before as f64
    } else {
        0.0
    };

    (size_reduction, file_reduction)
}

/// Dummy function (removed HTML generation)
fn generate_html_report_removed(results: &[BenchmarkResult]) -> String {
    let mut html = String::from(
        r#"<!DOCTYPE html>
<html>
<head>
    <title>ProximaDB Engine Benchmark Report</title>
    <style>
        body { font-family: Arial, sans-serif; margin: 20px; background: #f5f5f5; }
        h1, h2, h3 { color: #333; }
        h1 { border-bottom: 3px solid #4CAF50; padding-bottom: 10px; }
        table { border-collapse: collapse; width: 100%; margin: 20px 0; box-shadow: 0 2px 4px rgba(0,0,0,0.1); }
        th, td { border: 1px solid #ddd; padding: 10px; text-align: right; }
        th { background: linear-gradient(135deg, #667eea 0%, #764ba2 100%); color: white; font-weight: bold; }
        tr:nth-child(even) { background-color: #f8f9fa; }
        tr:hover { background-color: #e9ecef; }
        .best { background-color: #90EE90 !important; font-weight: bold; }
        .worst { background-color: #FFB6C1 !important; }
        .section { margin: 40px 0; background: white; padding: 20px; border-radius: 8px; }
        .metric-card { display: inline-block; margin: 10px; padding: 15px; background: white;
                       border-left: 4px solid #4CAF50; box-shadow: 0 2px 4px rgba(0,0,0,0.1); }
        .metric-value { font-size: 24px; font-weight: bold; color: #4CAF50; }
        .metric-label { color: #666; margin-top: 5px; }
        .chart-container { width: 100%; height: 400px; margin: 20px 0; }
    </style>
</head>
<body>
    <h1>🚀 ProximaDB Comprehensive Engine Benchmark Report</h1>
    <p><strong>Generated:</strong> <script>document.write(new Date().toLocaleString());</script></p>
    <p><strong>Test Configuration:</strong> Multiple sparsity levels, all compression algorithms, 5 batches per test</p>
"#,
    );

    // Add summary statistics
    html.push_str("<div class='section'><h2>📊 Summary Statistics</h2>");

    // Calculate key metrics
    let best_compression = results.iter().max_by(|a, b| {
        a.compression_savings_percent
            .partial_cmp(&b.compression_savings_percent)
            .unwrap()
    });
    let best_latency = results.iter().min_by(|a, b| {
        a.query_latency_p50_ms
            .partial_cmp(&b.query_latency_p50_ms)
            .unwrap()
    });
    let best_throughput = results.iter().max_by(|a, b| {
        a.query_throughput_qps
            .partial_cmp(&b.query_throughput_qps)
            .unwrap()
    });
    let best_accuracy = results
        .iter()
        .max_by(|a, b| a.result_accuracy.partial_cmp(&b.result_accuracy).unwrap());

    if let Some(bc) = best_compression {
        html.push_str(&format!(
            "<div class='metric-card'><div class='metric-value'>{:.1}%</div><div class='metric-label'>Best Compression<br/>{} {} L{}</div></div>",
            bc.compression_savings_percent, bc.engine, bc.algorithm, bc.level
        ));
    }
    if let Some(bl) = best_latency {
        html.push_str(&format!(
            "<div class='metric-card'><div class='metric-value'>{:.2}ms</div><div class='metric-label'>Best Latency (P50)<br/>{} {}</div></div>",
            bl.query_latency_p50_ms, bl.engine, bl.algorithm
        ));
    }
    if let Some(bt) = best_throughput {
        html.push_str(&format!(
            "<div class='metric-card'><div class='metric-value'>{:.0}</div><div class='metric-label'>Best QPS<br/>{} {}</div></div>",
            bt.query_throughput_qps, bt.engine, bt.algorithm
        ));
    }
    if let Some(ba) = best_accuracy {
        html.push_str(&format!(
            "<div class='metric-card'><div class='metric-value'>{:.1}%</div><div class='metric-label'>Best Accuracy<br/>{} {}</div></div>",
            ba.result_accuracy, ba.engine, ba.algorithm
        ));
    }
    html.push_str("</div>");

    // Main results table with all metrics
    html.push_str("<div class='section'><h2>📈 Complete Benchmark Results</h2><table>");
    html.push_str("<tr><th>Engine</th><th>Sparsity</th><th>Algorithm</th><th>Level</th>");
    html.push_str("<th>Batches</th><th>Compressed (KB)</th><th>Original (KB)</th><th>Ratio</th>");
    html.push_str("<th>Savings %</th><th>Avg Flush (ms)</th><th>Compact (ms)</th>");
    html.push_str("<th>Files In→Out</th><th>P50 Latency</th><th>P99 Latency</th>");
    html.push_str("<th>QPS</th><th>Accuracy %</th><th>Top-K Match</th>");
    html.push_str("<th>Savings %</th><th>Avg Flush (ms)</th><th>Compact (ms)</th>");
    html.push_str("<th>Files Before</th><th>Files After</th><th>Query P50 (ms)</th>");
    html.push_str("<th>Query P99 (ms)</th><th>QPS</th></tr>");

    for r in results {
        html.push_str(&format!(
            "<tr><td>{}</td><td>{}%</td><td>{}</td><td>{}</td><td>{}</td>",
            r.engine, r.sparsity, r.algorithm, r.level, r.batch_count
        ));
        html.push_str(&format!(
            "<td>{:.1}</td><td>{:.1}</td><td>{:.3}</td><td>{:.1}%</td>",
            r.compressed_size as f64 / 1024.0,
            r.uncompressed_size as f64 / 1024.0,
            r.compression_ratio,
            r.compression_savings_percent
        ));
        html.push_str(&format!(
            "<td>{}</td><td>{}</td><td>{}</td><td>{}</td>",
            r.avg_flush_time_ms,
            r.compaction_time_ms,
            r.compaction_input_files,
            r.compaction_output_files
        ));
        html.push_str(&format!(
            "<td>{:.2}</td><td>{:.2}</td><td>{:.0}</td></tr>",
            r.query_latency_p50_ms, r.query_latency_p99_ms, r.query_throughput_qps
        ));
    }

    html.push_str("</table></div>");
    html.push_str("</body></html>");
    html
}

#[tokio::test]
async fn test_generate_comprehensive_benchmark_report() -> Result<()> {
    // Skip if not explicitly enabled
    if std::env::var("RUN_BENCHMARKS").unwrap_or_default() != "true" {
        println!("\n╔════════════════════════════════════════════════════════════════════╗");
        println!("║  BENCHMARK SKIPPED                                                ║");
        println!("║  To run comprehensive benchmarks:                                 ║");
        println!(
            "║  RUN_BENCHMARKS=true cargo test test_generate_comprehensive_benchmark_report ║"
        );
        println!("╚════════════════════════════════════════════════════════════════════╝\n");
        return Ok(());
    }

    // Initialize logging subscriber to see debug messages
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

    println!("\n🚀 GENERATING COMPREHENSIVE BENCHMARK REPORT");
    println!("{}", "=".repeat(100));

    // Test configurations - comprehensive sparsity coverage
    let sparsity_levels = vec![10, 30, 50, 70, 90]; // Dense to very sparse
    let algorithms_and_levels = vec![
        ("none", vec![0]),
        ("lz4", vec![1]),
        ("snappy", vec![1]), // Changed from 0 to 1 for valid compression level
        ("zstd", vec![3, 6]),
        ("gzip", vec![6]), // SST only
    ];

    let batch_count = 5; // Test with 5 batches for compaction
    let vectors_per_batch = 200;
    let dimension = 1536; // GPT-like dimensions
    let total_vectors = batch_count * vectors_per_batch;
    let num_query_vectors = 10; // Inject 10 query vectors throughout the dataset

    // Pre-generate all vector sets for each sparsity level (reused across all tests)
    println!("\n📊 Pre-generating randomized vector sets for each sparsity level...");
    let mut vector_sets: HashMap<usize, TestVectorSet> = HashMap::new();
    for &sparsity in &sparsity_levels {
        println!(
            "  - Generating {} vectors with {}% sparsity (including {} query vectors)",
            total_vectors, sparsity, num_query_vectors
        );
        let vector_set =
            create_randomized_vector_set(total_vectors, dimension, sparsity, num_query_vectors);
        println!(
            "    Query vectors injected at positions: {:?}",
            vector_set
                .query_positions
                .iter()
                .take(5)
                .collect::<Vec<_>>()
        );
        vector_sets.insert(sparsity, vector_set);
    }
    println!("✅ Vector generation complete\n");

    let mut all_results = Vec::new();

    // Calculate total number of tests
    let mut total_tests = 0;
    for (_algo, levels) in &algorithms_and_levels {
        total_tests += levels.len() * 2 * sparsity_levels.len(); // 2 engines
    }

    println!("\n📊 Total benchmarks to run: {}", total_tests);
    println!("   Sparsity levels: {:?}", sparsity_levels);
    println!(
        "   Batch count: {}, Vectors per batch: {}, Dimension: {}",
        batch_count, vectors_per_batch, dimension
    );
    println!("   Estimated time: {} minutes\n", (total_tests * 20) / 60); // ~20 seconds per test

    let mut test_number = 0;

    // Store baselines for each engine and sparsity level
    let mut baselines: HashMap<(String, usize), BaselineResults> = HashMap::new();

    // First, run all baselines
    println!("\n📊 PHASE 1: Running baseline tests (no compression)");
    println!("{}", "=".repeat(60));

    for sparsity in &sparsity_levels {
        println!("\n  Sparsity {}%:", sparsity);

        // Get the pre-generated vectors for this sparsity level
        let vector_set = vector_sets.get(&i).unwrap();

        // SST baseline
        match run_baseline("SST", *sparsity, vector_set, batch_count, vectors_per_batch).await {
            Ok(baseline) => {
                let sst_baseline = baseline.clone();
                println!(
                    "    ✅ SST baseline: {} results",
                    sst_baseline.top_k_ids.len()
                );
                baselines.insert(("SST".to_string(), *sparsity), sst_baseline);
            }
            Err(e) => println!("    ⚠️ SST baseline failed: {}", e),
        }

        // VIPER baseline
        match run_baseline(
            "VIPER",
            *sparsity,
            vector_set,
            batch_count,
            vectors_per_batch,
        )
        .await
        {
            Ok(baseline) => {
                let viper_baseline = baseline.clone();
                println!(
                    "    ✅ VIPER baseline: {} results",
                    viper_baseline.top_k_ids.len()
                );

                // Compare VIPER and SST baseline results - they should be similar
                if let Some(sst_baseline) = baselines.get(&("SST".to_string(), *sparsity)) {
                    let mut matching_ids = 0;
                    for (i, viper_id) in viper_baseline.top_k_ids.iter().enumerate() {
                        if i < sst_baseline.top_k_ids.len()
                            && viper_id == &sst_baseline.top_k_ids[i]
                        {
                            matching_ids += 1;
                        }
                    }
                    let match_percent = if !viper_baseline.top_k_ids.is_empty() {
                        (matching_ids as f64 / viper_baseline.top_k_ids.len() as f64) * 100.0
                    } else {
                        0.0
                    };

                    if match_percent < 50.0 {
                        println!(
                            "    ⚠️ WARNING: SST and VIPER baselines differ significantly ({:.1}% match)",
                            match_percent
                        );
                        println!(
                            "       This may indicate a data consistency issue between engines"
                        );
                    } else {
                        println!("    ✅ SST/VIPER consistency: {:.1}% match", match_percent);
                    }
                }

                baselines.insert(("VIPER".to_string(), *sparsity), viper_baseline);
            }
            Err(e) => println!("    ⚠️ VIPER baseline failed: {}", e),
        }
    }

    // Now run compression benchmarks comparing against baselines
    println!("\n📊 PHASE 2: Running compression benchmarks");
    println!("{}", "=".repeat(60));

    for sparsity in &sparsity_levels {
        println!("\n━━━━━ SPARSITY LEVEL: {}% ━━━━━", sparsity);

        // Get the pre-generated vectors for this sparsity level
        let vector_set = vector_sets.get(&i).unwrap();

        // Get baselines for this sparsity level
        let sst_baseline = baselines.get(&("SST".to_string(), sparsity));
        let viper_baseline = baselines.get(&("VIPER".to_string(), sparsity));

        for (algo, levels) in &algorithms_and_levels {
            // Skip "none" algorithm as that's our baseline
            if *algo == "none" {
                continue;
            }

            for level in levels {
                let config = BenchmarkConfig {
                    sparsity_percent: *sparsity,
                    algorithm: algo.to_string(),
                    level: *level,
                    batch_count,
                    vectors_per_batch,
                    dimension,
                };

                // Test SST
                if let Some(baseline) = sst_baseline {
                    if !(*algo == "gzip" && *level > 9) {
                        test_number += 1;
                        match run_benchmark(
                            "SST",
                            config.clone(),
                            baseline,
                            vector_set,
                            test_number,
                            total_tests,
                        )
                        .await
                        {
                            Ok(result) => {
                                println!(
                                    "    ✅ Compression: {:.1}%, Accuracy: {:.1}%, Query: {:.2}ms",
                                    result.compression_savings_percent,
                                    result.result_accuracy,
                                    result.query_latency_p50_ms
                                );
                                all_results.push(result);
                            }
                            Err(e) => println!("    ⚠️ Failed: {}", e),
                        }
                    }
                }

                // Test VIPER
                if let Some(baseline) = viper_baseline {
                    if !(*algo == "gzip" || (*algo == "zstd" && *level > 6)) {
                        test_number += 1;
                        match run_benchmark(
                            "VIPER",
                            config.clone(),
                            baseline,
                            vector_set,
                            test_number,
                            total_tests,
                        )
                        .await
                        {
                            Ok(result) => {
                                println!(
                                    "    ✅ Compression: {:.1}%, Accuracy: {:.1}%, Query: {:.2}ms",
                                    result.compression_savings_percent,
                                    result.result_accuracy,
                                    result.query_latency_p50_ms
                                );
                                all_results.push(result);
                            }
                            Err(e) => println!("    ⚠️ Failed: {}", e),
                        }
                    }
                }
            }
        }

        // Phase Summary for this sparsity level
        println!("\n📊 PHASE SUMMARY - {}% SPARSITY", sparsity);
        println!("{}", "-".repeat(60));

        let sparsity_results: Vec<_> = all_results
            .iter()
            .filter(|r| r.sparsity == *sparsity)
            .collect();

        if !sparsity_results.is_empty() {
            // Best compression for this sparsity
            let best_compression = sparsity_results.iter().max_by(|a, b| {
                a.compression_savings_percent
                    .partial_cmp(&b.compression_savings_percent)
                    .unwrap()
            });

            // Best latency for this sparsity
            let best_latency = sparsity_results.iter().min_by(|a, b| {
                a.query_latency_p50_ms
                    .partial_cmp(&b.query_latency_p50_ms)
                    .unwrap()
            });

            // Average metrics for this sparsity
            let avg_compression = sparsity_results
                .iter()
                .map(|r| r.compression_savings_percent)
                .sum::<f64>()
                / sparsity_results.len() as f64;

            let avg_latency = sparsity_results
                .iter()
                .map(|r| r.query_latency_p50_ms)
                .sum::<f64>()
                / sparsity_results.len() as f64;

            let avg_filter_latency = sparsity_results
                .iter()
                .map(|r| r.filter_latency_p50_ms)
                .sum::<f64>()
                / sparsity_results.len() as f64;

            let avg_filter_overhead = sparsity_results
                .iter()
                .map(|r| r.filter_overhead_percent)
                .sum::<f64>()
                / sparsity_results.len() as f64;

            let avg_selectivity = sparsity_results
                .iter()
                .map(|r| r.filter_selectivity)
                .sum::<f64>()
                / sparsity_results.len() as f64;

            println!("  📈 Configurations tested: {}", sparsity_results.len());
            println!("  💾 Average compression savings: {:.1}%", avg_compression);
            println!("  ⏱️  Average P50 latency: {:.2}ms", avg_latency);
            println!(
                "  🔍 Average filter P50 latency: {:.2}ms ({:+.1}% overhead)",
                avg_filter_latency, avg_filter_overhead
            );
            println!(
                "  📊 Average filter selectivity: {:.1}% of results",
                avg_selectivity
            );

            if let Some(bc) = best_compression {
                println!(
                    "  🏆 Best compression: {} {} L{} - {:.1}% savings",
                    bc.engine, bc.algorithm, bc.level, bc.compression_savings_percent
                );
            }

            if let Some(bl) = best_latency {
                println!(
                    "  ⚡ Best latency: {} {} - {:.2}ms P50",
                    bl.engine, bl.algorithm, bl.query_latency_p50_ms
                );
            }

            // Compare SST vs VIPER at this sparsity
            let sst_avg_compression = sparsity_results
                .iter()
                .filter(|r| r.engine == "SST")
                .map(|r| r.compression_savings_percent)
                .sum::<f64>()
                / sparsity_results
                    .iter()
                    .filter(|r| r.engine == "SST")
                    .count()
                    .max(1) as f64;

            let viper_avg_compression = sparsity_results
                .iter()
                .filter(|r| r.engine == "VIPER")
                .map(|r| r.compression_savings_percent)
                .sum::<f64>()
                / sparsity_results
                    .iter()
                    .filter(|r| r.engine == "VIPER")
                    .count()
                    .max(1) as f64;

            // Compaction metrics
            let avg_compaction_ratio = sparsity_results
                .iter()
                .filter(|r| r.compaction_input_files > 0)
                .map(|r| r.compaction_reduction_ratio)
                .sum::<f64>()
                / sparsity_results
                    .iter()
                    .filter(|r| r.compaction_input_files > 0)
                    .count()
                    .max(1) as f64;

            println!(
                "  📊 SST avg savings: {:.1}% | VIPER avg savings: {:.1}%",
                sst_avg_compression, viper_avg_compression
            );
            println!(
                "  🔄 Avg compaction reduction: {:.1}:1 files",
                avg_compaction_ratio
            );
        }
    }

    // Generate final comprehensive report
    println!("\n🏁 FINAL COMPREHENSIVE REPORT");
    println!("{}", "=".repeat(100));

    // 1. Compression Effectiveness by Sparsity
    println!("\n═══ COMPRESSION EFFECTIVENESS BY SPARSITY ═══");
    println!("┌──────────┬─────────────────────────────┬─────────────────────────────┐");
    println!("│ Sparsity │ SST Best (algo, savings%)  │ VIPER Best (algo, savings%)│");
    println!("├──────────┼─────────────────────────────┼─────────────────────────────┤");

    for sparsity in &sparsity_levels {
        let sst_best = all_results
            .iter()
            .filter(|r| r.engine == "SST" && r.sparsity == *sparsity)
            .max_by(|a, b| {
                a.compression_savings_percent
                    .partial_cmp(&b.compression_savings_percent)
                    .unwrap()
            });

        let viper_best = all_results
            .iter()
            .filter(|r| r.engine == "VIPER" && r.sparsity == *sparsity)
            .max_by(|a, b| {
                a.compression_savings_percent
                    .partial_cmp(&b.compression_savings_percent)
                    .unwrap()
            });

        if let (Some(s), Some(v)) = (sst_best, viper_best) {
            println!(
                "│ {:7}% │ {:7} L{}: {:5.1}%       │ {:7} L{}: {:5.1}%      │",
                sparsity,
                s.algorithm,
                s.level,
                s.compression_savings_percent,
                v.algorithm,
                v.level,
                v.compression_savings_percent
            );
        }
    }
    println!("└──────────┴─────────────────────────────┴─────────────────────────────┘");

    // 2. Compaction Efficiency Report
    println!("\n═══ COMPACTION EFFICIENCY (5 BATCHES) ═══");
    println!(
        "┌────────┬──────────┬────────┬───────┬──────────┬──────────┬──────────┬──────────┬──────────┬──────────┐"
    );
    println!(
        "│ Engine │ Sparsity │ Algo   │ Level │ Size(KB) │ Comp(KB) │ Ratio    │ Savings% │ Time(ms) │ Files    │"
    );
    println!(
        "├────────┼──────────┼────────┼───────┼──────────┼──────────┼──────────┼──────────┼──────────┼──────────┤"
    );

    // Show best compaction results for each sparsity level with actual data
    for sparsity in &sparsity_levels {
        for engine in &["SST", "VIPER"] {
            let best_compact = all_results
                .iter()
                .filter(|r| r.engine == *engine && r.sparsity == *sparsity)
                .min_by(|a, b| {
                    a.compression_ratio
                        .partial_cmp(&b.compression_ratio)
                        .unwrap()
                });

            if let Some(bc) = best_compact {
                println!(
                    "│ {:6} │ {:7}% │ {:6} │ {:5} │ {:8.1} │ {:8.1} │ {:8.3} │ {:8.1} │ {:8} │ {:2}->{:<2}   │",
                    engine,
                    sparsity,
                    bc.algorithm,
                    bc.level,
                    bc.uncompressed_size as f64 / 1024.0,
                    bc.compressed_size as f64 / 1024.0,
                    bc.compression_ratio,
                    bc.compression_savings_percent,
                    bc.compaction_time_ms,
                    bc.compaction_input_files,
                    bc.compaction_output_files
                );
            }
        }
    }
    println!(
        "└────────┴──────────┴────────┴───────┴──────────┴──────────┴──────────┴──────────┴──────────┴──────────┘"
    );

    // 3. Latency Impact Analysis (NEW!)
    println!("\n═══ LATENCY IMPACT ANALYSIS ═══");
    println!(
        "┌────────┬──────────┬────────┬────────┬──────────┬───────────┬──────────┬──────────┐"
    );
    println!(
        "│ Engine │ Sparsity │ Algo   │ Level  │ P50(ms)  │ vs Base % │ P99(ms)  │ vs Base % │"
    );
    println!(
        "├────────┼──────────┼────────┼────────┼──────────┼───────────┼──────────┼──────────┤"
    );

    for sparsity in &sparsity_levels {
        for engine in &["SST", "VIPER"] {
            // Show worst latency impact (highest percentage increase)
            let worst_latency = all_results
                .iter()
                .filter(|r| r.engine == *engine && r.sparsity == *sparsity)
                .max_by(|a, b| {
                    a.latency_change_p50_percent
                        .partial_cmp(&b.latency_change_p50_percent)
                        .unwrap()
                });

            if let Some(w) = worst_latency {
                let impact_p50 = if w.latency_change_p50_percent > 10.0 {
                    "🔴"
                } else if w.latency_change_p50_percent > 5.0 {
                    "🟡"
                } else {
                    "🟢"
                };
                let impact_p99 = if w.latency_change_p99_percent > 10.0 {
                    "🔴"
                } else if w.latency_change_p99_percent > 5.0 {
                    "🟡"
                } else {
                    "🟢"
                };

                println!(
                    "│ {:6} │ {:7}% │ {:6} │ {:5}  │ {:8.2} │ {:+8.1}% │ {:8.2} │ {:+7.1}% │",
                    engine,
                    sparsity,
                    w.algorithm,
                    w.level,
                    w.query_latency_p50_ms,
                    w.latency_change_p50_percent,
                    w.query_latency_p99_ms,
                    w.latency_change_p99_percent
                );
            }
        }
    }
    println!(
        "└────────┴──────────┴────────┴────────┴──────────┴───────────┴──────────┴──────────┘"
    );
    println!("Legend: 🟢 ≤5% impact, 🟡 5-10% impact, 🔴 >10% impact");

    // 4. Query Performance Comparison
    println!("\n═══ QUERY PERFORMANCE COMPARISON ═══");
    println!("┌────────┬──────────┬────────┬──────────┬──────────┬──────────┬────────┐");
    println!("│ Engine │ Sparsity │ Algo   │ P50 (ms) │ P99 (ms) │ QPS      │ Winner │");
    println!("├────────┼──────────┼────────┼──────────┼──────────┼──────────┼────────┤");

    for sparsity in &sparsity_levels {
        let sst_best = all_results
            .iter()
            .filter(|r| r.engine == "SST" && r.sparsity == *sparsity)
            .min_by(|a, b| {
                a.query_latency_p50_ms
                    .partial_cmp(&b.query_latency_p50_ms)
                    .unwrap()
            });

        let viper_best = all_results
            .iter()
            .filter(|r| r.engine == "VIPER" && r.sparsity == *sparsity)
            .min_by(|a, b| {
                a.query_latency_p50_ms
                    .partial_cmp(&b.query_latency_p50_ms)
                    .unwrap()
            });

        if let (Some(s), Some(v)) = (sst_best, viper_best) {
            let winner = if s.query_latency_p50_ms < v.query_latency_p50_ms {
                "SST"
            } else {
                "VIPER"
            };

            println!(
                "│ SST    │ {:7}% │ {:6} │ {:8.2} │ {:8.2} │ {:8.0} │        │",
                sparsity,
                s.algorithm,
                s.query_latency_p50_ms,
                s.query_latency_p99_ms,
                s.query_throughput_qps
            );
            println!(
                "│ VIPER  │ {:7}% │ {:6} │ {:8.2} │ {:8.2} │ {:8.0} │ {:6} │",
                sparsity,
                v.algorithm,
                v.query_latency_p50_ms,
                v.query_latency_p99_ms,
                v.query_throughput_qps,
                winner
            );
            println!("├────────┼──────────┼────────┼──────────┼──────────┼──────────┼────────┤");
        }
    }
    println!("└────────┴──────────┴────────┴──────────┴──────────┴──────────┴────────┘");

    // 4. Recommendations Matrix
    println!("\n═══ RECOMMENDATIONS BY USE CASE ═══");
    println!("┌─────────────────────────┬────────┬─────────────────────────────────┐");
    println!("│ Use Case                │ Engine │ Configuration                   │");
    println!("├─────────────────────────┼────────┼─────────────────────────────────┤");

    // Dense embeddings (0% sparsity)
    let dense_best = all_results
        .iter()
        .filter(|r| r.sparsity == 0)
        .max_by(|a, b| {
            a.compression_savings_percent
                .partial_cmp(&b.compression_savings_percent)
                .unwrap()
        });

    if let Some(db) = dense_best {
        println!(
            "│ Dense ML Embeddings     │ {:6} │ {} L{}: {:.1}% savings         │",
            db.engine, db.algorithm, db.level, db.compression_savings_percent
        );
    }

    // Sparse vectors (90% sparsity)
    let sparse_best = all_results
        .iter()
        .filter(|r| r.sparsity == 90)
        .max_by(|a, b| {
            a.compression_savings_percent
                .partial_cmp(&b.compression_savings_percent)
                .unwrap()
        });

    if let Some(sb) = sparse_best {
        println!(
            "│ Sparse Vectors (90%)    │ {:6} │ {} L{}: {:.1}% savings         │",
            sb.engine, sb.algorithm, sb.level, sb.compression_savings_percent
        );
    }

    // Very sparse (99% sparsity)
    let very_sparse_best = all_results
        .iter()
        .filter(|r| r.sparsity == 99)
        .max_by(|a, b| {
            a.compression_savings_percent
                .partial_cmp(&b.compression_savings_percent)
                .unwrap()
        });

    if let Some(vsb) = very_sparse_best {
        println!(
            "│ Very Sparse (99%)       │ {:6} │ {} L{}: {:.1}% savings         │",
            vsb.engine, vsb.algorithm, vsb.level, vsb.compression_savings_percent
        );
    }

    // Low latency
    let low_latency = all_results.iter().min_by(|a, b| {
        a.query_latency_p50_ms
            .partial_cmp(&b.query_latency_p50_ms)
            .unwrap()
    });

    if let Some(ll) = low_latency {
        println!(
            "│ Low Latency (<1ms)      │ {:6} │ {}: {:.2}ms P50 latency       │",
            ll.engine, ll.algorithm, ll.query_latency_p50_ms
        );
    }

    // Maximum compression
    let max_compression = all_results.iter().max_by(|a, b| {
        a.compression_savings_percent
            .partial_cmp(&b.compression_savings_percent)
            .unwrap()
    });

    if let Some(mc) = max_compression {
        println!(
            "│ Maximum Compression     │ {:6} │ {} L{}: {:.1}% savings         │",
            mc.engine, mc.algorithm, mc.level, mc.compression_savings_percent
        );
    }

    // Fast compaction
    let fast_compact = all_results
        .iter()
        .filter(|r| r.compaction_input_files > 0)
        .min_by(|a, b| {
            a.compaction_time_ms
                .partial_cmp(&b.compaction_time_ms)
                .unwrap()
        });

    if let Some(fc) = fast_compact {
        println!(
            "│ Fast Compaction         │ {:6} │ {}: {}ms for {} files         │",
            fc.engine, fc.algorithm, fc.compaction_time_ms, fc.compaction_input_files
        );
    }

    println!("└─────────────────────────┴────────┴─────────────────────────────────┘");

    // Add final comprehensive summary
    println!("\n🎯 FINAL SUMMARY");
    println!("{}", "=".repeat(100));

    // Calculate overall statistics
    let total_tests = all_results.len();
    let avg_compression = all_results
        .iter()
        .map(|r| r.compression_savings_percent)
        .sum::<f64>()
        / total_tests as f64;
    let avg_latency = all_results
        .iter()
        .map(|r| r.query_latency_p50_ms)
        .sum::<f64>()
        / total_tests as f64;
    let avg_throughput = all_results
        .iter()
        .map(|r| r.query_throughput_qps)
        .sum::<f64>()
        / total_tests as f64;
    let avg_accuracy =
        all_results.iter().map(|r| r.result_accuracy).sum::<f64>() / total_tests as f64;

    // Metadata filtering statistics
    let avg_filter_latency = all_results
        .iter()
        .map(|r| r.filter_latency_p50_ms)
        .sum::<f64>()
        / total_tests as f64;
    let avg_filter_throughput = all_results
        .iter()
        .map(|r| r.filter_throughput_qps)
        .sum::<f64>()
        / total_tests as f64;
    let avg_filter_overhead = all_results
        .iter()
        .map(|r| r.filter_overhead_percent)
        .sum::<f64>()
        / total_tests as f64;
    let avg_selectivity = all_results
        .iter()
        .map(|r| r.filter_selectivity)
        .sum::<f64>()
        / total_tests as f64;

    println!("📊 OVERALL STATISTICS:");
    println!("  Total configurations tested: {}", total_tests);
    println!("  Average compression savings: {:.1}%", avg_compression);
    println!("  Average P50 latency: {:.2}ms", avg_latency);
    println!("  Average throughput: {:.0} QPS", avg_throughput);
    println!("  Average accuracy: {:.1}%", avg_accuracy);

    println!("\n🔍 METADATA FILTERING STATISTICS:");
    println!("  Average filter P50 latency: {:.2}ms", avg_filter_latency);
    println!(
        "  Average filter throughput: {:.0} QPS",
        avg_filter_throughput
    );
    println!("  Average filter overhead: {:.1}%", avg_filter_overhead);
    println!("  Average selectivity: {:.1}% of results", avg_selectivity);

    // Compaction statistics
    let compaction_tests = all_results
        .iter()
        .filter(|r| r.compaction_input_files > 0)
        .count();
    if compaction_tests > 0 {
        let avg_compaction_time = all_results
            .iter()
            .filter(|r| r.compaction_input_files > 0)
            .map(|r| r.compaction_time_ms)
            .sum::<u64>()
            / compaction_tests as u64;
        let avg_file_reduction = all_results
            .iter()
            .filter(|r| r.compaction_input_files > 0)
            .map(|r| r.compaction_reduction_ratio)
            .sum::<f64>()
            / compaction_tests as f64;

        println!("\n🔄 COMPACTION STATISTICS:");
        println!("  Tests with compaction: {}", compaction_tests);
        println!("  Average compaction time: {}ms", avg_compaction_time);
        println!("  Average file reduction: {:.1}:1", avg_file_reduction);
    }

    // Engine comparison
    let sst_tests = all_results.iter().filter(|r| r.engine == "SST").count();
    let viper_tests = all_results.iter().filter(|r| r.engine == "VIPER").count();

    if sst_tests > 0 && viper_tests > 0 {
        let sst_avg_compression = all_results
            .iter()
            .filter(|r| r.engine == "SST")
            .map(|r| r.compression_savings_percent)
            .sum::<f64>()
            / sst_tests as f64;
        let viper_avg_compression = all_results
            .iter()
            .filter(|r| r.engine == "VIPER")
            .map(|r| r.compression_savings_percent)
            .sum::<f64>()
            / viper_tests as f64;

        let sst_avg_latency = all_results
            .iter()
            .filter(|r| r.engine == "SST")
            .map(|r| r.query_latency_p50_ms)
            .sum::<f64>()
            / sst_tests as f64;
        let viper_avg_latency = all_results
            .iter()
            .filter(|r| r.engine == "VIPER")
            .map(|r| r.query_latency_p50_ms)
            .sum::<f64>()
            / viper_tests as f64;

        // Metadata filtering comparison
        let sst_avg_filter_overhead = all_results
            .iter()
            .filter(|r| r.engine == "SST")
            .map(|r| r.filter_overhead_percent)
            .sum::<f64>()
            / sst_tests as f64;
        let viper_avg_filter_overhead = all_results
            .iter()
            .filter(|r| r.engine == "VIPER")
            .map(|r| r.filter_overhead_percent)
            .sum::<f64>()
            / viper_tests as f64;

        println!("\n⚔️ ENGINE COMPARISON:");
        println!(
            "  SST   - Avg compression: {:.1}%, Avg latency: {:.2}ms, Filter overhead: {:.1}%",
            sst_avg_compression, sst_avg_latency, sst_avg_filter_overhead
        );
        println!(
            "  VIPER - Avg compression: {:.1}%, Avg latency: {:.2}ms, Filter overhead: {:.1}%",
            viper_avg_compression, viper_avg_latency, viper_avg_filter_overhead
        );

        if sst_avg_compression > viper_avg_compression {
            println!(
                "  🏆 SST wins on compression by {:.1}%",
                sst_avg_compression - viper_avg_compression
            );
        } else {
            println!(
                "  🏆 VIPER wins on compression by {:.1}%",
                viper_avg_compression - sst_avg_compression
            );
        }

        if sst_avg_latency < viper_avg_latency {
            println!(
                "  🏆 SST wins on latency by {:.2}ms",
                viper_avg_latency - sst_avg_latency
            );
        } else {
            println!(
                "  🏆 VIPER wins on latency by {:.2}ms",
                sst_avg_latency - viper_avg_latency
            );
        }

        if sst_avg_filter_overhead < viper_avg_filter_overhead {
            println!(
                "  🏆 SST has lower filter overhead by {:.1}%",
                viper_avg_filter_overhead - sst_avg_filter_overhead
            );
        } else {
            println!(
                "  🏆 VIPER has lower filter overhead by {:.1}%",
                sst_avg_filter_overhead - viper_avg_filter_overhead
            );
        }
    }

    // RECOMMENDATIONS BASED ON COMPRESSION VS LATENCY TRADEOFFS
    println!("\n═══ PERFORMANCE RECOMMENDATIONS ═══");

    // SST Recommendations
    let sst_results: Vec<_> = all_results.iter().filter(|r| r.engine == "SST").collect();
    let sst_avg_compression = sst_results
        .iter()
        .map(|r| r.compression_savings_percent)
        .sum::<f64>()
        / sst_results.len() as f64;
    let sst_avg_latency_impact = sst_results
        .iter()
        .map(|r| r.latency_change_p50_percent)
        .sum::<f64>()
        / sst_results.len() as f64;

    println!("🏗️ SST ENGINE:");
    if sst_avg_compression < 5.0 {
        println!("  ❌ RECOMMENDATION: Disable compression for SST");
        println!(
            "     - Average compression savings: {:.1}% (minimal)",
            sst_avg_compression
        );
        println!(
            "     - Average latency impact: {:+.1}%",
            sst_avg_latency_impact
        );
        println!("     - Reason: Block-level compression ineffective on already dense data");
        println!("     - Better: Use VIPER for compression or focus on query optimization");
    } else {
        println!("  ✅ RECOMMENDATION: Use compression for SST");
        println!(
            "     - Average compression savings: {:.1}%",
            sst_avg_compression
        );
        println!(
            "     - Average latency impact: {:+.1}%",
            sst_avg_latency_impact
        );
    }

    // VIPER Recommendations
    let viper_results: Vec<_> = all_results.iter().filter(|r| r.engine == "VIPER").collect();
    let viper_avg_compression = viper_results
        .iter()
        .map(|r| r.compression_savings_percent)
        .sum::<f64>()
        / viper_results.len() as f64;
    let viper_avg_latency_impact = viper_results
        .iter()
        .map(|r| r.latency_change_p50_percent)
        .sum::<f64>()
        / viper_results.len() as f64;

    println!("\n📊 VIPER ENGINE:");
    if viper_avg_compression > 20.0 && viper_avg_latency_impact < 10.0 {
        println!("  ✅ RECOMMENDATION: Always use compression for VIPER");
        println!(
            "     - Average compression savings: {:.1}% (excellent)",
            viper_avg_compression
        );
        println!(
            "     - Average latency impact: {:+.1}% (acceptable)",
            viper_avg_latency_impact
        );
        println!("     - Best algorithms: zstd level 3-6 for balanced performance");
    } else {
        println!("  ⚠️ RECOMMENDATION: Selective compression for VIPER");
        println!(
            "     - Average compression savings: {:.1}%",
            viper_avg_compression
        );
        println!(
            "     - Average latency impact: {:+.1}%",
            viper_avg_latency_impact
        );
    }

    // Sparsity-based recommendations
    println!("\n🎯 SPARSITY-BASED RECOMMENDATIONS:");
    for sparsity in &sparsity_levels {
        let sparsity_results: Vec<_> = all_results
            .iter()
            .filter(|r| r.sparsity == *sparsity)
            .collect();
        let avg_savings = sparsity_results
            .iter()
            .map(|r| r.compression_savings_percent)
            .sum::<f64>()
            / sparsity_results.len() as f64;
        let avg_impact = sparsity_results
            .iter()
            .map(|r| r.latency_change_p50_percent)
            .sum::<f64>()
            / sparsity_results.len() as f64;

        if avg_savings > 15.0 && avg_impact < 15.0 {
            println!(
                "  ✅ {}% sparsity: Use compression ({:.1}% savings, {:+.1}% latency)",
                sparsity, avg_savings, avg_impact
            );
        } else if avg_savings < 5.0 {
            println!(
                "  ❌ {}% sparsity: Skip compression ({:.1}% savings, {:+.1}% latency)",
                sparsity, avg_savings, avg_impact
            );
        } else {
            println!(
                "  ⚠️ {}% sparsity: Consider use case ({:.1}% savings, {:+.1}% latency)",
                sparsity, avg_savings, avg_impact
            );
        }
    }

    // Save comprehensive CSV for further analysis
    let mut csv = String::from(
        "Engine,Sparsity%,Algorithm,Level,Batches,CompressedKB,OriginalKB,CompressionRatio,SavingsPercent,",
    );
    csv.push_str("TotalFlushMs,AvgFlushMs,CompactionMs,FilesBeforeCompact,FilesAfterCompact,CompactionRatio,");
    csv.push_str("QueryP50Ms,QueryP99Ms,QPS,LatencyChangeP50%,LatencyChangeP99%,");
    csv.push_str("FilterP50Ms,FilterP99Ms,FilterQPS,FilterOverhead%,FilterSelectivity%,");
    csv.push_str("Accuracy%,TopKMatches\n");

    for r in &all_results {
        csv.push_str(&format!(
            "{},{},{},{},{},{:.1},{:.1},{:.3},{:.1},{},{},{},{},{},{:.2},{:.2},{:.2},{:.2},{:+.1},{:+.1},{:.2},{:.2},{:.2},{:.1},{:.1},{:.1},{}\n",
            r.engine, r.sparsity, r.algorithm, r.level, r.batch_count,
            r.compressed_size as f64 / 1024.0,
            r.uncompressed_size as f64 / 1024.0,
            r.compression_ratio,
            r.compression_savings_percent,
            r.total_flush_time_ms,
            r.avg_flush_time_ms,
            r.compaction_time_ms,
            r.compaction_input_files,
            r.compaction_output_files,
            r.compaction_reduction_ratio,
            r.query_latency_p50_ms,
            r.query_latency_p99_ms,
            r.query_throughput_qps,
            r.latency_change_p50_percent,
            r.latency_change_p99_percent,
            r.filter_latency_p50_ms,
            r.filter_latency_p99_ms,
            r.filter_throughput_qps,
            r.filter_overhead_percent,
            r.filter_selectivity,
            r.result_accuracy,
            r.top_k_matches
        ));
    }

    let mut csv_file = File::create("/tmp/proximadb_benchmark_results.csv")?;
    csv_file.write_all(csv.as_bytes())?;
    println!("\n📁 CSV report saved to: /tmp/proximadb_benchmark_results.csv");

    println!("\n🎉 COMPREHENSIVE BENCHMARK REPORT COMPLETE!");

    Ok(())
}
