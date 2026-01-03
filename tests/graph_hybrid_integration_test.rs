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

//! Integration tests for hybrid vector-graph features
//!
//! Tests the combination of:
//! - SemanticBFSTraversal (semantic graph traversal)
//! - Vector-guided A* pathfinding
//! - HybridRankingStrategy (vector + graph signal composition)

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

/// Create a test node with embedding
fn create_node(id: &str, label: &str, embedding: Vec<f32>) -> Node {
    Node {
        id: id.to_string(),
        labels: vec![label.to_string()],
        properties: HashMap::new(),
        embedding: Some(EmbeddingVersion {
            model_id: "test-model".to_string(),
            model_version: "1.0".to_string(),
            vector: embedding,
            dimension: 3,
            created_at_ms: 0,
            model_params: HashMap::new(),
            modality: 0,
        }),
        created_at_ms: 0,
        updated_at_ms: 0,
    }
}

/// Create a test edge
fn create_edge(from: &str, to: &str, edge_type: &str) -> Edge {
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

/// Build a knowledge graph with semantic embeddings
#[tokio::test]
async fn test_semantic_traversal_finds_similar_topics() {
    let engine = Arc::new(OrionGraphEngine::new());

    // Create nodes with embeddings
    let nodes = vec![
        create_node("AI", "Topic", vec![1.0, 0.0, 0.0]),
        create_node("ML", "Topic", vec![0.9, 0.1, 0.0]),
        create_node("DL", "Topic", vec![0.8, 0.3, 0.1]),
    ];

    for node in nodes {
        engine.insert_node(node).await.unwrap();
    }

    // Create edges
    engine
        .insert_edge(create_edge("AI", "ML", "RELATED_TO"))
        .await
        .unwrap();
    engine
        .insert_edge(create_edge("ML", "DL", "RELATED_TO"))
        .await
        .unwrap();

    let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));
    let semantic_bfs = SemanticBFSTraversal::new(
        Arc::clone(&engine) as Arc<dyn GraphEngine>,
        distance_compute,
        0.8, // High similarity threshold
        DistanceMetric::Cosine,
    );

    // Query: Find topics similar to AI
    let input = SemanticTraversalInput {
        start_node: "AI".to_string(),
        query_embedding: vec![1.0, 0.0, 0.0],
        max_depth: 2,
    };

    let result = semantic_bfs.execute(input).unwrap();

    // Should find AI and ML (close embedding)
    assert!(result.matches_found >= 2);
    println!("Semantic BFS found {} matches", result.matches_found);

    // AI should be in results with high similarity
    let ai_match = result.results.iter().find(|m| m.node.id == "AI");
    assert!(ai_match.is_some());
    assert!(ai_match.unwrap().similarity >= 0.99);
}

#[tokio::test]
async fn test_vector_guided_pathfinding() {
    let engine = Arc::new(OrionGraphEngine::new());

    // Create path: AI -> ML -> DL
    let nodes = vec![
        create_node("AI", "Topic", vec![1.0, 0.0, 0.0]),
        create_node("ML", "Topic", vec![0.9, 0.1, 0.0]),
        create_node("DL", "Topic", vec![0.8, 0.3, 0.1]),
    ];

    for node in nodes {
        engine.insert_node(node).await.unwrap();
    }

    engine
        .insert_edge(create_edge("AI", "ML", "RELATED_TO"))
        .await
        .unwrap();
    engine
        .insert_edge(create_edge("ML", "DL", "RELATED_TO"))
        .await
        .unwrap();

    let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));
    let guide_embedding = vec![0.85, 0.2, 0.05]; // Between AI and ML

    let result = vector_guided_astar(
        &engine,
        &"AI".to_string(),
        &"DL".to_string(),
        &guide_embedding,
        0.5, // Balanced
        distance_compute,
        DistanceMetric::Cosine,
        TraversalConfig::default(),
    )
    .await
    .unwrap();

    assert!(result.is_some());
    let (path, _cost) = result.unwrap();

    println!("Path from AI to DL: {:?}", path);
    assert!(path.len() >= 2);
    assert_eq!(path[0], "AI");
    assert_eq!(path[path.len() - 1], "DL");
}

