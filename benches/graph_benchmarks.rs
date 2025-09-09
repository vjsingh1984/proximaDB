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

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::collections::HashMap;
use proximadb::{
    graph::{GraphService, Node, Edge, PropertyValue},
    proto::proximadb_v1::{property_value::Value, TraversalRequest, TraversalAlgorithm, NodeQuery},
};

/// Benchmark node creation operations
fn bench_node_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("node_creation");
    
    for size in [100, 1000, 10000] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("create_nodes", size), &size, |b, &size| {
            b.iter(|| {
                let service = GraphService::new();
                
                for i in 0..size {
                    let node = Node {
                        id: format!("bench_node_{}", i),
                        labels: vec!["BenchmarkNode".to_string()],
                        properties: HashMap::from([
                            ("index".to_string(), PropertyValue {
                                value: Some(Value::IntValue(i as i64)),
                            }),
                            ("name".to_string(), PropertyValue {
                                value: Some(Value::StringValue(format!("Node {}", i))),
                            }),
                        ]),
                        embedding: None,
                        created_at: None,
                        updated_at: None,
                    };
                    
                    black_box(service.create_node(node).unwrap());
                }
                
                service
            });
        });
    }
    
    group.finish();
}

/// Benchmark edge creation operations
fn bench_edge_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("edge_creation");
    
    for size in [100, 1000, 5000] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("create_edges", size), &size, |b, &size| {
            b.iter(|| {
                let service = GraphService::new();
                
                // Create nodes first
                for i in 0..size {
                    let node = Node {
                        id: format!("bench_node_{}", i),
                        labels: vec!["BenchmarkNode".to_string()],
                        properties: HashMap::new(),
                        embedding: None,
                        created_at: None,
                        updated_at: None,
                    };
                    service.create_node(node).unwrap();
                }
                
                // Create edges
                for i in 0..(size - 1) {
                    let edge = Edge {
                        id: format!("bench_edge_{}", i),
                        from_node_id: format!("bench_node_{}", i),
                        to_node_id: format!("bench_node_{}", i + 1),
                        edge_type: "CONNECTS".to_string(),
                        properties: HashMap::from([
                            ("weight".to_string(), PropertyValue {
                                value: Some(Value::DoubleValue(1.0)),
                            }),
                        ]),
                        weight: Some(1.0),
                        created_at: None,
                        updated_at: None,
                    };
                    
                    black_box(service.create_edge(edge).unwrap());
                }
                
                service
            });
        });
    }
    
    group.finish();
}

/// Benchmark node lookup operations
fn bench_node_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("node_lookup");
    
    // Setup data once
    let service = GraphService::new();
    let size = 10000;
    
    for i in 0..size {
        let node = Node {
            id: format!("lookup_node_{}", i),
            labels: vec!["LookupTest".to_string()],
            properties: HashMap::from([
                ("index".to_string(), PropertyValue {
                    value: Some(Value::IntValue(i)),
                }),
            ]),
            embedding: None,
            created_at: None,
            updated_at: None,
        };
        service.create_node(node).unwrap();
    }
    
    group.throughput(Throughput::Elements(1000));
    group.bench_function("get_node_by_id", |b| {
        b.iter(|| {
            for i in (0..size).step_by(size / 1000) {
                let node_id = format!("lookup_node_{}", i);
                black_box(service.get_node(&node_id).unwrap());
            }
        });
    });
    
    group.finish();
}

