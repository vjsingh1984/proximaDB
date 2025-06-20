// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Vector search performance benchmarks

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::time::Duration;
use tokio::runtime::Runtime;

use super::{BenchmarkConfig, BenchmarkDataGenerator, BenchmarkRunner};
use proximadb::core::{Config, VectorRecord};
use proximadb::storage::StorageEngine;

/// Benchmark similarity search performance
pub fn benchmark_similarity_search(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let generator = BenchmarkDataGenerator::new(config.clone());
    let runner = BenchmarkRunner::new(config.clone());

    let mut group = c.benchmark_group("similarity_search");
    group.measurement_time(Duration::from_secs(30));

    let dataset_sizes = vec![1_000, 10_000, 50_000];
    let query_counts = vec![10, 100];
    let k_values = vec![10, 50, 100];

    for &dataset_size in &dataset_sizes {
        for &query_count in &query_counts {
            for &k in &k_values {
                if dataset_size <= 10_000 || k <= 50 {
                    // Skip large combinations for CI
                    let dimension = 256;
                    group.throughput(Throughput::Elements(query_count as u64));

                    group.bench_with_input(
                        BenchmarkId::new(
                            "ann_search",
                            format!("{}d_{}q_k{}", dataset_size, query_count, k),
                        ),
                        &(dataset_size, query_count, k, dimension),
                        |b, &(dataset_size, query_count, k, dimension)| {
                            let vectors = generator.generate_vectors(
                                dataset_size,
                                dimension,
                                0.1,
                                "search_bench",
                            );
                            let queries =
                                generator.generate_query_vectors(query_count, dimension, 0.1);

                            b.to_async(&runner.runtime).iter(|| async {
                                let storage = runner.create_test_storage().await;

                                // Insert dataset
                                {
                                    let mut storage_guard = storage.write().await;
                                    for vector in &vectors {
                                        let _ = storage_guard.insert_vector(vector.clone()).await;
                                    }
                                }

                                // Perform similarity searches
                                {
                                    let storage_guard = storage.read().await;
                                    for query in &queries {
                                        let _ = storage_guard
                                            .search_similar_vectors(
                                                black_box(&"search_bench".to_string()),
                                                black_box(query),
                                                black_box(k),
                                                black_box(Some(0.7)),
                                            )
                                            .await;
                                    }
                                }
                            });
                        },
                    );
                }
            }
        }
    }

    group.finish();
}

/// Benchmark range-based search
pub fn benchmark_range_search(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let generator = BenchmarkDataGenerator::new(config.clone());
    let runner = BenchmarkRunner::new(config.clone());

    let mut group = c.benchmark_group("range_search");
    group.measurement_time(Duration::from_secs(25));

    let dataset_size = 10_000;
    let dimension = 256;
    let query_count = 50;
    let similarity_thresholds = vec![0.5, 0.7, 0.9];

    for &threshold in &similarity_thresholds {
        group.throughput(Throughput::Elements(query_count as u64));

        group.bench_with_input(
            BenchmarkId::new("range_search", format!("threshold_{:.1}", threshold)),
            &threshold,
            |b, &threshold| {
                let vectors =
                    generator.generate_vectors(dataset_size, dimension, 0.1, "range_bench");
                let queries = generator.generate_query_vectors(query_count, dimension, 0.1);

                b.to_async(&runner.runtime).iter(|| async {
                    let storage = runner.create_test_storage().await;

                    // Insert dataset
                    {
                        let mut storage_guard = storage.write().await;
                        for vector in &vectors {
                            let _ = storage_guard.insert_vector(vector.clone()).await;
                        }
                    }

                    // Perform range searches
                    {
                        let storage_guard = storage.read().await;
                        for query in &queries {
                            let _ = storage_guard
                                .search_similar_vectors(
                                    black_box(&"range_bench".to_string()),
                                    black_box(query),
                                    black_box(1000), // Large k for range search
                                    black_box(Some(threshold)),
                                )
                                .await;
                        }
                    }
                });
            },
        );
    }

    group.finish();
}

