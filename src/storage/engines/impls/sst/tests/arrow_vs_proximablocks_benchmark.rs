/*
 * Copyright 2025 ProximaDB
 *
 * Benchmark: ArrowBlock vs ProximaBlocks Format Performance Comparison
 *
 * This test compares the performance characteristics of two block formats:
 * - ArrowBlock: Arrow IPC format for ecosystem interoperability
 * - ProximaBlocks: Native format optimized for vector workloads
 *
 * Metrics measured:
 * 1. Flush time (write performance)
 * 2. Search time (read performance)
 * 3. File size on disk (storage efficiency)
 *
 * Run with: cargo test --lib arrow_vs_proximablocks_benchmark -- --nocapture
 */

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Instant;
    use tempfile::TempDir;

    use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
    use crate::core::SstConfig;
    use crate::core::search::SearchParams;
    use crate::proto::proximadb_v1::{
        Collection, CollectionConfig, SqlValue, StorageAssignment, StorageConfig, VectorRecord,
    };
    use crate::storage::engines::impls::sst::core::SstEngine;
    use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
    use crate::storage::traits::{
        FlushParameters, StorageQueryContext, StorageQueryMetadata, UnifiedStorageEngine,
    };

    // Separator line constants for output formatting
    const SEPARATOR_DOUBLE: &str =
        "======================================================================";
    const SEPARATOR_SINGLE: &str = "--------------------------------------------------------------";

    /// Benchmark results for a single format
    #[derive(Debug)]
    struct FormatBenchmarkResult {
        format_name: String,
        flush_time_ms: f64,
        avg_search_time_ms: f64,
        total_file_size_bytes: u64,
        vectors_flushed: u64,
    }

    /// Generate test vectors with reproducible patterns
    fn generate_test_vectors(num_vectors: usize, dimension: usize) -> Vec<VectorRecord> {
        let mut vectors = Vec::with_capacity(num_vectors);

        for i in 0..num_vectors {
            let mut values = vec![0.0f32; dimension];
            // Create distinct patterns for each vector using sine waves
            for j in 0..dimension {
                values[j] = ((i as f32) * 0.1 + (j as f32) * 0.01).sin();
            }

            let mut metadata = HashMap::new();
            metadata.insert(
                "category".to_string(),
                SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                        format!("cat_{}", i % 10),
                    )),
                },
            );
            metadata.insert(
                "index".to_string(),
                SqlValue {
                    value: Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(
                        i as f64,
                    )),
                },
            );

            vectors.push(VectorRecord {
                id: format!("vec_{}", i),
                vector: values,
                metadata,
                timestamp: Some(i as i64),
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            });
        }

        vectors
    }

    /// Run benchmark for a specific block format
    async fn benchmark_format(
        format_name: &str,
        block_format: &str,
        vectors: &[VectorRecord],
        dimension: usize,
    ) -> Result<FormatBenchmarkResult> {
        // Create temporary directory for this format's test
        let temp_dir = TempDir::new()?;
        let base_path = temp_dir.path().to_str().unwrap().to_string();

        // Create filesystem factory
        let mut fs_config = FilesystemConfig::default();
        fs_config.default_fs = Some(format!("file://{}", base_path));
        let filesystem = Arc::new(FilesystemFactory::create(fs_config).await?);

        // Create SST engine with specified block format
        let mut sst_config = SstConfig::default();
        sst_config.block_format = block_format.to_string();
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());

        let engine =
            SstEngine::new_with_config(sst_config, filesystem.clone(), distance_compute.clone())
                .await?;

        let collection_id = format!("{}_collection", format_name.to_lowercase());

        // Create collection configuration
        let collection = Collection {
            id: collection_id.clone(),
            config: Some(CollectionConfig {
                name: collection_id.clone(),
                dimension: dimension as u32,
                storage_config: Some(StorageConfig::default()),
                ..Default::default()
            }),
            storage_assignment: Some(StorageAssignment {
                primary_path: base_path.clone(),
                base_location: base_path.clone(),
                ..Default::default()
            }),
            ..Default::default()
        };

        // Benchmark flush (write performance)
        let flush_start = Instant::now();

        let flush_params = FlushParameters {
            collection_id: Some(collection_id.clone()),
            vector_records: vectors.to_vec(),
            force: true,
            synchronous: true,
            collection_config: Some(collection.clone()),
            ..Default::default()
        };

        let flush_result = engine.do_flush(&flush_params).await?;
        let flush_time_ms = flush_start.elapsed().as_secs_f64() * 1000.0;

        assert!(
            flush_result.success,
            "Flush should succeed for {}",
            format_name
        );

        // Calculate total file size on disk
        let data_path = format!("{}/{}/data", base_path, collection_id);
        let total_file_size_bytes = calculate_directory_size(&data_path);

        // Benchmark search (read performance) - run 10 queries
        let num_queries = 10;
        let mut total_search_time_ms = 0.0;

        for query_idx in 0..num_queries {
            // Use different vectors as queries to get varied results
            let query_vector_idx = (query_idx * vectors.len() / num_queries) % vectors.len();
            let query_vector = vectors[query_vector_idx].vector.clone();

            let search_params = Arc::new(SearchParams {
                vector: Some(query_vector),
                top_k: Some(10),
                filters: None,
                filter_expression: None,
                ..Default::default()
            });

            let ctx = StorageQueryContext {
                search_params: search_params.clone(),
                collection: Arc::new(collection.clone()),
                metadata: StorageQueryMetadata {
                    collection_id: collection_id.clone(),
                    ..Default::default()
                },
            };

            let search_start = Instant::now();
            let _results = engine.search_vectors_unified(&ctx).await?;
            total_search_time_ms += search_start.elapsed().as_secs_f64() * 1000.0;
        }

        let avg_search_time_ms = total_search_time_ms / num_queries as f64;

        Ok(FormatBenchmarkResult {
            format_name: format_name.to_string(),
            flush_time_ms,
            avg_search_time_ms,
            total_file_size_bytes,
            vectors_flushed: flush_result.entries_flushed.unwrap_or(0),
        })
    }

    /// Calculate total size of files in a directory
    fn calculate_directory_size(path: &str) -> u64 {
        let mut total_size = 0u64;

        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                if let Ok(metadata) = entry.metadata() {
                    if metadata.is_file() {
                        total_size += metadata.len();
                    } else if metadata.is_dir() {
                        // Recursively calculate subdirectory sizes
                        if let Some(subpath) = entry.path().to_str() {
                            total_size += calculate_directory_size(subpath);
                        }
                    }
                }
            }
        }

        total_size
    }

    /// Format bytes into human-readable string
    fn format_bytes(bytes: u64) -> String {
        if bytes >= 1024 * 1024 {
            format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
        } else if bytes >= 1024 {
            format!("{:.2} KB", bytes as f64 / 1024.0)
        } else {
            format!("{} bytes", bytes)
        }
    }

    #[tokio::test]
    async fn arrow_vs_proximablocks_benchmark() -> Result<()> {
        // Initialize logging
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .try_init();

        println!("\n");
        println!("{}", SEPARATOR_DOUBLE);
        println!("  ArrowBlock vs ProximaBlocks Performance Benchmark");
        println!("{}", SEPARATOR_DOUBLE);

        // Test parameters
        let num_vectors = 1000;
        let dimension = 128;

        println!("\nTest Configuration:");
        println!("  Vectors: {}", num_vectors);
        println!("  Dimensions: {}", dimension);
        println!("  Search queries: 10");
        println!("  Top-k: 10");

        // Generate test vectors once (shared across both benchmarks)
        println!("\nGenerating {} test vectors...", num_vectors);
        let vectors = generate_test_vectors(num_vectors, dimension);
        println!("  Done.\n");

        // Benchmark ProximaBlocks (default format)
        println!("Benchmarking ProximaBlocks format...");
        let proximablocks_result =
            benchmark_format("ProximaBlocks", "ProximaBlocks", &vectors, dimension).await?;
        println!("  Done.\n");

        // Benchmark ArrowBlock format
        println!("Benchmarking ArrowBlock format...");
        let arrowblock_result =
            benchmark_format("ArrowBlock", "ArrowBlock", &vectors, dimension).await?;
        println!("  Done.\n");

        // Print results comparison
        println!("{}", SEPARATOR_DOUBLE);
        println!("  RESULTS");
        println!("{}", SEPARATOR_DOUBLE);
        println!();

        println!(
            "{:<20} {:>20} {:>20}",
            "Metric", "ProximaBlocks", "ArrowBlock"
        );
        println!("{}", SEPARATOR_SINGLE);

        // Flush time comparison
        let flush_diff = if proximablocks_result.flush_time_ms > 0.0 {
            ((arrowblock_result.flush_time_ms - proximablocks_result.flush_time_ms)
                / proximablocks_result.flush_time_ms)
                * 100.0
        } else {
            0.0
        };
        println!(
            "{:<20} {:>17.2} ms {:>17.2} ms  ({:+.1}%)",
            "Flush Time",
            proximablocks_result.flush_time_ms,
            arrowblock_result.flush_time_ms,
            flush_diff
        );

        // Search time comparison
        let search_diff = if proximablocks_result.avg_search_time_ms > 0.0 {
            ((arrowblock_result.avg_search_time_ms - proximablocks_result.avg_search_time_ms)
                / proximablocks_result.avg_search_time_ms)
                * 100.0
        } else {
            0.0
        };
        println!(
            "{:<20} {:>17.2} ms {:>17.2} ms  ({:+.1}%)",
            "Avg Search Time",
            proximablocks_result.avg_search_time_ms,
            arrowblock_result.avg_search_time_ms,
            search_diff
        );

        // File size comparison
        let size_diff = if proximablocks_result.total_file_size_bytes > 0 {
            ((arrowblock_result.total_file_size_bytes as f64
                - proximablocks_result.total_file_size_bytes as f64)
                / proximablocks_result.total_file_size_bytes as f64)
                * 100.0
        } else {
            0.0
        };
        println!(
            "{:<20} {:>20} {:>20}  ({:+.1}%)",
            "File Size",
            format_bytes(proximablocks_result.total_file_size_bytes),
            format_bytes(arrowblock_result.total_file_size_bytes),
            size_diff
        );

        // Vectors flushed (should be the same)
        println!(
            "{:<20} {:>20} {:>20}",
            "Vectors Flushed",
            proximablocks_result.vectors_flushed,
            arrowblock_result.vectors_flushed
        );

        println!();
        println!("{}", SEPARATOR_DOUBLE);
        println!("  SUMMARY");
        println!("{}", SEPARATOR_DOUBLE);

        // Determine winners for each metric
        let flush_winner = if proximablocks_result.flush_time_ms <= arrowblock_result.flush_time_ms
        {
            "ProximaBlocks"
        } else {
            "ArrowBlock"
        };

        let search_winner =
            if proximablocks_result.avg_search_time_ms <= arrowblock_result.avg_search_time_ms {
                "ProximaBlocks"
            } else {
                "ArrowBlock"
            };

        let size_winner = if proximablocks_result.total_file_size_bytes
            <= arrowblock_result.total_file_size_bytes
        {
            "ProximaBlocks"
        } else {
            "ArrowBlock"
        };

        println!();
        println!("  Faster Flush:    {}", flush_winner);
        println!("  Faster Search:   {}", search_winner);
        println!("  Smaller Files:   {}", size_winner);
        println!();

        // Note about use cases
        println!("Note:");
        println!("  - ProximaBlocks: Optimized for vector workloads, B+ tree index");
        println!("  - ArrowBlock: Interoperable with PyArrow, DuckDB, Polars ecosystem");
        println!();

        Ok(())
    }
}
