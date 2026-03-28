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
    proto::proximadb_v1::{
        CreateGraphRequest, NodeQuery, PropertyFilter, PropertyFilterOperator, TraversalAlgorithm,
        TraversalRequest, property_value::Value,
    },
    services::graph_collection::GraphCollectionService,
};
use std::collections::HashMap;
use std::sync::Arc;

const DEFAULT_GRAPH_ID: &str = "benchmark_graph";

/// Helper function to ensure graph collection exists before benchmarking
async fn ensure_benchmark_graph_exists(
    collection_service: &GraphCollectionService,
) -> Result<(), Box<dyn std::error::Error>> {
    // Check if graph already exists in the list
    let existing_graphs = collection_service.list_graphs().await?;
    if existing_graphs
        .iter()
        .any(|g| g.graph_id == DEFAULT_GRAPH_ID)
    {
        return Ok(());
    }

    // Create the benchmark graph collection
    let request = CreateGraphRequest {
        graph_id: DEFAULT_GRAPH_ID.to_string(),
        name: Some("Benchmark Graph".to_string()),
        description: Some("Graph collection for benchmarking operations".to_string()),
        schema: None,
        storage_config: None,
        engine_config: None,
        access_control: None,
    };

    collection_service.create_graph(request).await?;
    Ok(())
}

/// Benchmark node creation operations
fn bench_node_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("node_creation");
    let runtime = tokio::runtime::Runtime::new().unwrap();

    // Pre-initialize services outside the benchmark loop
    let (service, _collection_service) = runtime.block_on(async {
        // Use GraphCollectionService instead of GraphOperationsService for graph management
        let collection_service = Arc::new(GraphCollectionService::new());

        // Create graph operations service with collection service
        let service = Arc::new(GraphOperationsService::new_with_collection_service(
            collection_service.clone(),
        ));

        // Ensure benchmark graph exists
        ensure_benchmark_graph_exists(&collection_service)
            .await
            .unwrap();

        (service, collection_service)
    });

    for size in [32, 128, 1024] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("create_nodes", size), &size, |b, &size| {
            b.iter(|| {
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

                        // Use .ok() to handle "already exists" across benchmark iterations
                        black_box(service.create_node(DEFAULT_GRAPH_ID, node).await.ok());
                    }
                });
            });
        });
    }

    group.finish();
}

/// Benchmark edge creation operations
fn bench_edge_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("edge_creation");
    let runtime = tokio::runtime::Runtime::new().unwrap();

    // Pre-initialize services
    let (service, _collection_service) = runtime.block_on(async {
        let collection_service = Arc::new(GraphCollectionService::new());
        let service = Arc::new(GraphOperationsService::new_with_collection_service(
            collection_service.clone(),
        ));
        ensure_benchmark_graph_exists(&collection_service)
            .await
            .unwrap();
        (service, collection_service)
    });

    // Pre-create nodes for edges
    runtime.block_on(async {
        for i in 0..32 {
            let node = Node {
                id: format!("edge_node_{}", i),
                labels: vec!["EdgeNode".to_string()],
                properties: HashMap::new(),
                embedding: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            };
            service.create_node(DEFAULT_GRAPH_ID, node).await.ok();
        }
    });

    for size in [32, 128, 512] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("create_edges", size), &size, |b, &size| {
            b.iter(|| {
                runtime.block_on(async {
                    for i in 0..size {
                        let edge = Edge {
                            id: format!("bench_edge_{}", i),
                            from_node_id: format!("edge_node_{}", i % 32),
                            to_node_id: format!("edge_node_{}", (i + 1) % 32),
                            edge_type: "CONNECTS_TO".to_string(),
                            properties: HashMap::from([(
                                "weight".to_string(),
                                PropertyValue {
                                    value: Some(Value::DoubleValue((i % 10) as f64)),
                                },
                            )]),
                            weight: Some((i % 10) as f64),
                            created_at_ms: 0,
                            updated_at_ms: 0,
                        };

                        // Use .ok() to handle "already exists" across benchmark iterations
                        black_box(service.create_edge(DEFAULT_GRAPH_ID, edge).await.ok());
                    }
                });
            });
        });
    }

    group.finish();
}