/// Benchmark graph traversal operations
fn bench_traversal(c: &mut Criterion) {
    let mut group = c.benchmark_group("traversal");
    
    // Create a connected graph for traversal
    let service = GraphService::new();
    let size = 1000;
    
    // Create nodes
    for i in 0..size {
        let node = Node {
            id: format!("trav_node_{}", i),
            labels: vec!["TraversalTest".to_string()],
            properties: HashMap::from([
                ("level".to_string(), PropertyValue {
                    value: Some(Value::IntValue(i / 10)), // Group into levels
                }),
            ]),
            embedding: None,
            created_at: None,
            updated_at: None,
        };
        service.create_node(node).unwrap();
    }
    
    // Create edges in a tree-like structure
    for i in 0..size - 1 {
        let edge = Edge {
            id: format!("trav_edge_{}", i),
            from_node_id: format!("trav_node_{}", i / 2), // Parent node
            to_node_id: format!("trav_node_{}", i + 1),   // Child node
            edge_type: "PARENT_OF".to_string(),
            properties: HashMap::new(),
            weight: Some(1.0),
            created_at: None,
            updated_at: None,
        };
        service.create_edge(edge).unwrap();
    }
    
    // Benchmark BFS traversal
    group.bench_function("bfs_traversal", |b| {
        b.iter(|| {
            let request = TraversalRequest {
                start_node_id: "trav_node_0".to_string(),
                max_depth: 5,
                edge_types: vec!["PARENT_OF".to_string()],
                node_labels: vec![],
                filters: vec![],
                algorithm: TraversalAlgorithm::Bfs.into(),
                limit: Some(100),
            };
            
            black_box(futures::executor::block_on(service.traverse(request)).unwrap());
        });
    });
    
    // Benchmark DFS traversal
    group.bench_function("dfs_traversal", |b| {
        b.iter(|| {
            let request = TraversalRequest {
                start_node_id: "trav_node_0".to_string(),
                max_depth: 5,
                edge_types: vec!["PARENT_OF".to_string()],
                node_labels: vec![],
                filters: vec![],
                algorithm: TraversalAlgorithm::Dfs.into(),
                limit: Some(100),
            };
            
            black_box(futures::executor::block_on(service.traverse(request)).unwrap());
        });
    });
    
    group.finish();
}

