/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! # TD-035 Graph Query Executor Improvements - Performance Benchmarks
//!
//! This benchmark suite measures the performance improvements from TD-035 implementation:
//! - Parallel BFS traversal vs sequential BFS/DFS
//! - Arrow integration (HashMap → RecordBatch conversion)
//! - Query optimization effectiveness
//! - Edge filtering performance
//!
//! ## Expected Performance Improvements
//!
//! 1. **Parallel BFS**: Should be 1.5-3× faster than sequential BFS for large graphs
//! 2. **Arrow Integration**: Zero-copy conversion, < 1ms overhead
//! 3. **Query Optimization**: 20-50% reduction in query execution time
//! 4. **Edge Filtering**: 10-30% faster traversal with selective edge types

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use proximadb::{
    graph::{Edge, Node, PropertyValue, service::GraphOperationsService},
    proto::proximadb_v1::{
        CreateGraphRequest, PropertyFilter, PropertyFilterOperator, TraversalAlgorithm,
        TraversalRequest, property_value::Value,
    },
    query::arrow_graph_bridge::GraphArrowBridge,
    services::graph_collection::GraphCollectionService,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

const BENCHMARK_GRAPH_ID: &str = "td035_benchmark_graph";

/// Helper function to create benchmark graph with various structures
async fn create_td035_benchmark_graph(
    service: &Arc<GraphOperationsService>,
    collection_service: &Arc<GraphCollectionService>,
    node_count: usize,
    edge_count: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    // Create graph collection
    let request = CreateGraphRequest {
        graph_id: BENCHMARK_GRAPH_ID.to_string(),
        name: Some("TD-035 Benchmark Graph".to_string()),
        description: Some("Graph for TD-035 performance benchmarks".to_string()),
        schema: None,
        storage_config: None,
        engine_config: None,
        access_control: None,
    };

    collection_service.create_graph(request).await.ok(); // Ignore if already exists

    // Create nodes with different labels and properties
    for i in 0..node_count {
        let node = Node {
            id: format!("node_{}", i),
            labels: vec![
                "BenchmarkNode".to_string(),
                if i % 3 == 0 {
                    "TypeA"
                } else if i % 3 == 1 {
                    "TypeB"
                } else {
                    "TypeC"
                }
                .to_string(),
            ],
            properties: HashMap::from([
                (
                    "index".to_string(),
                    PropertyValue {
                        value: Some(Value::IntValue(i as i64)),
                    },
                ),
                (
                    "value".to_string(),
                    PropertyValue {
                        value: Some(Value::DoubleValue(i as f64 * 1.5)),
                    },
                ),
            ]),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        service.create_node(BENCHMARK_GRAPH_ID, node).await.ok();
    }

    // Create edges with different types and weights
    for i in 0..edge_count {
        let edge = Edge {
            id: format!("edge_{}", i),
            from_node_id: format!("node_{}", i % node_count),
            to_node_id: format!("node_{}", (i * 7 + 3) % node_count),
            edge_type: match i % 4 {
                0 => "CONNECTS",
                1 => "RELATES_TO",
                2 => "DEPENDS_ON",
                _ => "REFERENCES",
            }
            .to_string(),
            properties: HashMap::from([(
                "weight".to_string(),
                PropertyValue {
                    value: Some(Value::DoubleValue((i % 10) as f64 * 0.1 + 0.1)),
                },
            )]),
            weight: Some((i % 10) as f64 * 0.1 + 0.1),
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        service.create_edge(BENCHMARK_GRAPH_ID, edge).await.ok();
    }

    Ok(())
}

/// Benchmark traversal algorithms (BFS vs Parallel BFS)
fn bench_traversal_algorithms(c: &mut Criterion) {
    let mut group = c.benchmark_group("td035_traversal_algorithms");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(50);

    let runtime = tokio::runtime::Runtime::new().unwrap();

    // Pre-initialize benchmark graph
    let (service, collection_service) = runtime.block_on(async {
        let collection_service = Arc::new(GraphCollectionService::new());
        let service = Arc::new(GraphOperationsService::new_with_collection_service(
            collection_service.clone(),
        ));

        create_td035_benchmark_graph(&service, &collection_service, 100, 150)
            .await
            .unwrap();

        (service, collection_service)
    });

    // Benchmark BFS (baseline)
    group.bench_function("bfs_baseline", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let request = TraversalRequest {
                    graph_id: BENCHMARK_GRAPH_ID.to_string(),
                    start_node_id: "node_0".to_string(),
                    algorithm: TraversalAlgorithm::Bfs as i32,
                    max_depth: 5,
                    edge_types: vec![],
                    node_labels: vec![],
                    filters: vec![],
                    limit: Some(50),
                    timeout_ms: None,
                    max_frontier: None,
                };
                black_box(service.traverse(BENCHMARK_GRAPH_ID, request).await.unwrap());
            });
        });
    });

    // Benchmark DFS (baseline)
    group.bench_function("dfs_baseline", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let request = TraversalRequest {
                    graph_id: BENCHMARK_GRAPH_ID.to_string(),
                    start_node_id: "node_0".to_string(),
                    algorithm: TraversalAlgorithm::Dfs as i32,
                    max_depth: 5,
                    edge_types: vec![],
                    node_labels: vec![],
                    filters: vec![],
                    limit: Some(50),
                    timeout_ms: None,
                    max_frontier: None,
                };
                black_box(service.traverse(BENCHMARK_GRAPH_ID, request).await.unwrap());
            });
        });
    });

    // Benchmark Parallel BFS (TD-035 improvement)
    group.bench_function("parallel_bfs_improvement", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let request = TraversalRequest {
                    graph_id: BENCHMARK_GRAPH_ID.to_string(),
                    start_node_id: "node_0".to_string(),
                    algorithm: TraversalAlgorithm::ParallelBfs as i32,
                    max_depth: 5,
                    edge_types: vec![],
                    node_labels: vec![],
                    filters: vec![],
                    limit: Some(50),
                    timeout_ms: None,
                    max_frontier: None,
                };
                black_box(service.traverse(BENCHMARK_GRAPH_ID, request).await.unwrap());
            });
        });
    });

    group.finish();
}