/// Benchmark node query operations
fn bench_node_queries(c: &mut Criterion) {
    let mut group = c.benchmark_group("node_queries");
    let runtime = tokio::runtime::Runtime::new().unwrap();

    // Pre-initialize and populate graph
    let (service, _collection_service) = runtime.block_on(async {
        let collection_service = Arc::new(GraphCollectionService::new());
        let service = Arc::new(GraphOperationsService::new_with_collection_service(
            collection_service.clone(),
        ));
        ensure_benchmark_graph_exists(&collection_service)
            .await
            .unwrap();

        // Create test nodes
        for i in 0..128 {
            let node = Node {
                id: format!("query_node_{}", i),
                labels: vec![
                    "QueryNode".to_string(),
                    if i % 2 == 0 { "Even" } else { "Odd" }.to_string(),
                ],
                properties: HashMap::from([(
                    "value".to_string(),
                    PropertyValue {
                        value: Some(Value::IntValue(i)),
                    },
                )]),
                embedding: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            };
            service.create_node(DEFAULT_GRAPH_ID, node).await.ok();
        }

        (service, collection_service)
    });

    // Benchmark different query types
    group.bench_function("query_by_id", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let query = NodeQuery {
                    graph_id: DEFAULT_GRAPH_ID.to_string(),
                    labels: vec![],
                    filters: vec![],
                    limit: Some(1),
                    offset: None,
                    continuation_token: None,
                };
                black_box(
                    service
                        .query_nodes(DEFAULT_GRAPH_ID, query.clone())
                        .await
                        .unwrap(),
                );
            });
        });
    });

    group.bench_function("query_by_label", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let query = NodeQuery {
                    graph_id: DEFAULT_GRAPH_ID.to_string(),
                    labels: vec!["Even".to_string()],
                    filters: vec![],
                    limit: Some(10),
                    offset: None,
                    continuation_token: None,
                };
                black_box(
                    service
                        .query_nodes(DEFAULT_GRAPH_ID, query.clone())
                        .await
                        .unwrap(),
                );
            });
        });
    });

    group.bench_function("query_by_property", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let query = NodeQuery {
                    graph_id: DEFAULT_GRAPH_ID.to_string(),
                    labels: vec![],
                    filters: vec![PropertyFilter {
                        key: "value".to_string(),
                        operator: PropertyFilterOperator::GreaterThan as i32,
                        value: Some(PropertyValue {
                            value: Some(Value::IntValue(64)),
                        }),
                    }],
                    limit: Some(20),
                    offset: None,
                    continuation_token: None,
                };
                black_box(
                    service
                        .query_nodes(DEFAULT_GRAPH_ID, query.clone())
                        .await
                        .unwrap(),
                );
            });
        });
    });

    group.finish();
}

/// Benchmark graph traversal operations
fn bench_traversal(c: &mut Criterion) {
    let mut group = c.benchmark_group("traversal");
    let runtime = tokio::runtime::Runtime::new().unwrap();

    // Pre-initialize and create a connected graph
    let (service, _collection_service) = runtime.block_on(async {
        let collection_service = Arc::new(GraphCollectionService::new());
        let service = Arc::new(GraphOperationsService::new_with_collection_service(
            collection_service.clone(),
        ));
        ensure_benchmark_graph_exists(&collection_service)
            .await
            .unwrap();

        // Create a small connected graph for traversal
        for i in 0..32 {
            let node = Node {
                id: format!("trav_node_{}", i),
                labels: vec!["TraversalNode".to_string()],
                properties: HashMap::new(),
                embedding: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            };
            service.create_node(DEFAULT_GRAPH_ID, node).await.ok();
        }

        // Create edges to form a connected graph
        for i in 0..40 {
            let edge = Edge {
                id: format!("trav_edge_{}", i),
                from_node_id: format!("trav_node_{}", i % 32),
                to_node_id: format!("trav_node_{}", (i * 7 + 3) % 32), // Create interesting connections
                edge_type: "TRAVERSES".to_string(),
                properties: HashMap::new(),
                weight: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            };
            service.create_edge(DEFAULT_GRAPH_ID, edge).await.ok();
        }

        (service, collection_service)
    });

    // Benchmark BFS traversal
    group.bench_function("bfs_traversal", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let request = TraversalRequest {
                    graph_id: DEFAULT_GRAPH_ID.to_string(),
                    start_node_id: "trav_node_0".to_string(),
                    algorithm: TraversalAlgorithm::Bfs as i32,
                    max_depth: 3,
                    edge_types: vec![],
                    node_labels: vec![],
                    filters: vec![],
                    limit: Some(64),
                    timeout_ms: None,
                    max_frontier: None,
                };
                black_box(service.traverse(DEFAULT_GRAPH_ID, request).await.unwrap());
            });
        });
    });

    // Benchmark DFS traversal
    group.bench_function("dfs_traversal", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let request = TraversalRequest {
                    graph_id: DEFAULT_GRAPH_ID.to_string(),
                    start_node_id: "trav_node_0".to_string(),
                    algorithm: TraversalAlgorithm::Dfs as i32,
                    max_depth: 4,
                    edge_types: vec![],
                    node_labels: vec![],
                    filters: vec![],
                    limit: Some(64),
                    timeout_ms: None,
                    max_frontier: None,
                };
                black_box(service.traverse(DEFAULT_GRAPH_ID, request).await.unwrap());
            });
        });
    });

    group.finish();
}