/// Benchmark search with different vector dimensions
pub fn benchmark_dimensional_search(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let generator = BenchmarkDataGenerator::new(config.clone());
    let runner = BenchmarkRunner::new(config.clone());

    let mut group = c.benchmark_group("dimensional_search");
    group.measurement_time(Duration::from_secs(30));

    let dataset_size = 5_000;
    let query_count = 25;
    let k = 20;
    let dimensions = vec![128, 256, 512, 1024, 1536]; // Common embedding dimensions

    for &dimension in &dimensions {
        group.throughput(Throughput::Elements(query_count as u64));

        group.bench_with_input(
            BenchmarkId::new("dimension_search", format!("{}d", dimension)),
            &dimension,
            |b, &dimension| {
                let vectors = generator.generate_vectors(dataset_size, dimension, 0.1, "dim_bench");
                let queries = generator.generate_query_vectors(query_count, dimension, 0.1);

                b.to_async(&runner.runtime).iter(|| async {
                    let storage = runner.create_test_storage().await;

                    // Insert dataset
                    {
                        let mut storage_guard = storage.write().await;
                        for vector in &vectors {
                            let _ = storage_guard.insert_vector(vector.clone()).await;
                        }
                    }

                    // Perform searches
                    {
                        let storage_guard = storage.read().await;
                        for query in &queries {
                            let _ = storage_guard
                                .search_similar_vectors(
                                    black_box(&"dim_bench".to_string()),
                                    black_box(query),
                                    black_box(k),
                                    black_box(Some(0.8)),
                                )
                                .await;
                        }
                    }
                });
            },
        );
    }

    group.finish();
}

/// Benchmark sparse vector search
pub fn benchmark_sparse_vector_search(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let generator = BenchmarkDataGenerator::new(config.clone());
    let runner = BenchmarkRunner::new(config.clone());

    let mut group = c.benchmark_group("sparse_vector_search");
    group.measurement_time(Duration::from_secs(25));

    let dataset_size = 5_000;
    let dimension = 512;
    let query_count = 20;
    let k = 15;

    for &sparsity in &config.sparsity_levels {
        if sparsity >= 0.5 {
            // Focus on sparse vectors
            group.throughput(Throughput::Elements(query_count as u64));

            group.bench_with_input(
                BenchmarkId::new("sparse_search", format!("sparsity_{:.1}", sparsity)),
                &sparsity,
                |b, &sparsity| {
                    let vectors = generator.generate_vectors(
                        dataset_size,
                        dimension,
                        sparsity,
                        "sparse_bench",
                    );
                    let queries =
                        generator.generate_query_vectors(query_count, dimension, sparsity);

                    b.to_async(&runner.runtime).iter(|| async {
                        let storage = runner.create_test_storage().await;

                        // Insert sparse dataset
                        {
                            let mut storage_guard = storage.write().await;
                            for vector in &vectors {
                                let _ = storage_guard.insert_vector(vector.clone()).await;
                            }
                        }

                        // Perform sparse searches
                        {
                            let storage_guard = storage.read().await;
                            for query in &queries {
                                let _ = storage_guard
                                    .search_similar_vectors(
                                        black_box(&"sparse_bench".to_string()),
                                        black_box(query),
                                        black_box(k),
                                        black_box(Some(0.6)),
                                    )
                                    .await;
                            }
                        }
                    });
                },
            );
        }
    }

    group.finish();
}

/// Benchmark hybrid search (vector + metadata filtering)
pub fn benchmark_hybrid_search(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let generator = BenchmarkDataGenerator::new(config.clone());
    let runner = BenchmarkRunner::new(config.clone());

    let mut group = c.benchmark_group("hybrid_search");
    group.measurement_time(Duration::from_secs(30));

    let dataset_size = 10_000;
    let dimension = 256;
    let query_count = 30;
    let k = 25;

    // Test different metadata selectivity
    let filter_selectivities = vec![
        ("high_selectivity", 0.05),   // Filters match 5% of data
        ("medium_selectivity", 0.20), // Filters match 20% of data
        ("low_selectivity", 0.50),    // Filters match 50% of data
    ];

    for (name, selectivity) in filter_selectivities {
        group.throughput(Throughput::Elements(query_count as u64));

        group.bench_with_input(
            BenchmarkId::new("hybrid_search", name),
            &selectivity,
            |b, &selectivity| {
                let vectors =
                    generator.generate_vectors(dataset_size, dimension, 0.1, "hybrid_bench");
                let queries = generator.generate_query_vectors(query_count, dimension, 0.1);

                b.to_async(&runner.runtime).iter(|| async {
                    let storage = runner.create_test_storage().await;

                    // Insert dataset
                    {
                        let mut storage_guard = storage.write().await;
                        for vector in &vectors {
                            let _ = storage_guard.insert_vector(vector.clone()).await;
                        }
                    }

                    // Perform hybrid searches
                    {
                        let storage_guard = storage.read().await;
                        for query in &queries {
                            // Create metadata filter based on selectivity
                            let mut filter = std::collections::HashMap::new();
                            let category_count = (10.0 * selectivity) as usize;
                            filter.insert(
                                "category".to_string(),
                                serde_json::json!(format!("cat_{}", category_count)),
                            );

                            let _ = storage_guard
                                .search_with_metadata_filters(
                                    black_box(&"hybrid_bench".to_string()),
                                    black_box(&filter),
                                    black_box(Some(k)),
                                )
                                .await;
                        }
                    }
                });
            },
        );
    }

    group.finish();
}

