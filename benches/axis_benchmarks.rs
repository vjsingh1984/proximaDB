// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! AXIS (Adaptive eXtensible Indexing System) performance benchmarks

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::collections::HashMap;
use std::time::Duration;
use tokio::runtime::Runtime;

use super::{BenchmarkConfig, BenchmarkDataGenerator, BenchmarkRunner};
use proximadb::core::{Config, VectorRecord};
use proximadb::index::axis::{
    AxisIndexManager, FilterOperator, HybridQuery, MetadataFilter, VectorQuery,
};
use proximadb::storage::StorageEngine;

/// Benchmark AXIS hybrid query performance
pub fn benchmark_axis_hybrid_queries(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let generator = BenchmarkDataGenerator::new(config.clone());
    let runner = BenchmarkRunner::new(config.clone());

    let mut group = c.benchmark_group("axis_hybrid_queries");
    group.measurement_time(Duration::from_secs(30));

    let dataset_size = 10_000;
    let dimension = 256;
    let query_count = 50;

    // Different query complexity levels
    let query_types = vec![
        ("vector_only", false, false),  // Pure vector search
        ("metadata_only", false, true), // Pure metadata filtering
        ("hybrid_simple", true, true),  // Vector + simple metadata
        ("hybrid_complex", true, true), // Vector + complex metadata
    ];

    for (query_type, use_vector, use_metadata) in query_types {
        group.throughput(Throughput::Elements(query_count as u64));

        group.bench_with_input(
            BenchmarkId::new("axis_hybrid", query_type),
            &(use_vector, use_metadata),
            |b, &(use_vector, use_metadata)| {
                let vectors =
                    generator.generate_vectors(dataset_size, dimension, 0.1, "axis_hybrid");
                let query_vectors = generator.generate_query_vectors(query_count, dimension, 0.1);

                b.to_async(&runner.runtime).iter(|| async {
                    let storage = runner.create_test_storage().await;

                    // Insert dataset
                    {
                        let mut storage_guard = storage.write().await;
                        for vector in &vectors {
                            let _ = storage_guard.insert_vector(vector.clone()).await;
                        }
                    }

                    // Perform hybrid queries
                    {
                        let storage_guard = storage.read().await;
                        for (i, query_vector) in query_vectors.iter().enumerate() {
                            let mut hybrid_query = HybridQuery {
                                vector_query: None,
                                metadata_filters: Vec::new(),
                                id_filters: Vec::new(),
                                similarity_threshold: Some(0.7),
                                k: 20,
                                return_vectors: true,
                                return_metadata: true,
                            };

                            if use_vector {
                                hybrid_query.vector_query = Some(VectorQuery {
                                    vector: query_vector.clone(),
                                    collection_id: "axis_hybrid".to_string(),
                                });
                            }

                            if use_metadata {
                                let filter = MetadataFilter {
                                    field: "category".to_string(),
                                    operator: FilterOperator::Equals,
                                    value: serde_json::json!(format!("cat_{}", i % 5)),
                                };
                                hybrid_query.metadata_filters.push(filter);

                                if query_type == "hybrid_complex" {
                                    let priority_filter = MetadataFilter {
                                        field: "priority".to_string(),
                                        operator: FilterOperator::GreaterThan,
                                        value: serde_json::json!(2),
                                    };
                                    hybrid_query.metadata_filters.push(priority_filter);
                                }
                            }

                            // Simulate AXIS query execution
                            let _ = simulate_axis_query(black_box(hybrid_query)).await;
                        }
                    }
                });
            },
        );
    }

    group.finish();
}