/// Benchmark shortest path operations
fn bench_shortest_path(c: &mut Criterion) {
    let mut group = c.benchmark_group("shortest_path");
    let runtime = tokio::runtime::Runtime::new().unwrap();

    // Pre-initialize with a weighted graph
    let (service, _collection_service) = runtime.block_on(async {
        let collection_service = Arc::new(GraphCollectionService::new());
        let service = Arc::new(GraphOperationsService::new_with_collection_service(
            collection_service.clone(),
        ));
        ensure_benchmark_graph_exists(&collection_service)
            .await
            .unwrap();

        // Create a grid-like graph for pathfinding (reduced for OOM prevention)
        let grid_size = 8;
        for x in 0..grid_size {
            for y in 0..grid_size {
                let node = Node {
                    id: format!("grid_{}_{}", x, y),
                    labels: vec!["GridNode".to_string()],
                    properties: HashMap::from([
                        (
                            "x".to_string(),
                            PropertyValue {
                                value: Some(Value::IntValue(x)),
                            },
                        ),
                        (
                            "y".to_string(),
                            PropertyValue {
                                value: Some(Value::IntValue(y)),
                            },
                        ),
                    ]),
                    embedding: None,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                };
                service.create_node(DEFAULT_GRAPH_ID, node).await.ok();

                // Connect to adjacent nodes (right and down)
                if x < grid_size - 1 {
                    let edge = Edge {
                        id: format!("grid_edge_{}_{}_right", x, y),
                        from_node_id: format!("grid_{}_{}", x, y),
                        to_node_id: format!("grid_{}_{}", x + 1, y),
                        edge_type: "GRID_EDGE".to_string(),
                        properties: HashMap::from([(
                            "weight".to_string(),
                            PropertyValue {
                                value: Some(Value::DoubleValue(1.0)),
                            },
                        )]),
                        weight: Some(1.0),
                        created_at_ms: 0,
                        updated_at_ms: 0,
                    };
                    service.create_edge(DEFAULT_GRAPH_ID, edge).await.ok();
                }

                if y < grid_size - 1 {
                    let edge = Edge {
                        id: format!("grid_edge_{}_{}_down", x, y),
                        from_node_id: format!("grid_{}_{}", x, y),
                        to_node_id: format!("grid_{}_{}", x, y + 1),
                        edge_type: "GRID_EDGE".to_string(),
                        properties: HashMap::from([(
                            "weight".to_string(),
                            PropertyValue {
                                value: Some(Value::DoubleValue(1.0)),
                            },
                        )]),
                        weight: Some(1.0),
                        created_at_ms: 0,
                        updated_at_ms: 0,
                    };
                    service.create_edge(DEFAULT_GRAPH_ID, edge).await.ok();
                }
            }
        }

        (service, collection_service)
    });

    // Benchmark shortest path finding
    group.bench_function("dijkstra_path", |b| {
        b.iter(|| {
            runtime.block_on(async {
                black_box(
                    service
                        .shortest_path(
                            DEFAULT_GRAPH_ID,
                            &"grid_0_0".to_string(),
                            &"grid_7_7".to_string(),
                            Some(50),
                            None,
                            None,
                            None,
                            None,
                            None,
                        )
                        .await
                        .unwrap(),
                );
            });
        });
    });

    group.finish();
}