#[tokio::test]
async fn test_hybrid_ranking() {
    let engine = Arc::new(OrionGraphEngine::new());

    // Create nodes
    let nodes = vec![
        create_node("AI", "Topic", vec![1.0, 0.0, 0.0]),
        create_node("ML", "Topic", vec![0.9, 0.1, 0.0]),
    ];

    for node in nodes {
        engine.insert_node(node).await.unwrap();
    }

    let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));
    let centrality_cache = Arc::new(DashMap::new());
    centrality_cache.insert("AI".to_string(), 0.9);
    centrality_cache.insert("ML".to_string(), 0.85);

    let strategy = HybridRankingStrategy::new(
        0.6, // Vector weight
        0.4, // Graph weight
        distance_compute,
        centrality_cache,
    );

    let context = RankingContext {
        query_embedding: vec![0.9, 0.1, 0.0], // ML's embedding
        distance_metric: DistanceMetric::Cosine,
    };

    let ai_node = engine.get_node(&"AI".to_string()).unwrap().unwrap();
    let ml_node = engine.get_node(&"ML".to_string()).unwrap().unwrap();

    let ai_score = strategy.compute_score(&ai_node, &context).unwrap();
    let ml_score = strategy.compute_score(&ml_node, &context).unwrap();

    println!("AI score: {:.3}, ML score: {:.3}", ai_score, ml_score);

    // AI should have highest score due to higher centrality (0.9 vs 0.85)
    // even though ML has perfect vector match
    // AI: 0.6 * 0.993 + 0.4 * 0.9 = 0.956
    // ML: 0.6 * 1.0 + 0.4 * 0.85 = 0.94
    assert!(ai_score > ml_score);
    assert!(ai_score > 0.95); // High combined score
    assert!(ml_score > 0.93); // Still high due to perfect vector match
}

#[tokio::test]
async fn test_end_to_end_semantic_search_with_ranking() {
    let engine = Arc::new(OrionGraphEngine::new());

    // Create knowledge graph
    let nodes = vec![
        create_node("AI", "Topic", vec![1.0, 0.0, 0.0]),
        create_node("ML", "Topic", vec![0.9, 0.1, 0.0]),
        create_node("DL", "Topic", vec![0.8, 0.3, 0.1]),
        create_node("NLP", "Topic", vec![0.9, 0.0, 0.2]),
    ];

    for node in nodes {
        engine.insert_node(node).await.unwrap();
    }

    engine
        .insert_edge(create_edge("AI", "ML", "RELATED_TO"))
        .await
        .unwrap();
    engine
        .insert_edge(create_edge("ML", "DL", "RELATED_TO"))
        .await
        .unwrap();
    engine
        .insert_edge(create_edge("AI", "NLP", "SIMILAR_TO"))
        .await
        .unwrap();

    // Step 1: Semantic traversal
    let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));
    let semantic_bfs = SemanticBFSTraversal::new(
        Arc::clone(&engine) as Arc<dyn GraphEngine>,
        Arc::clone(&distance_compute),
        0.7, // Relaxed threshold
        DistanceMetric::Cosine,
    );

    let query_embedding = vec![0.95, 0.05, 0.0];
    let input = SemanticTraversalInput {
        start_node: "AI".to_string(),
        query_embedding: query_embedding.clone(),
        max_depth: 2,
    };

    let candidates = semantic_bfs.execute(input).unwrap();
    println!("Found {} candidates", candidates.matches_found);

    // Step 2: Hybrid ranking
    let centrality_cache = Arc::new(DashMap::new());
    centrality_cache.insert("AI".to_string(), 0.9);
    centrality_cache.insert("ML".to_string(), 0.85);
    centrality_cache.insert("DL".to_string(), 0.7);
    centrality_cache.insert("NLP".to_string(), 0.5);

    let strategy = HybridRankingStrategy::balanced(distance_compute, centrality_cache);

    let context = RankingContext {
        query_embedding,
        distance_metric: DistanceMetric::Cosine,
    };

    let mut ranked: Vec<_> = candidates
        .results
        .iter()
        .map(|m| {
            let score = strategy.compute_score(&m.node, &context).unwrap();
            (m.node.id.clone(), score)
        })
        .collect();

    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    println!("Ranked results:");
    for (id, score) in &ranked {
        println!("  {}: {:.3}", id, score);
    }

    assert!(!ranked.is_empty());
    assert!(ranked[0].1 > 0.5); // Top result should be relevant
}
