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

//! Hybrid ranking strategies for graph search results
//!
//! This module provides trait-based ranking strategies that compose:
//! - Vector similarity scores (from UnifiedDistanceCompute)
//! - Graph centrality scores (PageRank, closeness, etc.)
//! - Custom relevance signals
//!
//! # Design Principles
//!
//! 1. **Reuse**: Leverages UnifiedDistanceCompute for SIMD-accelerated similarity
//! 2. **Trait-Based**: Extensible RankingStrategy trait for custom strategies
//! 3. **Caching**: Uses DashMap for concurrent centrality score caching
//! 4. **Composition**: Combines multiple signals with configurable weights
//!
//! # Example
//!
//! ```rust,ignore
//! use proximadb::graph::hybrid::ranking::{HybridRankingStrategy, RankingContext};
//!
//! let strategy = HybridRankingStrategy::new(
//!     0.7, // vector_weight
//!     0.3, // graph_weight
//!     distance_compute,
//!     centrality_cache,
//! );
//!
//! let context = RankingContext {
//!     query_embedding: vec![0.1; 768],
//!     distance_metric: DistanceMetric::Cosine,
//! };
//!
//! let score = strategy.compute_score(&node, &context)?;
//! ```

use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::graph::Node;
use crate::proto::proximadb_v1::DistanceMetric;
use dashmap::DashMap;
use proximadb_kernel::error::ProximaDBError;
use std::sync::Arc;

/// Type alias for node identifiers
pub type NodeId = String;

/// Context for ranking computations
///
/// Contains query-specific information needed for scoring:
/// - Query embedding for vector similarity
/// - Distance metric (cosine, L2, dot product)
/// - Optional metadata filters
#[derive(Clone)]
pub struct RankingContext {
    /// Query embedding vector (e.g., 768-dim from BERT)
    pub query_embedding: Vec<f32>,
    /// Distance metric to use for similarity computation
    pub distance_metric: DistanceMetric,
}

/// Trait for ranking strategies
///
/// Allows composition of multiple signals (vector similarity, graph centrality, etc.)
/// into a single relevance score.
///
/// # Design
///
/// - **Single Responsibility**: Each strategy implements one ranking approach
/// - **Open-Closed**: New strategies without modifying existing code
/// - **Dependency Inversion**: Depends on trait, not concrete implementations
pub trait RankingStrategy: Send + Sync {
    /// Compute relevance score for a node given a query context
    ///
    /// # Arguments
    ///
    /// * `node` - The graph node to score
    /// * `context` - Query context (embedding, metric, filters)
    ///
    /// # Returns
    ///
    /// Relevance score (typically 0.0 to 1.0, but can exceed 1.0 for weighted combinations)
    fn compute_score(&self, node: &Node, context: &RankingContext) -> Result<f64, ProximaDBError>;

    /// Get a human-readable name for this strategy
    fn strategy_name(&self) -> &str;
}

/// Hybrid ranking strategy combining vector similarity and graph centrality
///
/// Computes a weighted combination:
/// ```text
/// score = vector_weight * similarity(query, node) + graph_weight * centrality(node)
/// ```
///
/// # Design
///
/// - **REUSE**: UnifiedDistanceCompute for SIMD-accelerated similarity
/// - **REUSE**: Centrality cache for pre-computed PageRank/closeness scores
/// - **Thread-Safe**: Uses Arc for shared state, DashMap for concurrent access
pub struct HybridRankingStrategy {
    /// Weight for vector similarity component (0.0 to 1.0)
    vector_weight: f64,
    /// Weight for graph centrality component (0.0 to 1.0)
    graph_weight: f64,
    /// SIMD-accelerated distance computation engine
    distance_compute: Arc<UnifiedDistanceCompute>,
    /// Concurrent cache of centrality scores (NodeId -> score)
    ///
    /// Populated by pre-computing PageRank, closeness, or other centrality metrics
    centrality_cache: Arc<DashMap<NodeId, f64>>,
}