/// Benchmark A* shortest path operations
fn bench_astar_shortest_path(c: &mut Criterion) {
    let mut group = c.benchmark_group("astar_shortest_path");
    let runtime = tokio::runtime::Runtime::new().unwrap();

    // Pre-initialize with a weighted grid
    let (service, _collection_service) = runtime.block_on(async {
        let collection_service = Arc::new(GraphCollectionService::new());
        let service = Arc::new(GraphOperationsService::new_with_collection_service(
            collection_service.clone(),
        ));
        ensure_benchmark_graph_exists(&collection_service)
            .await
            .unwrap();

        // Create a grid-like graph for pathfinding (reduced for OOM prevention)
        let grid_size = 8;
        for x in 0..grid_size {
            for y in 0..grid_size {
                let node = Node {
                    id: format!("astar_{}_{}", x, y),
                    labels: vec!["GridNode".to_string()],
                    properties: HashMap::new(),
                    embedding: None,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                };
                service.create_node(DEFAULT_GRAPH_ID, node).await.ok();

                // Connect to adjacent nodes (right and down)
                if x < grid_size - 1 {
                    let edge = Edge {
                        id: format!("astar_edge_{}_{}_right", x, y),
                        from_node_id: format!("astar_{}_{}", x, y),
                        to_node_id: format!("astar_{}_{}", x + 1, y),
                        edge_type: "GRID_EDGE".to_string(),
                        properties: HashMap::from([(
                            "weight".to_string(),
                            PropertyValue {
                                value: Some(Value::DoubleValue(1.0)),
                            },
                        )]),
                        weight: Some(1.0),
                        created_at_ms: 0,
                        updated_at_ms: 0,
                    };
                    service.create_edge(DEFAULT_GRAPH_ID, edge).await.ok();
                }

                if y < grid_size - 1 {
                    let edge = Edge {
                        id: format!("astar_edge_{}_{}_down", x, y),
                        from_node_id: format!("astar_{}_{}", x, y),
                        to_node_id: format!("astar_{}_{}", x, y + 1),
                        edge_type: "GRID_EDGE".to_string(),
                        properties: HashMap::from([(
                            "weight".to_string(),
                            PropertyValue {
                                value: Some(Value::DoubleValue(1.0)),
                            },
                        )]),
                        weight: Some(1.0),
                        created_at_ms: 0,
                        updated_at_ms: 0,
                    };
                    service.create_edge(DEFAULT_GRAPH_ID, edge).await.ok();
                }
            }
        }

        (service, collection_service)
    });

    // Benchmark A* shortest path finding
    group.bench_function("astar_path", |b| {
        b.iter(|| {
            runtime.block_on(async {
                black_box(
                    service
                        .shortest_path(
                            DEFAULT_GRAPH_ID,
                            &"astar_0_0".to_string(),
                            &"astar_7_7".to_string(),
                            Some(50),
                            None,
                            Some(proximadb::proto::proximadb_v1::ShortestPathAlgorithm::Astar),
                            None,
                            None,
                            None,
                        )
                        .await
                        .unwrap(),
                );
            });
        });
    });

    group.finish();
}

