// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Vector operations benchmarks

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::time::Duration;
use tokio::runtime::Runtime;

use super::{BenchmarkConfig, BenchmarkDataGenerator, BenchmarkRunner};
use proximadb::core::{Config, VectorRecord};
use proximadb::storage::StorageEngine;

/// Benchmark vector insertion performance
pub fn benchmark_vector_insertion(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let generator = BenchmarkDataGenerator::new(config.clone());
    let runner = BenchmarkRunner::new(config.clone());

    let mut group = c.benchmark_group("vector_insertion");
    group.measurement_time(Duration::from_secs(30));
    group.warm_up_time(Duration::from_secs(5));

    for &count in &config.vector_counts {
        for &dimension in &config.dimensions {
            if count <= 10_000 || dimension <= 512 {
                // Skip very large combinations for CI
                group.throughput(Throughput::Elements(count as u64));

                group.bench_with_input(
                    BenchmarkId::new("insert", format!("{}x{}", count, dimension)),
                    &(count, dimension),
                    |b, &(count, dimension)| {
                        let vectors =
                            generator.generate_vectors(count, dimension, 0.1, "bench_collection");

                        b.to_async(&runner.runtime).iter(|| async {
                            let storage = runner.create_test_storage().await;

                            for vector in &vectors {
                                let mut storage_guard = storage.write().await;
                                let _ =
                                    storage_guard.insert_vector(black_box(vector.clone())).await;
                            }
                        });
                    },
                );
            }
        }
    }

    group.finish();
}

/// Benchmark vector retrieval performance
pub fn benchmark_vector_retrieval(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let generator = BenchmarkDataGenerator::new(config.clone());
    let runner = BenchmarkRunner::new(config.clone());

    let mut group = c.benchmark_group("vector_retrieval");
    group.measurement_time(Duration::from_secs(30));

    for &count in &[1_000, 10_000] {
        // Smaller datasets for retrieval
        for &dimension in &[128, 256, 512] {
            group.throughput(Throughput::Elements(count as u64));

            group.bench_with_input(
                BenchmarkId::new("get_by_id", format!("{}x{}", count, dimension)),
                &(count, dimension),
                |b, &(count, dimension)| {
                    let vectors =
                        generator.generate_vectors(count, dimension, 0.1, "bench_collection");

                    b.to_async(&runner.runtime).iter(|| async {
                        let storage = runner.create_test_storage().await;

                        // First insert all vectors
                        {
                            let mut storage_guard = storage.write().await;
                            for vector in &vectors {
                                let _ = storage_guard.insert_vector(vector.clone()).await;
                            }
                        }

                        // Then benchmark retrieval
                        {
                            let storage_guard = storage.read().await;
                            for vector in &vectors {
                                let _ = storage_guard.get_vector(black_box(&vector.id)).await;
                            }
                        }
                    });
                },
            );
        }
    }

    group.finish();
}

/// Benchmark vector batch operations
pub fn benchmark_vector_batch_operations(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let generator = BenchmarkDataGenerator::new(config.clone());
    let runner = BenchmarkRunner::new(config.clone());

    let mut group = c.benchmark_group("vector_batch_operations");
    group.measurement_time(Duration::from_secs(30));

    let batch_sizes = vec![10, 100, 1000];

    for &batch_size in &batch_sizes {
        for &dimension in &[128, 256] {
            group.throughput(Throughput::Elements(batch_size as u64));

            group.bench_with_input(
                BenchmarkId::new(
                    "batch_insert",
                    format!("batch_{}x{}", batch_size, dimension),
                ),
                &(batch_size, dimension),
                |b, &(batch_size, dimension)| {
                    let vectors =
                        generator.generate_vectors(batch_size, dimension, 0.1, "bench_collection");

                    b.to_async(&runner.runtime).iter(|| async {
                        let storage = runner.create_test_storage().await;
                        let mut storage_guard = storage.write().await;

                        // Batch insert all vectors at once
                        for vector in &vectors {
                            let _ = storage_guard.insert_vector(black_box(vector.clone())).await;
                        }
                    });
                },
            );
        }
    }

    group.finish();
}

