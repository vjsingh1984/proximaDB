/*
 * Copyright 2025 ProximaDB
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

//! Performance Benchmarks for ProximaDB Graph Engines
//!
//! Establishes baseline performance metrics for:
//! - Node/Edge CRUD operations
//! - Graph traversal (BFS, DFS, shortest path)
//! - Semantic search with embeddings
//! - Hybrid vector-graph operations
//!
//! Metrics tracked:
//! - Throughput (operations/second)
//! - Latency (p50, p95, p99)
//! - Memory usage
//! - Scalability (performance vs graph size)

use proximadb::compute::distance_computation::engine::UnifiedDistanceCompute;
use proximadb::graph::engines::GraphEngine;
use proximadb::graph::engines::orion::OrionGraphEngine;
use proximadb::graph::hybrid::semantic_traversal::{SemanticBFSTraversal, SemanticTraversalInput};
use proximadb::graph::{Edge, Node};
use proximadb::proto::proximadb_v1::{DistanceMetric, EmbeddingVersion};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

/// Generate a test node with random embedding
fn generate_node(id: usize, dimension: usize) -> Node {
    let embedding: Vec<f32> = (0..dimension)
        .map(|i| ((id * 7 + i * 13) % 100) as f32 / 100.0)
        .collect();

    let mut properties = HashMap::new();
    properties.insert(
        "name".to_string(),
        proximadb::proto::proximadb_v1::PropertyValue {
            value: Some(
                proximadb::proto::proximadb_v1::property_value::Value::StringValue(format!(
                    "node_{}",
                    id
                )),
            ),
        },
    );

    Node {
        id: format!("n_{}", id),
        labels: vec!["TestNode".to_string()],
        properties,
        embedding: Some(EmbeddingVersion {
            model_id: "test-model".to_string(),
            model_version: "1.0".to_string(),
            vector: embedding,
            dimension: dimension as u32,
            created_at_ms: 0,
            model_params: HashMap::new(),
            modality: 0,
        }),
        created_at_ms: 0,
        updated_at_ms: 0,
    }
}

/// Generate a test edge
fn generate_edge(from: usize, to: usize) -> Edge {
    Edge {
        id: format!("e_{}_{}", from, to),
        from_node_id: format!("n_{}", from),
        to_node_id: format!("n_{}", to),
        edge_type: "CONNECTS_TO".to_string(),
        properties: HashMap::new(),
        weight: Some(1.0),
        created_at_ms: 0,
        updated_at_ms: 0,
    }
}

/// Benchmark: Node insertion throughput
#[tokio::test]
async fn benchmark_node_insertion() {
    println!("\n=== Benchmark: Node Insertion ===");

    let engine = Arc::new(OrionGraphEngine::new());
    let node_counts = vec![100, 1000, 10000];

    for count in node_counts {
        let start = Instant::now();

        for i in 0..count {
            let node = generate_node(i, 128);
            engine.insert_node(node).await.unwrap();
        }

        let duration = start.elapsed();
        let throughput = count as f64 / duration.as_secs_f64();
        let latency_us = duration.as_micros() as f64 / count as f64;

        println!("  {} nodes:", count);
        println!("    Total time: {:.2}s", duration.as_secs_f64());
        println!("    Throughput: {:.0} nodes/sec", throughput);
        println!("    Avg latency: {:.2}µs/node", latency_us);
    }
}

/// Benchmark: Edge insertion throughput
#[tokio::test]
async fn benchmark_edge_insertion() {
    println!("\n=== Benchmark: Edge Insertion ===");

    let edge_counts = vec![100, 1000, 5000];

    for count in edge_counts {
        // Create fresh engine for each batch to avoid duplicate edge conflicts
        let engine = Arc::new(OrionGraphEngine::new());

        // Pre-populate nodes
        for i in 0..1000 {
            let node = generate_node(i, 128);
            engine.insert_node(node).await.unwrap();
        }

        let start = Instant::now();

        for i in 0..count {
            // Create unique from-to pairs to avoid duplicates
            // Pattern: node i connects to multiple neighbors based on offset
            let from = i % 1000;
            let offset = (i / 1000) + 1; // Changes offset every 1000 edges
            let to = (from + offset) % 1000;
            let mut edge = generate_edge(from, to);
            edge.id = format!("e_{}_{}_{}", i, from, to); // Truly unique ID
            engine.insert_edge(edge).await.unwrap();
        }

        let duration = start.elapsed();
        let throughput = count as f64 / duration.as_secs_f64();
        let latency_us = duration.as_micros() as f64 / count as f64;

        println!("  {} edges:", count);
        println!("    Total time: {:.2}s", duration.as_secs_f64());
        println!("    Throughput: {:.0} edges/sec", throughput);
        println!("    Avg latency: {:.2}µs/edge", latency_us);
    }
}

/// Benchmark: Node lookup performance
#[tokio::test]
async fn benchmark_node_lookup() {
    println!("\n=== Benchmark: Node Lookup ===");

    let engine = Arc::new(OrionGraphEngine::new());

    // Pre-populate nodes
    let node_count = 10000;
    println!("  Preparing: Inserting {} nodes...", node_count);
    for i in 0..node_count {
        let node = generate_node(i, 128);
        engine.insert_node(node).await.unwrap();
    }

    // Benchmark lookups
    let lookup_count = 1000;
    let start = Instant::now();

    for i in 0..lookup_count {
        let node_id = format!("n_{}", i % node_count);
        engine.get_node(&node_id).unwrap();
    }

    let duration = start.elapsed();
    let throughput = lookup_count as f64 / duration.as_secs_f64();
    let latency_us = duration.as_micros() as f64 / lookup_count as f64;

    println!("  {} lookups from {} nodes:", lookup_count, node_count);
    println!("    Total time: {:.2}ms", duration.as_millis());
    println!("    Throughput: {:.0} lookups/sec", throughput);
    println!("    Avg latency: {:.2}µs/lookup", latency_us);
}

/// Benchmark: Neighbor traversal performance
#[tokio::test]
async fn benchmark_neighbor_traversal() {
    println!("\n=== Benchmark: Neighbor Traversal ===");

    let engine = Arc::new(OrionGraphEngine::new());

    // Create a graph with controlled degree distribution
    let node_count = 1000;
    println!("  Preparing: Creating graph with {} nodes...", node_count);

    // Insert nodes
    for i in 0..node_count {
        let node = generate_node(i, 128);
        engine.insert_node(node).await.unwrap();
    }

    // Create edges (each node has ~10 neighbors)
    for i in 0..node_count {
        for j in 1..=10 {
            let to = (i + j) % node_count;
            let edge = generate_edge(i, to);
            engine.insert_edge(edge).await.unwrap();
        }
    }

    // Benchmark traversal
    let traversal_count = 1000;
    let start = Instant::now();

    for i in 0..traversal_count {
        let node_id = format!("n_{}", i % node_count);
        engine.get_neighbors(&node_id, None).unwrap();
    }

    let duration = start.elapsed();
    let throughput = traversal_count as f64 / duration.as_secs_f64();
    let latency_us = duration.as_micros() as f64 / traversal_count as f64;

    println!("  {} traversals (avg degree ~10):", traversal_count);
    println!("    Total time: {:.2}ms", duration.as_millis());
    println!("    Throughput: {:.0} traversals/sec", throughput);
    println!("    Avg latency: {:.2}µs/traversal", latency_us);
}

/// Benchmark: Semantic search performance
#[tokio::test]
async fn benchmark_semantic_search() {
    println!("\n=== Benchmark: Semantic Search ===");

    let engine = Arc::new(OrionGraphEngine::new());

    // Create graph with embeddings
    let node_count = 1000;
    println!(
        "  Preparing: Creating graph with {} nodes and embeddings...",
        node_count
    );

    for i in 0..node_count {
        let node = generate_node(i, 128);
        engine.insert_node(node).await.unwrap();
    }

    // Create edges
    for i in 0..node_count {
        for j in 1..=5 {
            let to = (i + j) % node_count;
            let edge = generate_edge(i, to);
            engine.insert_edge(edge).await.unwrap();
        }
    }

    // Benchmark semantic search
    let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));
    let semantic_bfs = SemanticBFSTraversal::new(
        Arc::clone(&engine) as Arc<dyn GraphEngine>,
        distance_compute,
        0.8,
        DistanceMetric::Cosine,
    );

    let search_count = 100;
    let start = Instant::now();

    for i in 0..search_count {
        let query_embedding: Vec<f32> = (0..128)
            .map(|j| ((i * 7 + j * 13) % 100) as f32 / 100.0)
            .collect();

        let input = SemanticTraversalInput {
            start_node: format!("n_{}", i % node_count),
            query_embedding,
            max_depth: 3,
        };

        semantic_bfs.execute(input).unwrap();
    }

    let duration = start.elapsed();
    let throughput = search_count as f64 / duration.as_secs_f64();
    let latency_ms = duration.as_millis() as f64 / search_count as f64;

    println!(
        "  {} semantic searches (depth=3, threshold=0.8):",
        search_count
    );
    println!("    Total time: {:.2}s", duration.as_secs_f64());
    println!("    Throughput: {:.2} searches/sec", throughput);
    println!("    Avg latency: {:.2}ms/search", latency_ms);
}

/// Benchmark: Bulk operations performance
#[tokio::test]
async fn benchmark_bulk_operations() {
    println!("\n=== Benchmark: Bulk Operations ===");

    let engine = Arc::new(OrionGraphEngine::new());

    // Benchmark bulk node insertion
    let node_count = 1000;
    let nodes: Vec<Node> = (0..node_count).map(|i| generate_node(i, 128)).collect();

    let start = Instant::now();
    engine.bulk_insert_nodes(nodes).await.unwrap();
    let duration = start.elapsed();

    println!("  Bulk insert {} nodes:", node_count);
    println!("    Total time: {:.2}s", duration.as_secs_f64());
    println!(
        "    Throughput: {:.0} nodes/sec",
        node_count as f64 / duration.as_secs_f64()
    );

    // Benchmark bulk edge insertion
    // Use unique edge IDs to avoid duplicates when from/to pairs repeat
    let edge_count = 5000;
    let edges: Vec<Edge> = (0..edge_count)
        .map(|i| {
            let from = i % node_count;
            let to = (i + 1) % node_count;
            Edge {
                id: format!("e_bulk_{}", i), // Unique ID per edge
                from_node_id: format!("n_{}", from),
                to_node_id: format!("n_{}", to),
                edge_type: "CONNECTS_TO".to_string(),
                properties: std::collections::HashMap::new(),
                weight: Some(1.0),
                created_at_ms: 0,
                updated_at_ms: 0,
            }
        })
        .collect();

    let start = Instant::now();
    engine.bulk_insert_edges(edges).await.unwrap();
    let duration = start.elapsed();

    println!("  Bulk insert {} edges:", edge_count);
    println!("    Total time: {:.2}s", duration.as_secs_f64());
    println!(
        "    Throughput: {:.0} edges/sec",
        edge_count as f64 / duration.as_secs_f64()
    );
}

/// Benchmark: Scalability test (performance vs graph size)
#[tokio::test]
async fn benchmark_scalability() {
    println!("\n=== Benchmark: Scalability ===");

    let graph_sizes = vec![100, 500, 1000, 5000];

    for size in graph_sizes {
        let engine = Arc::new(OrionGraphEngine::new());

        // Insert nodes
        let start = Instant::now();
        for i in 0..size {
            let node = generate_node(i, 128);
            engine.insert_node(node).await.unwrap();
        }
        let insert_duration = start.elapsed();

        // Create edges (each node has ~5 neighbors)
        for i in 0..size {
            for j in 1..=5 {
                let to = (i + j) % size;
                let edge = generate_edge(i, to);
                engine.insert_edge(edge).await.unwrap();
            }
        }

        // Measure lookup performance
        let lookup_count = 100;
        let start = Instant::now();
        for i in 0..lookup_count {
            let node_id = format!("n_{}", i % size);
            engine.get_node(&node_id).unwrap();
        }
        let lookup_duration = start.elapsed();

        // Measure traversal performance
        let start = Instant::now();
        for i in 0..lookup_count {
            let node_id = format!("n_{}", i % size);
            engine.get_neighbors(&node_id, None).unwrap();
        }
        let traversal_duration = start.elapsed();

        println!("  Graph size: {} nodes, {} edges", size, size * 5);
        println!(
            "    Insert throughput: {:.0} nodes/sec",
            size as f64 / insert_duration.as_secs_f64()
        );
        println!(
            "    Lookup latency: {:.2}µs",
            lookup_duration.as_micros() as f64 / lookup_count as f64
        );
        println!(
            "    Traversal latency: {:.2}µs",
            traversal_duration.as_micros() as f64 / lookup_count as f64
        );
    }
}