/// Benchmark K-shortest paths (returns best of K for timing consistency)
fn bench_k_shortest_paths(c: &mut Criterion) {
    let mut group = c.benchmark_group("k_shortest_paths");
    let runtime = tokio::runtime::Runtime::new().unwrap();

    // Build a small layered DAG to provide multiple alternative routes
    let (service, _collection_service) = runtime.block_on(async {
        let collection_service = Arc::new(GraphCollectionService::new());
        let service = Arc::new(GraphOperationsService::new_with_collection_service(
            collection_service.clone(),
        ));
        ensure_benchmark_graph_exists(&collection_service)
            .await
            .unwrap();

        // Layers L0..L4 each with 10 nodes, connect Lk to Lk+1 densely
        let layers = 5;
        let width = 10;
        for l in 0..layers {
            for i in 0..width {
                let node = Node {
                    id: format!("ksp_{}_{}", l, i),
                    labels: vec!["KSP".to_string()],
                    properties: HashMap::new(),
                    embedding: None,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                };
                service.create_node(DEFAULT_GRAPH_ID, node).await.ok();
            }
        }
        for l in 0..(layers - 1) {
            for i in 0..width {
                for j in 0..width {
                    let edge = Edge {
                        id: format!("ksp_e_{}_{}_to_{}_{}", l, i, l + 1, j),
                        from_node_id: format!("ksp_{}_{}", l, i),
                        to_node_id: format!("ksp_{}_{}", l + 1, j),
                        edge_type: "KSP_EDGE".to_string(),
                        properties: HashMap::from([(
                            "weight".to_string(),
                            PropertyValue {
                                value: Some(Value::DoubleValue(1.0)),
                            },
                        )]),
                        weight: Some(1.0),
                        created_at_ms: 0,
                        updated_at_ms: 0,
                    };
                    service.create_edge(DEFAULT_GRAPH_ID, edge).await.ok();
                }
            }
        }

        (service, collection_service)
    });

    group.bench_function("ksp_k5", |b| {
        b.iter(|| {
            runtime.block_on(async {
                black_box(
                    service
                        .shortest_path(
                            DEFAULT_GRAPH_ID,
                            &"ksp_0_0".to_string(),
                            &format!("ksp_{}_{}", 4, 9),
                            Some(10),
                            Some(vec!["KSP_EDGE".to_string()]),
                            None,
                            Some(5),
                            None,
                            None,
                        )
                        .await
                        .unwrap(),
                );
            });
        });
    });

    group.finish();
}

/// Benchmark connected components detection
fn bench_connected_components(c: &mut Criterion) {
    let mut group = c.benchmark_group("connected_components");
    let runtime = tokio::runtime::Runtime::new().unwrap();

    let (service, _collection_service) = runtime.block_on(async {
        let collection_service = Arc::new(GraphCollectionService::new());
        let service = Arc::new(GraphOperationsService::new_with_collection_service(
            collection_service.clone(),
        ));
        ensure_benchmark_graph_exists(&collection_service)
            .await
            .unwrap();

        // Build 4 disjoint components, each a line of 10 nodes
        for cidx in 0..4 {
            let base = cidx * 1000;
            for i in 0..10 {
                let node = Node {
                    id: format!("cc_{}_{}", cidx, base + i),
                    labels: vec!["CC".to_string()],
                    properties: HashMap::new(),
                    embedding: None,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                };
                service.create_node(DEFAULT_GRAPH_ID, node).await.ok();
                if i > 0 {
                    let edge = Edge {
                        id: format!("cc_e_{}_{}", cidx, base + i),
                        from_node_id: format!("cc_{}_{}", cidx, base + i - 1),
                        to_node_id: format!("cc_{}_{}", cidx, base + i),
                        edge_type: "CC_EDGE".to_string(),
                        properties: HashMap::new(),
                        weight: None,
                        created_at_ms: 0,
                        updated_at_ms: 0,
                    };
                    service.create_edge(DEFAULT_GRAPH_ID, edge).await.ok();
                }
            }
        }

        (service, collection_service)
    });

    group.bench_function("components", |b| {
        b.iter(|| {
            runtime.block_on(async {
                black_box(
                    service
                        .connected_components(DEFAULT_GRAPH_ID)
                        .await
                        .unwrap(),
                );
            });
        });
    });

    group.finish();
}