impl HybridRankingStrategy {
    /// Create a new hybrid ranking strategy
    ///
    /// # Arguments
    ///
    /// * `vector_weight` - Weight for vector similarity (0.0 to 1.0)
    /// * `graph_weight` - Weight for graph centrality (0.0 to 1.0)
    /// * `distance_compute` - SIMD-accelerated distance engine
    /// * `centrality_cache` - Pre-computed centrality scores
    ///
    /// # Note
    ///
    /// Weights don't need to sum to 1.0. For example:
    /// - (0.7, 0.3): Favor vector similarity
    /// - (0.5, 0.5): Equal weighting
    /// - (0.0, 1.0): Pure graph ranking (ignore vectors)
    /// - (1.0, 0.0): Pure vector ranking (ignore graph structure)
    pub fn new(
        vector_weight: f64,
        graph_weight: f64,
        distance_compute: Arc<UnifiedDistanceCompute>,
        centrality_cache: Arc<DashMap<NodeId, f64>>,
    ) -> Self {
        Self {
            vector_weight,
            graph_weight,
            distance_compute,
            centrality_cache,
        }
    }

    /// Create a strategy with equal weighting (0.5, 0.5)
    pub fn balanced(
        distance_compute: Arc<UnifiedDistanceCompute>,
        centrality_cache: Arc<DashMap<NodeId, f64>>,
    ) -> Self {
        Self::new(0.5, 0.5, distance_compute, centrality_cache)
    }

    /// Create a vector-only strategy (1.0, 0.0)
    pub fn vector_only(distance_compute: Arc<UnifiedDistanceCompute>) -> Self {
        Self::new(1.0, 0.0, distance_compute, Arc::new(DashMap::new()))
    }

    /// Create a graph-only strategy (0.0, 1.0)
    pub fn graph_only(centrality_cache: Arc<DashMap<NodeId, f64>>) -> Self {
        // Create dummy distance compute (won't be used)
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));
        Self::new(0.0, 1.0, distance_compute, centrality_cache)
    }
}

impl RankingStrategy for HybridRankingStrategy {
    fn compute_score(&self, node: &Node, context: &RankingContext) -> Result<f64, ProximaDBError> {
        // Compute vector similarity score (REUSE UnifiedDistanceCompute)
        // Skip computation if vector_weight is 0.0 to avoid NaN issues with zero embeddings
        let vector_score = if self.vector_weight > 0.0 {
            if let Some(embedding) = &node.embedding {
                // Use calculate_distance which returns SimilarityResult with semantic normalization
                // This returns normalized_score (0-1 where 1 = most similar) that's consistent
                // across all distance metrics
                let similarity_result = self.distance_compute.calculate_distance(
                    &context.query_embedding,
                    &embedding.vector,
                    &context.distance_metric,
                );

                // Use normalized_score: 1.0 = most similar, 0.0 = least similar
                // This is semantically consistent and handles all metrics correctly
                similarity_result.normalized_score as f64
            } else {
                // No embedding: zero vector score
                0.0
            }
        } else {
            // Vector weight is 0.0, skip computation
            0.0
        };

        // Get graph centrality score from cache (REUSE pre-computed centrality)
        let graph_score = self
            .centrality_cache
            .get(&node.id)
            .map_or(0.0, |entry| *entry);

        // Combine scores with weights
        let combined_score = self.vector_weight * vector_score + self.graph_weight * graph_score;

        Ok(combined_score)
    }

    fn strategy_name(&self) -> &str {
        "HybridRankingStrategy"
    }
}

/// Pure vector similarity ranking strategy
///
/// Ranks nodes solely by vector similarity to query embedding.
/// Useful for semantic search without graph structure influence.
pub struct VectorSimilarityStrategy {
    distance_compute: Arc<UnifiedDistanceCompute>,
}

impl VectorSimilarityStrategy {
    /// Create a new vector similarity ranking strategy with the given distance computation engine.
    pub fn new(distance_compute: Arc<UnifiedDistanceCompute>) -> Self {
        Self { distance_compute }
    }
}

