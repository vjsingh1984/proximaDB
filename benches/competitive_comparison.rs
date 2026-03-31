// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Competitive Benchmarks: ProximaDB vs Industry Leaders
//!
//! This module benchmarks ProximaDB against published performance numbers from:
//! - Milvus: https://milvus.io/benchmark
//! - Weaviate: https://weaviate.io/blog/benchmarking-performance
//! - Pinecone: (proprietary, estimated from public data)

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use proximadb::storage::engines::factory::UnifiedStorageEngine;
use proximadb::core::search::DistanceMetric;
use rand::Rng;
use std::time::Duration;

const DATASET_SIZES: &[usize] = &[1_000, 10_000, 100_000, 1_000_000];
const DIMENSIONS: &[usize] = &[128, 384, 768, 1536];

/// Benchmark vector search performance
fn bench_vector_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("vector_search");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(100);

    for &dim in DIMENSIONS {
        for &size in DATASET_SIZES {
            // Skip configs that would take too long
            if size * dim > 50_000_000 {
                continue;
            }

            group.throughput(Throughput::Elements(size as u64));

            group.bench_with_input(
                BenchmarkId::from_parameter(format!("{}_d{}_k10", size, dim)),
                &(size, dim, 10),
                |b, &(size, dim, top_k)| {
                    let rt = tokio::runtime::Runtime::new().unwrap();
                    b.to_async(rt).iter(|| async {
                        let engine = create_test_engine(size, dim).await;
                        let query = random_vector(dim);
                        black_box(
                            engine
                                .search_vectors(
                                    black_box("test_collection"),
                                    query,
                                    top_k,
                                    DistanceMetric::Cosine,
                                    None,
                                )
                                .await
                                .unwrap(),
                        )
                    });
                },
            );
        }
    }

    group.finish();
}

/// Benchmark hybrid search (vector + metadata filter)
fn bench_hybrid_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("hybrid_search");
    group.measurement_time(Duration::from_secs(10));

    for &size in [10_000, 100_000].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            &size,
            |b, &size| {
                let rt = tokio::runtime::Runtime::new().unwrap();
                b.to_async(rt).iter(|| async {
                    let engine = create_test_engine_with_metadata(size, 128).await;
                    let query = random_vector(128);
                    black_box(
                        engine
                            .search_vectors(
                                black_box("test_collection"),
                                query,
                                100,
                                DistanceMetric::Euclidean,
                                Some("category = 'electronics'".to_string()),
                            )
                            .await
                            .unwrap(),
                    )
                });
            },
        );
    }

    group.finish();
}

/// Benchmark indexing throughput (vectors/second)
fn bench_indexing_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("indexing_throughput");
    group.measurement_time(Duration::from_secs(30));

    for &size in [1_000, 10_000, 100_000].iter() {
        group.throughput(Throughput::Elements(size as u64));

        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            let rt = tokio::runtime::Runtime::new().unwrap();
            b.to_async(rt).iter(|| async {
                let engine = create_empty_engine(128).await;
                let vectors: Vec<_> = (0..size)
                    .map(|_| random_vector(128))
                    .collect();

                let start = std::time::Instant::now();
                for (i, vec) in vectors.iter().enumerate() {
                    engine
                        .insert_vector(
                            black_box("test_collection"),
                            format!("id_{}", i),
                            vec.clone(),
                            Default::default(),
                        )
                        .await
                        .unwrap();
                }
                black_box(start.elapsed());
            });
        });
    }

    group.finish();
}

/// Benchmark multi-hop graph queries
fn bench_graph_queries(c: &mut Criterion) {
    let mut group = c.benchmark_group("graph_queries");
    group.measurement_time(Duration::from_secs(10));

    for &nodes in [1_000, 10_000, 100_000].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("nodes_{}", nodes)),
            &nodes,
            |b, &nodes| {
                let rt = tokio::runtime::Runtime::new().unwrap();
                b.to_async(rt).iter(|| async {
                    let graph = create_test_graph(nodes, 5).await;
                    black_box(
                        graph
                            .execute_query(
                                black_box("MATCH (a)-[:KNOWS]->(b)-[:WORKS_AT]->(c) RETURN count(*)"),
                            )
                            .await
                            .unwrap(),
                    )
                });
            },
        );
    }

    group.finish();
}

/// Helper: Create test engine with vectors
async fn create_test_engine(size: usize, dim: usize) -> UnifiedStorageEngine {
    let engine = create_empty_engine(dim).await;

    for i in 0..size {
        let vec = random_vector(dim);
        engine
            .insert_vector(
                "test_collection",
                format!("id_{}", i),
                vec,
                Default::default(),
            )
            .await
            .unwrap();
    }

    engine
}

/// Helper: Create empty engine
async fn create_empty_engine(dim: usize) -> UnifiedStorageEngine {
    UnifiedStorageEngine::new_swift("test", dim, DistanceMetric::Cosine)
        .await
        .unwrap()
}

/// Helper: Create engine with metadata for hybrid search
async fn create_test_engine_with_metadata(size: usize, dim: usize) -> UnifiedStorageEngine {
    let engine = create_empty_engine(dim).await;

    for i in 0..size {
        let vec = random_vector(dim);
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("category".to_string(), format!("category_{}", i % 10));
        metadata.insert("price".to_string(), ((i * 100) as f64).into());

        engine
            .insert_vector("test_collection", format!("id_{}", i), vec, metadata)
            .await
            .unwrap();
    }

    engine
}

/// Helper: Create test graph with nodes and edges
async fn create_test_graph(nodes: usize, avg_degree: usize) -> proximadb::graph::Graph {
    use proximadb::graph::Graph;

    let graph = Graph::new_orion("test_graph").await.unwrap();

    // Create nodes
    for i in 0..nodes {
        graph
            .add_node(i, format!("node_{}", i))
            .await
            .unwrap();
    }

    // Create edges
    for i in 0..nodes {
        for j in 1..=avg_degree {
            let target = (i + j) % nodes;
            graph
                .add_edge(i, target, "KNOWS", Default::default())
                .await
                .unwrap();
        }
    }

    graph
}

/// Generate random vector
fn random_vector(dim: usize) -> Vec<f32> {
    let mut rng = rand::thread_rng();
    (0..dim).map(|_| rng.gen::<f32>()).collect()
}

criterion_group!(
    name = competitive_benchmarks;
    config = Criterion::default().measurement_time(Duration::from_secs(30));
    targets = bench_vector_search, bench_hybrid_search, bench_indexing_throughput,
    bench_graph_queries
);

criterion_main!(competitive_benchmarks);
