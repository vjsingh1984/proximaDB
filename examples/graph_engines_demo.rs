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

//! # Graph Engines Demo
//!
//! Demonstrates the usage of PULSAR and QUASAR graph engines

use proximadb::graph::PropertyValue;
use proximadb::graph::engines::GraphEngine;
use proximadb::graph::engines::pulsar::PulsarConfig;
use proximadb::graph::engines::quasar::QuasarConfig;
use proximadb::graph::{
    Edge, GraphEngineConfig, GraphEngineFactory, GraphEngineType, Node, PulsarGraphEngine,
    QuasarGraphEngine,
};
use proximadb::proto::proximadb_v1::property_value::Value;
use std::collections::HashMap;
use tempfile::TempDir;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 ProximaDB Graph Engines Demo");
    println!("================================\n");

    demo_pulsar_engine().await?;
    demo_quasar_engine().await?;
    demo_engine_factory().await?;

    println!("✅ All demos completed successfully!");
    Ok(())
}

async fn demo_pulsar_engine() -> Result<(), Box<dyn std::error::Error>> {
    println!("📡 PULSAR Engine Demo (Distributed Graph)");
    println!("-----------------------------------------");

    let config = PulsarConfig {
        shard_count: 4,
        replication_factor: 1,
        ..PulsarConfig::default()
    };

    let engine = PulsarGraphEngine::new(config)?;
    println!("✓ Created PULSAR engine with 4 shards");

    // Create test nodes
    let node1 = Node {
        id: "alice".to_string(),
        labels: vec!["Person".to_string()],
        properties: HashMap::from([
            (
                "name".to_string(),
                PropertyValue {
                    value: Some(Value::StringValue("Alice".to_string())),
                },
            ),
            (
                "age".to_string(),
                PropertyValue {
                    value: Some(Value::IntValue(30)),
                },
            ),
        ]),
        embedding: None,
        created_at_ms: chrono::Utc::now().timestamp_millis(),
        updated_at_ms: chrono::Utc::now().timestamp_millis(),
    };

    let node2 = Node {
        id: "bob".to_string(),
        labels: vec!["Person".to_string()],
        properties: HashMap::from([
            (
                "name".to_string(),
                PropertyValue {
                    value: Some(Value::StringValue("Bob".to_string())),
                },
            ),
            (
                "age".to_string(),
                PropertyValue {
                    value: Some(Value::IntValue(25)),
                },
            ),
        ]),
        embedding: None,
        created_at_ms: chrono::Utc::now().timestamp_millis(),
        updated_at_ms: chrono::Utc::now().timestamp_millis(),
    };

    // Insert nodes
    engine.insert_node(node1).await?;
    engine.insert_node(node2).await?;
    println!("✓ Inserted 2 nodes across shards");

    // Create edge
    let edge = Edge {
        id: "alice_knows_bob".to_string(),
        from_node_id: "alice".to_string(),
        to_node_id: "bob".to_string(),
        edge_type: "KNOWS".to_string(),
        properties: HashMap::from([(
            "since".to_string(),
            PropertyValue {
                value: Some(Value::StringValue("2020".to_string())),
            },
        )]),
        weight: Some(1.0),
        created_at_ms: chrono::Utc::now().timestamp_millis(),
        updated_at_ms: chrono::Utc::now().timestamp_millis(),
    };

    engine.insert_edge(edge).await?;
    println!("✓ Created relationship: Alice KNOWS Bob");

    // Wait for async operations
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Query operations
    let alice = engine.get_node(&"alice".to_string())?.unwrap();
    println!(
        "✓ Retrieved Alice: {} (age: {})",
        alice.id,
        alice
            .properties
            .get("age")
            .map(|v| match &v.value {
                Some(Value::IntValue(i)) => i.to_string(),
                _ => "unknown".to_string(),
            })
            .unwrap_or("unknown".to_string())
    );

    let neighbors = engine.get_neighbors(&"alice".to_string(), None)?;
    println!("✓ Alice's neighbors: {} people", neighbors.len());

    // Cross-shard traversal
    let traversal_result = engine
        .cross_shard_traversal(&"alice".to_string(), 2)
        .await?;
    println!(
        "✓ Cross-shard traversal found {} nodes",
        traversal_result.len()
    );

    // Statistics
    let stats = engine.get_stats().await;
    println!("📊 PULSAR Stats:");
    println!("   - Total nodes: {}", stats.total_nodes);
    println!("   - Total edges: {}", stats.total_edges);
    println!("   - Active shards: {}", stats.shards_active);
    println!("   - Cross-shard queries: {}", stats.cross_shard_queries);

    println!();
    Ok(())
}

