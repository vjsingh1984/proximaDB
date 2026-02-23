//! QUASAR Tiered Graph Engine Example
//!
//! This example demonstrates how to use QUASAR for hot/cold tiered graph storage.
//!
//! **WARNING**: QUASAR is experimental and not production-ready.
//!
//! Run with:
//! ```bash
//! cargo run --release --example quasar_tiered_graph --features tiered-graph
//! ```

use proximadb::graph::engines::quasar::{QuasarGraphEngine, QuasarConfig, ColdStorageBackend};
use proximadb::graph::{Node, Edge, PropertyValue};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tempfile::TempDir;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== QUASAR Tiered Graph Engine Example ===");
    println!();

    // Check if QUASAR feature is enabled
    #[cfg(not(feature = "tiered-graph"))]
    {
        eprintln!("ERROR: QUASAR engine requires 'tiered-graph' feature.");
        eprintln!("Build with: cargo build --features tiered-graph");
        return Ok(());
    }

    #[cfg(feature = "tiered-graph")]
    {
        // Create temporary directory for cold tier
        let temp_dir = TempDir::new()?;
        let cold_path = temp_dir.path().to_path_buf();

        // Configure QUASAR engine
        let config = QuasarConfig {
            hot_tier_max_nodes: 10, // Small limit for demonstration
            hot_tier_max_memory_mb: 100,
            cold_tier_path: cold_path.clone(),
            cold_migration_threshold: Duration::from_secs(2), // Aggressive migration
            hot_promotion_threshold: Duration::from_millis(100),
            migration_interval: Duration::from_secs(1),
            cold_storage_backend: ColdStorageBackend::Json,
        };

        println!("Creating QUASAR engine with configuration:");
        println!("  Hot tier max nodes: {}", config.hot_tier_max_nodes);
        println!("  Cold tier path: {}", config.cold_tier_path.display());
        println!("  Migration threshold: {:?}", config.cold_migration_threshold);
        println!("  Promotion threshold: {:?}", config.hot_promotion_threshold);
        println!("  Migration interval: {:?}", config.migration_interval);
        println!("  Cold storage backend: {:?}", config.cold_storage_backend);
        println!();

        // Create QUASAR engine
        let engine = QuasarGraphEngine::new(config).await?;
        println!("QUASAR engine created successfully");
        println!();

        // Example 1: Insert nodes into hot tier
        println!("=== Example 1: Hot Tier Insert ===");
        for i in 0..5 {
            let node = Node {
                id: format!("hot_node_{}", i),
                labels: vec!["Hot".to_string()],
                properties: {
                    let mut props = HashMap::new();
                    props.insert("index".to_string(), PropertyValue::from(i));
                    props
                },
                embedding: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            };
            engine.insert_node(node).await?;
        }

        let stats = engine.get_stats().await;
        println!("Hot tier nodes: {}", stats.hot_tier_nodes);
        println!("Cold tier nodes: {}", stats.cold_tier_nodes);
        println!();

        // Example 2: Fill hot tier to trigger migration
        println!("=== Example 2: Hot Tier Fill & Migration ===");
        println!("Inserting nodes beyond hot tier limit...");
        for i in 5..15 {
            let node = Node {
                id: format!("node_{}", i),
                labels: vec!["Node".to_string()],
                properties: {
                    let mut props = HashMap::new();
                    props.insert("index".to_string(), PropertyValue::from(i));
                    props
                },
                embedding: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            };
            engine.insert_node(node).await?;
        }

        // Wait for background migration
        tokio::time::sleep(Duration::from_secs(2)).await;

        let stats = engine.get_stats().await;
        println!("After migration:");
        println!("  Hot tier nodes: {}", stats.hot_tier_nodes);
        println!("  Cold tier nodes: {}", stats.cold_tier_nodes);
        println!("  Migration operations: {}", stats.migration_operations);
        println!();

        // Example 3: Access cold tier data
        println!("=== Example 3: Cold Tier Access ===");
        let start = Instant::now();
        let cold_node = engine.get_node(&"node_5".to_string())?;
        let access_time = start.elapsed();

        match cold_node {
            Some(node) => {
                println!("Retrieved node '{}' in {:?}", node.id, access_time);
                println!("  (Likely from cold tier)");
            }
            None => {
                println!("Node not found in sync path (cold tier not accessible in sync get_node)");
                println!("  This is a known limitation of QUASAR");

                // Try async get_node_from_tiers
                let cold_node_async = engine.get_node_from_tiers(&"node_5".to_string()).await?;
                if let Some(node) = cold_node_async {
                    println!("  Retrieved via async get_node_from_tiers: {}", node.id);
                }
            }
        }
        println!();

        // Example 4: Force migration
        println!("=== Example 4: Force Migration ===");
        engine.force_migration().await?;
        let stats = engine.get_stats().await;
        println!("After forced migration:");
        println!("  Hot tier nodes: {}", stats.hot_tier_nodes);
        println!("  Cold tier nodes: {}", stats.cold_tier_nodes);
        println!("  Demotions to cold: {}", stats.demotions_to_cold);
        println!();

        // Example 5: Cache hit/miss statistics
        println!("=== Example 5: Cache Statistics ===");
        // Access hot tier nodes
        for i in 0..3 {
            let _ = engine.get_node(&format!("hot_node_{}", i));
        }

        tokio::time::sleep(Duration::from_millis(100)).await;

        let stats = engine.get_stats().await;
        println!("Cache hits: {}", stats.cache_hits);
        println!("Cache misses: {}", stats.cache_misses);
        let hit_ratio = if stats.cache_hits + stats.cache_misses > 0 {
            stats.cache_hits as f64 / (stats.cache_hits + stats.cache_misses) as f64
        } else {
            0.0
        };
        println!("Hit ratio: {:.2}%", hit_ratio * 100.0);
        println!("Average access latency: {:.2} ms", stats.average_access_latency_ms);
        println!();

        // Example 6: Storage cost savings
        println!("=== Example 6: Cost Savings ===");
        let stats = engine.get_stats().await;
        println!("Hot tier nodes: {}", stats.hot_tier_nodes);
        println!("Cold tier nodes: {}", stats.cold_tier_nodes);
        println!("Storage cost savings: {:.2}%", stats.storage_cost_savings_ratio * 100.0);
        println!();

        // Example 7: Edges across tiers
        println!("=== Example 7: Edges ===");
        let edge = Edge {
            id: "edge1".to_string(),
            from_node_id: "hot_node_0".to_string(),
            to_node_id: "node_5".to_string(),
            edge_type: "CONNECTS_TO".to_string(),
            properties: HashMap::new(),
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        engine.insert_edge(edge).await?;

        let stats = engine.get_stats().await;
        println!("Hot tier edges: {}", stats.hot_tier_edges);
        println!("Cold tier edges: {}", stats.cold_tier_edges);
        println!();

        // Example 8: Access pattern statistics
        println!("=== Example 8: Access Pattern Statistics ===");
        let access_stats = engine.get_access_stats().await;
        println!("Total accesses tracked: {}", access_stats.total_accesses);
        println!("Unique nodes accessed: {}", access_stats.unique_nodes_accessed);
        println!();

        println!("=== Example Complete ===");
        println!();
        println!("QUASAR Features Demonstrated:");
        println!("  ✓ Hot tier (in-memory ORION)");
        println!("  ✓ Cold tier (JSON file storage)");
        println!("  ✓ Automatic migration based on access patterns");
        println!("  ✓ LRU-based cache management");
        println!("  ✓ Access pattern tracking");
        println!();
        println!("QUASAR Limitations:");
        println!("  ✗ Sync get_node doesn't check cold tier");
        println!("  ✗ Minimal tiering logic (time-based only)");
        println!("  ✗ Not integrated with storage engines");
        println!("  ✗ No ML-based access prediction");
        println!();
        println!("For production use, consider ORION with appropriate memory sizing.");
    }

    Ok(())
}
