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

//! # Graph Database Benchmarks
//!
//! Performance benchmarks for ProximaDB's native graph database operations,
//! measuring throughput and latency for various graph operations.

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use proximadb::{
    graph::{Edge, Node, PropertyValue, service::GraphOperationsService},
    proto::proximadb_v1::{NodeQuery, TraversalAlgorithm, TraversalRequest, PropertyFilter, PropertyFilterOperator, property_value::Value},
};
use std::collections::HashMap;
use std::sync::Arc;

const DEFAULT_GRAPH_ID: &str = "benchmark_graph";

/// Benchmark node creation operations
fn bench_node_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("node_creation");
    let runtime = tokio::runtime::Runtime::new().unwrap();

    for size in [100, 1000, 10000] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("create_nodes", size), &size, |b, &size| {
            b.iter(|| {
                let service = Arc::new(GraphOperationsService::new());

                runtime.block_on(async {
                    for i in 0..size {
                        let node = Node {
                            id: format!("bench_node_{}", i),
                            labels: vec!["BenchmarkNode".to_string()],
                            properties: HashMap::from([
                                (
                                    "index".to_string(),
                                    PropertyValue {
                                        value: Some(Value::IntValue(i as i64)),
                                    },
                                ),
                                (
                                    "name".to_string(),
                                    PropertyValue {
                                        value: Some(Value::StringValue(format!("Node {}", i))),
                                },
                                ),
                            ]),
                            embedding: None,
                            created_at_ms: 0,
                            updated_at_ms: 0,
                        };

                        black_box(service.create_node(DEFAULT_GRAPH_ID, node).await.unwrap());
                    }
                });

                service
            });
        });
    }

    group.finish();
}

/// Benchmark edge creation operations
fn bench_edge_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("edge_creation");
    let runtime = tokio::runtime::Runtime::new().unwrap();

    for size in [100, 1000, 5000] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("create_edges", size), &size, |b, &size| {
            b.iter(|| {
                let service = Arc::new(GraphOperationsService::new());

                runtime.block_on(async {
                    // Create nodes first
                    for i in 0..size {
                        let node = Node {
                            id: format!("bench_node_{}", i),
                            labels: vec!["BenchmarkNode".to_string()],
                            properties: HashMap::new(),
                            embedding: None,
                            created_at_ms: 0,
                            updated_at_ms: 0,
                        };
                        service.create_node(DEFAULT_GRAPH_ID, node).await.unwrap();
                    }

                    // Create edges
                    for i in 0..(size - 1) {
                        let edge = Edge {
                            id: format!("bench_edge_{}", i),
                            from_node_id: format!("bench_node_{}", i),
                            to_node_id: format!("bench_node_{}", (i + 1) % size),
                            edge_type: "CONNECTS".to_string(),
                            properties: HashMap::new(),
                            weight: None,
                            created_at_ms: 0,
                            updated_at_ms: 0,
                        };

                        black_box(service.create_edge(DEFAULT_GRAPH_ID, edge).await.unwrap());
                    }
                });

                service
            });
        });
    }

    group.finish();
}

/// Benchmark node retrieval operations
fn bench_node_retrieval(c: &mut Criterion) {
    let mut group = c.benchmark_group("node_retrieval");
    let runtime = tokio::runtime::Runtime::new().unwrap();

    for size in [100, 1000, 10000] {
        // Setup: Create nodes
        let service = Arc::new(GraphOperationsService::new());
        runtime.block_on(async {
            for i in 0..size {
                let node = Node {
                    id: format!("bench_node_{}", i),
                    labels: vec!["BenchmarkNode".to_string()],
                    properties: HashMap::new(),
                    embedding: None,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                };
                service.create_node(DEFAULT_GRAPH_ID, node).await.unwrap();
            }
        });

        group.throughput(Throughput::Elements(1));
        group.bench_with_input(BenchmarkId::new("get_node", size), &size, |b, &_size| {
            b.iter(|| {
                runtime.block_on(async {
                    let node_id = "bench_node_50";
                    black_box(service.get_node(DEFAULT_GRAPH_ID, &node_id.to_string()).await.unwrap());
                });
            });
        });
    }

    group.finish();
}

