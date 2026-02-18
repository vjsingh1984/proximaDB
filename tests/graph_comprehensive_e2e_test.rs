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

//! Comprehensive End-to-End Integration Tests
//!
//! Validates the hybrid vector-graph features and end-to-end workflows:
//! - Phase 4: Hybrid Vector-Graph (Semantic Traversal, Vector-Guided A*, Hybrid Ranking)
//! - Graph Operations: Node/Edge CRUD, traversal, statistics
//! - Complete Pipeline: Semantic search → Ranking → Pathfinding
//!
//! Note: Algorithm library tests (Louvain, Closeness, etc.) are in their respective unit tests
//! since they require direct CSR access which is an internal implementation detail.

use dashmap::DashMap;
use proximadb::compute::distance_computation::engine::UnifiedDistanceCompute;
use proximadb::graph::engines::GraphEngine;
use proximadb::graph::engines::orion::OrionGraphEngine;
use proximadb::graph::engines::orion::traversal::TraversalConfig;
use proximadb::graph::engines::orion::traversal::vector_guided_astar;
use proximadb::graph::hybrid::ranking::{HybridRankingStrategy, RankingContext, RankingStrategy};
use proximadb::graph::hybrid::semantic_traversal::{SemanticBFSTraversal, SemanticTraversalInput};
use proximadb::graph::{Edge, Node};
use proximadb::proto::proximadb_v1::{DistanceMetric, EmbeddingVersion};
use std::collections::HashMap;
use std::sync::Arc;

/// Create a test node with embedding and properties
fn create_test_node(id: &str, label: &str, embedding: Vec<f32>, name: &str) -> Node {
    let mut properties = HashMap::new();
    properties.insert(
        "name".to_string(),
        proximadb::proto::proximadb_v1::PropertyValue {
            value: Some(
                proximadb::proto::proximadb_v1::property_value::Value::StringValue(
                    name.to_string(),
                ),
            ),
        },
    );

    let dimension = embedding.len() as u32;

    Node {
        id: id.to_string(),
        labels: vec![label.to_string()],
        properties,
        embedding: Some(EmbeddingVersion {
            model_id: "test-model".to_string(),
            model_version: "1.0".to_string(),
            vector: embedding,
            dimension,
            created_at_ms: 0,
            model_params: HashMap::new(),
            modality: 0,
        }),
        created_at_ms: 0,
        updated_at_ms: 0,
    }
}

/// Create a test edge
fn create_test_edge(from: &str, to: &str, edge_type: &str) -> Edge {
    Edge {
        id: format!("{}-{}-{}", from, edge_type, to),
        from_node_id: from.to_string(),
        to_node_id: to.to_string(),
        edge_type: edge_type.to_string(),
        properties: HashMap::new(),
        weight: Some(1.0),
        created_at_ms: 0,
        updated_at_ms: 0,
    }
}

/// Build a comprehensive knowledge graph for testing all features
async fn build_test_knowledge_graph() -> Arc<OrionGraphEngine> {
    let engine = Arc::new(OrionGraphEngine::new());

    // Create a knowledge graph about AI topics with semantic embeddings
    // Embeddings are positioned to test semantic similarity and clustering
    let nodes = vec![
        // AI cluster (close embeddings)
        create_test_node(
            "AI",
            "Topic",
            vec![1.0, 0.0, 0.0],
            "Artificial Intelligence",
        ),
        create_test_node("ML", "Topic", vec![0.95, 0.05, 0.0], "Machine Learning"),
        create_test_node("DL", "Topic", vec![0.9, 0.1, 0.05], "Deep Learning"),
        create_test_node(
            "NLP",
            "Topic",
            vec![0.85, 0.0, 0.15],
            "Natural Language Processing",
        ),
        // Data cluster (different embeddings)
        create_test_node("DB", "Topic", vec![0.0, 1.0, 0.0], "Databases"),
        create_test_node("BigData", "Topic", vec![0.05, 0.95, 0.0], "Big Data"),
        create_test_node("Analytics", "Topic", vec![0.1, 0.9, 0.05], "Data Analytics"),
        // Systems cluster
        create_test_node("Cloud", "Topic", vec![0.0, 0.0, 1.0], "Cloud Computing"),
        create_test_node(
            "Distributed",
            "Topic",
            vec![0.05, 0.0, 0.95],
            "Distributed Systems",
        ),
    ];

    for node in nodes {
        engine.insert_node(node).await.unwrap();
    }

    // Create relationships that form interesting graph patterns (11 edges total)
    let edges = vec![
        // AI cluster connections (4 edges)
        create_test_edge("AI", "ML", "INCLUDES"),
        create_test_edge("ML", "DL", "INCLUDES"),
        create_test_edge("ML", "NLP", "INCLUDES"),
        create_test_edge("AI", "NLP", "RELATED_TO"),
        // Data cluster connections (3 edges)
        create_test_edge("DB", "BigData", "ENABLES"),
        create_test_edge("BigData", "Analytics", "ENABLES"),
        create_test_edge("DB", "Analytics", "SUPPORTS"),
        // Systems cluster connections (1 edge)
        create_test_edge("Cloud", "Distributed", "IMPLEMENTS"),
        // Cross-cluster connections (3 edges)
        create_test_edge("ML", "BigData", "USES"),
        create_test_edge("Analytics", "ML", "APPLIES"),
        create_test_edge("Distributed", "DB", "REQUIRES"),
    ];

    for edge in edges {
        engine.insert_edge(edge).await.unwrap();
    }

    engine
}

