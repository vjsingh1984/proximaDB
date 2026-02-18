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

//! # Multi-Modal Query Execution Benchmarks
//!
//! Performance benchmarks for ProximaDB's multi-modal query capabilities,
//! measuring throughput and latency for cross-model queries combining:
//! - Vector similarity search
//! - Graph traversal
//! - Result fusion strategies (RRF, intersection, union)
//!
//! ## Benchmark Categories
//!
//! 1. **Baseline measurements**: Individual model query performance
//! 2. **Combined queries**: Vector + Graph combined execution
//! 3. **Cross-model joins**: Performance of joining results across models
//! 4. **Query planner overhead**: Cost of query parsing and optimization
//! 5. **Fusion strategies**: Comparison of RRF, intersection, and union fusion

mod common;

use common::benchmark_utils::print_system_info;
use common::{EmbeddingGenerator, EmbeddingModel};
use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use proximadb::{
    compute::distance_computation::{DistanceMetric, engine::UnifiedDistanceCompute},
    graph::{Edge, Node, PropertyValue, service::GraphOperationsService},
    proto::proximadb_v1::{
        CreateGraphRequest, TraversalAlgorithm, TraversalRequest, property_value::Value,
    },
    query::federated::FederatedParser,
    query::unified::{
        DataModel, FusionStrategy, ResultFuser, UnifiedRecord, fusion::SubQueryResult,
    },
    services::graph_collection::GraphCollectionService,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

// ============================================================================
// CONSTANTS AND CONFIGURATION
// ============================================================================

/// Default dimension for vector embeddings
const VECTOR_DIMENSION: usize = 768;

/// Default graph ID for benchmarks
const BENCHMARK_GRAPH_ID: &str = "benchmark_multimodal_graph";

/// Standard dataset sizes for benchmarks
const DATASET_SIZES: &[usize] = &[1000, 5000, 10000];

/// Standard top-k values for search
#[allow(dead_code)]
const TOP_K_VALUES: &[usize] = &[10, 50, 100];

// ============================================================================
// DATA GENERATION UTILITIES
// ============================================================================

/// Generate synthetic vector records for benchmarking
fn generate_vectors(count: usize, dimension: usize) -> Vec<(String, Vec<f32>)> {
    let mut generator = EmbeddingGenerator::new(EmbeddingModel::Bert);
    (0..count)
        .map(|i| {
            let vector = generator.generate(dimension);
            (format!("vec_{}", i), vector)
        })
        .collect()
}

/// Generate synthetic graph nodes with optional embeddings
fn generate_graph_nodes(count: usize) -> Vec<Node> {
    (0..count)
        .map(|i| {
            let mut properties = HashMap::new();
            properties.insert(
                "index".to_string(),
                PropertyValue {
                    value: Some(Value::IntValue(i as i64)),
                },
            );
            properties.insert(
                "name".to_string(),
                PropertyValue {
                    value: Some(Value::StringValue(format!("Entity_{}", i))),
                },
            );
            properties.insert(
                "category".to_string(),
                PropertyValue {
                    value: Some(Value::StringValue(format!("cat_{}", i % 10))),
                },
            );

            Node {
                id: format!("node_{}", i),
                labels: vec!["BenchmarkNode".to_string()],
                properties,
                embedding: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            }
        })
        .collect()
}

/// Generate synthetic graph edges with varying connectivity patterns
fn generate_graph_edges(node_count: usize, edge_factor: usize) -> Vec<Edge> {
    let edge_count = node_count * edge_factor;
    (0..edge_count)
        .map(|i| {
            let from_idx = i % node_count;
            let to_idx = (i * 7 + 3) % node_count; // Create pseudo-random connections

            Edge {
                id: format!("edge_{}", i),
                from_node_id: format!("node_{}", from_idx),
                to_node_id: format!("node_{}", to_idx),
                edge_type: "RELATES_TO".to_string(),
                properties: HashMap::from([(
                    "weight".to_string(),
                    PropertyValue {
                        value: Some(Value::DoubleValue((i % 10) as f64 * 0.1)),
                    },
                )]),
                weight: Some((i % 10) as f64 * 0.1),
                created_at_ms: 0,
                updated_at_ms: 0,
            }
        })
        .collect()
}

/// Generate synthetic unified records for fusion benchmarks
fn generate_unified_records(count: usize, model: DataModel, base_score: f64) -> Vec<UnifiedRecord> {
    (0..count)
        .map(|i| UnifiedRecord {
            id: format!("record_{}", i),
            source_model: model.clone(),
            data: serde_json::json!({
                "id": format!("record_{}", i),
                "value": i,
                "category": format!("cat_{}", i % 5),
            }),
            score: Some(base_score - (i as f64 * 0.01)),
            metadata: HashMap::from([
                ("source".to_string(), format!("{:?}", model)),
                ("rank".to_string(), i.to_string()),
            ]),
        })
        .collect()
}

/// Create sub-query results for fusion benchmarks
fn create_sub_query_results(sizes: &[usize], models: &[DataModel]) -> Vec<SubQueryResult> {
    sizes
        .iter()
        .zip(models.iter())
        .enumerate()
        .map(|(idx, (&size, model))| {
            let records = generate_unified_records(size, model.clone(), 1.0 - (idx as f64 * 0.1));
            SubQueryResult {
                source_model: model.clone(),
                records_returned: records.len() as u64,
                records,
                total_count: Some(size as u64),
                execution_time_us: 1000 + (idx as u64 * 100),
                records_scanned: size as u64 * 10,
            }
        })
        .collect()
}

// ============================================================================
// BENCHMARK 1: VECTOR-ONLY SEARCH BASELINE
// ============================================================================

/// Benchmark pure vector similarity search performance
fn bench_vector_search_baseline(c: &mut Criterion) {
    print_system_info("Multi-Modal Query Benchmarks");

    let mut group = c.benchmark_group("vector_search_baseline");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(50);
    group.warm_up_time(Duration::from_secs(2));

    let distance_compute = UnifiedDistanceCompute::new(DistanceMetric::Cosine);

    for &dataset_size in DATASET_SIZES {
        let vectors = generate_vectors(dataset_size, VECTOR_DIMENSION);
        let query_vector = EmbeddingGenerator::new(EmbeddingModel::Bert).generate(VECTOR_DIMENSION);

        group.throughput(Throughput::Elements(dataset_size as u64));

        for &top_k in &[10, 50] {
            group.bench_with_input(
                BenchmarkId::new(format!("size_{}_top{}", dataset_size, top_k), dataset_size),
                &dataset_size,
                |b, _| {
                    b.iter(|| {
                        // Compute distances to all vectors
                        let mut distances: Vec<(usize, f32)> = vectors
                            .iter()
                            .enumerate()
                            .map(|(idx, (_, vec))| {
                                let result = distance_compute.calculate_distance(
                                    &query_vector,
                                    vec,
                                    &DistanceMetric::Cosine,
                                );
                                (idx, result.distance)
                            })
                            .collect();

                        // Sort and take top-k
                        distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
                        distances.truncate(top_k);

                        black_box(distances)
                    })
                },
            );
        }
    }

    group.finish();
}

// ============================================================================
// BENCHMARK 2: GRAPH TRAVERSAL BASELINE
// ============================================================================

/// Benchmark pure graph traversal performance
fn bench_graph_traversal_baseline(c: &mut Criterion) {
    let mut group = c.benchmark_group("graph_traversal_baseline");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(40);
    group.warm_up_time(Duration::from_secs(2));

    let runtime = tokio::runtime::Runtime::new().unwrap();

    // Setup graph service
    let (service, _collection_service) = runtime.block_on(async {
        let collection_service = Arc::new(GraphCollectionService::new());
        let service = Arc::new(GraphOperationsService::new_with_collection_service(
            collection_service.clone(),
        ));

        // Create benchmark graph
        let existing_graphs = collection_service.list_graphs().await.unwrap_or_default();
        if !existing_graphs
            .iter()
            .any(|g| g.graph_id == BENCHMARK_GRAPH_ID)
        {
            let request = CreateGraphRequest {
                graph_id: BENCHMARK_GRAPH_ID.to_string(),
                name: Some("Multi-Modal Benchmark Graph".to_string()),
                description: Some("Graph for multi-modal query benchmarking".to_string()),
                schema: None,
                storage_config: None,
                engine_config: None,
                access_control: None,
            };
            let _ = collection_service.create_graph(request).await;
        }

        (service, collection_service)
    });

    // Populate graph with test data
    let node_count = 1000;
    let edge_factor = 3;

    runtime.block_on(async {
        let nodes = generate_graph_nodes(node_count);
        for node in nodes {
            let _ = service.create_node(BENCHMARK_GRAPH_ID, node).await;
        }

        let edges = generate_graph_edges(node_count, edge_factor);
        for edge in edges {
            let _ = service.create_edge(BENCHMARK_GRAPH_ID, edge).await;
        }
    });

    // Benchmark BFS traversal at different depths
    for max_depth in [2, 3, 4] {
        group.bench_with_input(
            BenchmarkId::new("bfs_depth", max_depth),
            &max_depth,
            |b, &depth| {
                b.iter(|| {
                    runtime.block_on(async {
                        let request = TraversalRequest {
                            graph_id: BENCHMARK_GRAPH_ID.to_string(),
                            start_node_id: "node_0".to_string(),
                            algorithm: TraversalAlgorithm::Bfs as i32,
                            max_depth: depth,
                            edge_types: vec![],
                            node_labels: vec![],
                            filters: vec![],
                            limit: Some(100),
                            timeout_ms: None,
                            max_frontier: None,
                        };
                        black_box(service.traverse(BENCHMARK_GRAPH_ID, request).await)
                    })
                })
            },
        );
    }

    // Benchmark DFS traversal
    for max_depth in [2, 3, 4] {
        group.bench_with_input(
            BenchmarkId::new("dfs_depth", max_depth),
            &max_depth,
            |b, &depth| {
                b.iter(|| {
                    runtime.block_on(async {
                        let request = TraversalRequest {
                            graph_id: BENCHMARK_GRAPH_ID.to_string(),
                            start_node_id: "node_0".to_string(),
                            algorithm: TraversalAlgorithm::Dfs as i32,
                            max_depth: depth,
                            edge_types: vec![],
                            node_labels: vec![],
                            filters: vec![],
                            limit: Some(100),
                            timeout_ms: None,
                            max_frontier: None,
                        };
                        black_box(service.traverse(BENCHMARK_GRAPH_ID, request).await)
                    })
                })
            },
        );
    }

    group.finish();
}

// ============================================================================
// BENCHMARK 3: VECTOR + GRAPH COMBINED QUERY
// ============================================================================

/// Benchmark combined vector search + graph traversal
fn bench_vector_graph_combined(c: &mut Criterion) {
    let mut group = c.benchmark_group("vector_graph_combined");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(40);
    group.warm_up_time(Duration::from_secs(2));

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let distance_compute = UnifiedDistanceCompute::new(DistanceMetric::Cosine);

    // Setup graph service
    let (service, _collection_service) = runtime.block_on(async {
        let collection_service = Arc::new(GraphCollectionService::new());
        let service = Arc::new(GraphOperationsService::new_with_collection_service(
            collection_service.clone(),
        ));
        (service, collection_service)
    });

    // Generate combined dataset
    let dataset_size = 1000;
    let vectors = generate_vectors(dataset_size, VECTOR_DIMENSION);
    let query_vector = EmbeddingGenerator::new(EmbeddingModel::Bert).generate(VECTOR_DIMENSION);

    // Benchmark: Vector search followed by graph traversal on top results
    group.bench_function("vector_then_graph", |b| {
        b.iter(|| {
            // Phase 1: Vector search
            let mut distances: Vec<(usize, f32)> = vectors
                .iter()
                .enumerate()
                .map(|(idx, (_, vec))| {
                    let result = distance_compute.calculate_distance(
                        &query_vector,
                        vec,
                        &DistanceMetric::Cosine,
                    );
                    (idx, result.distance)
                })
                .collect();

            distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
            let top_k_indices: Vec<usize> =
                distances.iter().take(10).map(|(idx, _)| *idx).collect();

            // Phase 2: Graph traversal from top vector results
            let graph_results: Vec<_> = runtime.block_on(async {
                let mut results = Vec::new();
                for idx in &top_k_indices {
                    let request = TraversalRequest {
                        graph_id: BENCHMARK_GRAPH_ID.to_string(),
                        start_node_id: format!("node_{}", idx),
                        algorithm: TraversalAlgorithm::Bfs as i32,
                        max_depth: 2,
                        edge_types: vec![],
                        node_labels: vec![],
                        filters: vec![],
                        limit: Some(10),
                        timeout_ms: None,
                        max_frontier: None,
                    };
                    if let Ok(result) = service.traverse(BENCHMARK_GRAPH_ID, request).await {
                        results.push(result);
                    }
                }
                results
            });

            black_box((top_k_indices, graph_results))
        })
    });

    // Benchmark: Parallel vector search and graph traversal
    group.bench_function("parallel_vector_graph", |b| {
        b.iter(|| {
            runtime.block_on(async {
                // Run vector search and graph traversal in parallel
                let vector_future = async {
                    let mut distances: Vec<(usize, f32)> = vectors
                        .iter()
                        .enumerate()
                        .map(|(idx, (_, vec))| {
                            let result = distance_compute.calculate_distance(
                                &query_vector,
                                vec,
                                &DistanceMetric::Cosine,
                            );
                            (idx, result.distance)
                        })
                        .collect();

                    distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
                    distances.truncate(10);
                    distances
                };

                let graph_future = async {
                    let request = TraversalRequest {
                        graph_id: BENCHMARK_GRAPH_ID.to_string(),
                        start_node_id: "node_0".to_string(),
                        algorithm: TraversalAlgorithm::Bfs as i32,
                        max_depth: 3,
                        edge_types: vec![],
                        node_labels: vec![],
                        filters: vec![],
                        limit: Some(50),
                        timeout_ms: None,
                        max_frontier: None,
                    };
                    service.traverse(BENCHMARK_GRAPH_ID, request).await
                };

                let (vector_results, graph_results) = tokio::join!(vector_future, graph_future);

                black_box((vector_results, graph_results))
            })
        })
    });

    group.finish();
}

// ============================================================================
// BENCHMARK 4: CROSS-MODEL JOIN PERFORMANCE
// ============================================================================

/// Benchmark cross-model join operations
fn bench_cross_model_join(c: &mut Criterion) {
    let mut group = c.benchmark_group("cross_model_join");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(50);
    group.warm_up_time(Duration::from_secs(2));

    // Test different result set sizes for joining
    let join_sizes = [(100, 100), (100, 1000), (1000, 1000), (1000, 10000)];

    for (left_size, right_size) in join_sizes {
        // Generate left (vector) and right (graph) result sets
        let left_records = generate_unified_records(left_size, DataModel::Vector, 1.0);
        let right_records = generate_unified_records(right_size, DataModel::Graph, 0.9);

        // Create overlapping IDs (50% overlap)
        let _left_ids: std::collections::HashSet<_> =
            left_records.iter().map(|r| r.id.clone()).collect();

        group.bench_with_input(
            BenchmarkId::new(
                format!("hash_join_{}x{}", left_size, right_size),
                left_size * right_size,
            ),
            &(left_size, right_size),
            |b, _| {
                b.iter(|| {
                    // Simulate hash join: build hash table on smaller set, probe with larger
                    let hash_table: HashMap<&String, &UnifiedRecord> =
                        left_records.iter().map(|r| (&r.id, r)).collect();

                    let joined: Vec<(&UnifiedRecord, &UnifiedRecord)> = right_records
                        .iter()
                        .filter_map(|right| hash_table.get(&right.id).map(|left| (*left, right)))
                        .collect();

                    black_box(joined)
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new(
                format!("nested_loop_join_{}x{}", left_size, right_size),
                left_size * right_size,
            ),
            &(left_size, right_size),
            |b, _| {
                b.iter(|| {
                    // Nested loop join (baseline for comparison)
                    let joined: Vec<(&UnifiedRecord, &UnifiedRecord)> = left_records
                        .iter()
                        .flat_map(|left| {
                            right_records
                                .iter()
                                .filter(|right| left.id == right.id)
                                .map(move |right| (left, right))
                        })
                        .collect();

                    black_box(joined)
                })
            },
        );
    }

    group.finish();
}

// ============================================================================
// BENCHMARK 5: QUERY PLANNER OVERHEAD
// ============================================================================

/// Benchmark query parsing and optimization overhead
fn bench_query_planner_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("query_planner_overhead");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(100);
    group.warm_up_time(Duration::from_secs(1));

    let parser = FederatedParser::new();

    // Test queries of varying complexity
    let queries = vec![
        // Simple vector search
        (
            "simple_vector",
            "SELECT * FROM VECTOR_SEARCH('embeddings', '[0.1, 0.2, 0.3]', 10)",
        ),
        // Simple graph query
        (
            "simple_graph",
            "SELECT * FROM GRAPH_QUERY('MATCH (a:Person)-[:KNOWS]->(b) RETURN b.name')",
        ),
        // Cross-model: vector + filter
        (
            "vector_with_filter",
            "SELECT * FROM VECTOR_SEARCH('products', '[0.1, 0.2]', 100) WHERE price < 500",
        ),
        // Cross-model: vector + graph
        (
            "vector_graph_join",
            "SELECT v.*, g.* FROM VECTOR_SEARCH('products', '[0.1, 0.2]', 10) v \
             JOIN LATERAL GRAPH_QUERY('MATCH (p:Product {id: \"' || v.id || '\"})-[:RELATED]->(r) RETURN r') g ON true",
        ),
        // Complex federated query
        (
            "complex_federated",
            "SELECT u.name, v.product_id, v.score, d.review_text \
             FROM users u \
             JOIN LATERAL VECTOR_SEARCH('products', u.preference_vector, 10) v ON true \
             JOIN LATERAL DOCUMENT_QUERY('reviews', concat('product_id = \"', v.product_id, '\"')) d ON true",
        ),
    ];

    // Benchmark parsing overhead
    for (name, query) in &queries {
        group.bench_with_input(BenchmarkId::new("parse", *name), query, |b, &q| {
            b.iter(|| {
                let parsed = parser.parse(q);
                black_box(parsed)
            })
        });
    }

    // Benchmark full parsing cycle breakdown with timing
    group.bench_function("full_parsing_breakdown", |b| {
        let complex_query = "SELECT v.*, g.* FROM VECTOR_SEARCH('products', '[0.1, 0.2]', 10) v \
             JOIN LATERAL GRAPH_QUERY('MATCH (p:Product)-[:RELATED]->(r) RETURN r') g ON true";

        b.iter(|| {
            let start = Instant::now();

            // Parse
            let parse_start = Instant::now();
            let parsed = parser.parse(complex_query);
            let parse_time = parse_start.elapsed();

            // Validate and analyze (simulate optimizer work)
            let analyze_start = Instant::now();
            if let Ok(ref p) = parsed {
                // Analyze query structure
                let _has_vector = p.extensions.iter().any(|e| {
                    matches!(
                        e,
                        proximadb::query::federated::parser::SqlExtension::VectorSearch { .. }
                    )
                });
                let _has_graph = p.extensions.iter().any(|e| {
                    matches!(
                        e,
                        proximadb::query::federated::parser::SqlExtension::GraphQuery { .. }
                    )
                });
                let _is_cross_model = p.is_cross_model_join;
            }
            let analyze_time = analyze_start.elapsed();

            let total_time = start.elapsed();

            black_box((parse_time, analyze_time, total_time))
        })
    });

    // Benchmark repeated parsing (cache-warm scenario)
    group.bench_function("repeated_parsing", |b| {
        let queries_to_parse = vec![
            "SELECT * FROM VECTOR_SEARCH('products', '[0.1, 0.2]', 10)",
            "SELECT * FROM GRAPH_QUERY('MATCH (a)-[:KNOWS]->(b) RETURN b')",
            "SELECT * FROM DOCUMENT_QUERY('orders', 'status = \"pending\"')",
        ];

        b.iter(|| {
            let results: Vec<_> = queries_to_parse.iter().map(|q| parser.parse(q)).collect();
            black_box(results)
        })
    });

    group.finish();
}

// ============================================================================
// BENCHMARK 6: RESULT FUSION STRATEGIES
// ============================================================================

/// Benchmark different result fusion strategies
fn bench_fusion_strategies(c: &mut Criterion) {
    let mut group = c.benchmark_group("fusion_strategies");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(50);
    group.warm_up_time(Duration::from_secs(2));

    // Test with different result set configurations
    let configurations = vec![
        (
            "small_2way",
            vec![100, 100],
            vec![DataModel::Vector, DataModel::Graph],
        ),
        (
            "medium_2way",
            vec![1000, 1000],
            vec![DataModel::Vector, DataModel::Graph],
        ),
        (
            "large_2way",
            vec![10000, 10000],
            vec![DataModel::Vector, DataModel::Graph],
        ),
        (
            "3way_mixed",
            vec![500, 1000, 2000],
            vec![DataModel::Vector, DataModel::Graph, DataModel::Document],
        ),
        (
            "4way_full",
            vec![500, 500, 500, 500],
            vec![
                DataModel::Vector,
                DataModel::Graph,
                DataModel::Document,
                DataModel::Observability,
            ],
        ),
    ];

    for (config_name, sizes, models) in configurations {
        let sub_results = create_sub_query_results(&sizes, &models);

        // Benchmark RRF fusion (k=60)
        {
            let fuser = ResultFuser::new(FusionStrategy::ReciprocalRankFusion { k: 60 });
            let results_clone = sub_results.clone();

            group.bench_with_input(
                BenchmarkId::new("rrf_k60", config_name),
                &config_name,
                |b, _| {
                    b.iter(|| {
                        let result = fuser.fuse(
                            results_clone.clone(),
                            &FusionStrategy::ReciprocalRankFusion { k: 60 },
                        );
                        black_box(result)
                    })
                },
            );
        }

        // Benchmark RRF with different k values
        for k in [20, 60, 100] {
            let fuser = ResultFuser::new(FusionStrategy::ReciprocalRankFusion { k });
            let results_clone = sub_results.clone();

            group.bench_with_input(
                BenchmarkId::new(format!("rrf_k{}", k), config_name),
                &config_name,
                |b, _| {
                    b.iter(|| {
                        let result = fuser.fuse(
                            results_clone.clone(),
                            &FusionStrategy::ReciprocalRankFusion { k },
                        );
                        black_box(result)
                    })
                },
            );
        }

        // Benchmark intersection fusion
        {
            let fuser = ResultFuser::new(FusionStrategy::Intersection);
            let results_clone = sub_results.clone();

            group.bench_with_input(
                BenchmarkId::new("intersection", config_name),
                &config_name,
                |b, _| {
                    b.iter(|| {
                        let result =
                            fuser.fuse(results_clone.clone(), &FusionStrategy::Intersection);
                        black_box(result)
                    })
                },
            );
        }

        // Benchmark union fusion
        {
            let fuser = ResultFuser::new(FusionStrategy::Union);
            let results_clone = sub_results.clone();

            group.bench_with_input(
                BenchmarkId::new("union", config_name),
                &config_name,
                |b, _| {
                    b.iter(|| {
                        let result = fuser.fuse(results_clone.clone(), &FusionStrategy::Union);
                        black_box(result)
                    })
                },
            );
        }

        // Benchmark ranked fusion with weights
        {
            let mut weights = HashMap::new();
            weights.insert(DataModel::Vector, 1.5);
            weights.insert(DataModel::Graph, 1.0);
            weights.insert(DataModel::Document, 0.8);
            weights.insert(DataModel::Observability, 0.5);

            let fuser = ResultFuser::new(FusionStrategy::RankedFusion {
                weights: weights.clone(),
                normalize: true,
            });
            let results_clone = sub_results.clone();

            group.bench_with_input(
                BenchmarkId::new("ranked_weighted", config_name),
                &config_name,
                |b, _| {
                    b.iter(|| {
                        let result = fuser.fuse(
                            results_clone.clone(),
                            &FusionStrategy::RankedFusion {
                                weights: weights.clone(),
                                normalize: true,
                            },
                        );
                        black_box(result)
                    })
                },
            );
        }

        // Benchmark FirstWithFilter fusion
        {
            let fuser = ResultFuser::new(FusionStrategy::FirstWithFilter);
            let results_clone = sub_results.clone();

            group.bench_with_input(
                BenchmarkId::new("first_with_filter", config_name),
                &config_name,
                |b, _| {
                    b.iter(|| {
                        let result =
                            fuser.fuse(results_clone.clone(), &FusionStrategy::FirstWithFilter);
                        black_box(result)
                    })
                },
            );
        }
    }

    group.finish();
}

// ============================================================================
// BENCHMARK 7: FUSION STRATEGY COMPARISON (SCALING)
// ============================================================================

/// Benchmark how fusion strategies scale with result size
fn bench_fusion_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("fusion_scaling");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(40);
    group.warm_up_time(Duration::from_secs(2));

    let result_sizes = [100, 500, 1000, 5000, 10000];

    for &size in &result_sizes {
        let sub_results =
            create_sub_query_results(&[size, size], &[DataModel::Vector, DataModel::Graph]);

        group.throughput(Throughput::Elements(size as u64 * 2));

        // RRF scaling
        {
            let fuser = ResultFuser::new(FusionStrategy::ReciprocalRankFusion { k: 60 });
            let results_clone = sub_results.clone();

            group.bench_with_input(BenchmarkId::new("rrf", size), &size, |b, _| {
                b.iter(|| {
                    let result = fuser.fuse(
                        results_clone.clone(),
                        &FusionStrategy::ReciprocalRankFusion { k: 60 },
                    );
                    black_box(result)
                })
            });
        }

        // Intersection scaling
        {
            let fuser = ResultFuser::new(FusionStrategy::Intersection);
            let results_clone = sub_results.clone();

            group.bench_with_input(BenchmarkId::new("intersection", size), &size, |b, _| {
                b.iter(|| {
                    let result = fuser.fuse(results_clone.clone(), &FusionStrategy::Intersection);
                    black_box(result)
                })
            });
        }

        // Union scaling
        {
            let fuser = ResultFuser::new(FusionStrategy::Union);
            let results_clone = sub_results.clone();

            group.bench_with_input(BenchmarkId::new("union", size), &size, |b, _| {
                b.iter(|| {
                    let result = fuser.fuse(results_clone.clone(), &FusionStrategy::Union);
                    black_box(result)
                })
            });
        }
    }

    group.finish();
}

// ============================================================================
// CRITERION CONFIGURATION
// ============================================================================

criterion_group! {
    name = baseline_benchmarks;
    config = Criterion::default()
        .sample_size(50)
        .measurement_time(Duration::from_secs(10))
        .warm_up_time(Duration::from_secs(2));
    targets = bench_vector_search_baseline,
              bench_graph_traversal_baseline
}

criterion_group! {
    name = combined_benchmarks;
    config = Criterion::default()
        .sample_size(40)
        .measurement_time(Duration::from_secs(10))
        .warm_up_time(Duration::from_secs(2));
    targets = bench_vector_graph_combined,
              bench_cross_model_join
}

criterion_group! {
    name = planner_benchmarks;
    config = Criterion::default()
        .sample_size(100)
        .measurement_time(Duration::from_secs(5))
        .warm_up_time(Duration::from_secs(1));
    targets = bench_query_planner_overhead
}

criterion_group! {
    name = fusion_benchmarks;
    config = Criterion::default()
        .sample_size(50)
        .measurement_time(Duration::from_secs(10))
        .warm_up_time(Duration::from_secs(2));
    targets = bench_fusion_strategies,
              bench_fusion_scaling
}

criterion_main!(
    baseline_benchmarks,
    combined_benchmarks,
    planner_benchmarks,
    fusion_benchmarks
);