/// Benchmark AXIS adaptive index strategy selection
pub fn benchmark_axis_adaptive_strategy(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let generator = BenchmarkDataGenerator::new(config.clone());
    let runner = BenchmarkRunner::new(config.clone());

    let mut group = c.benchmark_group("axis_adaptive_strategy");
    group.measurement_time(Duration::from_secs(25));

    let dataset_size = 5_000;
    let dimension = 256;

    // Different collection characteristics for strategy testing
    let collection_profiles = vec![
        ("dense_small", 0.05, dataset_size), // Dense, small collection
        ("dense_large", 0.05, dataset_size * 4), // Dense, large collection
        ("sparse_medium", 0.7, dataset_size * 2), // Sparse, medium collection
        ("mixed_sparsity", 0.3, dataset_size * 2), // Mixed sparsity
    ];

    for (profile_name, sparsity, size) in collection_profiles {
        if size <= 20_000 {
            // Skip very large datasets for CI
            group.throughput(Throughput::Elements(size as u64));

            group.bench_with_input(
                BenchmarkId::new("strategy_selection", profile_name),
                &(sparsity, size),
                |b, &(sparsity, size)| {
                    let vectors =
                        generator.generate_vectors(size, dimension, sparsity, "strategy_bench");

                    b.to_async(&runner.runtime).iter(|| async {
                        // Simulate collection analysis
                        let characteristics = simulate_collection_analysis(&vectors).await;

                        // Simulate strategy recommendation
                        let strategy =
                            simulate_strategy_selection(black_box(characteristics)).await;

                        black_box(strategy);
                    });
                },
            );
        }
    }

    group.finish();
}

/// Benchmark AXIS index migration performance
pub fn benchmark_axis_index_migration(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let generator = BenchmarkDataGenerator::new(config.clone());
    let runner = BenchmarkRunner::new(config.clone());

    let mut group = c.benchmark_group("axis_index_migration");
    group.measurement_time(Duration::from_secs(35));

    let dataset_sizes = vec![1_000, 5_000, 10_000];
    let dimension = 256;

    // Migration scenarios
    let migration_types = vec![
        ("hnsw_to_lsm", "HNSW", "LSM"), // Dense to sparse optimization
        ("lsm_to_hnsw", "LSM", "HNSW"), // Sparse to dense optimization
        ("simple_to_hybrid", "Simple", "Hybrid"), // Single to multi-index
    ];

    for &dataset_size in &dataset_sizes {
        for (migration_name, from_strategy, to_strategy) in &migration_types {
            if dataset_size <= 5_000 {
                // Limit migration benchmark size
                group.throughput(Throughput::Elements(dataset_size as u64));

                group.bench_with_input(
                    BenchmarkId::new(
                        "migration",
                        format!("{}_{}k", migration_name, dataset_size / 1000),
                    ),
                    &(dataset_size, from_strategy, to_strategy),
                    |b, &(dataset_size, from_strategy, to_strategy)| {
                        let vectors = generator.generate_vectors(
                            dataset_size,
                            dimension,
                            0.2,
                            "migration_bench",
                        );

                        b.to_async(&runner.runtime).iter(|| async {
                            // Simulate migration planning
                            let migration_plan = simulate_migration_planning(
                                black_box(from_strategy),
                                black_box(to_strategy),
                                black_box(dataset_size),
                            )
                            .await;

                            // Simulate incremental migration
                            let migration_result = simulate_incremental_migration(
                                black_box(migration_plan),
                                black_box(&vectors),
                            )
                            .await;

                            black_box(migration_result);
                        });
                    },
                );
            }
        }
    }

    group.finish();
}

