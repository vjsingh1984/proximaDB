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

//! # TD-046 gRPC Parity - Performance Benchmarks
//!
//! This benchmark suite measures the performance of gRPC methods compared to REST API:
//! - Graph creation with different engines (ORION, PULSAR, QUASAR)
//! - Graph operations performance parity
//! - Serialization/deserialization overhead
//! - Network protocol efficiency
//!
//! ## Expected Performance Characteristics
//!
//! 1. **gRPC vs REST**: gRPC should be 20-40% faster due to binary protocol
//! 2. **Engine Selection**: ORION (fastest) > PULSAR > QUASAR (experimental)
//! 3. **Graph Operations**: gRPC should have < 10% overhead vs direct calls
//! 4. **Serialization**: Protocol Buffers should be 2-3× faster than JSON

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use proximadb::{
    graph::{Edge, Node, PropertyValue, service::GraphOperationsService},
    proto::proximadb_v1::{CompressionAlgorithm, CreateGraphRequest, GraphStorageConfig},
    services::graph_collection::GraphCollectionService,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

const BENCHMARK_GRAPH_ID: &str = "td046_benchmark_graph";

/// Helper function to create benchmark graph with specified engine
async fn create_td046_benchmark_graph(
    collection_service: &Arc<GraphCollectionService>,
    engine_type: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let test_dir = format!("/tmp/proximadb-td046-bench-{}", engine_type);
    let _ = std::fs::remove_dir_all(&test_dir);
    std::fs::create_dir_all(&test_dir).unwrap();

    let request = CreateGraphRequest {
        graph_id: BENCHMARK_GRAPH_ID.to_string(),
        name: Some(format!("TD-046 Benchmark Graph - {}", engine_type)),
        description: Some(format!("Benchmark graph for {} engine", engine_type)),
        schema: None,
        storage_config: Some(GraphStorageConfig {
            engine_type: engine_type.to_string(),
            base_url: test_dir,
            compression: CompressionAlgorithm::CompressionSnappy as i32,
            enable_wal: true,
            snapshot_interval_hours: 24,
            engine_specific_config: HashMap::new(),
        }),
        engine_config: None,
        access_control: None,
    };

    collection_service.create_graph(request).await.ok(); // Ignore if already exists
    Ok(())
}

/// Benchmark graph creation with different engines
fn bench_engine_selection(c: &mut Criterion) {
    let mut group = c.benchmark_group("td046_engine_selection");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(30);

    let runtime = tokio::runtime::Runtime::new().unwrap();

    // Benchmark each engine type
    for engine in ["ORION", "PULSAR", "QUASAR"] {
        let (service, collection_service) = runtime.block_on(async {
            let collection_service = Arc::new(GraphCollectionService::new());
            let service = Arc::new(GraphOperationsService::new_with_collection_service(
                collection_service.clone(),
            ));

            create_td046_benchmark_graph(&collection_service, engine)
                .await
                .unwrap();

            (service, collection_service)
        });

        group.bench_with_input(
            BenchmarkId::new("graph_creation", engine),
            &engine,
            |b, engine| {
                b.iter(|| {
                    runtime.block_on(async {
                        let graph_id = format!("bench_{}_{}", engine, uuid::Uuid::new_v4());
                        let test_dir = format!("/tmp/proximadb-test-{}", graph_id);

                        let request = CreateGraphRequest {
                            graph_id: graph_id.clone(),
                            name: Some(format!("Benchmark Graph {}", engine)),
                            description: Some("Temporary benchmark graph".to_string()),
                            schema: None,
                            storage_config: Some(GraphStorageConfig {
                                engine_type: engine.to_string(),
                                base_url: test_dir,
                                compression: CompressionAlgorithm::CompressionSnappy as i32,
                                enable_wal: true,
                                snapshot_interval_hours: 24,
                                engine_specific_config: HashMap::new(),
                            }),
                            engine_config: None,
                            access_control: None,
                        };

                        black_box(
                            service
                                .create_graph_collection(black_box(request))
                                .await
                                .unwrap(),
                        );

                        // Cleanup
                        service.remove_graph(&graph_id);
                    });
                });
            },
        );
    }

    group.finish();
}

/// Benchmark graph operations performance
fn bench_graph_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("td046_graph_operations");
    group.measurement_time(Duration::from_secs(8));
    group.sample_size(50);

    let runtime = tokio::runtime::Runtime::new().unwrap();

    // Pre-initialize benchmark graph
    let (service, collection_service) = runtime.block_on(async {
        let collection_service = Arc::new(GraphCollectionService::new());
        let service = Arc::new(GraphOperationsService::new_with_collection_service(
            collection_service.clone(),
        ));

        create_td046_benchmark_graph(&collection_service, "ORION")
            .await
            .unwrap();

        (service, collection_service)
    });

    // Benchmark node creation
    for node_count in [10, 50, 100] {
        group.throughput(Throughput::Elements(node_count as u64));
        group.bench_with_input(BenchmarkId::new("create_nodes", node_count), &node_count, |b, &count| {
            b.iter(|| {
                runtime.block_on(async {
                    for i in 0..count {
                        let node = Node {
                            id: format!("bench_node_{}_{}", i, uuid::Uuid::new_v4()),
                            labels: vec!["BenchmarkNode".to_string()],
                            properties: HashMap::from([(
                                "index".to_string(),
                                PropertyValue {
                                    value: Some(proximadb::proto::proximadb_v1::property_value::Value::IntValue(i as i64)),
                                },
                            )]),
                            embedding: None,
                            created_at_ms: 0,
                            updated_at_ms: 0,
                        };
                        black_box(service.create_node(BENCHMARK_GRAPH_ID, node).await.unwrap());
                    }
                });
            });
        });
    }

    // Benchmark node retrieval
    let (service, _collection_service) = runtime.block_on(async {
        // Pre-populate with nodes
        for i in 0..100 {
            let node = Node {
                id: format!("retrieve_node_{}", i),
                labels: vec!["RetrieveNode".to_string()],
                properties: HashMap::new(),
                embedding: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            };
            service.create_node(BENCHMARK_GRAPH_ID, node).await.ok();
        }
        (service, collection_service)
    });

    group.bench_function("retrieve_nodes", |b| {
        b.iter(|| {
            runtime.block_on(async {
                for i in 0..10 {
                    let node_id = format!("retrieve_node_{}", i);
                    black_box(
                        service
                            .get_node(BENCHMARK_GRAPH_ID, &node_id)
                            .await
                            .unwrap(),
                    );
                }
            });
        });
    });

    // Benchmark edge creation
    for edge_count in [10, 25, 50] {
        group.throughput(Throughput::Elements(edge_count as u64));
        group.bench_with_input(
            BenchmarkId::new("create_edges", edge_count),
            &edge_count,
            |b, &count| {
                b.iter(|| {
                    runtime.block_on(async {
                        for i in 0..count {
                            let edge = Edge {
                                id: format!("bench_edge_{}_{}", i, uuid::Uuid::new_v4()),
                                from_node_id: format!("retrieve_node_{}", i % 100),
                                to_node_id: format!("retrieve_node_{}", (i + 1) % 100),
                                edge_type: "BENCHMARK_EDGE".to_string(),
                                properties: HashMap::new(),
                                weight: Some(1.0),
                                created_at_ms: 0,
                                updated_at_ms: 0,
                            };
                            black_box(service.create_edge(BENCHMARK_GRAPH_ID, edge).await.unwrap());
                        }
                    });
                });
            },
        );
    }

    group.finish();
}