/// Benchmark search index building performance
pub fn benchmark_index_building(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let generator = BenchmarkDataGenerator::new(config.clone());
    let runner = BenchmarkRunner::new(config.clone());

    let mut group = c.benchmark_group("index_building");
    group.measurement_time(Duration::from_secs(40));

    let dataset_sizes = vec![1_000, 5_000, 25_000];
    let dimension = 256;

    for &dataset_size in &dataset_sizes {
        group.throughput(Throughput::Elements(dataset_size as u64));

        group.bench_with_input(
            BenchmarkId::new("build_index", format!("{}vectors", dataset_size)),
            &dataset_size,
            |b, &dataset_size| {
                let vectors =
                    generator.generate_vectors(dataset_size, dimension, 0.1, "index_bench");

                b.to_async(&runner.runtime).iter(|| async {
                    let storage = runner.create_test_storage().await;
                    let mut storage_guard = storage.write().await;

                    // Insert all vectors (this triggers index building)
                    for vector in &vectors {
                        let _ = storage_guard.insert_vector(black_box(vector.clone())).await;
                    }
                });
            },
        );
    }

    group.finish();
}

/// Benchmark concurrent search operations
pub fn benchmark_concurrent_search(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let generator = BenchmarkDataGenerator::new(config.clone());
    let runner = BenchmarkRunner::new(config.clone());

    let mut group = c.benchmark_group("concurrent_search");
    group.measurement_time(Duration::from_secs(30));

    let dataset_size = 10_000;
    let dimension = 256;
    let queries_per_task = 10;
    let k = 20;
    let concurrent_tasks = vec![1, 2, 4, 8];

    for &tasks in &concurrent_tasks {
        group.throughput(Throughput::Elements((queries_per_task * tasks) as u64));

        group.bench_with_input(
            BenchmarkId::new("concurrent_search", format!("{}tasks", tasks)),
            &tasks,
            |b, &tasks| {
                let vectors =
                    generator.generate_vectors(dataset_size, dimension, 0.1, "concurrent_search");

                b.to_async(&runner.runtime).iter(|| async {
                    let storage = runner.create_test_storage().await;

                    // Insert dataset once
                    {
                        let mut storage_guard = storage.write().await;
                        for vector in &vectors {
                            let _ = storage_guard.insert_vector(vector.clone()).await;
                        }
                    }

                    // Concurrent search tasks
                    let mut handles = Vec::new();

                    for task_id in 0..tasks {
                        let queries =
                            generator.generate_query_vectors(queries_per_task, dimension, 0.1);
                        let storage_clone = storage.clone();

                        let handle = tokio::spawn(async move {
                            let storage_guard = storage_clone.read().await;
                            for query in queries {
                                let _ = storage_guard
                                    .search_similar_vectors(
                                        &"concurrent_search".to_string(),
                                        &query,
                                        k,
                                        Some(0.7),
                                    )
                                    .await;
                            }
                        });

                        handles.push(handle);
                    }

                    // Wait for all search tasks to complete
                    for handle in handles {
                        let _ = handle.await;
                    }
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    search_benches,
    benchmark_similarity_search,
    benchmark_range_search,
    benchmark_dimensional_search,
    benchmark_sparse_vector_search,
    benchmark_hybrid_search,
    benchmark_index_building,
    benchmark_concurrent_search
);

criterion_main!(search_benches);
