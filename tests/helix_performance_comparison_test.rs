//! Performance comparison tests between HELIX and other storage engines
//!
//! This test suite compares HELIX against SST, VIPER, and RAPTOR engines
//! across various workloads and metrics.

#[cfg(test)]
mod performance_comparison_tests {
    use proximadb::compute::distance_computation::DistanceMetric;
    use proximadb::proto::proximadb_v1::VectorRecord;
    
    use proximadb::storage::engines::impls::helix::{HelixConfig, HelixEngine};
    use proximadb::storage::engines::impls::sst::SstEngine;
    use proximadb::storage::engines::impls::viper::engine::ViperEngine;
    use proximadb::storage::traits::StorageQueryMetadata;
    use proximadb::storage::traits::{
        CompactionParameters, FlushParameters, StorageQueryContext, UnifiedStorageEngine,
    };
    use rand::{Rng, SeedableRng};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    /// Test configuration
    const VECTOR_DIMS: usize = 384;
    const NUM_VECTORS: usize = 10000;
    const NUM_QUERIES: usize = 100;
    const K_NEIGHBORS: usize = 10;

    /// Helper to create test vectors with different distributions
    fn create_test_vectors(
        count: usize,
        dims: usize,
        distribution: &str,
        seed: u64,
    ) -> Vec<VectorRecord> {
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);

