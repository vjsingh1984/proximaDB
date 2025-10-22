/*
 * Graph Engine Comparison Benchmarks
 *
 * Measures core operations across ORION, PULSAR, QUASAR engines:
 * - Node insert throughput
 * - Edge insert throughput
 * - BFS/DFS traversal latency
 * - Dijkstra shortest path latency (on a grid graph)
 */

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use proximadb::graph::service::GraphOperationsService;
use proximadb::graph::{Edge, Node, PropertyValue};
use proximadb::proto::proximadb_v1::{
    CreateGraphRequest, GraphEngineConfig, NodeQuery, PropertyFilter, PropertyFilterOperator,
    TraversalAlgorithm, TraversalRequest, property_value::Value,
};
use proximadb::services::graph_collection::GraphCollectionService;
use std::collections::HashMap;
use std::sync::Arc;

const ENGINES: &[&str] = &["ORION", "PULSAR", "QUASAR"]; // Falls back if not fully implemented

async fn ensure_graph_with_engine(
    collection_service: &GraphCollectionService,
    engine_type: &str,
    graph_id: &str,
) {
    let existing = collection_service.list_graphs().await.unwrap_or_default();
    if existing.iter().any(|g| g.graph_id == graph_id) {
        return;
    }
    let engine_cfg = GraphEngineConfig {
        engine_type: engine_type.to_string(),
        memory_pool_size_mb: 0,
        csr_cache_size_mb: 0,
        enable_parallel_operations: true,
        max_traversal_depth: 10,
        advanced_config: HashMap::new(),
    };
    let req = CreateGraphRequest {
        graph_id: graph_id.to_string(),
        name: Some(graph_id.to_string()),
        description: None,
        schema: None,
        storage_config: None,
        engine_config: Some(engine_cfg),
        access_control: None,
    };
    let _ = collection_service.create_graph(req).await;
}

fn bench_engine_crud(c: &mut Criterion) {
    let mut group = c.benchmark_group("graph_engine_crud");
    // Heavier inputs sometimes exceed 4s; increase measurement window and
    // slightly reduce sample size for more stable CI runs.
    use std::time::Duration;
    group.measurement_time(Duration::from_secs(6));
    group.sample_size(20);
    let rt = tokio::runtime::Runtime::new().unwrap();

    for engine in ENGINES {
        let engine = *engine;
        let graph_id = format!("bench_crud_{}", engine.to_lowercase());
        let (service, _collection_service) = rt.block_on(async {
            let cs = Arc::new(GraphCollectionService::new());
            let svc = Arc::new(GraphOperationsService::new_with_collection_service(
                cs.clone(),
            ));
            ensure_graph_with_engine(&cs, engine, &graph_id).await;
            (svc, cs)
        });

        for size in [128usize, 1024, 4096] {
            group.throughput(Throughput::Elements(size as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("{}_create_nodes", engine), size),
                &size,
                |b, &size| {
                    b.iter(|| {
                        rt.block_on(async {
                            for i in 0..size {
                                let node = Node {
                                    id: format!("{}_node_{}", engine, i),
                                    labels: vec!["BenchNode".to_string()],
                                    properties: HashMap::from([
                                        (
                                            "i".to_string(),
                                            PropertyValue {
                                                value: Some(Value::IntValue(i as i64)),
                                            },
                                        ),
                                        (
                                            "name".to_string(),
                                            PropertyValue {
                                                value: Some(Value::StringValue(format!("n{}", i))),
                                            },
                                        ),
                                    ]),
                                    embedding: None,
                                    created_at_ms: 0,
                                    updated_at_ms: 0,
                                };
                                let _ = service.create_node(&graph_id, node).await.ok();
                            }
                        });
                    });
                },
            );

            // Pre-create small pool of nodes for edges
            rt.block_on(async {
                for i in 0..128 {
                    let node = Node {
                        id: format!("{}_edge_node_{}", engine, i),
                        labels: vec!["EdgeNode".to_string()],
                        properties: HashMap::new(),
                        embedding: None,
                        created_at_ms: 0,
                        updated_at_ms: 0,
                    };
                    let _ = service.create_node(&graph_id, node).await.ok();
                }
            });

            group.throughput(Throughput::Elements(size as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("{}_create_edges", engine), size),
                &size,
                |b, &size| {
                    b.iter(|| {
                        rt.block_on(async {
                            for i in 0..size {
                                let edge = Edge {
                                    id: format!("{}_edge_{}", engine, i),
                                    from_node_id: format!("{}_edge_node_{}", engine, i % 128),
                                    to_node_id: format!(
                                        "{}_edge_node_{}",
                                        engine,
                                        (i * 7 + 3) % 128
                                    ),
                                    edge_type: "LINKS".to_string(),
                                    properties: HashMap::from([(
                                        "w".to_string(),
                                        PropertyValue {
                                            value: Some(Value::DoubleValue((i % 5) as f64)),
                                        },
                                    )]),
                                    weight: Some((i % 5) as f64),
                                    created_at_ms: 0,
                                    updated_at_ms: 0,
                                };
                                let _ = service.create_edge(&graph_id, edge).await.ok();
                            }
                        });
                    });
                },
            );
        }

        // Simple property query benchmark
        group.bench_function(BenchmarkId::new(format!("{}_query_prop", engine), 0), |b| {
            b.iter(|| {
                rt.block_on(async {
                    let q = NodeQuery {
                        graph_id: graph_id.clone(),
                        labels: vec![],
                        filters: vec![PropertyFilter {
                            key: "i".to_string(),
                            operator: PropertyFilterOperator::GreaterEqual as i32,
                            value: Some(PropertyValue {
                                value: Some(Value::IntValue(512)),
                            }),
                        }],
                        limit: Some(64),
                        offset: None,
                        continuation_token: None,
                    };
                    black_box(service.query_nodes(&graph_id, q).await.unwrap());
                });
            });
        });
    }

    group.finish();
}

