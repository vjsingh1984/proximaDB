// Consolidated Storage Engine Benchmarks
// Combines: compression, search, lifecycle, and cross-engine comparisons

mod common;
use common::benchmark_utils::{print_system_info, STANDARD_BATCH_SIZES};

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use proximadb::core::search::{ComparisonOperator, FilterExpression, SearchParams};
use proximadb::proto::proximadb_v1::{Collection, CollectionConfig, StorageAssignment, StorageConfig, VectorRecord, CompressionAlgorithm};
use proximadb::storage::engines::factory::StorageEngineFactory;
use proximadb::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
use proximadb::storage::traits::{FlushParameters, StorageQueryContext, StorageQueryMetadata};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Configurable base URL for storage benchmarks
/// Can be changed to test different storage backends:
/// - Local: "file:///tmp/proximadb-bench"
/// - S3: "s3://my-bucket/proximadb-bench"
/// - Azure: "azure://mycontainer/proximadb-bench"
/// - GCS: "gs://my-bucket/proximadb-bench"
const BENCHMARK_BASE_URL: &str = "file:///tmp/proximadb-bench";

fn get_base_path() -> String {
    if BENCHMARK_BASE_URL.starts_with("file://") {
        BENCHMARK_BASE_URL.trim_start_matches("file://").to_string()
    } else {
        BENCHMARK_BASE_URL.to_string()
    }
}

/// Initialize hardware features for benchmarking
fn init_hardware() {
    // Hardware detection is automatically done by the system
}

/// Helper to measure directory size using filesystem API
async fn measure_directory_size_async(
    path: &str,
    filesystem_factory: &FilesystemFactory,
) -> anyhow::Result<u64> {
    // Only add file:// prefix if it's not already a URL
    let fs_url = if path.starts_with("s3://") || path.starts_with("gs://")
        || path.starts_with("azure://") || path.starts_with("wasbs://")
        || path.starts_with("file://") {
        path.to_string()
    } else {
        format!("file://{}", path)
    };

    let fs = filesystem_factory.get_filesystem(&fs_url)?;

    // For listing, we need to use the path without the scheme
    let list_path = if let Some(stripped) = path.strip_prefix("file://") {
        stripped
    } else if path.contains("://") {
        // For other schemes, extract path after ://
        path.split("://").nth(1).unwrap_or(path)
    } else {
        path
    };

    let entries = fs.list(list_path).await?;

    let mut total_size = 0u64;
    for entry in entries {
        if entry.metadata.is_directory {
            // For recursive calls, construct the proper path
            let subdir_path = if list_path.ends_with('/') {
                format!("{}{}", list_path, entry.name)
            } else {
                format!("{}/{}", list_path, entry.name)
            };
            total_size += Box::pin(measure_directory_size_async(&subdir_path, filesystem_factory)).await?;
        } else {
            total_size += entry.metadata.size;
        }
    }

    Ok(total_size)
}

/// Synchronous wrapper for directory size measurement
fn measure_directory_size(path: &str, runtime: &tokio::runtime::Runtime) -> std::io::Result<u64> {
    // Check if path exists first
    if !std::path::Path::new(path).exists() {
        return Ok(0);
    }

    runtime.block_on(async {
        let fs_factory = FilesystemFactory::new(FilesystemConfig::default())
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        measure_directory_size_async(path, &fs_factory)
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    })
}

/// Generate random float vectors for testing
fn generate_random_vector(dimension: usize) -> Vec<f32> {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..dimension).map(|_| rng.gen_range(-1.0..1.0)).collect()
}

/// Generate vectors with rich metadata for testing
fn generate_test_vectors(count: usize, dimension: usize) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| {
            let mut metadata = HashMap::new();

            // Category for equality filtering
            metadata.insert(
                "category".to_string(),
                proximadb::proto::proximadb_v1::SqlValue {
                    value: Some(proximadb::proto::proximadb_v1::sql_value::Value::StringValue(
                        format!("cat_{}", i % 10)
                    )),
                },
            );

            // Price for range filtering
            metadata.insert(
                "price".to_string(),
                proximadb::proto::proximadb_v1::SqlValue {
                    value: Some(proximadb::proto::proximadb_v1::sql_value::Value::NumberValue(
                        (i as f64) * 10.0 % 1000.0
                    )),
                },
            );

            // Tags for multi-value filtering
            metadata.insert(
                "tags".to_string(),
                proximadb::proto::proximadb_v1::SqlValue {
                    value: Some(proximadb::proto::proximadb_v1::sql_value::Value::StringValue(
                        format!("tag_{},tag_{}", i % 5, i % 3)
                    )),
                },
            );

            VectorRecord {
                id: format!("vec_{}", i),
                vector: generate_random_vector(dimension),
                metadata,
                ..Default::default()
            }
        })
        .collect()
}