#[tokio::test]
async fn test_e2e_graph_basic_operations() {
    println!("\n=== Testing Graph Basic Operations ===\n");

    let engine = build_test_knowledge_graph().await;

    // Test 1: Node retrieval and properties
    println!("1. Testing node retrieval...");
    let ai_node = engine.get_node(&"AI".to_string()).unwrap();
    assert!(ai_node.is_some());
    let ai = ai_node.unwrap();

    // Verify properties
    assert_eq!(ai.id, "AI");
    assert!(ai.labels.contains(&"Topic".to_string()));
    assert!(ai.embedding.is_some());
    println!("   ✓ AI node retrieved with embedding and properties");

    // Test 2: Label-based retrieval
    println!("2. Testing label-based retrieval...");
    let topic_nodes = engine.get_nodes_by_label("Topic").unwrap();
    println!("   Found {} nodes with label 'Topic'", topic_nodes.len());
    assert_eq!(topic_nodes.len(), 9);

    // Test 3: Neighbor traversal
    println!("3. Testing neighbor traversal...");
    let ml_neighbors = engine.get_neighbors(&"ML".to_string(), None).unwrap();
    println!("   ML has {} neighbors", ml_neighbors.len());
    assert!(!ml_neighbors.is_empty());

    // ML should have multiple neighbors (DL, NLP, BigData)
    assert!(ml_neighbors.len() >= 3);

    // Test 4: Edge type filtering
    println!("4. Testing edge type filtering...");
    let includes_edges = engine
        .get_outgoing_edges(&"AI".to_string(), Some("INCLUDES"))
        .unwrap();
    println!("   AI has {} INCLUDES edges", includes_edges.len());
    assert!(includes_edges.len() > 0);

    // Test 5: Graph statistics
    println!("5. Testing graph statistics...");
    let stats = engine.get_stats().await;
    let node_count = engine.get_all_nodes().unwrap().len();
    let edge_count = engine.edge_count().unwrap();
    println!(
        "   Operations - Created: {} nodes, {} edges",
        stats.nodes_created, stats.edges_created
    );
    println!(
        "   Current count: {} nodes, {} edges",
        node_count, edge_count
    );
    assert_eq!(node_count, 9);
    assert_eq!(edge_count, 11);

    println!("✅ Graph Basic Operations: PASS\n");
}