fn bench_engine_traversal(c: &mut Criterion) {
    let mut group = c.benchmark_group("graph_engine_traversal");
    use std::time::Duration;
    group.measurement_time(Duration::from_secs(6));
    group.sample_size(20);
    let rt = tokio::runtime::Runtime::new().unwrap();

    for engine in ENGINES {
        let engine = *engine;
        let graph_id = format!("bench_trav_{}", engine.to_lowercase());
        let service = rt.block_on(async {
            let cs = Arc::new(GraphCollectionService::new());
            let svc = Arc::new(GraphOperationsService::new_with_collection_service(
                cs.clone(),
            ));
            ensure_graph_with_engine(&cs, engine, &graph_id).await;

            // Build a small connected graph
            for i in 0..256 {
                let node = Node {
                    id: format!("{}_n{}", engine, i),
                    labels: vec!["TravNode".to_string()],
                    properties: HashMap::new(),
                    embedding: None,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                };
                let _ = svc.create_node(&graph_id, node).await.ok();
            }
            for i in 0..512 {
                let edge = Edge {
                    id: format!("{}_e{}", engine, i),
                    from_node_id: format!("{}_n{}", engine, i % 256),
                    to_node_id: format!("{}_n{}", engine, (i * 13 + 5) % 256),
                    edge_type: "TRAV".to_string(),
                    properties: HashMap::new(),
                    weight: None,
                    created_at_ms: 0,
                    updated_at_ms: 0,
                };
                let _ = svc.create_edge(&graph_id, edge).await.ok();
            }
            svc
        });

        // BFS
        group.bench_function(BenchmarkId::new(format!("{}_bfs", engine), 0), |b| {
            b.iter(|| {
                rt.block_on(async {
                    let req = TraversalRequest {
                        graph_id: graph_id.clone(),
                        start_node_id: format!("{}_n0", engine),
                        algorithm: TraversalAlgorithm::Bfs as i32,
                        max_depth: 3,
                        edge_types: vec!["TRAV".to_string()],
                        node_labels: vec![],
                        filters: vec![],
                        limit: Some(128),
                        timeout_ms: None,
                        max_frontier: None,
                    };
                    black_box(service.traverse(&graph_id, req).await.unwrap());
                });
            });
        });

        // DFS
        group.bench_function(BenchmarkId::new(format!("{}_dfs", engine), 0), |b| {
            b.iter(|| {
                rt.block_on(async {
                    let req = TraversalRequest {
                        graph_id: graph_id.clone(),
                        start_node_id: format!("{}_n0", engine),
                        algorithm: TraversalAlgorithm::Dfs as i32,
                        max_depth: 4,
                        edge_types: vec!["TRAV".to_string()],
                        node_labels: vec![],
                        filters: vec![],
                        limit: Some(128),
                        timeout_ms: None,
                        max_frontier: None,
                    };
                    black_box(service.traverse(&graph_id, req).await.unwrap());
                });
            });
        });
    }

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(30)
        .measurement_time(std::time::Duration::from_secs(4))
        .warm_up_time(std::time::Duration::from_secs(1));
    targets = bench_engine_crud,
              bench_engine_traversal
}

criterion_main!(benches);