/// Benchmark edge filtering performance
fn bench_edge_filtering(c: &mut Criterion) {
    let mut group = c.benchmark_group("td035_edge_filtering");
    group.measurement_time(Duration::from_secs(8));
    group.sample_size(50);

    let runtime = tokio::runtime::Runtime::new().unwrap();

    // Pre-initialize benchmark graph
    let (service, collection_service) = runtime.block_on(async {
        let collection_service = Arc::new(GraphCollectionService::new());
        let service = Arc::new(GraphOperationsService::new_with_collection_service(
            collection_service.clone(),
        ));

        create_td035_benchmark_graph(&service, &collection_service, 100, 150)
            .await
            .unwrap();

        (service, collection_service)
    });

    // Benchmark traversal without edge filtering (baseline)
    group.bench_function("no_filter_baseline", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let request = TraversalRequest {
                    graph_id: BENCHMARK_GRAPH_ID.to_string(),
                    start_node_id: "node_0".to_string(),
                    algorithm: TraversalAlgorithm::Bfs as i32,
                    max_depth: 4,
                    edge_types: vec![], // No filtering
                    node_labels: vec![],
                    filters: vec![],
                    limit: Some(50),
                    timeout_ms: None,
                    max_frontier: None,
                };
                black_box(service.traverse(BENCHMARK_GRAPH_ID, request).await.unwrap());
            });
        });
    });

    // Benchmark traversal with single edge type filter
    group.bench_function("single_edge_filter", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let request = TraversalRequest {
                    graph_id: BENCHMARK_GRAPH_ID.to_string(),
                    start_node_id: "node_0".to_string(),
                    algorithm: TraversalAlgorithm::Bfs as i32,
                    max_depth: 4,
                    edge_types: vec!["CONNECTS".to_string()], // Single filter
                    node_labels: vec![],
                    filters: vec![],
                    limit: Some(50),
                    timeout_ms: None,
                    max_frontier: None,
                };
                black_box(service.traverse(BENCHMARK_GRAPH_ID, request).await.unwrap());
            });
        });
    });

    // Benchmark traversal with multiple edge type filters
    group.bench_function("multiple_edge_filters", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let request = TraversalRequest {
                    graph_id: BENCHMARK_GRAPH_ID.to_string(),
                    start_node_id: "node_0".to_string(),
                    algorithm: TraversalAlgorithm::Bfs as i32,
                    max_depth: 4,
                    edge_types: vec!["CONNECTS".to_string(), "RELATES_TO".to_string()], // Multiple filters
                    node_labels: vec![],
                    filters: vec![],
                    limit: Some(50),
                    timeout_ms: None,
                    max_frontier: None,
                };
                black_box(service.traverse(BENCHMARK_GRAPH_ID, request).await.unwrap());
            });
        });
    });

    group.finish();
}