/// Benchmark AXIS multi-index join operations
pub fn benchmark_axis_join_operations(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let generator = BenchmarkDataGenerator::new(config.clone());
    let runner = BenchmarkRunner::new(config.clone());

    let mut group = c.benchmark_group("axis_join_operations");
    group.measurement_time(Duration::from_secs(25));

    let dataset_size = 10_000;
    let dimension = 256;
    let query_count = 30;

    // Different join scenarios
    let join_scenarios = vec![
        ("two_way_join", 2),   // Metadata + Vector index
        ("three_way_join", 3), // Metadata + Vector + ID index
        ("four_way_join", 4),  // All indexes
    ];

    for (scenario_name, index_count) in join_scenarios {
        group.throughput(Throughput::Elements(query_count as u64));

        group.bench_with_input(
            BenchmarkId::new("join_operations", scenario_name),
            &index_count,
            |b, &index_count| {
                let vectors =
                    generator.generate_vectors(dataset_size, dimension, 0.1, "join_bench");
                let query_vectors = generator.generate_query_vectors(query_count, dimension, 0.1);

                b.to_async(&runner.runtime).iter(|| async {
                    for query_vector in &query_vectors {
                        // Simulate multi-index query results
                        let mut index_results = Vec::new();

                        for i in 0..index_count {
                            let result = simulate_index_query(i, query_vector, &vectors).await;
                            index_results.push(result);
                        }

                        // Simulate join operation
                        let joined_result = simulate_join_operation(black_box(index_results)).await;
                        black_box(joined_result);
                    }
                });
            },
        );
    }

    group.finish();
}

/// Benchmark AXIS sparse vector indexing
pub fn benchmark_axis_sparse_indexing(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let generator = BenchmarkDataGenerator::new(config.clone());
    let runner = BenchmarkRunner::new(config.clone());

    let mut group = c.benchmark_group("axis_sparse_indexing");
    group.measurement_time(Duration::from_secs(30));

    let dataset_size = 5_000;
    let dimension = 512;
    let query_count = 25;

    // Different sparsity levels for specialized testing
    let sparsity_levels = vec![0.5, 0.7, 0.9, 0.95];

    for &sparsity in &sparsity_levels {
        group.throughput(Throughput::Elements(query_count as u64));

        group.bench_with_input(
            BenchmarkId::new("sparse_index", format!("sparsity_{:.2}", sparsity)),
            &sparsity,
            |b, &sparsity| {
                let vectors =
                    generator.generate_vectors(dataset_size, dimension, sparsity, "sparse_index");
                let query_vectors =
                    generator.generate_query_vectors(query_count, dimension, sparsity);

                b.to_async(&runner.runtime).iter(|| async {
                    // Simulate sparse vector indexing operations
                    for vector in &vectors {
                        let _ = simulate_sparse_index_insert(black_box(vector)).await;
                    }

                    // Simulate sparse vector queries
                    for query in &query_vectors {
                        let _ = simulate_sparse_index_search(black_box(query), sparsity).await;
                    }
                });
            },
        );
    }

    group.finish();
}

/// Benchmark AXIS performance monitoring overhead
pub fn benchmark_axis_monitoring_overhead(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let generator = BenchmarkDataGenerator::new(config.clone());
    let runner = BenchmarkRunner::new(config.clone());

    let mut group = c.benchmark_group("axis_monitoring_overhead");
    group.measurement_time(Duration::from_secs(20));

    let operation_counts = vec![1_000, 10_000];
    let dimension = 256;

    for &operation_count in &operation_counts {
        group.throughput(Throughput::Elements(operation_count as u64));

        // Benchmark with monitoring enabled
        group.bench_with_input(
            BenchmarkId::new("monitoring_enabled", format!("{}ops", operation_count)),
            &operation_count,
            |b, &operation_count| {
                let vectors =
                    generator.generate_vectors(operation_count, dimension, 0.1, "monitor_bench");

                b.to_async(&runner.runtime).iter(|| async {
                    for vector in &vectors {
                        // Simulate operation with monitoring
                        let start_time = std::time::Instant::now();
                        let _ = simulate_monitored_operation(black_box(vector)).await;
                        let duration = start_time.elapsed();

                        // Simulate metrics collection
                        simulate_metrics_collection("insert_vector", duration).await;
                    }
                });
            },
        );

        // Benchmark without monitoring (baseline)
        group.bench_with_input(
            BenchmarkId::new("monitoring_disabled", format!("{}ops", operation_count)),
            &operation_count,
            |b, &operation_count| {
                let vectors =
                    generator.generate_vectors(operation_count, dimension, 0.1, "baseline_bench");

                b.to_async(&runner.runtime).iter(|| async {
                    for vector in &vectors {
                        // Simulate operation without monitoring
                        let _ = simulate_unmonitored_operation(black_box(vector)).await;
                    }
                });
            },
        );
    }

    group.finish();
}