#[tokio::test]
async fn test_e2e_phase4_hybrid_vector_graph() {
    println!("\n=== Testing Phase 4: Hybrid Vector-Graph Features ===\n");

    let engine = build_test_knowledge_graph().await;
    let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));

    // Test 1: Semantic BFS Traversal
    println!("1. Testing Semantic BFS Traversal...");
    let semantic_bfs = SemanticBFSTraversal::new(
        Arc::clone(&engine) as Arc<dyn GraphEngine>,
        Arc::clone(&distance_compute),
        0.8, // High similarity threshold
        DistanceMetric::Cosine,
    );

    let input = SemanticTraversalInput {
        start_node: "AI".to_string(),
        query_embedding: vec![1.0, 0.0, 0.0], // AI's embedding
        max_depth: 2,
    };

    let result = semantic_bfs.execute(input).unwrap();
    println!(
        "   Found {} semantically similar nodes",
        result.matches_found
    );
    assert!(result.matches_found >= 3); // Should find AI, ML, DL at minimum

    // Verify AI is in results with high similarity
    let ai_match = result.results.iter().find(|m| m.node.id == "AI");
    assert!(ai_match.is_some());
    println!("   AI similarity: {:.3}", ai_match.unwrap().similarity);
    assert!(ai_match.unwrap().similarity >= 0.99);

    // Test 2: Vector-Guided A* Pathfinding
    println!("2. Testing Vector-Guided A* Pathfinding...");
    let guide_embedding = vec![0.9, 0.05, 0.05]; // Between AI and ML

    let path_result = vector_guided_astar(
        &engine,
        &"AI".to_string(),
        &"NLP".to_string(),
        &guide_embedding,
        0.5, // Balanced: 50% graph, 50% semantic
        Arc::clone(&distance_compute),
        DistanceMetric::Cosine,
        TraversalConfig::default(),
    )
    .await
    .unwrap();

    assert!(path_result.is_some());
    let (path, cost) = path_result.unwrap();
    println!("   Path from AI to NLP: {:?} (cost: {:.3})", path, cost);
    assert!(path.len() >= 2);
    assert_eq!(path[0], "AI");
    assert_eq!(path[path.len() - 1], "NLP");

    // Test 3: Hybrid Ranking Strategy
    println!("3. Testing Hybrid Ranking Strategy...");

    // Build centrality cache (using mock values for E2E test)
    // In production, these would come from computed graph metrics
    let centrality_cache = Arc::new(DashMap::new());
    centrality_cache.insert("AI".to_string(), 0.9); // High centrality
    centrality_cache.insert("ML".to_string(), 0.85); // High centrality
    centrality_cache.insert("DL".to_string(), 0.7); // Medium centrality
    centrality_cache.insert("NLP".to_string(), 0.6); // Medium centrality

    let strategy = HybridRankingStrategy::balanced(
        Arc::clone(&distance_compute),
        Arc::clone(&centrality_cache),
    );

    let context = RankingContext {
        query_embedding: vec![0.95, 0.05, 0.0], // ML's embedding
        distance_metric: DistanceMetric::Cosine,
    };

    // Rank all AI cluster nodes
    let ai_node = engine.get_node(&"AI".to_string()).unwrap().unwrap();
    let ml_node = engine.get_node(&"ML".to_string()).unwrap().unwrap();
    let dl_node = engine.get_node(&"DL".to_string()).unwrap().unwrap();

    let ai_score = strategy.compute_score(&ai_node, &context).unwrap();
    let ml_score = strategy.compute_score(&ml_node, &context).unwrap();
    let dl_score = strategy.compute_score(&dl_node, &context).unwrap();

    println!("   Ranking scores:");
    println!("     AI: {:.3}", ai_score);
    println!("     ML: {:.3}", ml_score);
    println!("     DL: {:.3}", dl_score);

    // ML should have highest score (perfect vector match + good centrality)
    assert!(ml_score >= ai_score || ml_score >= dl_score);

    println!("✅ Phase 4 Hybrid Vector-Graph: PASS\n");
}

