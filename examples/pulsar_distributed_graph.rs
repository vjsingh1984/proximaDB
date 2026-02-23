//! PULSAR Distributed Graph Engine Example
//!
//! This example demonstrates how to use PULSAR for distributed graph storage.
//!
//! **WARNING**: PULSAR is experimental and not production-ready.
//!
//! Run with:
//! ```bash
//! cargo run --release --example pulsar_distributed_graph --features distributed-graph
//! ```

use proximadb::graph::engines::pulsar::{PulsarGraphEngine, PulsarConfig, ConsistencyLevel};
use proximadb::graph::{Node, Edge, PropertyValue};
use std::collections::HashMap;
use std::time::Instant;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== PULSAR Distributed Graph Engine Example ===");
    println!();

    // Check if PULSAR feature is enabled
    #[cfg(not(feature = "distributed-graph"))]
    {
        eprintln!("ERROR: PULSAR engine requires 'distributed-graph' feature.");
        eprintln!("Build with: cargo build --features distributed-graph");
        return Ok(());
    }

    #[cfg(feature = "distributed-graph")]
    {
        // Configure PULSAR engine
        let config = PulsarConfig {
            shard_count: 4, // Use 4 shards for this example
            replication_factor: 1,
            consistency_level: ConsistencyLevel::Quorum,
            cross_shard_optimization: true,
            max_concurrent_queries: 100,
        };

        println!("Creating PULSAR engine with configuration:");
        println!("  Shard count: {}", config.shard_count);
        println!("  Replication factor: {}", config.replication_factor);
        println!("  Consistency level: {:?}", config.consistency_level);
        println!("  Cross-shard optimization: {}", config.cross_shard_optimization);
        println!();

        // Create PULSAR engine (in-memory, no persistence)
        let engine = PulsarGraphEngine::new(config)?;
        println!("PULSAR engine created successfully");
        println!();

        // Example 1: Insert nodes and observe sharding
        println!("=== Example 1: Node Sharding ===");
        let nodes = vec![
            Node {
                id: "alice".to_string(),
                labels: vec!["User".to_string()],
                properties: {
                    let mut props = HashMap::new();
                    props.insert("name".to_string(), PropertyValue::from("Alice"));
                    props.insert("age".to_string(), PropertyValue::from(30));
                    props
                },
                embedding: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            },
            Node {
                id: "bob".to_string(),
                labels: vec!["User".to_string()],
                properties: {
                    let mut props = HashMap::new();
                    props.insert("name".to_string(), PropertyValue::from("Bob"));
                    props.insert("age".to_string(), PropertyValue::from(25));
                    props
                },
                embedding: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            },
            Node {
                id: "charlie".to_string(),
                labels: vec!["User".to_string()],
                properties: {
                    let mut props = HashMap::new();
                    props.insert("name".to_string(), PropertyValue::from("Charlie"));
                    props.insert("age".to_string(), PropertyValue::from(35));
                    props
                },
                embedding: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            },
        ];

        // Track which shard each node goes to
        for node in &nodes {
            let shard_id = engine.get_shard_for_node(&node.id).await?;
            println!("  Node '{}' -> Shard {}", node.id, shard_id);
        }
        println!();

        // Insert nodes
        let start = Instant::now();
        for node in nodes {
            engine.insert_node(node).await?;
        }
        let insert_time = start.elapsed();
        println!("Inserted 3 nodes in {:?}", insert_time);
        println!();

        // Example 2: Insert edges (may span shards)
        println!("=== Example 2: Cross-Shard Edges ===");
        let edges = vec![
            Edge {
                id: "edge1".to_string(),
                from_node_id: "alice".to_string(),
                to_node_id: "bob".to_string(),
                edge_type: "FRIENDS_WITH".to_string(),
                properties: HashMap::new(),
                created_at_ms: 0,
                updated_at_ms: 0,
            },
            Edge {
                id: "edge2".to_string(),
                from_node_id: "bob".to_string(),
                to_node_id: "charlie".to_string(),
                edge_type: "FRIENDS_WITH".to_string(),
                properties: HashMap::new(),
                created_at_ms: 0,
                updated_at_ms: 0,
            },
            Edge {
                id: "edge3".to_string(),
                from_node_id: "alice".to_string(),
                to_node_id: "charlie".to_string(),
                edge_type: "FOLLOWS".to_string(),
                properties: HashMap::new(),
                created_at_ms: 0,
                updated_at_ms: 0,
            },
        ];

        for edge in &edges {
            let from_shard = engine.get_shard_for_node(&edge.from_node_id).await?;
            let to_shard = engine.get_shard_for_node(&edge.to_node_id).await?;
            println!("  Edge '{}' ({} -> {}): {} -> Shard {}",
                edge.id, edge.from_node_id, edge.to_node_id, from_shard, to_shard);
        }
        println!();

        // Insert edges
        let start = Instant::now();
        for edge in edges {
            engine.insert_edge(edge).await?;
        }
        let insert_time = start.elapsed();
        println!("Inserted 3 edges in {:?}", insert_time);
        println!();

        // Example 3: Query neighbors (cross-shard)
        println!("=== Example 3: Cross-Shard Query ===");
        let start = Instant::now();
        let neighbors = engine.get_neighbors(&"alice".to_string(), None)?;
        let query_time = start.elapsed();
        println!("Neighbors of 'alice' (queried in {:?}):", query_time);
        for neighbor in neighbors {
            println!("  - {}", neighbor.id);
        }
        println!();

        // Example 4: Cross-shard traversal
        println!("=== Example 4: Cross-Shard Traversal ===");
        let start = Instant::now();
        let result = engine.cross_shard_traversal(&"alice".to_string(), 2).await;
        let traversal_time = start.elapsed();

        match result {
            Ok(nodes) => {
                println!("BFS traversal from 'alice' (depth=2, took {:?}):", traversal_time);
                for node in nodes {
                    println!("  - {}", node.id);
                }
            }
            Err(e) => {
                println!("Cross-shard traversal failed (expected - incomplete implementation): {:?}", e);
            }
        }
        println!();

        // Example 5: Get statistics
        println!("=== Example 5: Engine Statistics ===");
        let stats = engine.get_stats().await;
        println!("Total nodes: {}", stats.total_nodes);
        println!("Total edges: {}", stats.total_edges);
        println!("Active shards: {}", stats.shards_active);
        println!("Cross-shard queries: {}", stats.cross_shard_queries);
        println!();

        // Example 6: Bulk operations
        println!("=== Example 6: Bulk Insert ===");
        let bulk_nodes: Vec<Node> = (0..100)
            .map(|i| Node {
                id: format!("user_{}", i),
                labels: vec!["User".to_string()],
                properties: HashMap::new(),
                embedding: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            })
            .collect();

        let start = Instant::now();
        engine.bulk_insert_nodes(bulk_nodes).await?;
        let bulk_time = start.elapsed();

        let stats = engine.get_stats().await;
        println!("Inserted 100 nodes in {:?} ({:.2} nodes/sec)",
            bulk_time, 100.0 / bulk_time.as_secs_f64());
        println!("Total nodes in engine: {}", stats.total_nodes);
        println!();

        println!("=== Example Complete ===");
        println!();
        println!("PULSAR Features Demonstrated:");
        println!("  ✓ Consistent hash-based sharding");
        println!("  ✓ Cross-shard edge storage");
        println!("  ✓ Cross-shard neighbor queries");
        println!("  ✓ Bulk insert operations");
        println!();
        println!("PULSAR Limitations:");
        println!("  ✗ Cross-shard traversal incomplete");
        println!("  ✗ No distributed transactions");
        println!("  ✗ Eventual consistency (async replication)");
        println!();
        println!("For production use, consider ORION with application-level sharding.");
    }

    Ok(())
}