/// Benchmark graph traversal operations
fn bench_traversal(c: &mut Criterion) {
    let mut group = c.benchmark_group("traversal");
    let runtime = tokio::runtime::Runtime::new().unwrap();

    for depth in [1, 2, 3] {
        // Setup: Create a simple graph
        let service = Arc::new(GraphOperationsService::new());
        runtime.block_on(async {
            // Create nodes
            for i in 0..100 {
                let node = Node {
                    id: format!("bench_node_{}", i),
                    labels: vec!["BenchmarkNode".to_string()],
                    properties: HashMap::new(),
                    embedding: None,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                };
                service.create_node(DEFAULT_GRAPH_ID, node).await.unwrap();
            }

            // Create edges in a chain
            for i in 0..99 {
                let edge = Edge {
                    id: format!("bench_edge_{}", i),
                    from_node_id: format!("bench_node_{}", i),
                    to_node_id: format!("bench_node_{}", i + 1),
                    edge_type: "CONNECTS".to_string(),
                    properties: HashMap::new(),
                    weight: None,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                };
                service.create_edge(DEFAULT_GRAPH_ID, edge).await.unwrap();
            }
        });

        group.throughput(Throughput::Elements(1));

        // BFS traversal
        group.bench_with_input(BenchmarkId::new("bfs", depth), &depth, |b, &depth| {
            b.iter(|| {
                runtime.block_on(async {
                    let request = TraversalRequest {
                        graph_id: DEFAULT_GRAPH_ID.to_string(),
                        start_node_id: "bench_node_0".to_string(),
                        algorithm: TraversalAlgorithm::Bfs as i32,
                        max_depth: depth,
                        edge_types: vec!["CONNECTS".to_string()],
                        filters: vec![],
                        node_labels: vec![],
                        timeout_ms: None,
                        max_frontier: None,
                        limit: Some(100),
                    };
                    black_box(service.traverse(DEFAULT_GRAPH_ID, request).await.unwrap());
                });
            });
        });

        // DFS traversal
        group.bench_with_input(BenchmarkId::new("dfs", depth), &depth, |b, &depth| {
            b.iter(|| {
                runtime.block_on(async {
                    let request = TraversalRequest {
                        graph_id: DEFAULT_GRAPH_ID.to_string(),
                        start_node_id: "bench_node_0".to_string(),
                        algorithm: TraversalAlgorithm::Dfs as i32,
                        max_depth: depth,
                        edge_types: vec!["CONNECTS".to_string()],
                        filters: vec![],
                        node_labels: vec![],
                        timeout_ms: None,
                        max_frontier: None,
                        limit: Some(100),
                    };
                    black_box(service.traverse(DEFAULT_GRAPH_ID, request).await.unwrap());
                });
            });
        });
    }

    group.finish();
}

/// Benchmark neighbor retrieval operations
fn bench_neighbor_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("neighbor_operations");
    let runtime = tokio::runtime::Runtime::new().unwrap();

    // Setup: Create a graph with varying connectivity
    for connectivity in [10, 50, 100] {
        let service = Arc::new(GraphOperationsService::new());
        runtime.block_on(async {
            // Create hub node
            let hub_node = Node {
                id: "hub_node".to_string(),
                labels: vec!["HubNode".to_string()],
                properties: HashMap::new(),
                embedding: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            };
            service.create_node(DEFAULT_GRAPH_ID, hub_node).await.unwrap();

            // Create connected nodes
            for i in 0..connectivity {
                let node = Node {
                    id: format!("connected_node_{}", i),
                    labels: vec!["ConnectedNode".to_string()],
                    properties: HashMap::new(),
                    embedding: None,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                };
                service.create_node(DEFAULT_GRAPH_ID, node).await.unwrap();

                // Create edges from hub to connected nodes
                let edge = Edge {
                    id: format!("edge_{}", i),
                    from_node_id: "hub_node".to_string(),
                    to_node_id: format!("connected_node_{}", i),
                    edge_type: "CONNECTS".to_string(),
                    properties: HashMap::new(),
                    weight: None,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                };
                service.create_edge(DEFAULT_GRAPH_ID, edge).await.unwrap();
            }
        });

        group.throughput(Throughput::Elements(1));
        group.bench_with_input(
            BenchmarkId::new("get_neighbors", connectivity),
            &connectivity,
            |b, _| {
                b.iter(|| {
                    runtime.block_on(async {
                        let node_id = "hub_node";
                        black_box(service.get_neighbors(DEFAULT_GRAPH_ID, &node_id.to_string()).await.unwrap());
                    });
                });
            },
        );
    }

    group.finish();
}