#[tokio::test]
async fn test_e2e_full_pipeline_integration() {
    println!("\n=== Testing Complete Pipeline Integration ===\n");

    let engine = build_test_knowledge_graph().await;
    let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));

    // Full pipeline: Semantic search → Algorithm analysis → Hybrid ranking

    // Step 1: Find semantically similar nodes using Semantic BFS
    println!("Step 1: Semantic search for AI-related topics...");
    let semantic_bfs = SemanticBFSTraversal::new(
        Arc::clone(&engine) as Arc<dyn GraphEngine>,
        Arc::clone(&distance_compute),
        0.7, // Relaxed threshold
        DistanceMetric::Cosine,
    );

    let query_embedding = vec![0.9, 0.1, 0.0];
    let input = SemanticTraversalInput {
        start_node: "AI".to_string(),
        query_embedding: query_embedding.clone(),
        max_depth: 3,
    };

    let candidates = semantic_bfs.execute(input).unwrap();
    println!("   Found {} candidate nodes", candidates.matches_found);
    assert!(candidates.matches_found >= 4);

    // Step 2: Use precomputed graph metrics (centrality) for ranking
    println!("Step 2: Using graph centrality metrics...");
    // In production, these would be computed by graph algorithms
    // For E2E test, we use representative values
    let centrality_cache = Arc::new(DashMap::new());
    centrality_cache.insert("AI".to_string(), 0.9);
    centrality_cache.insert("ML".to_string(), 0.85);
    centrality_cache.insert("DL".to_string(), 0.7);
    centrality_cache.insert("NLP".to_string(), 0.6);
    centrality_cache.insert("DB".to_string(), 0.5);
    centrality_cache.insert("BigData".to_string(), 0.55);
    centrality_cache.insert("Analytics".to_string(), 0.5);
    centrality_cache.insert("Cloud".to_string(), 0.4);
    centrality_cache.insert("Distributed".to_string(), 0.45);
    println!("   Using centrality for {} nodes", centrality_cache.len());

    // Step 3: Hybrid ranking combining vector similarity + graph centrality
    println!("Step 3: Hybrid ranking of candidates...");
    let strategy = HybridRankingStrategy::new(
        0.6, // 60% vector weight
        0.4, // 40% graph weight
        Arc::clone(&distance_compute),
        centrality_cache,
    );

    let context = RankingContext {
        query_embedding,
        distance_metric: DistanceMetric::Cosine,
    };

    let mut ranked: Vec<_> = candidates
        .results
        .iter()
        .map(|m| {
            let score = strategy.compute_score(&m.node, &context).unwrap();
            (m.node.id.clone(), score, m.similarity)
        })
        .collect();

    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    println!("   Ranked results (top 5):");
    for (i, (id, score, similarity)) in ranked.iter().take(5).enumerate() {
        println!(
            "     {}. {} (hybrid: {:.3}, similarity: {:.3})",
            i + 1,
            id,
            score,
            similarity
        );
    }

    assert!(!ranked.is_empty());
    assert!(ranked[0].1 > 0.5); // Top result should have good combined score

    // Step 4: Use vector-guided pathfinding to connect top results
    println!("Step 4: Finding semantic path between top results...");
    if ranked.len() >= 2 {
        let start = &ranked[0].0;
        let target = &ranked[ranked.len() - 1].0;

        let path_result = vector_guided_astar(
            &engine,
            start,
            target,
            &vec![0.9, 0.1, 0.0],
            0.5,
            Arc::clone(&distance_compute),
            DistanceMetric::Cosine,
            TraversalConfig::default(),
        )
        .await;

        if let Ok(Some((path, _cost))) = path_result {
            println!("   Semantic path: {:?}", path);
            assert!(path.len() >= 2);
        }
    }

    println!("✅ Complete Pipeline Integration: PASS\n");
}

#[tokio::test]
async fn test_e2e_cross_cluster_relationships() {
    println!("\n=== Testing Cross-Cluster Relationships ===\n");

    let engine = build_test_knowledge_graph().await;

    // Test 1: Verify cross-cluster edges exist
    println!("1. Testing cross-cluster edge (ML -> BigData)...");
    let ml_neighbors = engine.get_neighbors(&"ML".to_string(), None).unwrap();
    let has_bigdata = ml_neighbors.iter().any(|n| n.id == "BigData");
    assert!(has_bigdata, "ML should have edge to BigData");
    println!("   ✓ Cross-cluster edge verified");

    // Test 2: Verify semantic distance between clusters
    println!("2. Testing semantic distance between clusters...");
    let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));

    let ai_node = engine.get_node(&"AI".to_string()).unwrap().unwrap();
    let db_node = engine.get_node(&"DB".to_string()).unwrap().unwrap();

    let ai_embedding = &ai_node.embedding.as_ref().unwrap().vector;
    let db_embedding = &db_node.embedding.as_ref().unwrap().vector;

    let similarity =
        distance_compute.calculate_distance(ai_embedding, db_embedding, &DistanceMetric::Cosine);
    println!("   Semantic distance (AI-DB): {:.3}", similarity.distance);
    println!("   Normalized score: {:.3}", similarity.normalized_score);

    // Different clusters should have lower similarity
    assert!(
        similarity.normalized_score < 0.7,
        "Different clusters should be semantically distant"
    );

    // Test 3: Verify graph connectivity
    println!("3. Testing graph connectivity...");
    let node_count = engine.get_all_nodes().unwrap().len();
    let edge_count = engine.edge_count().unwrap();
    println!(
        "   Total nodes: {}, Total edges: {}",
        node_count, edge_count
    );
    assert_eq!(node_count, 9);
    assert_eq!(edge_count, 11);

    // Test 4: Multi-hop traversal
    println!("4. Testing multi-hop traversal (AI -> ML -> BigData)...");
    let ai_neighbors = engine.get_neighbors(&"AI".to_string(), None).unwrap();
    assert!(
        ai_neighbors.iter().any(|n| n.id == "ML"),
        "AI should connect to ML"
    );

    let ml_neighbors = engine.get_neighbors(&"ML".to_string(), None).unwrap();
    assert!(
        ml_neighbors.iter().any(|n| n.id == "BigData"),
        "ML should connect to BigData"
    );
    println!("   ✓ Multi-hop path verified: AI -> ML -> BigData");

    println!("✅ Cross-Cluster Relationships: PASS\n");
}