async fn demo_quasar_engine() -> Result<(), Box<dyn std::error::Error>> {
    println!("🌟 QUASAR Engine Demo (Hybrid Hot/Cold Storage)");
    println!("----------------------------------------------");

    let temp_dir = TempDir::new()?;
    let config = QuasarConfig {
        hot_tier_max_nodes: 3, // Small for demo
        cold_tier_path: temp_dir.path().to_path_buf(),
        ..QuasarConfig::default()
    };

    let engine = QuasarGraphEngine::new(config).await?;
    println!("✓ Created QUASAR engine with hot tier limit of 3 nodes");

    // Insert nodes beyond hot tier capacity
    for i in 1..=5 {
        let node = Node {
            id: format!("person_{}", i),
            labels: vec!["Person".to_string()],
            properties: HashMap::from([
                (
                    "name".to_string(),
                    PropertyValue {
                        value: Some(Value::StringValue(format!("Person {}", i))),
                    },
                ),
                (
                    "index".to_string(),
                    PropertyValue {
                        value: Some(Value::IntValue(i)),
                    },
                ),
            ]),
            embedding: None,
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            updated_at_ms: chrono::Utc::now().timestamp_millis(),
        };

        engine.insert_node(node).await?;
        println!("✓ Inserted person_{}", i);
    }

    // Wait for background migration
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    println!("✓ Inserted 5 nodes (should trigger hot→cold migration)");

    // Test access across tiers
    for i in 1..=5 {
        let node_id = format!("person_{}", i);
        let retrieved = engine.get_node(&node_id)?;
        if retrieved.is_some() {
            println!("✓ Successfully accessed {} across tiers", node_id);
        }
    }

    // Create some edges
    for i in 1..4 {
        let edge = Edge {
            id: format!("edge_{}", i),
            from_node_id: format!("person_{}", i),
            to_node_id: format!("person_{}", i + 1),
            edge_type: "CONNECTED_TO".to_string(),
            properties: HashMap::new(),
            weight: Some(1.0),
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            updated_at_ms: chrono::Utc::now().timestamp_millis(),
        };
        engine.insert_edge(edge).await?;
    }

    println!("✓ Created 3 edges between persons");

    // Statistics
    let stats = engine.get_stats().await;
    println!("📊 QUASAR Stats:");
    println!("   - Hot tier nodes: {}", stats.hot_tier_nodes);
    println!("   - Cold tier nodes: {}", stats.cold_tier_nodes);
    println!("   - Hot tier edges: {}", stats.hot_tier_edges);
    println!("   - Cold tier edges: {}", stats.cold_tier_edges);
    println!("   - Cache hits: {}", stats.cache_hits);
    println!("   - Cache misses: {}", stats.cache_misses);
    println!("   - Promotions to hot: {}", stats.promotions_to_hot);
    println!("   - Demotions to cold: {}", stats.demotions_to_cold);

    let access_stats = engine.get_access_stats().await;
    println!("📈 Access Pattern Stats:");
    println!("   - Total accesses: {}", access_stats.total_accesses);
    println!(
        "   - Unique items tracked: {}",
        access_stats.unique_items_tracked
    );

    println!();
    Ok(())
}

async fn demo_engine_factory() -> Result<(), Box<dyn std::error::Error>> {
    println!("🏭 Engine Factory Demo");
    println!("---------------------");

    // List available engines
    let engines = GraphEngineFactory::available_engines();
    println!("Available engines: {:?}", engines);

    // Show capabilities
    for &engine_type in &engines {
        let caps = GraphEngineFactory::get_engine_capabilities(engine_type);
        println!("\n🔧 {} Engine:", caps.name);
        println!("   Description: {}", caps.description);
        println!("   Features: {}", caps.features.join(", "));
        println!("   Use cases: {}", caps.use_cases.join(", "));
    }

    // Create ORION engine via factory
    let orion_engine =
        GraphEngineFactory::create_engine(GraphEngineType::Orion, GraphEngineConfig::default())?;
    println!("\n✓ Created ORION engine via factory");
    println!("   Node count: {}", orion_engine.node_count()?);

    // Create PULSAR engine via factory
    let config = GraphEngineConfig {
        pulsar_config: Some(PulsarConfig {
            shard_count: 2,
            ..PulsarConfig::default()
        }),
        ..GraphEngineConfig::default()
    };

    let pulsar_engine = GraphEngineFactory::create_engine(GraphEngineType::Pulsar, config)?;
    println!("✓ Created PULSAR engine via factory");
    println!("   Node count: {}", pulsar_engine.node_count()?);

    // Test engine type parsing
    let engine_names = ["orion", "PULSAR", "Quasar", "unknown"];
    for name in &engine_names {
        match GraphEngineFactory::engine_type_from_string(name) {
            Some(engine_type) => println!("✓ '{}' → {:?}", name, engine_type),
            None => println!("✗ '{}' → Not recognized", name),
        }
    }

    println!();
    Ok(())
}