impl RankingStrategy for VectorSimilarityStrategy {
    fn compute_score(&self, node: &Node, context: &RankingContext) -> Result<f64, ProximaDBError> {
        if let Some(embedding) = &node.embedding {
            // Use calculate_distance for semantic consistency
            let similarity_result = self.distance_compute.calculate_distance(
                &context.query_embedding,
                &embedding.vector,
                &context.distance_metric,
            );

            // Use normalized_score (1.0 = most similar, consistent across all metrics)
            Ok(similarity_result.normalized_score as f64)
        } else {
            Ok(0.0)
        }
    }

    fn strategy_name(&self) -> &str {
        "VectorSimilarityStrategy"
    }
}

/// Pure graph centrality ranking strategy
///
/// Ranks nodes solely by pre-computed centrality scores (PageRank, closeness, etc.).
/// Useful for finding structurally important nodes.
pub struct GraphCentralityStrategy {
    centrality_cache: Arc<DashMap<NodeId, f64>>,
}

impl GraphCentralityStrategy {
    /// Create a new graph centrality ranking strategy backed by a pre-computed centrality cache.
    pub fn new(centrality_cache: Arc<DashMap<NodeId, f64>>) -> Self {
        Self { centrality_cache }
    }
}

impl RankingStrategy for GraphCentralityStrategy {
    fn compute_score(&self, node: &Node, _context: &RankingContext) -> Result<f64, ProximaDBError> {
        Ok(self
            .centrality_cache
            .get(&node.id)
            .map_or(0.0, |entry| *entry))
    }

    fn strategy_name(&self) -> &str {
        "GraphCentralityStrategy"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::EmbeddingVersion;
    use std::collections::HashMap;

    fn create_test_node(id: &str, embedding: Option<Vec<f32>>) -> Node {
        Node {
            id: id.to_string(),
            labels: vec!["TestNode".to_string()],
            properties: HashMap::new(),
            embedding: embedding.map(|vec| EmbeddingVersion {
                model_id: "test-model".to_string(),
                model_version: "1.0".to_string(),
                vector: vec.clone(),
                dimension: vec.len() as u32,
                created_at_ms: 0,
                model_params: HashMap::new(),
                modality: 0, // Text modality
            }),
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    #[test]
    fn test_hybrid_ranking_balanced() {
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));
        let centrality_cache = Arc::new(DashMap::new());

        // Populate centrality cache
        centrality_cache.insert("node1".to_string(), 0.8); // High centrality
        centrality_cache.insert("node2".to_string(), 0.2); // Low centrality

        let strategy = HybridRankingStrategy::balanced(
            Arc::clone(&distance_compute),
            Arc::clone(&centrality_cache),
        );

        // Create nodes with embeddings
        let node1 = create_test_node("node1", Some(vec![1.0, 0.0, 0.0]));
        let node2 = create_test_node("node2", Some(vec![0.0, 1.0, 0.0]));

        let context = RankingContext {
            query_embedding: vec![1.0, 0.0, 0.0], // Close to node1
            distance_metric: DistanceMetric::Cosine,
        };

        let score1 = strategy
            .compute_score(&node1, &context)
            .map_err(|e| anyhow::anyhow!("Failed to compute score for node1: {}", e))
            .unwrap();
        let score2 = strategy
            .compute_score(&node2, &context)
            .map_err(|e| anyhow::anyhow!("Failed to compute score for node2: {}", e))
            .unwrap();

        // node1 should have higher score (high centrality + high similarity)
        assert!(score1 > score2);
        println!("node1 score: {}, node2 score: {}", score1, score2);
    }

    #[test]
    fn test_vector_only_strategy() {
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));
        let strategy = HybridRankingStrategy::vector_only(distance_compute);

        let node1 = create_test_node("node1", Some(vec![1.0, 0.0, 0.0]));
        let node2 = create_test_node("node2", Some(vec![0.0, 1.0, 0.0]));

        let context = RankingContext {
            query_embedding: vec![1.0, 0.0, 0.0],
            distance_metric: DistanceMetric::Cosine,
        };

        let score1 = strategy
            .compute_score(&node1, &context)
            .map_err(|e| anyhow::anyhow!("Failed to compute score for node1: {}", e))
            .unwrap();
        let score2 = strategy
            .compute_score(&node2, &context)
            .map_err(|e| anyhow::anyhow!("Failed to compute score for node2: {}", e))
            .unwrap();