/// Benchmark vector operations with different sparsity levels
pub fn benchmark_vector_sparsity_impact(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let generator = BenchmarkDataGenerator::new(config.clone());
    let runner = BenchmarkRunner::new(config.clone());

    let mut group = c.benchmark_group("vector_sparsity_impact");
    group.measurement_time(Duration::from_secs(20));

    let count = 1_000;
    let dimension = 256;

    for &sparsity in &config.sparsity_levels {
        group.throughput(Throughput::Elements(count as u64));

        group.bench_with_input(
            BenchmarkId::new("insert_sparsity", format!("sparsity_{:.1}", sparsity)),
            &sparsity,
            |b, &sparsity| {
                let vectors =
                    generator.generate_vectors(count, dimension, sparsity, "bench_collection");

                b.to_async(&runner.runtime).iter(|| async {
                    let storage = runner.create_test_storage().await;
                    let mut storage_guard = storage.write().await;

                    for vector in &vectors {
                        let _ = storage_guard.insert_vector(black_box(vector.clone())).await;
                    }
                });
            },
        );
    }

    group.finish();
}

/// Benchmark vector metadata filtering performance
pub fn benchmark_vector_metadata_filtering(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let generator = BenchmarkDataGenerator::new(config.clone());
    let runner = BenchmarkRunner::new(config.clone());

    let mut group = c.benchmark_group("vector_metadata_filtering");
    group.measurement_time(Duration::from_secs(20));

    let count = 5_000;
    let dimension = 256;

    group.throughput(Throughput::Elements(count as u64));

    group.bench_function("metadata_filter_search", |b| {
        let vectors = generator.generate_vectors(count, dimension, 0.1, "bench_collection");

        b.to_async(&runner.runtime).iter(|| async {
            let storage = runner.create_test_storage().await;

            // Insert vectors
            {
                let mut storage_guard = storage.write().await;
                for vector in &vectors {
                    let _ = storage_guard.insert_vector(vector.clone()).await;
                }
            }

            // Search with metadata filter
            {
                let storage_guard = storage.read().await;
                let mut filter = std::collections::HashMap::new();
                filter.insert("category".to_string(), serde_json::json!("cat_1"));

                let _ = storage_guard
                    .search_with_metadata_filters(
                        black_box(&"bench_collection".to_string()),
                        black_box(&filter),
                        black_box(Some(100)),
                    )
                    .await;
            }
        });
    });

    group.finish();
}

/// Benchmark vector deletion performance
pub fn benchmark_vector_deletion(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let generator = BenchmarkDataGenerator::new(config.clone());
    let runner = BenchmarkRunner::new(config.clone());

    let mut group = c.benchmark_group("vector_deletion");
    group.measurement_time(Duration::from_secs(20));

    let count = 1_000;
    let dimension = 256;

    group.throughput(Throughput::Elements(count as u64));

    group.bench_function("soft_delete", |b| {
        let vectors = generator.generate_vectors(count, dimension, 0.1, "bench_collection");

        b.to_async(&runner.runtime).iter(|| async {
            let storage = runner.create_test_storage().await;

            // Insert vectors first
            {
                let mut storage_guard = storage.write().await;
                for vector in &vectors {
                    let _ = storage_guard.insert_vector(vector.clone()).await;
                }
            }

            // Then delete them
            {
                let mut storage_guard = storage.write().await;
                for vector in &vectors {
                    let _ = storage_guard
                        .delete_vector(black_box(&vector.collection_id), black_box(&vector.id))
                        .await;
                }
            }
        });
    });

    group.finish();
}

criterion_group!(
    vector_ops_benches,
    benchmark_vector_insertion,
    benchmark_vector_retrieval,
    benchmark_vector_batch_operations,
    benchmark_vector_sparsity_impact,
    benchmark_vector_metadata_filtering,
    benchmark_vector_deletion
);

criterion_main!(vector_ops_benches);