/// Benchmark node query operations
fn bench_node_query(c: &mut Criterion) {
    let mut group = c.benchmark_group("node_query");
    let runtime = tokio::runtime::Runtime::new().unwrap();

    // Setup: Create nodes with properties
    let service = Arc::new(GraphOperationsService::new());
    runtime.block_on(async {
        for i in 0..1000 {
            let node = Node {
                id: format!("query_node_{}", i),
                labels: vec![format!("Label{}", i % 10)],
                properties: HashMap::from([
                    (
                        "category".to_string(),
                        PropertyValue {
                            value: Some(Value::StringValue(format!("cat_{}", i % 5))),
                        },
                    ),
                    (
                        "value".to_string(),
                        PropertyValue {
                            value: Some(Value::IntValue((i * 10) as i64)),
                        },
                    ),
                ]),
                embedding: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            };
            service.create_node(DEFAULT_GRAPH_ID, node).await.unwrap();
        }
    });

    group.throughput(Throughput::Elements(1));

    // Query by label
    group.bench_function("query_by_label", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let query = NodeQuery {
                    graph_id: DEFAULT_GRAPH_ID.to_string(),
                    labels: vec!["Label5".to_string()],
                    filters: vec![],
                    limit: Some(100),
                    offset: Some(0),
                    continuation_token: None,
                };
                black_box(service.query_nodes(DEFAULT_GRAPH_ID, query).await.unwrap());
            });
        });
    });

    // Query by property
    group.bench_function("query_by_property", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let query = NodeQuery {
                    graph_id: DEFAULT_GRAPH_ID.to_string(),
                    labels: vec![],
                    filters: vec![PropertyFilter {
                        key: "category".to_string(),
                        operator: PropertyFilterOperator::Equals as i32,
                        value: Some(PropertyValue {
                            value: Some(Value::StringValue("cat_3".to_string())),
                        }),
                    }],
                    limit: Some(100),
                    offset: Some(0),
                    continuation_token: None,
                };
                black_box(service.query_nodes(DEFAULT_GRAPH_ID, query).await.unwrap());
            });
        });
    });

    group.finish();
}

/// Benchmark batch operations
fn bench_batch_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("batch_operations");
    let runtime = tokio::runtime::Runtime::new().unwrap();

    for batch_size in [10, 100, 1000] {
        group.throughput(Throughput::Elements(batch_size as u64));

        // Batch node creation
        group.bench_with_input(
            BenchmarkId::new("batch_create_nodes", batch_size),
            &batch_size,
            |b, &batch_size| {
                b.iter(|| {
                    let service = Arc::new(GraphOperationsService::new());
                    runtime.block_on(async {
                        let mut nodes = Vec::new();
                        for i in 0..batch_size {
                            nodes.push(Node {
                                id: format!("batch_node_{}", i),
                                labels: vec!["BatchNode".to_string()],
                                properties: HashMap::new(),
                                embedding: None,
                                created_at_ms: 0,
                                updated_at_ms: 0,
                            });
                        }
                        black_box(service.batch_create_nodes(DEFAULT_GRAPH_ID, nodes).await.unwrap());
                    });
                });
            },
        );

        // Batch edge creation
        group.bench_with_input(
            BenchmarkId::new("batch_create_edges", batch_size),
            &batch_size,
            |b, &batch_size| {
                b.iter(|| {
                    let service = Arc::new(GraphOperationsService::new());
                    runtime.block_on(async {
                        // First create nodes
                        for i in 0..(batch_size + 1) {
                            let node = Node {
                                id: format!("batch_node_{}", i),
                                labels: vec!["BatchNode".to_string()],
                                properties: HashMap::new(),
                                embedding: None,
                                created_at_ms: 0,
                                updated_at_ms: 0,
                            };
                            service.create_node(DEFAULT_GRAPH_ID, node).await.unwrap();
                        }

                        // Create edges in batch
                        let mut edges = Vec::new();
                        for i in 0..batch_size {
                            edges.push(Edge {
                                id: format!("batch_edge_{}", i),
                                from_node_id: format!("batch_node_{}", i),
                                to_node_id: format!("batch_node_{}", i + 1),
                                edge_type: "CONNECTS".to_string(),
                                properties: HashMap::new(),
                                weight: None,
                                created_at_ms: 0,
                                updated_at_ms: 0,
                            });
                        }
                        black_box(service.batch_create_edges(DEFAULT_GRAPH_ID, edges).await.unwrap());
                    });
                });
            },
        );
    }

    group.finish();
}

/// Benchmark statistics operations
fn bench_stats_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("stats");
    let runtime = tokio::runtime::Runtime::new().unwrap();

    // Setup: Create a reasonably sized graph
    let service = Arc::new(GraphOperationsService::new());
    runtime.block_on(async {
        for i in 0..1000 {
            let node = Node {
                id: format!("stats_node_{}", i),
                labels: vec![format!("Label{}", i % 10)],
                properties: HashMap::new(),
                embedding: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            };
            service.create_node(DEFAULT_GRAPH_ID, node).await.unwrap();
        }

        for i in 0..500 {
            let edge = Edge {
                id: format!("stats_edge_{}", i),
                from_node_id: format!("stats_node_{}", i),
                to_node_id: format!("stats_node_{}", (i + 1) * 2 % 1000),
                edge_type: format!("TYPE{}", i % 5),
                properties: HashMap::new(),
                weight: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            };
            service.create_edge(DEFAULT_GRAPH_ID, edge).await.unwrap();
        }
    });

    group.bench_function("get_stats", |b| {
        b.iter(|| {
            runtime.block_on(async {
                black_box(service.get_stats(DEFAULT_GRAPH_ID).await.unwrap());
            });
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_node_creation,
    bench_edge_creation,
    bench_node_retrieval,
    bench_traversal,
    bench_neighbor_operations,
    bench_node_query,
    bench_batch_operations,
    bench_stats_operations
);

criterion_main!(benches);