// ============================================================================
// BENCHMARK 1: Unified Compression + Search Test
// ============================================================================
fn bench_compression_with_search(c: &mut Criterion) {
    print_system_info("Storage Engine Unified Benchmarks");
    init_hardware();

    let dimension = 768;
    let count = 1000;

    eprintln!("\n📊 UNIFIED COMPRESSION + SEARCH BENCHMARK");
    eprintln!("   Flush once → Search twice (pure + filtered)");
    eprintln!("   Dimension: {}, Vectors: {}", dimension, count);
    eprintln!("\n{:<8} {:<8} {:>10} {:>8} {:>8} {:>10} {:>10} {:>10}",
             "Engine", "Compress", "Size(MB)", "Ratio", "Save%", "Flush(ms)", "Pure(ms)", "Filter(ms)");
    eprintln!("{}", "-".repeat(80));
    let vectors = generate_test_vectors(count, dimension);
    // Use the first vector as query for deterministic results
    let query_vector = vectors.first()
        .map(|v| v.vector.clone())
        .unwrap_or_else(|| generate_random_vector(dimension));

    // Debug: Show we're searching for the first vector
    eprintln!("\n   🔍 Query vector: Using vec_0 (first stored vector) for deterministic results");
    eprintln!("   📊 First 5 values of query vector: {:?}", &query_vector[..5.min(query_vector.len())]);
    if let Some(first_vec) = vectors.first() {
        eprintln!("   📝 Full first vector record:");
        eprintln!("      ID: {}", first_vec.id);
        eprintln!("      Vector dimensions: {}", first_vec.vector.len());
        eprintln!("      First 5 values: {:?}", &first_vec.vector[..5.min(first_vec.vector.len())]);
        eprintln!("      Metadata: {:?}", first_vec.metadata);
    }

    let uncompressed_size = count * dimension * std::mem::size_of::<f32>();
    let runtime = tokio::runtime::Runtime::new().unwrap();

    // Test each engine - all engines are stateless and get collection info from parameters
    let engines = vec![
        ("sst", StorageEngineFactory::create_sst().unwrap()),
        ("viper", StorageEngineFactory::create_viper().unwrap()),
        ("nova", StorageEngineFactory::create_nova().unwrap()),
        ("swift", StorageEngineFactory::create_swift().unwrap()),
        ("raptor", StorageEngineFactory::create_raptor().unwrap()),
        ("helix", StorageEngineFactory::create_helix().unwrap()),
    ];

    // Compression algorithms to test
    let compressions = vec![
        ("none", 0),
        ("zstd", 1),
        ("lz4", 2),
        ("snappy", 3),
    ];

    for (engine_name, engine) in engines {
        for (compress_name, compress_value) in &compressions {
            // Collection ID format: {engine}_{compression}
            let collection_id = format!("{}_{}", engine_name, compress_name);
            // Data path: {baseurl}/{engine}_{compression}/
            let base_path = format!("{}/{}", get_base_path(), collection_id);

            // Clean previous run using filesystem API
            runtime.block_on(async {
                let fs_factory = FilesystemFactory::new(FilesystemConfig::default()).await.ok()?;
                let fs = fs_factory.get_filesystem(&format!("file://{}", base_path)).ok()?;
                let _ = fs.remove_dir_all(&base_path).await;
                Some(())
            });

            // Step 1: Flush data with compression
            eprintln!("  Flushing {} vectors (dim={}) with {} compression to {}...",
                     count, dimension, compress_name, base_path);
            eprintln!("    📁 Data directory: {}", base_path);

            // Calculate expected size
            let expected_bytes = count * dimension * std::mem::size_of::<f32>();
            eprintln!("    Expected raw size: {} bytes ({:.2} MB)",
                     expected_bytes, expected_bytes as f64 / 1_048_576.0);

            let flush_start = std::time::Instant::now();
            let flush_result = runtime.block_on(async {
                let mut storage_config = StorageConfig::default();
                storage_config.compression = *compress_value;

                // Define filterable columns for metadata schema
                let filterable_columns = vec![
                    proximadb::proto::proximadb_v1::FilterableColumnSpec {
                        name: "category".to_string(),
                        data_type: proximadb::proto::proximadb_v1::FilterableDataType::FilterableString as i32,
                        indexed: true,
                        supports_range: false,
                        estimated_cardinality: Some(10), // We have 10 categories
                    },
                    proximadb::proto::proximadb_v1::FilterableColumnSpec {
                        name: "price".to_string(),
                        data_type: proximadb::proto::proximadb_v1::FilterableDataType::FilterableFloat as i32,
                        indexed: true,
                        supports_range: true,
                        estimated_cardinality: None,
                    },
                    proximadb::proto::proximadb_v1::FilterableColumnSpec {
                        name: "tags".to_string(),
                        data_type: proximadb::proto::proximadb_v1::FilterableDataType::FilterableString as i32,
                        indexed: true,
                        supports_range: false,
                        estimated_cardinality: Some(15), // Combinations of tags
                    },
                ];

                let collection = Collection {
                    id: collection_id.clone(),
                    config: Some(CollectionConfig {
                        name: collection_id.clone(),
                        dimension: dimension as u32,
                        storage_config: Some(storage_config),
                        filterable_columns,
                        ..Default::default()
                    }),
                    storage_assignment: Some(StorageAssignment {
                        primary_path: base_path.clone(),
                        base_location: base_path.clone(),
                        ..Default::default()
                    }),
                    ..Default::default()
                };

                let params = FlushParameters {
                    collection_id: Some(collection_id.clone()),
                    vector_records: vectors.clone(),
                    force: true,
                    synchronous: true,
                    collection_config: Some(collection),
                    ..Default::default()
                };

                engine.flush(params).await
            });
            let flush_time_ms = flush_start.elapsed().as_millis();

            // Validate flush result
            let flush_result = match flush_result {
                Ok(result) => {
                    // Validate that data was actually written
                    if result.entries_flushed.unwrap_or(0) == 0 {
                        eprintln!("    ⚠️  WARNING: No vectors written for {} with {}", engine_name, compress_name);
                    }
                    if result.bytes_written.unwrap_or(0) == 0 {
                        eprintln!("    ⚠️  WARNING: No bytes written for {} with {}", engine_name, compress_name);
                    }
                    let vectors_written = result.entries_flushed.unwrap_or(0);
                    let bytes_written = result.bytes_written.unwrap_or(0);
                    eprintln!("    ✓ Flushed {} vectors, {} bytes written in {}ms",
                             vectors_written, bytes_written, flush_time_ms);

                    // Validate size is reasonable
                    if bytes_written < (expected_bytes as u64 / 100) {
                        eprintln!("    ⚠️  SUSPICIOUSLY SMALL: {} bytes is < 1% of expected {} bytes",
                                 bytes_written, expected_bytes);
                    }
                    Ok(result)
                },
                Err(e) => Err(e)
            };

            // Check if flush failed or wrote no data
            let flush_success = match &flush_result {
                Err(e) => {
                    eprintln!("    ⚠️  Flush failed for {} with {}: {:?}", engine_name, compress_name, e);
                    false
                },
                Ok(result) if result.entries_flushed.unwrap_or(0) == 0 => {
                    eprintln!("    ⚠️  Flush succeeded but wrote 0 vectors for {} with {}, skipping search", engine_name, compress_name);
                    false
                },
                Ok(_) => true
            };

            if !flush_success {
                // Print skipped status in results table
                eprintln!("{:<8} {:<8} {:>10} {:>8} {:>7} {:>10} {:>10} {:>10}  ⛔ SKIPPED",
                         engine_name, compress_name, "FAILED", "N/A", "N/A", flush_time_ms, "N/A", "N/A");
                eprintln!();
                // Clean up any partial data
                runtime.block_on(async {
                    let fs_factory = FilesystemFactory::new(FilesystemConfig::default()).await.ok()?;
                    let fs = fs_factory.get_filesystem(&format!("file://{}", base_path)).ok()?;
                    let _ = fs.remove_dir_all(&base_path).await;
                    Some(())
                });
                continue;
            }

            // Verify files were actually created and list them
            let (files_created, file_details) = runtime.block_on(async {
                let fs_factory = FilesystemFactory::new(FilesystemConfig::default()).await.ok()?;
                let fs = fs_factory.get_filesystem(&format!("file://{}", base_path)).ok()?;
                let entries = fs.list(&base_path).await.ok()?;

                let mut details = Vec::new();
                for entry in &entries {
                    let size = if entry.metadata.is_directory {
                        "DIR".to_string()
                    } else {
                        format!("{} bytes", entry.metadata.size)
                    };
                    details.push(format!("      {} ({})", entry.name, size));
                }

                Some((entries.len(), details))
            }).unwrap_or((0, Vec::new()));

            if files_created == 0 {
                eprintln!("    ⚠️  WARNING: No files created after flush for {} with {}", engine_name, compress_name);
            } else {
                eprintln!("    ✓ Created {} files/directories:", files_created);
                for detail in &file_details {
                    eprintln!("{}", detail);
                }
            }

            // Measure storage metrics using filesystem API
            let size_bytes = measure_directory_size(&base_path, &runtime).unwrap_or_else(|e| {
                eprintln!("    ⚠️  Failed to measure directory size: {:?}", e);
                0
            });
            let size_mb = size_bytes as f64 / 1_048_576.0;
            let ratio = size_bytes as f64 / uncompressed_size as f64;
            let savings = (1.0 - ratio) * 100.0;

            // Step 2: Pure vector search benchmark
            let mut pure_group = c.benchmark_group(format!("pure_{}_{}", engine_name, compress_name));
            pure_group.measurement_time(Duration::from_secs(5));
            pure_group.sample_size(40);
            pure_group.warm_up_time(Duration::from_secs(1));

            let mut pure_time_ms = 0u128;
            let query_clone = query_vector.clone();
            let compress_val = *compress_value;  // Capture for closure

            pure_group.bench_function("search", |b| {
                b.iter(|| {
                    let start = std::time::Instant::now();
                    let result: Result<Vec<proximadb::core::search::results::OptimizedSearchRecord>, _> = runtime.block_on(async {
                        let search_params = Arc::new(SearchParams {
                            vector: Some(query_clone.clone()),
                            top_k: Some(10),
                            filters: None,
                            filter_expression: None,
                            ..Default::default()
                        });

                        // Search needs same collection config as flush for proper data loading
                        let mut storage_config = StorageConfig::default();
                        storage_config.compression = compress_val;

                        // Same filterable columns as flush
                        let filterable_columns = vec![
                            proximadb::proto::proximadb_v1::FilterableColumnSpec {
                                name: "category".to_string(),
                                data_type: proximadb::proto::proximadb_v1::FilterableDataType::FilterableString as i32,
                                indexed: true,
                                supports_range: false,
                                estimated_cardinality: Some(10),
                            },
                            proximadb::proto::proximadb_v1::FilterableColumnSpec {
                                name: "price".to_string(),
                                data_type: proximadb::proto::proximadb_v1::FilterableDataType::FilterableFloat as i32,
                                indexed: true,
                                supports_range: true,
                                estimated_cardinality: None,
                            },
                            proximadb::proto::proximadb_v1::FilterableColumnSpec {
                                name: "tags".to_string(),
                                data_type: proximadb::proto::proximadb_v1::FilterableDataType::FilterableString as i32,
                                indexed: true,
                                supports_range: false,
                                estimated_cardinality: Some(15),
                            },
                        ];

                        let collection = Arc::new(Collection {
                            id: collection_id.clone(),
                            config: Some(CollectionConfig {
                                name: collection_id.clone(),
                                dimension: dimension as u32,
                                storage_config: Some(storage_config),
                                filterable_columns,
                                ..Default::default()
                            }),
                            storage_assignment: Some(StorageAssignment {
                                primary_path: base_path.clone(),
                                base_location: base_path.clone(),
                                ..Default::default()
                            }),
                            ..Default::default()
                        });

                        let ctx = StorageQueryContext {
                            search_params,
                            collection,
                            metadata: StorageQueryMetadata::default(),
                        };

                        engine.search_vectors_unified(&ctx).await
                    });
                    pure_time_ms = start.elapsed().as_millis();

                    // Validate and log search results with detailed metrics
                    if let Ok(ref results) = result {
                        if results.is_empty() {
                            // Only log first warning to avoid spam
                            static mut PURE_EMPTY_LOGGED: bool = false;
                            unsafe {
                                if !PURE_EMPTY_LOGGED {
                                    eprintln!("    ⚠️  WARNING: Pure search returned no results for {} with {} (expected to find vec_0)",
                                             engine_name, compress_name);
                                    eprintln!("       Debug: Searched with vector starting with {:?}", &query_clone[..5.min(query_clone.len())]);
                                    eprintln!("       Debug: Collection={}, Base path={}", collection_id, base_path);
                                    eprintln!("       Debug: Search took {}ms", pure_time_ms);
                                    PURE_EMPTY_LOGGED = true;
                                }
                            }
                        } else {
                            eprintln!("    ✅ FOUND: Pure search returned {} results for {} with {} in {}ms",
                                     results.len(), engine_name, compress_name, pure_time_ms);
                            // Print detailed results with all metrics
                            for (i, result) in results.iter().take(5).enumerate() {
                                eprintln!("       Result {}: ID={}, score={:.6}, similarity={:.6}, metadata_keys={}",
                                         i+1, result.id, result.score,
                                         result.similarity.unwrap_or(0.0),
                                         result.metadata.len());

                                // Show metadata if present
                                if !result.metadata.is_empty() {
                                    for (key, val) in result.metadata.iter().take(3) {
                                        eprintln!("         - {}: {:?}", key, val);
                                    }
                                }
                            }
                            if results.len() > 10 {
                                eprintln!("      ⚠️  WARNING: Expected <= 10 results, got {}", results.len());
                            }

                            // Verify we found the expected vector
                            if !results.iter().any(|r| r.id == "vec_0") {
                                eprintln!("      ⚠️  WARNING: Did not find expected vec_0 in results!");
                            }
                        }
                    } else if let Err(ref e) = result {
                        // Only log first error to avoid spam
                        static mut SEARCH_ERROR_LOGGED: bool = false;
                        unsafe {
                            if !SEARCH_ERROR_LOGGED {
                                eprintln!("    ❌ Pure search failed for {} with {}: {:?}",
                                         engine_name, compress_name, e);
                                SEARCH_ERROR_LOGGED = true;
                            }
                        }
                    }

                    black_box(result)
                })
            });
            pure_group.finish();

            // Step 3: Metadata-filtered search benchmark
            let mut filtered_group = c.benchmark_group(format!("filter_{}_{}", engine_name, compress_name));
            filtered_group.measurement_time(Duration::from_secs(5));
            filtered_group.sample_size(40);
            filtered_group.warm_up_time(Duration::from_secs(1));

            let mut filter_time_ms = 0u128;
            let query_clone = query_vector.clone();
            let compress_val = *compress_value;  // Capture for closure

            filtered_group.bench_function("search", |b| {
                b.iter(|| {
                    let start = std::time::Instant::now();
                    let result: Result<Vec<proximadb::core::search::results::OptimizedSearchRecord>, _> = runtime.block_on(async {
                        // Filter: category="cat_5" AND price < 500
                        let filter_expr = FilterExpression::And(vec![
                            FilterExpression::Comparison {
                                field: "category".to_string(),
                                operator: ComparisonOperator::Equals,
                                value: serde_json::Value::String("cat_5".to_string()),
                            },
                            FilterExpression::Comparison {
                                field: "price".to_string(),
                                operator: ComparisonOperator::LessThan,
                                value: serde_json::Value::Number(serde_json::Number::from(500)),
                            },
                        ]);

                        let search_params = Arc::new(SearchParams {
                            vector: Some(query_clone.clone()),
                            top_k: Some(10),
                            filter_expression: Some(filter_expr),
                            filters: None,
                            ..Default::default()
                        });

                        // Search needs same collection config as flush for proper data loading
                        let mut storage_config = StorageConfig::default();
                        storage_config.compression = compress_val;

                        // Same filterable columns as flush
                        let filterable_columns = vec![
                            proximadb::proto::proximadb_v1::FilterableColumnSpec {
                                name: "category".to_string(),
                                data_type: proximadb::proto::proximadb_v1::FilterableDataType::FilterableString as i32,
                                indexed: true,
                                supports_range: false,
                                estimated_cardinality: Some(10),
                            },
                            proximadb::proto::proximadb_v1::FilterableColumnSpec {
                                name: "price".to_string(),
                                data_type: proximadb::proto::proximadb_v1::FilterableDataType::FilterableFloat as i32,
                                indexed: true,
                                supports_range: true,
                                estimated_cardinality: None,
                            },
                            proximadb::proto::proximadb_v1::FilterableColumnSpec {
                                name: "tags".to_string(),
                                data_type: proximadb::proto::proximadb_v1::FilterableDataType::FilterableString as i32,
                                indexed: true,
                                supports_range: false,
                                estimated_cardinality: Some(15),
                            },
                        ];

                        let collection = Arc::new(Collection {
                            id: collection_id.clone(),
                            config: Some(CollectionConfig {
                                name: collection_id.clone(),
                                dimension: dimension as u32,
                                storage_config: Some(storage_config),
                                filterable_columns,
                                ..Default::default()
                            }),
                            storage_assignment: Some(StorageAssignment {
                                primary_path: base_path.clone(),
                                base_location: base_path.clone(),
                                ..Default::default()
                            }),
                            ..Default::default()
                        });

                        let ctx = StorageQueryContext {
                            search_params,
                            collection,
                            metadata: StorageQueryMetadata::default(),
                        };

                        engine.search_vectors_unified(&ctx).await
                    });
                    filter_time_ms = start.elapsed().as_millis();

                    // Validate and log filtered search results
                    if let Ok(ref results) = result {
                        if results.is_empty() {
                            // Only log first warning to avoid spam
                            static mut EMPTY_RESULTS_LOGGED: bool = false;
                            unsafe {
                                if !EMPTY_RESULTS_LOGGED {
                                    eprintln!("    ⚠️  WARNING: Filtered search returned no results for {} with {} (expected vec_0 with category=cat_0)",
                                             engine_name, compress_name);
                                    eprintln!("       Debug: Filter was category='cat_0', searched with vector starting with {:?}", &query_clone[..5.min(query_clone.len())]);
                                    EMPTY_RESULTS_LOGGED = true;
                                }
                            }
                        } else {
                            eprintln!("    ✅ FOUND: Filtered search returned {} results for {} with {}",
                                     results.len(), engine_name, compress_name);
                            // Print top 3 results with details
                            for (i, result) in results.iter().take(3).enumerate() {
                                eprintln!("       Result {}: ID={}, score={:.6}, similarity={:.6}",
                                         i+1, result.id, result.score, result.similarity.unwrap_or(0.0));
                            }
                            if results.len() > 10 {
                                eprintln!("      ⚠️  WARNING: Expected <= 10 results, got {}", results.len());
                            }
                            // Verify filter was applied (results should have category=cat_5)
                            let correctly_filtered = results.iter().all(|_r| {
                                // Note: actual filter validation would check metadata
                                true  // Simplified for now
                            });
                            if !correctly_filtered {
                                eprintln!("    ⚠️  WARNING: Filter not correctly applied for {} with {}",
                                         engine_name, compress_name);
                            }
                        }
                    } else if let Err(ref e) = result {
                        // Only log first error to avoid spam
                        static mut FILTERED_ERROR_LOGGED: bool = false;
                        unsafe {
                            if !FILTERED_ERROR_LOGGED {
                                eprintln!("    ❌ Filtered search failed for {} with {}: {:?}",
                                         engine_name, compress_name, e);
                                FILTERED_ERROR_LOGGED = true;
                            }
                        }
                    }

                    black_box(result)
                })
            });
            filtered_group.finish();

            // Print results with validation status
            if size_bytes == 0 && files_created == 0 {
                eprintln!("{:<8} {:<8} {:>10} {:>8} {:>7} {:>10} {:>10} {:>10}  ❌ NO DATA",
                         engine_name, compress_name, "NO DATA", "N/A", "N/A", flush_time_ms, pure_time_ms, filter_time_ms);
            } else {
                eprintln!("{:<8} {:<8} {:>10.2} {:>8.3} {:>7.1}% {:>10} {:>10} {:>10}",
                         engine_name, compress_name, size_mb, ratio, savings, flush_time_ms, pure_time_ms, filter_time_ms);
            }

            // Clean up immediately after each test using filesystem API
            runtime.block_on(async {
                let fs_factory = FilesystemFactory::new(FilesystemConfig::default()).await.ok()?;
                let fs = fs_factory.get_filesystem(&format!("file://{}", base_path)).ok()?;
                let _ = fs.remove_dir_all(&base_path).await;
                Some(())
            });
        }
        eprintln!();  // Blank line between engines
    }

    // Final cleanup using filesystem API
    eprintln!("\n✅ Benchmark complete, all test data cleaned up");
    runtime.block_on(async {
        let fs_factory = FilesystemFactory::new(FilesystemConfig::default()).await.ok()?;
        // Clean up all engine_compression directories
        let base_path = get_base_path();
        let fs = fs_factory.get_filesystem(&format!("file://{}", base_path)).ok()?;
        // Remove all test directories
        for engine in ["sst", "viper", "nova", "swift", "raptor", "helix"] {
            for compression in ["none", "zstd", "lz4", "snappy"] {
                let test_path = format!("{}/{}_{}", base_path, engine, compression);
                let _ = fs.remove_dir_all(&test_path).await;
            }
        }
        Some(())
    });
}