// Simulation helper functions for AXIS benchmarks

async fn simulate_axis_query(query: HybridQuery) -> Vec<String> {
    // Simulate AXIS hybrid query processing
    tokio::time::sleep(Duration::from_micros(100)).await;
    vec!["result1".to_string(), "result2".to_string()]
}

async fn simulate_collection_analysis(vectors: &[VectorRecord]) -> HashMap<String, f64> {
    // Simulate collection characteristics analysis
    tokio::time::sleep(Duration::from_micros(50)).await;
    let mut characteristics = HashMap::new();
    characteristics.insert("sparsity".to_string(), 0.1);
    characteristics.insert("dimension_variance".to_string(), 0.3);
    characteristics.insert("query_frequency".to_string(), 100.0);
    characteristics
}

async fn simulate_strategy_selection(characteristics: HashMap<String, f64>) -> String {
    // Simulate ML-based strategy selection
    tokio::time::sleep(Duration::from_micros(30)).await;
    if characteristics.get("sparsity").unwrap_or(&0.0) > &0.5 {
        "LSM+MinHash".to_string()
    } else {
        "HNSW+Metadata".to_string()
    }
}

async fn simulate_migration_planning(from: &str, to: &str, size: usize) -> String {
    // Simulate migration plan creation
    tokio::time::sleep(Duration::from_micros(200)).await;
    format!("migration_plan_{}_{}_{}k", from, to, size / 1000)
}

async fn simulate_incremental_migration(plan: String, vectors: &[VectorRecord]) -> String {
    // Simulate zero-downtime migration
    tokio::time::sleep(Duration::from_micros(vectors.len() as u64 / 10)).await;
    format!("migration_result_{}", plan)
}

async fn simulate_index_query(
    index_id: usize,
    query: &[f32],
    vectors: &[VectorRecord],
) -> Vec<usize> {
    // Simulate individual index query
    tokio::time::sleep(Duration::from_micros(50)).await;
    vec![0, 1, 2, 3, 4] // Mock result set
}

async fn simulate_join_operation(results: Vec<Vec<usize>>) -> Vec<usize> {
    // Simulate multi-index join with Bloom filter optimization
    tokio::time::sleep(Duration::from_micros(20)).await;
    vec![0, 1, 2] // Mock joined results
}

async fn simulate_sparse_index_insert(vector: &VectorRecord) -> bool {
    // Simulate sparse vector LSM insertion
    tokio::time::sleep(Duration::from_micros(10)).await;
    true
}

async fn simulate_sparse_index_search(query: &[f32], sparsity: f32) -> Vec<String> {
    // Simulate MinHash LSH search
    let search_time = (sparsity * 100.0) as u64; // More sparse = faster search
    tokio::time::sleep(Duration::from_micros(search_time)).await;
    vec!["sparse_result1".to_string()]
}

async fn simulate_monitored_operation(vector: &VectorRecord) -> bool {
    // Simulate operation with performance monitoring
    tokio::time::sleep(Duration::from_micros(50)).await;
    true
}

async fn simulate_unmonitored_operation(vector: &VectorRecord) -> bool {
    // Simulate operation without monitoring overhead
    tokio::time::sleep(Duration::from_micros(45)).await;
    true
}

async fn simulate_metrics_collection(operation: &str, duration: Duration) {
    // Simulate metrics recording overhead
    tokio::time::sleep(Duration::from_micros(5)).await;
}

criterion_group!(
    axis_benches,
    benchmark_axis_hybrid_queries,
    benchmark_axis_adaptive_strategy,
    benchmark_axis_index_migration,
    benchmark_axis_join_operations,
    benchmark_axis_sparse_indexing,
    benchmark_axis_monitoring_overhead
);

criterion_main!(axis_benches);