/// Benchmark cycle detection
fn bench_cycle_detection(c: &mut Criterion) {
    let mut group = c.benchmark_group("cycle_detection");
    let runtime = tokio::runtime::Runtime::new().unwrap();

    // Build a graph with cycles (a few interlinked rings)
    let (service, _collection_service) = runtime.block_on(async {
        let collection_service = Arc::new(GraphCollectionService::new());
        let service = Arc::new(GraphOperationsService::new_with_collection_service(
            collection_service.clone(),
        ));
        ensure_benchmark_graph_exists(&collection_service)
            .await
            .unwrap();

        // Three rings of size 16 each, plus cross-links
        for ring in 0..3 {
            let base = ring * 10000;
            let size = 16;
            for i in 0..size {
                let node = Node {
                    id: format!("cy_{}_{}", ring, base + i),
                    labels: vec!["CY".to_string()],
                    properties: HashMap::new(),
                    embedding: None,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                };
                service.create_node(DEFAULT_GRAPH_ID, node).await.ok();
            }
            for i in 0..size {
                let edge = Edge {
                    id: format!("cy_e_{}_{}", ring, base + i),
                    from_node_id: format!("cy_{}_{}", ring, base + i),
                    to_node_id: format!("cy_{}_{}", ring, base + ((i + 1) % size)),
                    edge_type: "CY_EDGE".to_string(),
                    properties: HashMap::new(),
                    weight: None,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                };
                service.create_edge(DEFAULT_GRAPH_ID, edge).await.ok();
            }
        }

        // Cross links to create more cycles
        for i in 0..16 {
            let edge = Edge {
                id: format!("cy_link_{}", i),
                from_node_id: format!("cy_0_{}", i),
                to_node_id: format!("cy_1_{}", 10000 + (i * 3) % 16),
                edge_type: "CY_EDGE".to_string(),
                properties: HashMap::new(),
                weight: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            };
            service.create_edge(DEFAULT_GRAPH_ID, edge).await.ok();
        }

        (service, collection_service)
    });

    group.bench_function("has_cycle", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let _ = black_box(service.has_cycle(DEFAULT_GRAPH_ID).await.unwrap());
            });
        });
    });

    group.finish();
}

/// Connected components scaling study (vary node count)
fn bench_connected_components_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("connected_components_scaling");
    let runtime = tokio::runtime::Runtime::new().unwrap();

    // Scale line sizes for each component
    for line_len in [10usize, 25, 80] {
        let service = runtime.block_on(async {
            let collection_service = Arc::new(GraphCollectionService::new());
            let service = Arc::new(GraphOperationsService::new_with_collection_service(
                collection_service.clone(),
            ));
            ensure_benchmark_graph_exists(&collection_service)
                .await
                .unwrap();

            // Build 4 components, each a simple path of line_len nodes
            for cidx in 0..4 {
                let base = cidx * 100000 + line_len * 10;
                for i in 0..line_len {
                    let node = Node {
                        id: format!("ccs_{}_{}", cidx, base + i),
                        labels: vec!["CCS".to_string()],
                        properties: HashMap::new(),
                        embedding: None,
                        created_at_ms: 0,
                        updated_at_ms: 0,
                    };
                    service.create_node(DEFAULT_GRAPH_ID, node).await.ok();
                    if i > 0 {
                        let edge = Edge {
                            id: format!("ccs_e_{}_{}", cidx, base + i),
                            from_node_id: format!("ccs_{}_{}", cidx, base + i - 1),
                            to_node_id: format!("ccs_{}_{}", cidx, base + i),
                            edge_type: "CCS_EDGE".to_string(),
                            properties: HashMap::new(),
                            weight: None,
                            created_at_ms: 0,
                            updated_at_ms: 0,
                        };
                        service.create_edge(DEFAULT_GRAPH_ID, edge).await.ok();
                    }
                }
            }
            service
        });

        group.bench_with_input(
            BenchmarkId::new("components", line_len),
            &line_len,
            |b, _| {
                b.iter(|| {
                    runtime.block_on(async {
                        black_box(
                            service
                                .connected_components(DEFAULT_GRAPH_ID)
                                .await
                                .unwrap(),
                        );
                    });
                });
            },
        );
    }

    group.finish();
}
// Configure with consistent settings across all benchmarks
criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(15)
        .measurement_time(std::time::Duration::from_secs(3))
        .warm_up_time(std::time::Duration::from_secs(1));
    targets = bench_node_creation,
              bench_edge_creation,
              bench_node_queries,
              bench_traversal,
              bench_shortest_path,
              bench_astar_shortest_path,
              bench_k_shortest_paths,
              bench_connected_components,
              bench_cycle_detection,
              bench_connected_components_scaling
}

criterion_main!(benches);