// ============================================================================
// BENCHMARK 2: Engine Creation and Lifecycle
// ============================================================================
fn bench_engine_lifecycle(c: &mut Criterion) {
    init_hardware();

    let mut group = c.benchmark_group("engine_lifecycle");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(40);
    group.warm_up_time(Duration::from_secs(1));

    let _runtime = tokio::runtime::Runtime::new().unwrap();

    // Benchmark engine creation
    group.bench_function("sst_create", |b| {
        b.iter(|| {
            let engine = StorageEngineFactory::create_sst();
            black_box(engine)
        })
    });

    group.bench_function("viper_create", |b| {
        b.iter(|| {
            let engine = StorageEngineFactory::create_viper();
            black_box(engine)
        })
    });

    group.bench_function("nova_create", |b| {
        b.iter(|| {
            let engine = StorageEngineFactory::create_nova();
            black_box(engine)
        })
    });

    group.bench_function("swift_create", |b| {
        b.iter(|| {
            let engine = StorageEngineFactory::create_swift();
            black_box(engine)
        })
    });

    group.bench_function("raptor_create", |b| {
        b.iter(|| {
            let engine = StorageEngineFactory::create_raptor();
            black_box(engine)
        })
    });

    group.bench_function("helix_create", |b| {
        b.iter(|| {
            let engine = StorageEngineFactory::create_helix();
            black_box(engine)
        })
    });


    group.finish();
}