        match distribution {
            "uniform" => {
                // Uniformly distributed vectors
                (0..count)
                    .map(|i| VectorRecord {
                        id: format!("vec_{}", i),
                        vector: (0..dims).map(|_| rng.gen_range(-1.0..1.0)).collect(),
                        metadata: {
                            let mut metadata = std::collections::HashMap::new();
                            metadata.insert("distribution".to_string(), proximadb::proto::proximadb_v1::SqlValue {
                                value: Some(proximadb::proto::proximadb_v1::sql_value::Value::StringValue("uniform".to_string()))
                            });
                            metadata
                        },
                        timestamp: Some(i as i64),
                        updated_at: None,
                        expires_at: None,
                        version: None,
                        source: None,
                    })
                    .collect()
            }
            "clustered" => {
                // Create 10 clusters
                let mut vectors = Vec::new();
                let num_clusters = 10;
                let vectors_per_cluster = count / num_clusters;

                for cluster_id in 0..num_clusters {
                    // Generate cluster center
                    let center: Vec<f32> = (0..dims).map(|_| rng.gen_range(-10.0..10.0)).collect();

                    // Generate vectors around center
                    for i in 0..vectors_per_cluster {
                        let mut vector = center.clone();
                        for v in &mut vector {
                            *v += rng.gen_range(-0.5..0.5);
                        }

                        vectors.push(VectorRecord {
                            id: format!("cluster_{}_vec_{}", cluster_id, i),
                            vector,
                            metadata: {
                                let mut metadata = std::collections::HashMap::new();
                                metadata.insert("distribution".to_string(), proximadb::proto::proximadb_v1::SqlValue {
                                    value: Some(proximadb::proto::proximadb_v1::sql_value::Value::StringValue("clustered".to_string()))
                                });
                                metadata.insert("cluster_id".to_string(), proximadb::proto::proximadb_v1::SqlValue {
                                    value: Some(proximadb::proto::proximadb_v1::sql_value::Value::StringValue(cluster_id.to_string()))
                                });
                                metadata
                            },
                            timestamp: Some((cluster_id * vectors_per_cluster + i) as i64),
                            updated_at: None,
                            expires_at: None,
                            version: None,
                            source: None,
                        });
                    }
                }
                vectors
            }
            "skewed" => {
                // Skewed distribution with hot spots
                (0..count)
                    .map(|i| {
                        let skew = if i % 10 == 0 { 10.0 } else { 1.0 };
                        VectorRecord {
                            id: format!("vec_{}", i),
                            vector: (0..dims).map(|_| rng.gen_range(-1.0..1.0) * skew).collect(),
                            metadata: {
                                let mut metadata = std::collections::HashMap::new();
                                metadata.insert("distribution".to_string(), proximadb::proto::proximadb_v1::SqlValue {
                                    value: Some(proximadb::proto::proximadb_v1::sql_value::Value::StringValue("skewed".to_string()))
                                });
                                metadata
                            },
                            timestamp: Some(i as i64),
                            updated_at: None,
                            expires_at: None,
                            version: None,
                            source: None,
                        }
                    })
                    .collect()
            }
            _ => panic!("Unknown distribution: {}", distribution),
        }
    }

    /// Performance metrics for a single engine
    #[derive(Debug, Clone)]
    struct PerformanceMetrics {
        engine_name: String,
        flush_time: Duration,
        flush_throughput: f64,
        compaction_time: Option<Duration>,
        compaction_throughput: Option<f64>,
        query_times: Vec<Duration>,
        avg_query_time: Duration,
        p50_query_time: Duration,
        p95_query_time: Duration,
        p99_query_time: Duration,
        memory_usage_mb: f64,
        disk_usage_mb: f64,
    }

    impl PerformanceMetrics {
        fn calculate_percentiles(times: &mut Vec<Duration>) -> (Duration, Duration, Duration) {
            times.sort();
            let p50 = times[times.len() / 2];
            let p95 = times[times.len() * 95 / 100];
            let p99 = times[times.len() * 99 / 100];
            (p50, p95, p99)
        }

        fn print_summary(&self) {
            println!("\n=== {} Performance ===", self.engine_name);
            println!(
                "Flush: {:.2}ms ({:.0} vectors/sec)",
                self.flush_time.as_millis(),
                self.flush_throughput
            );

            if let Some(compact_time) = self.compaction_time {
                println!(
                    "Compaction: {:.2}ms ({:.2} MB/s)",
                    compact_time.as_millis(),
                    self.compaction_throughput.unwrap_or(0.0)
                );
            }

            println!("Query Latency:");
            println!(
                "  Avg: {:.2}ms",
                self.avg_query_time.as_micros() as f64 / 1000.0
            );
            println!(
                "  P50: {:.2}ms",
                self.p50_query_time.as_micros() as f64 / 1000.0
            );
            println!(
                "  P95: {:.2}ms",
                self.p95_query_time.as_micros() as f64 / 1000.0
            );
            println!(
                "  P99: {:.2}ms",
                self.p99_query_time.as_micros() as f64 / 1000.0
            );
            println!("Memory: {:.2} MB", self.memory_usage_mb);
            println!("Disk: {:.2} MB", self.disk_usage_mb);
        }
    }

    /// Benchmark a storage engine
    async fn benchmark_engine(
        engine: Arc<dyn UnifiedStorageEngine>,
        vectors: &[VectorRecord],
        query_vectors: &[Vec<f32>],
        collection_id: &str,
    ) -> PerformanceMetrics {
        let engine_name = engine.engine_name().to_string();

        // Measure flush performance
        let flush_start = Instant::now();
        let flush_params = FlushParameters {
            collection_id: Some(collection_id.to_string()),
            vector_records: vectors.iter().map(|v| v.clone().into()).collect(),
            force: true,
            synchronous: true,
            hints: HashMap::new(),
            timeout_ms: None,
            trigger_compaction: false,
            batch_ids: vec![],
            collection_config: None,
            estimated_size: 0,
        };
        let flush_result = engine.do_flush(&flush_params).await.unwrap();
        let flush_time = flush_start.elapsed();
        let flush_throughput = vectors.len() as f64 / flush_time.as_secs_f64();

        // Measure compaction performance (if applicable)
        let (compaction_time, compaction_throughput) = if engine_name != "viper" {
            let compact_start = Instant::now();
            let compact_params = CompactionParameters {
                collection_id: Some(collection_id.to_string()),
                force: true,
                synchronous: true,
                hints: HashMap::new(),
                timeout_ms: None,
                priority: proximadb::storage::traits::OperationPriority::Medium,
                collection_config: None,
                estimated_input_size: 0,
            };
            let compact_result = engine.do_compact(&compact_params).await.unwrap();
            let compact_time = compact_start.elapsed();
            let throughput = if compact_result.bytes_written.unwrap_or(0) > 0 {
                (compact_result.bytes_written.unwrap_or(0) as f64 / 1_048_576.0)
                    / compact_time.as_secs_f64()
            } else {
                0.0
            };
            (Some(compact_time), Some(throughput))
        } else {
            (None, None)
        };

        // Measure query performance
        let mut query_times = Vec::new();
        for query in query_vectors {
            let query_start = Instant::now();
            let search_params = Arc::new(proximadb::core::search::SearchParams {
                query_vectors: Some(vec![query.clone()]),
                top_k: Some(K_NEIGHBORS),
                distance_metric: Some(DistanceMetric::Euclidean),
                filter_expression: None,
                ..Default::default()
            });

            let collection = Arc::new(proximadb::proto::proximadb_v1::Collection {
                id: collection_id.to_string(),
                config: Some(proximadb::proto::proximadb_v1::CollectionConfig {
                    name: collection_id.to_string(),
                    dimension: VECTOR_DIMS as u32,
                    distance_metric: Some(DistanceMetric::Euclidean as i32),
                    storage_engine: Some(proximadb::proto::proximadb_v1::StorageEngine::Sst as i32),
                    tags: vec![],
                    description: None,
                    filterable_columns: vec![],
                    index_configs: vec![],
                    quantization: None,
                    primary_index: Some("HNSW".to_string()),
                    auto_index_selection: Some(false),
                    owner: None,
                    embedding_models: vec![],
                    storage_config: None,
                }),
                stats: None,
                created_at: 0,
                updated_at: 0,
                storage_assignment: None,
            });

            let ctx = StorageQueryContext {
                search_params,
                collection,
                metadata: StorageQueryMetadata::default(),
            };

            let _ = engine.search_vectors_unified(&ctx).await.unwrap();
            query_times.push(query_start.elapsed());
        }

        // Calculate percentiles
        let avg_query_time = query_times.iter().sum::<Duration>() / query_times.len() as u32;
        let (p50, p95, p99) = PerformanceMetrics::calculate_percentiles(&mut query_times.clone());

        // Get metrics
        let metrics = engine.collect_engine_metrics().await.unwrap();
        let memory_usage_mb = metrics
            .get("memory_usage_mb")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let disk_usage_mb = flush_result.bytes_written.unwrap_or(0) as f64 / 1_048_576.0;

        PerformanceMetrics {
            engine_name,
            flush_time,
            flush_throughput,
            compaction_time,
            compaction_throughput,
            query_times,
            avg_query_time,
            p50_query_time: p50,
            p95_query_time: p95,
            p99_query_time: p99,
            memory_usage_mb,
            disk_usage_mb,
        }
    }

    #[tokio::test]
    async fn test_performance_comparison_uniform_distribution() {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

        println!("\n===== UNIFORM DISTRIBUTION COMPARISON =====");

        // Create test data
        let vectors = create_test_vectors(NUM_VECTORS, VECTOR_DIMS, "uniform", 42);
        let query_vectors: Vec<Vec<f32>> = (0..NUM_QUERIES)
            .map(|i| vectors[i * 100].vector.clone())
            .collect();

        // Create engines
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        let helix_engine = {
            let config = HelixConfig::default();
            Arc::new(
                HelixEngine::new()
                .await
                .unwrap(),
            ) as Arc<dyn UnifiedStorageEngine>
        };

        use proximadb::compute::distance_computation::engine::UnifiedDistanceCompute;
        use proximadb::core::config::SstConfig;
        use proximadb::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};

        let sst_config = SstConfig::default();
        let fs_config = FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::create(fs_config).await.unwrap());
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Euclidean));

        let sst_engine = Arc::new(
            SstEngine::new()
                .await
                .unwrap(),
        ) as Arc<dyn UnifiedStorageEngine>;

        use proximadb::core::config::ViperConfig;
        let viper_config = ViperConfig::default();

        let viper_engine = Arc::new(
            ViperEngine::new()
            .await
            .unwrap(),
        ) as Arc<dyn UnifiedStorageEngine>;

        // Benchmark each engine
        let helix_metrics =
            benchmark_engine(helix_engine, &vectors, &query_vectors, "test_collection").await;

        let sst_metrics =
            benchmark_engine(sst_engine, &vectors, &query_vectors, "test_collection").await;

        let viper_metrics =
            benchmark_engine(viper_engine, &vectors, &query_vectors, "test_collection").await;

        // Print results
        helix_metrics.print_summary();
        sst_metrics.print_summary();
        viper_metrics.print_summary();

        // Compare HELIX performance
        println!("\n=== Performance Comparison ===");
        println!(
            "HELIX vs SST Query Speedup: {:.2}x",
            sst_metrics.avg_query_time.as_micros() as f64
                / helix_metrics.avg_query_time.as_micros() as f64
        );
        println!(
            "HELIX vs VIPER Query Speedup: {:.2}x",
            viper_metrics.avg_query_time.as_micros() as f64
                / helix_metrics.avg_query_time.as_micros() as f64
        );
    }

    #[tokio::test]
    async fn test_performance_comparison_clustered_distribution() {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

        println!("\n===== CLUSTERED DISTRIBUTION COMPARISON =====");

        // Create clustered test data
        let vectors = create_test_vectors(NUM_VECTORS, VECTOR_DIMS, "clustered", 42);
        let query_vectors: Vec<Vec<f32>> = (0..NUM_QUERIES)
            .map(|i| vectors[i * 100].vector.clone())
            .collect();

        // Create engines
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        let helix_engine = {
            let config = HelixConfig::default();
            Arc::new(
                HelixEngine::new()
                .await
                .unwrap(),
            ) as Arc<dyn UnifiedStorageEngine>
        };

        use proximadb::compute::distance_computation::engine::UnifiedDistanceCompute;
        use proximadb::core::config::SstConfig;
        use proximadb::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};

        let sst_config = SstConfig::default();
        let fs_config = FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::create(fs_config).await.unwrap());
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Euclidean));

        let sst_engine = Arc::new(
            SstEngine::new()
                .await
                .unwrap(),
        ) as Arc<dyn UnifiedStorageEngine>;

        // Benchmark each engine
        let helix_metrics =
            benchmark_engine(helix_engine, &vectors, &query_vectors, "test_collection").await;

        let sst_metrics =
            benchmark_engine(sst_engine, &vectors, &query_vectors, "test_collection").await;

        // Print results
        helix_metrics.print_summary();
        sst_metrics.print_summary();

        // HELIX should perform much better on clustered data
        println!("\n=== Clustered Data Performance ===");
        println!(
            "HELIX vs SST Query Speedup: {:.2}x",
            sst_metrics.avg_query_time.as_micros() as f64
                / helix_metrics.avg_query_time.as_micros() as f64
        );

        // Verify HELIX performs better on clustered data
        assert!(
            helix_metrics.avg_query_time < sst_metrics.avg_query_time,
            "HELIX should outperform SST on clustered data"
        );
    }

    #[tokio::test]
    async fn test_scalability_comparison() {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

        println!("\n===== SCALABILITY COMPARISON =====");

        let sizes = vec![1000, 5000, 10000, 20000];
        let mut helix_times = Vec::new();
        let mut sst_times = Vec::new();

        for &size in &sizes {
            let vectors = create_test_vectors(size, VECTOR_DIMS, "uniform", 42);
            let query_vectors: Vec<Vec<f32>> = (0..10).map(|i| vectors[i].vector.clone()).collect();

            // Test HELIX
            {
                let temp_dir = TempDir::new().unwrap();
                let config = HelixConfig::default();
                let engine = Arc::new(
                    HelixEngine::new()
                    .await
                    .unwrap(),
                ) as Arc<dyn UnifiedStorageEngine>;

                let metrics =
                    benchmark_engine(engine, &vectors, &query_vectors, "test_collection").await;

                helix_times.push((size, metrics.avg_query_time));
            }

            // Test SST
            {
                use proximadb::compute::distance_computation::engine::UnifiedDistanceCompute;
                use proximadb::core::config::SstConfig;
                use proximadb::storage::persistence::filesystem::{
                    FilesystemConfig, FilesystemFactory,
                };

                let temp_dir = TempDir::new().unwrap();
                let sst_config = SstConfig::default();
                let fs_config = FilesystemConfig::default();
                let filesystem = Arc::new(FilesystemFactory::create(fs_config).await.unwrap());
                let distance_compute =
                    Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Euclidean));

                let engine = Arc::new(
                    SstEngine::new()
                        .await
                        .unwrap(),
                ) as Arc<dyn UnifiedStorageEngine>;

                let metrics =
                    benchmark_engine(engine, &vectors, &query_vectors, "test_collection").await;

                sst_times.push((size, metrics.avg_query_time));
            }
        }

        // Print scalability results
        println!("\nDataset Size | HELIX Query Time | SST Query Time | Speedup");
        println!("-------------|------------------|----------------|--------");
        for i in 0..sizes.len() {
            let (size, helix_time) = helix_times[i];
            let (_, sst_time) = sst_times[i];
            let speedup = sst_time.as_micros() as f64 / helix_time.as_micros() as f64;

            println!(
                "{:12} | {:14.2}ms | {:12.2}ms | {:.2}x",
                size,
                helix_time.as_micros() as f64 / 1000.0,
                sst_time.as_micros() as f64 / 1000.0,
                speedup
            );
        }

        // HELIX should scale better than SST
        let helix_scaling = helix_times.last().unwrap().1.as_micros() as f64
            / helix_times.first().unwrap().1.as_micros() as f64;
        let sst_scaling = sst_times.last().unwrap().1.as_micros() as f64
            / sst_times.first().unwrap().1.as_micros() as f64;

        println!("\nScaling Factor (20K vs 1K):");
        println!("HELIX: {:.2}x", helix_scaling);
        println!("SST: {:.2}x", sst_scaling);

        assert!(
            helix_scaling < sst_scaling,
            "HELIX should scale better than SST"
        );
    }

    #[tokio::test]
    async fn test_pruning_effectiveness() {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

        println!("\n===== PRUNING EFFECTIVENESS TEST =====");

        // Create highly clustered data to test pruning
        let vectors = create_test_vectors(10000, VECTOR_DIMS, "clustered", 42);

        let temp_dir = TempDir::new().unwrap();
        let config = HelixConfig {
            level0_file_num_compaction_trigger: 2,
            ..Default::default()
        };

        let engine = Arc::new(
            HelixEngine::new()
            .await
            .unwrap(),
        );

        // Flush data in multiple batches to create multiple files
        for chunk in vectors.chunks(1000) {
            let flush_params = FlushParameters {
                collection_id: Some("test_collection".to_string()),
                vector_records: chunk.iter().map(|v| v.clone().into()).collect(),
                force: true,
                synchronous: true,
                hints: HashMap::new(),
                timeout_ms: None,
                trigger_compaction: false,
                batch_ids: vec![],
                collection_config: None,
                estimated_size: 0,
            };
            engine.do_flush(&flush_params).await.unwrap();
        }

        // Force compaction to organize data
        let compact_params = CompactionParameters {
            collection_id: Some("test_collection".to_string()),
            force: true,
            synchronous: true,
            hints: HashMap::new(),
            timeout_ms: None,
            priority: proximadb::storage::traits::OperationPriority::Medium,
            collection_config: None,
            estimated_input_size: 0,
        };
        engine.do_compact(&compact_params).await.unwrap();

        // Query from different clusters and measure performance
        let cluster_queries = vec![
            vectors[50].vector.clone(),   // Cluster 0
            vectors[1050].vector.clone(), // Cluster 1
            vectors[2050].vector.clone(), // Cluster 2
        ];

        for (i, query) in cluster_queries.iter().enumerate() {
            let start = Instant::now();
            let search_params = Arc::new(proximadb::core::search::SearchParams {
                query_vectors: Some(vec![query.clone()]),
                top_k: Some(10),
                distance_metric: Some(DistanceMetric::Euclidean),
                filter_expression: None,
                ..Default::default()
            });

            let collection = Arc::new(proximadb::proto::proximadb_v1::Collection {
                id: "test_collection".to_string(),
                config: Some(proximadb::proto::proximadb_v1::CollectionConfig {
                    name: "test_collection".to_string(),
                    dimension: VECTOR_DIMS as u32,
                    distance_metric: Some(DistanceMetric::Euclidean as i32),
                    storage_engine: Some(proximadb::proto::proximadb_v1::StorageEngine::Sst as i32),
                    tags: vec![],
                    description: None,
                    filterable_columns: vec![],
                    index_configs: vec![],
                    quantization: None,
                    primary_index: Some("HNSW".to_string()),
                    auto_index_selection: Some(false),
                    owner: None,
                    embedding_models: vec![],
                    storage_config: None,
                }),
                stats: None,
                created_at: 0,
                updated_at: 0,
                storage_assignment: None,
            });

            let ctx = StorageQueryContext {
                search_params,
                collection,
                metadata: StorageQueryMetadata::default(),
            };

            let results = engine.search_vectors_unified(&ctx).await.unwrap();
            let elapsed = start.elapsed();

            // Check that results are from the expected cluster
            let expected_cluster = i.to_string();
            let correct_cluster = results
                .iter()
                .filter(|r| {
                    // Convert to tuple access for HashMap-style metadata
                    r.metadata.iter().any(|(key, value)| {
                        key == "cluster_id"
                            && match &value.value {
                                Some(proximadb::proto::proximadb_v1::sql_value::Value::StringValue(s)) => s == &expected_cluster,
                                _ => false,
                            }
                    })
                })
                .count();

            println!(
                "Cluster {} query: {:.2}ms, {}/{} from correct cluster",
                i,
                elapsed.as_micros() as f64 / 1000.0,
                correct_cluster,
                results.len()
            );

            // Most results should be from the correct cluster
            assert!(
                correct_cluster >= results.len() * 7 / 10,
                "Pruning should effectively locate clustered data"
            );
        }
    }

    #[tokio::test]
    async fn test_memory_efficiency() {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

        println!("\n===== MEMORY EFFICIENCY COMPARISON =====");

        let vectors = create_test_vectors(5000, VECTOR_DIMS, "uniform", 42);

        // Measure memory usage for each engine
        let engines = vec![
            ("HELIX", {
                let temp_dir = TempDir::new().unwrap();
                let config = HelixConfig::default();
                Arc::new(
                    HelixEngine::new()
                    .await
                    .unwrap(),
                ) as Arc<dyn UnifiedStorageEngine>
            }),
            ("SST", {
                use proximadb::compute::distance_computation::engine::UnifiedDistanceCompute;
                use proximadb::core::config::SstConfig;
                use proximadb::storage::persistence::filesystem::{
                    FilesystemConfig, FilesystemFactory,
                };

                let temp_dir = TempDir::new().unwrap();
                let sst_config = SstConfig::default();
                let fs_config = FilesystemConfig::default();
                let filesystem = Arc::new(FilesystemFactory::create(fs_config).await.unwrap());
                let distance_compute =
                    Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Euclidean));

                Arc::new(
                    SstEngine::new()
                        .await
                        .unwrap(),
                ) as Arc<dyn UnifiedStorageEngine>
            }),
            ("VIPER", {
                use proximadb::compute::distance_computation::engine::UnifiedDistanceCompute;
                use proximadb::core::config::ViperConfig;
                use proximadb::storage::persistence::filesystem::{
                    FilesystemConfig, FilesystemFactory,
                };

                let temp_dir = TempDir::new().unwrap();
                let viper_config = ViperConfig::default();
                let fs_config = FilesystemConfig::default();
                let filesystem = Arc::new(FilesystemFactory::create(fs_config).await.unwrap());
                let distance_compute =
                    Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Euclidean));

                Arc::new(
                    ViperEngine::new()
                    .await
                    .unwrap(),
                ) as Arc<dyn UnifiedStorageEngine>
            }),
        ];

        for (name, engine) in engines {
            // Flush data
            let flush_params = FlushParameters {
                collection_id: Some("test_collection".to_string()),
                vector_records: vectors.iter().map(|v| v.clone().into()).collect(),
                force: true,
                synchronous: true,
                hints: HashMap::new(),
                timeout_ms: None,
                trigger_compaction: false,
                batch_ids: vec![],
                collection_config: None,
                estimated_size: 0,
            };
            let flush_result = engine.do_flush(&flush_params).await.unwrap();

            // Get metrics
            let metrics = engine.collect_engine_metrics().await.unwrap();

            let memory_mb = metrics
                .get("memory_usage_mb")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let disk_mb = flush_result.bytes_written.unwrap_or(0) as f64 / 1_048_576.0;
            let vectors_per_mb = vectors.len() as f64 / memory_mb;

            println!("{} Engine:", name);
            println!("  Memory: {:.2} MB", memory_mb);
            println!("  Disk: {:.2} MB", disk_mb);
            println!("  Vectors per MB RAM: {:.0}", vectors_per_mb);
            println!(
                "  Compression ratio: {:.2}x",
                (vectors.len() * VECTOR_DIMS * 4) as f64
                    / flush_result.bytes_written.unwrap_or(0) as f64
            );
        }
    }

    #[tokio::test]
    async fn test_concurrent_query_performance() {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

        println!("\n===== CONCURRENT QUERY PERFORMANCE =====");

        let vectors = create_test_vectors(10000, VECTOR_DIMS, "uniform", 42);
        let query_vectors: Vec<Vec<f32>> =
            (0..50).map(|i| vectors[i * 200].vector.clone()).collect();

        // Create HELIX engine
        let temp_dir = TempDir::new().unwrap();
        let config = HelixConfig::default();
        let engine = Arc::new(
            HelixEngine::new()
            .await
            .unwrap(),
        );

        // Load data
        let flush_params = FlushParameters {
            collection_id: Some("test_collection".to_string()),
            vector_records: vectors.into_iter().map(|v| v.into()).collect(),
            force: true,
            synchronous: true,
            hints: HashMap::new(),
            timeout_ms: None,
            trigger_compaction: false,
            batch_ids: vec![],
            collection_config: None,
            estimated_size: 0,
        };
        engine.do_flush(&flush_params).await.unwrap();

        // Test with different concurrency levels
        for concurrency in [1, 5, 10, 20] {
            let start = Instant::now();
            let mut handles = Vec::new();

            for batch in query_vectors.chunks(concurrency) {
                for query in batch {
                    let engine_clone = engine.clone();
                    let query_clone = query.clone();
                    let handle = tokio::spawn(async move {
                        let search_params = Arc::new(proximadb::core::search::SearchParams {
                            query_vectors: Some(vec![query_clone]),
                            top_k: Some(10),
                            distance_metric: Some(DistanceMetric::Euclidean),
                            filter_expression: None,
                            ..Default::default()
                        });

                        let collection = Arc::new(proximadb::proto::proximadb_v1::Collection {
                            id: "test_collection".to_string(),
                            config: Some(proximadb::proto::proximadb_v1::CollectionConfig {
                                name: "test_collection".to_string(),
                                dimension: VECTOR_DIMS as u32,
                                distance_metric: Some(DistanceMetric::Euclidean as i32),
                                storage_engine: Some(proximadb::proto::proximadb_v1::StorageEngine::Sst as i32),
                                tags: vec![],
                                description: None,
                                filterable_columns: vec![],
                                index_configs: vec![],
                                quantization: None,
                                storage_config: None,
                                primary_index: Some("default".to_string()),
                                auto_index_selection: Some(true),
                                owner: Some("test".to_string()),
                                embedding_models: vec![],
            //                    auto_create_shards: None, // Field not in proto
            //                    auto_balance: None, // Field not in proto
            //                    replication_factor: None, // Field not in proto
                                        }),
                            stats: None,
                            created_at: 0,
                            updated_at: 0,
                            storage_assignment: None,
                        });

                        let ctx = StorageQueryContext {
                            search_params,
                            collection,
                            metadata: StorageQueryMetadata::default(),
                        };
                        engine_clone.search_vectors_unified(&ctx).await
                    });
                    handles.push(handle);
                }

                // Wait for batch to complete
                for handle in handles.drain(..) {
                    handle.await.unwrap().unwrap();
                }
            }

            let elapsed = start.elapsed();
            let qps = query_vectors.len() as f64 / elapsed.as_secs_f64();

            println!(
                "Concurrency {}: {:.2} QPS ({:.2}ms avg latency)",
                concurrency,
                qps,
                elapsed.as_millis() as f64 / query_vectors.len() as f64
            );
        }
    }
}