/// Benchmark neighbor queries
fn bench_neighbor_queries(c: &mut Criterion) {
    let mut group = c.benchmark_group("neighbor_queries");
    
    // Create a graph with varying degrees
    let service = GraphService::new();
    let num_nodes = 1000;
    let avg_degree = 10;
    
    // Create nodes
    for i in 0..num_nodes {
        let node = Node {
            id: format!("neighbor_node_{}", i),
            labels: vec!["NeighborTest".to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at: None,
            updated_at: None,
        };
        service.create_node(node).unwrap();
    }
    
    // Create random connections
    use std::collections::HashSet;
    let mut rng = 12345u64; // Simple LCG for reproducible randomness
    
    for i in 0..num_nodes {
        let mut connected = HashSet::new();
        let degree = avg_degree + ((rng % 10) as i32 - 5); // ±5 variation
        rng = rng.wrapping_mul(1103515245).wrapping_add(12345);
        
        for j in 0..degree.max(1) {
            let target = (rng as usize) % num_nodes;
            rng = rng.wrapping_mul(1103515245).wrapping_add(12345);
            
            if target != i && !connected.contains(&target) {
                connected.insert(target);
                
                let edge = Edge {
                    id: format!("neighbor_edge_{}_{}", i, target),
                    from_node_id: format!("neighbor_node_{}", i),
                    to_node_id: format!("neighbor_node_{}", target),
                    edge_type: "CONNECTED_TO".to_string(),
                    properties: HashMap::new(),
                    weight: Some(1.0),
                    created_at: None,
                    updated_at: None,
                };
                service.create_edge(edge).unwrap();
            }
        }
    }
    
    group.throughput(Throughput::Elements(100));
    group.bench_function("get_neighbors", |b| {
        b.iter(|| {
            for i in (0..num_nodes).step_by(num_nodes / 100) {
                let node_id = format!("neighbor_node_{}", i);
                black_box(service.get_neighbors(&node_id).unwrap());
            }
        });
    });
    
    group.finish();
}

/// Benchmark node queries by labels
fn bench_node_queries(c: &mut Criterion) {
    let mut group = c.benchmark_group("node_queries");
    
    let service = GraphService::new();
    let num_nodes = 5000;
    let labels = vec!["User", "Admin", "Guest", "Bot", "Service"];
    
    // Create nodes with different labels
    for i in 0..num_nodes {
        let label = labels[i % labels.len()];
        let node = Node {
            id: format!("query_node_{}", i),
            labels: vec![label.to_string(), "Entity".to_string()],
            properties: HashMap::from([
                ("type".to_string(), PropertyValue {
                    value: Some(Value::StringValue(label.to_string())),
                }),
                ("index".to_string(), PropertyValue {
                    value: Some(Value::IntValue(i as i64)),
                }),
            ]),
            embedding: None,
            created_at: None,
            updated_at: None,
        };
        service.create_node(node).unwrap();
    }
    
    group.bench_function("query_by_single_label", |b| {
        b.iter(|| {
            let query = NodeQuery {
                labels: vec!["User".to_string()],
                filters: vec![],
                limit: Some(1000),
                offset: Some(0),
            };
            
            black_box(service.query_nodes(query).unwrap());
        });
    });
    
    group.bench_function("query_by_multiple_labels", |b| {
        b.iter(|| {
            let query = NodeQuery {
                labels: vec!["User".to_string(), "Entity".to_string()],
                filters: vec![],
                limit: Some(1000),
                offset: Some(0),
            };
            
            black_box(service.query_nodes(query).unwrap());
        });
    });
    
    group.finish();
}

/// Benchmark batch operations
fn bench_batch_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("batch_operations");
    
    for batch_size in [10, 100, 1000] {
        group.throughput(Throughput::Elements(batch_size as u64));
        
        group.bench_with_input(BenchmarkId::new("batch_create_nodes", batch_size), &batch_size, |b, &batch_size| {
            b.iter(|| {
                let service = GraphService::new();
                
                let nodes = (0..batch_size).map(|i| {
                    Node {
                        id: format!("batch_node_{}", i),
                        labels: vec!["BatchTest".to_string()],
                        properties: HashMap::from([
                            ("index".to_string(), PropertyValue {
                                value: Some(Value::IntValue(i as i64)),
                            }),
                        ]),
                        embedding: None,
                        created_at: None,
                        updated_at: None,
                    }
                }).collect::<Vec<_>>();
                
                black_box(service.batch_create_nodes(nodes).unwrap());
                service
            });
        });
        
        group.bench_with_input(BenchmarkId::new("batch_create_edges", batch_size), &batch_size, |b, &batch_size| {
            b.iter(|| {
                let service = GraphService::new();
                
                // Create nodes first
                for i in 0..batch_size {
                    let node = Node {
                        id: format!("batch_node_{}", i),
                        labels: vec!["BatchTest".to_string()],
                        properties: HashMap::new(),
                        embedding: None,
                        created_at: None,
                        updated_at: None,
                    };
                    service.create_node(node).unwrap();
                }
                
                let edges = (0..batch_size - 1).map(|i| {
                    Edge {
                        id: format!("batch_edge_{}", i),
                        from_node_id: format!("batch_node_{}", i),
                        to_node_id: format!("batch_node_{}", i + 1),
                        edge_type: "CONNECTS".to_string(),
                        properties: HashMap::new(),
                        weight: Some(1.0),
                        created_at: None,
                        updated_at: None,
                    }
                }).collect::<Vec<_>>();
                
                black_box(service.batch_create_edges(edges).unwrap());
                service
            });
        });
    }
    
    group.finish();
}

/// Benchmark memory usage and statistics
fn bench_statistics(c: &mut Criterion) {
    let mut group = c.benchmark_group("statistics");
    
    let service = GraphService::new();
    let size = 1000;
    
    // Create test data
    for i in 0..size {
        let node = Node {
            id: format!("stats_node_{}", i),
            labels: vec!["StatsTest".to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at: None,
            updated_at: None,
        };
        service.create_node(node).unwrap();
    }
    
    for i in 0..size - 1 {
        let edge = Edge {
            id: format!("stats_edge_{}", i),
            from_node_id: format!("stats_node_{}", i),
            to_node_id: format!("stats_node_{}", i + 1),
            edge_type: "CONNECTS".to_string(),
            properties: HashMap::new(),
            weight: Some(1.0),
            created_at: None,
            updated_at: None,
        };
        service.create_edge(edge).unwrap();
    }
    
    group.bench_function("get_statistics", |b| {
        b.iter(|| {
            black_box(service.get_stats().unwrap());
        });
    });
    
    group.finish();
}

criterion_group!(
    benches,
    bench_node_creation,
    bench_edge_creation,
    bench_node_lookup,
    bench_traversal,
    bench_neighbor_queries,
    bench_node_queries,
    bench_batch_operations,
    bench_statistics
);
criterion_main!(benches);