/// Benchmark Arrow conversion performance (TD-035 integration)
fn bench_arrow_integration(c: &mut Criterion) {
    let mut group = c.benchmark_group("td035_arrow_integration");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(100);

    // Create sample graph results in HashMap format (current format)
    let mut results_small = Vec::new();
    for i in 0..10 {
        results_small.push(HashMap::from([
            (
                "node_id".to_string(),
                serde_json::json!(format!("node_{}", i)),
            ),
            ("label".to_string(), serde_json::json!("BenchmarkNode")),
            ("index".to_string(), serde_json::json!(i)),
            ("value".to_string(), serde_json::json!(i as f64 * 1.5)),
        ]));
    }

    let mut results_medium = Vec::new();
    for i in 0..100 {
        results_medium.push(HashMap::from([
            (
                "node_id".to_string(),
                serde_json::json!(format!("node_{}", i)),
            ),
            ("label".to_string(), serde_json::json!("BenchmarkNode")),
            ("index".to_string(), serde_json::json!(i)),
            ("value".to_string(), serde_json::json!(i as f64 * 1.5)),
        ]));
    }

    let mut results_large = Vec::new();
    for i in 0..1000 {
        results_large.push(HashMap::from([
            (
                "node_id".to_string(),
                serde_json::json!(format!("node_{}", i)),
            ),
            ("label".to_string(), serde_json::json!("BenchmarkNode")),
            ("index".to_string(), serde_json::json!(i)),
            ("value".to_string(), serde_json::json!(i as f64 * 1.5)),
        ]));
    }

    // Benchmark Arrow conversion for different result sizes
    for (size, results) in [
        (10, results_small),
        (100, results_medium),
        (1000, results_large),
    ] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("arrow_conversion", size), &size, |b, _| {
            b.iter(|| {
                black_box(
                    GraphArrowBridge::graph_results_to_arrow(black_box(&results), black_box(false))
                        .unwrap(),
                );
            });
        });
    }

    group.finish();
}

/// Benchmark query optimization effectiveness
fn bench_query_optimization(c: &mut Criterion) {
    let mut group = c.benchmark_group("td035_query_optimization");
    group.measurement_time(Duration::from_secs(8));
    group.sample_size(50);

    let runtime = tokio::runtime::Runtime::new().unwrap();

    // Pre-initialize benchmark graph
    let (service, collection_service) = runtime.block_on(async {
        let collection_service = Arc::new(GraphCollectionService::new());
        let service = Arc::new(GraphOperationsService::new_with_collection_service(
            collection_service.clone(),
        ));

        create_td035_benchmark_graph(&service, &collection_service, 100, 150)
            .await
            .unwrap();

        (service, collection_service)
    });

    // Benchmark simple query without optimization
    group.bench_function("unoptimized_query", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let request = TraversalRequest {
                    graph_id: BENCHMARK_GRAPH_ID.to_string(),
                    start_node_id: "node_0".to_string(),
                    algorithm: TraversalAlgorithm::Bfs as i32,
                    max_depth: 6,
                    edge_types: vec![],
                    node_labels: vec!["TypeA".to_string()],
                    filters: vec![],
                    limit: Some(100),
                    timeout_ms: None,
                    max_frontier: None,
                };
                black_box(service.traverse(BENCHMARK_GRAPH_ID, request).await.unwrap());
            });
        });
    });

    // Benchmark optimized query with selective filters
    group.bench_function("optimized_query", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let request = TraversalRequest {
                    graph_id: BENCHMARK_GRAPH_ID.to_string(),
                    start_node_id: "node_0".to_string(),
                    algorithm: TraversalAlgorithm::Bfs as i32,
                    max_depth: 6,
                    edge_types: vec!["CONNECTS".to_string()],
                    node_labels: vec!["TypeA".to_string()],
                    filters: vec![PropertyFilter {
                        key: "index".to_string(),
                        operator: PropertyFilterOperator::LessThan as i32,
                        value: Some(PropertyValue {
                            value: Some(Value::IntValue(50)),
                        }),
                    }],
                    limit: Some(50), // Lower limit due to filtering
                    timeout_ms: None,
                    max_frontier: None,
                };
                black_box(service.traverse(BENCHMARK_GRAPH_ID, request).await.unwrap());
            });
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_traversal_algorithms,
    bench_edge_filtering,
    bench_arrow_integration,
    bench_query_optimization
);
criterion_main!(benches);