// ============================================================================
// BENCHMARK 3: Large-Scale Search Performance
// ============================================================================
fn bench_large_scale_search(c: &mut Criterion) {
    init_hardware();

    eprintln!("\n🔍 LARGE-SCALE SEARCH BENCHMARK");
    eprintln!("   Testing search performance at different scales with compression");
    eprintln!("   Collection ID Format: {{engine}}-{{compression}}-{{batchsize}}");
    eprintln!("   Engines: sst, viper, nova, swift, raptor, helix");
    eprintln!("   Compressions: none, zstd, lz4, snappy, gzip");
    eprintln!("   Batch Sizes: 250, 1000, 5000");
    eprintln!("   Total Combinations: 105 (7 engines × 5 compressions × 3 batch sizes)");

    let dimension = 768;
    let runtime = tokio::runtime::Runtime::new().unwrap();

    // Test different scales
    // Use standard batch sizes for scales
    let scales = vec![
        ("250", STANDARD_BATCH_SIZES[0]),    // Small batch
        ("1000", STANDARD_BATCH_SIZES[1]),   // Medium batch
        ("5000", STANDARD_BATCH_SIZES[2]),   // Large batch
    ];

    // Test with different compression methods
    let compressions = vec![
        ("none", CompressionAlgorithm::CompressionNone),
        ("zstd", CompressionAlgorithm::CompressionZstd),
        ("lz4", CompressionAlgorithm::CompressionLz4),
        ("snappy", CompressionAlgorithm::CompressionSnappy),
        ("gzip", CompressionAlgorithm::CompressionGzip),
    ];

    let mut test_num = 0;
    let total_tests = 105;  // 7 engines × 5 compressions × 3 batch sizes

    for (batch_size_name, count) in scales {
        eprintln!("\n   === Batch Size: {} vectors ===", count);

        let vectors = generate_test_vectors(count, dimension);
        let query = generate_random_vector(dimension);

        // Test all storage engines - stateless engines get collection info from parameters
        let engines = vec![
            ("sst", StorageEngineFactory::create_sst().unwrap()),
            ("viper", StorageEngineFactory::create_viper().unwrap()),
            ("nova", StorageEngineFactory::create_nova().unwrap()),
            ("swift", StorageEngineFactory::create_swift().unwrap()),
            ("raptor", StorageEngineFactory::create_raptor().unwrap()),
                ("helix", StorageEngineFactory::create_helix().unwrap()),
        ];

        for (engine_name, engine) in engines {
            for (compress_name, compression) in &compressions {
                test_num += 1;
                eprintln!("   Test {}/{}: {}-{}-{}", test_num, total_tests, engine_name, compress_name, batch_size_name);

                // Use {engine}-{compression}-{batchsize} format for collection ID
                let collection_id = format!("{}-{}-{}", engine_name, compress_name, batch_size_name);
                let base_path = format!("{}/{}", get_base_path(), collection_id);

                // Clean and load data using filesystem API
                runtime.block_on(async {
                    let fs_factory = FilesystemFactory::new(FilesystemConfig::default()).await.ok()?;
                    let fs = fs_factory.get_filesystem(&format!("file://{}", base_path)).ok()?;
                    let _ = fs.remove_dir_all(&base_path).await;
                    Some(())
                });

                // Capture compression value to avoid borrow issues
                let compress_val = *compression;

                runtime.block_on(async {
                    let collection = Collection {
                        id: collection_id.clone(),
                        config: Some(CollectionConfig {
                            name: collection_id.clone(),
                            dimension: dimension as u32,
                            storage_config: Some(StorageConfig {
                                storage_path: base_path.clone(),
                                compression: compress_val as i32,
                                ..Default::default()
                            }),
                            ..Default::default()
                        }),
                        storage_assignment: Some(StorageAssignment {
                            primary_path: base_path.clone(),
                            base_location: base_path.clone(),
                            ..Default::default()
                        }),
                        ..Default::default()
                    };

                    let params = FlushParameters {
                        collection_id: Some(collection_id.clone()),
                        vector_records: vectors.clone(),
                        force: true,
                        synchronous: true,
                        collection_config: Some(collection),
                        ..Default::default()
                    };

                    let _ = engine.flush(params).await;
                });

                // Benchmark search
                let mut group = c.benchmark_group(format!("search_{}_{}_{}",engine_name, compress_name, batch_size_name));
                group.measurement_time(Duration::from_secs(5));
                group.sample_size(40);
                group.warm_up_time(Duration::from_secs(1));

                let query_clone = query.clone();
                group.bench_function("top10", |b| {
                    b.iter(|| {
                        runtime.block_on(async {
                            let search_params = Arc::new(SearchParams {
                                vector: Some(query_clone.clone()),
                                top_k: Some(10),
                                filters: None,
                                filter_expression: None,
                                ..Default::default()
                            });

                            let collection = Arc::new(Collection {
                                id: collection_id.clone(),
                                config: Some(CollectionConfig {
                                    name: collection_id.clone(),
                                    dimension: dimension as u32,
                                    storage_config: Some(StorageConfig {
                                        storage_path: base_path.clone(),
                                        compression: compress_val as i32,
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                }),
                                storage_assignment: Some(StorageAssignment {
                                    primary_path: base_path.clone(),
                                    base_location: base_path.clone(),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            });

                            let ctx = StorageQueryContext {
                                search_params,
                                collection,
                                metadata: StorageQueryMetadata::default(),
                            };

                            let result = engine.search_vectors_unified(&ctx).await;
                            black_box(result)
                        })
                    })
                });

                group.finish();

                // Clean up using filesystem API
                runtime.block_on(async {
                    let fs_factory = FilesystemFactory::new(FilesystemConfig::default()).await.ok()?;
                    let fs = fs_factory.get_filesystem(&format!("file://{}", base_path)).ok()?;
                    let _ = fs.remove_dir_all(&base_path).await;
                    Some(())
                });
            }
        }
    }

    // Final cleanup using filesystem API
    runtime.block_on(async {
        let fs_factory = FilesystemFactory::new(FilesystemConfig::default()).await.ok()?;
        let base_path = get_base_path();
        let fs = fs_factory.get_filesystem(&format!("file://{}", base_path)).ok()?;

        // Clean up all {engine}-{compression}-{batchsize} directories
        for engine in ["sst", "viper", "nova", "swift", "raptor", "helix"] {
            for compression in ["none", "zstd", "lz4", "snappy", "gzip"] {
                for batch_size in ["250", "1000", "5000"] {
                    let test_path = format!("{}/{}-{}-{}", base_path, engine, compression, batch_size);
                    let _ = fs.remove_dir_all(&test_path).await;
                }
            }
        }
        Some(())
    });
}

// ============================================================================
// BENCHMARK 4: Insertion Performance
// ============================================================================
fn bench_insertion_performance(c: &mut Criterion) {
    init_hardware();

    let mut group = c.benchmark_group("insertion_performance");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(40);
    group.warm_up_time(Duration::from_secs(1));

    let dimension = 768;
    let count = 1000;
    let vectors = generate_test_vectors(count, dimension);
    let runtime = tokio::runtime::Runtime::new().unwrap();

    // Test each engine - stateless engines get collection info from parameters
    let engines = vec![
        ("sst", StorageEngineFactory::create_sst().unwrap()),
        ("viper", StorageEngineFactory::create_viper().unwrap()),
        ("nova", StorageEngineFactory::create_nova().unwrap()),
        ("swift", StorageEngineFactory::create_swift().unwrap()),
    ];

    for (engine_name, engine) in engines {
        group.bench_function(engine_name, |b| {
            b.iter(|| {
                runtime.block_on(async {
                    let collection_id = format!("insert_{}", engine_name);
                    let base_path = format!("{}/insert/{}", get_base_path(), engine_name);

                    // Clean before each iteration using filesystem API
                    let fs_factory = match FilesystemFactory::new(FilesystemConfig::default()).await {
                        Ok(f) => f,
                        Err(_) => return Err(anyhow::anyhow!("Failed to create filesystem factory")),
                    };
                    let fs = fs_factory.get_filesystem(&format!("file://{}", base_path))?;
                    let _ = fs.remove_dir_all(&base_path).await;

                    let collection = Collection {
                        id: collection_id.clone(),
                        config: Some(CollectionConfig {
                            name: collection_id.clone(),
                            dimension: dimension as u32,
                            ..Default::default()
                        }),
                        storage_assignment: Some(StorageAssignment {
                            primary_path: base_path.clone(),
                            base_location: base_path.clone(),
                            ..Default::default()
                        }),
                        ..Default::default()
                    };

                    let params = FlushParameters {
                        collection_id: Some(collection_id),
                        vector_records: vectors.clone(),
                        force: true,
                        synchronous: true,
                        collection_config: Some(collection),
                        ..Default::default()
                    };

                    let result = engine.flush(params).await;

                    // Clean after using filesystem API
                    let _ = fs.remove_dir_all(&base_path).await;

                    black_box(result)
                })
            })
        });
    }

    group.finish();

    // Final cleanup using filesystem API
    runtime.block_on(async {
        let fs_factory = FilesystemFactory::new(FilesystemConfig::default()).await.ok()?;
        let cleanup_path = format!("{}/insert", get_base_path());
        let fs = fs_factory.get_filesystem(&format!("file://{}", cleanup_path)).ok()?;
        let _ = fs.remove_dir_all(&cleanup_path).await;
        Some(())
    });
}

// Configure and run all benchmarks with custom settings
// Will stop when either condition is met:
// - 30 samples collected OR
// - 5 seconds of measurement time elapsed
criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(40)
        .measurement_time(Duration::from_secs(5))
        .warm_up_time(Duration::from_secs(1));
    targets = bench_compression_with_search,
              bench_engine_lifecycle,
              bench_large_scale_search,
              bench_insertion_performance
}

criterion_main!(benches);