        // node1 should have higher score (exact match)
        assert!(score1 > score2);
        // node1 should have perfect cosine similarity (1.0)
        assert!((score1 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_graph_only_strategy() {
        let centrality_cache = Arc::new(DashMap::new());
        centrality_cache.insert("node1".to_string(), 0.9);
        centrality_cache.insert("node2".to_string(), 0.1);

        let strategy = HybridRankingStrategy::graph_only(Arc::clone(&centrality_cache));

        // Embeddings don't matter for graph-only strategy
        let node1 = create_test_node("node1", Some(vec![0.0, 0.0, 0.0]));
        let node2 = create_test_node("node2", Some(vec![1.0, 1.0, 1.0]));

        let context = RankingContext {
            query_embedding: vec![1.0, 1.0, 1.0],
            distance_metric: DistanceMetric::Cosine,
        };

        let score1 = strategy
            .compute_score(&node1, &context)
            .map_err(|e| anyhow::anyhow!("Failed to compute score for node1: {}", e))
            .unwrap();
        let score2 = strategy
            .compute_score(&node2, &context)
            .map_err(|e| anyhow::anyhow!("Failed to compute score for node2: {}", e))
            .unwrap();

        println!("test_graph_only: score1={}, score2={}", score1, score2);

        // node1 should have higher score (higher centrality)
        assert!(score1 > score2);
        assert!((score1 - 0.9).abs() < 1e-6);
        assert!((score2 - 0.1).abs() < 1e-6);
    }

    #[test]
    fn test_nodes_without_embeddings() {
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));
        let centrality_cache = Arc::new(DashMap::new());
        centrality_cache.insert("node1".to_string(), 0.5);

        let strategy = HybridRankingStrategy::balanced(
            Arc::clone(&distance_compute),
            Arc::clone(&centrality_cache),
        );

        // Node without embedding
        let node = create_test_node("node1", None);

        let context = RankingContext {
            query_embedding: vec![1.0, 0.0, 0.0],
            distance_metric: DistanceMetric::Cosine,
        };

        let score = strategy
            .compute_score(&node, &context)
            .map_err(|e| anyhow::anyhow!("Failed to compute score for node: {}", e))
            .unwrap();

        // Should get graph score only (0.5 * 0.5 = 0.25)
        assert!((score - 0.25).abs() < 1e-6);
    }

    #[test]
    fn test_l2_distance_conversion() {
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));

        let strategy = HybridRankingStrategy::vector_only(distance_compute);

        let node1 = create_test_node("node1", Some(vec![0.0, 0.0, 0.0])); // Distance 0 from origin
        let node2 = create_test_node("node2", Some(vec![1.0, 0.0, 0.0])); // Distance 1 from origin

        let context = RankingContext {
            query_embedding: vec![0.0, 0.0, 0.0],
            distance_metric: DistanceMetric::Euclidean,
        };

        let score1 = strategy
            .compute_score(&node1, &context)
            .map_err(|e| anyhow::anyhow!("Failed to compute score for node1: {}", e))
            .unwrap();
        let score2 = strategy
            .compute_score(&node2, &context)
            .map_err(|e| anyhow::anyhow!("Failed to compute score for node2: {}", e))
            .unwrap();

        // node1 should have higher score (closer in L2 space)
        assert!(score1 > score2);
        // Distance 0 should give similarity 1.0
        assert!((score1 - 1.0).abs() < 1e-6);
        // Distance 1 should give similarity 0.5
        assert!((score2 - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_strategy_names() {
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));
        let centrality_cache = Arc::new(DashMap::new());

        let hybrid = HybridRankingStrategy::balanced(
            Arc::clone(&distance_compute),
            Arc::clone(&centrality_cache),
        );
        let vector_only = VectorSimilarityStrategy::new(Arc::clone(&distance_compute));
        let graph_only = GraphCentralityStrategy::new(Arc::clone(&centrality_cache));

        assert_eq!(hybrid.strategy_name(), "HybridRankingStrategy");
        assert_eq!(vector_only.strategy_name(), "VectorSimilarityStrategy");
        assert_eq!(graph_only.strategy_name(), "GraphCentralityStrategy");
    }
}