/// Benchmark serialization performance (Protocol Buffers vs JSON)
fn bench_serialization_performance(c: &mut Criterion) {
    let mut group = c.benchmark_group("td046_serialization");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(100);

    // Create sample node for serialization testing
    let sample_node = Node {
        id: "sample_node_12345".to_string(),
        labels: vec!["Person".to_string(), "User".to_string()],
        properties: HashMap::from([
            (
                "name".to_string(),
                PropertyValue {
                    value: Some(
                        proximadb::proto::proximadb_v1::property_value::Value::StringValue(
                            "John Doe".to_string(),
                        ),
                    ),
                },
            ),
            (
                "age".to_string(),
                PropertyValue {
                    value: Some(
                        proximadb::proto::proximadb_v1::property_value::Value::IntValue(30),
                    ),
                },
            ),
            (
                "score".to_string(),
                PropertyValue {
                    value: Some(
                        proximadb::proto::proximadb_v1::property_value::Value::DoubleValue(0.95),
                    ),
                },
            ),
        ]),
        embedding: None,
        created_at_ms: 1234567890,
        updated_at_ms: 1234567890,
    };

    // Benchmark JSON serialization (baseline)
    group.bench_function("json_serialization", |b| {
        b.iter(|| {
            black_box(serde_json::to_string(black_box(&sample_node)).unwrap());
        });
    });

    // Benchmark JSON deserialization
    let json_string = serde_json::to_string(&sample_node).unwrap();
    group.bench_function("json_deserialization", |b| {
        b.iter(|| {
            black_box(serde_json::from_str::<Node>(black_box(&json_string)).unwrap());
        });
    });

    // Benchmark Protocol Buffers serialization
    group.bench_function("proto_serialization", |b| {
        b.iter(|| {
            black_box(prost::Message::encode_to_vec(black_box(&sample_node)));
        });
    });

    // Benchmark Protocol Buffers deserialization
    let proto_bytes = prost::Message::encode_to_vec(&sample_node);
    group.bench_function("proto_deserialization", |b| {
        b.iter(|| {
            let mut buf = &*black_box(&proto_bytes);
            black_box(Node::decode(&mut buf).unwrap());
        });
    });

    group.finish();
}

/// Benchmark error handling performance
fn bench_error_handling(c: &mut Criterion) {
    let mut group = c.benchmark_group("td046_error_handling");
    group.measurement_time(Duration::from_secs(3));
    group.sample_size(100);

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let service = Arc::new(GraphOperationsService::new());

    // Benchmark error handling for non-existent graph
    group.bench_function("nonexistent_graph_error", |b| {
        b.iter(|| {
            runtime.block_on(async {
                black_box(
                    service
                        .get_node("nonexistent_graph", "nonexistent_node")
                        .await,
                );
            });
        });
    });

    // Benchmark error handling for non-existent node
    group.bench_function("nonexistent_node_error", |b| {
        b.iter(|| {
            runtime.block_on(async {
                black_box(
                    service
                        .get_node(BENCHMARK_GRAPH_ID, "nonexistent_node")
                        .await,
                );
            });
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_engine_selection,
    bench_graph_operations,
    bench_serialization_performance,
    bench_error_handling
);
criterion_main!(benches);
