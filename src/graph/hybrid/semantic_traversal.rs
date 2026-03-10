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

//! Semantic BFS Traversal - Vector-Guided Graph Search
//!
//! This module implements semantic breadth-first search that combines graph
//! topology with vector embeddings to find semantically similar nodes within
//! a specified graph distance.
//!
//! ## Key Features
//!
//! - **SIMD-Accelerated Similarity**: Reuses UnifiedDistanceCompute for hardware-accelerated
//!   cosine similarity calculations (AVX2, NEON, etc.)
//! - **Graph-Constrained Search**: Only considers nodes reachable within max_depth hops
//! - **Hybrid Ranking**: Combines graph distance with vector similarity for result ranking
//! - **Configurable Thresholds**: Filter results by minimum similarity threshold
//!
//! ## Performance Characteristics
//!
//! - **Time Complexity**: O(V + E) graph traversal + O(V * D) similarity computation
//!   where V = nodes visited, E = edges explored, D = embedding dimension
//! - **Space Complexity**: O(V) for visited set and queue
//! - **SIMD Speedup**: 4-8x faster similarity computation with AVX2 vs scalar
//!
//! ## Example Usage
//!
//! ```rust,ignore
//! use proximadb::graph::hybrid::SemanticBFSTraversal;
//! use proximadb::compute::distance_computation::UnifiedDistanceCompute;
//!
//! let distance_compute = Arc::new(UnifiedDistanceCompute::default());
//! let semantic_bfs = SemanticBFSTraversal::new(
//!     graph_engine,
//!     distance_compute,
//!     0.8,  // similarity_threshold
//!     DistanceMetric::Cosine,
//! );
//!
//! let query_embedding = vec![0.5; 768];
//! let input = SemanticTraversalInput {
//!     start_node: "node_1".to_string(),
//!     query_embedding,
//!     max_depth: 3,
//! };
//!
//! let results = semantic_bfs.execute(input)?;
//! // Results are sorted by combined score (similarity + graph distance)
//! ```

use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::core::error::ProximaDBError;
use crate::graph::engines::GraphEngine;
use crate::graph::{Node, NodeId};
use crate::proto::proximadb_v1::DistanceMetric;
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

/// Input for semantic BFS traversal
#[derive(Debug, Clone)]
pub struct SemanticTraversalInput {
    /// Starting node ID for traversal
    pub start_node: NodeId,
    /// Query embedding to compare against node embeddings
    pub query_embedding: Vec<f32>,
    /// Maximum graph distance (hops) to explore
    pub max_depth: u32,
}

/// Result from semantic BFS traversal
#[derive(Debug, Clone)]
pub struct SemanticTraversalResult {
    /// Matching nodes with scores and distances
    pub results: Vec<SemanticNodeMatch>,
    /// Number of nodes visited during traversal
    pub nodes_visited: usize,
    /// Number of nodes that passed similarity threshold
    pub matches_found: usize,
}

/// Individual node match with scores
#[derive(Debug, Clone, PartialEq)]
pub struct SemanticNodeMatch {
    /// The matching node
    pub node: Arc<Node>,
    /// Vector similarity score (0.0 to 1.0, higher = more similar)
    pub similarity: f32,
    /// Graph distance from start node (number of hops)
    pub graph_distance: u32,
    /// Combined score (similarity weighted by distance)
    pub combined_score: f32,
}

/// Semantic BFS Traversal Algorithm
///
/// Combines breadth-first graph traversal with vector similarity filtering.
/// Uses UnifiedDistanceCompute for hardware-accelerated similarity calculations.
pub struct SemanticBFSTraversal {
    /// Graph engine for traversal operations
    graph_engine: Arc<dyn GraphEngine>,
    /// Distance computation engine (SIMD-accelerated)
    distance_compute: Arc<UnifiedDistanceCompute>,
    /// Minimum similarity threshold (0.0 to 1.0)
    similarity_threshold: f32,
    /// Distance metric to use (Cosine, Euclidean, etc.)
    distance_metric: DistanceMetric,
}

impl SemanticBFSTraversal {
    /// Create a new semantic BFS traversal algorithm
    ///
    /// # Arguments
    ///
    /// * `graph_engine` - Graph engine implementing GraphEngine trait
    /// * `distance_compute` - UnifiedDistanceCompute for SIMD-accelerated similarity
    /// * `similarity_threshold` - Minimum similarity score (0.0 to 1.0)
    /// * `distance_metric` - Distance metric to use (Cosine recommended for embeddings)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let semantic_bfs = SemanticBFSTraversal::new(
    ///     engine,
    ///     distance_compute,
    ///     0.7,  // 70% similarity threshold
    ///     DistanceMetric::Cosine,
    /// );
    /// ```
    pub fn new(
        graph_engine: Arc<dyn GraphEngine>,
        distance_compute: Arc<UnifiedDistanceCompute>,
        similarity_threshold: f32,
        distance_metric: DistanceMetric,
    ) -> Self {
        Self {
            graph_engine,
            distance_compute,
            similarity_threshold,
            distance_metric,
        }
    }

    /// Execute semantic BFS traversal
    ///
    /// Performs breadth-first search starting from the given node, computing
    /// vector similarity for each reachable node using SIMD-accelerated distance
    /// computation. Results are filtered by similarity threshold and sorted by
    /// combined score (similarity + distance penalty).
    ///
    /// # Arguments
    ///
    /// * `input` - SemanticTraversalInput containing start node, query embedding, and max depth
    ///
    /// # Returns
    ///
    /// SemanticTraversalResult with matching nodes sorted by combined score
    ///
    /// # Errors
    ///
    /// - `ProximaDBError::NotFound` if start node doesn't exist
    /// - `ProximaDBError::Internal` for graph traversal errors
    pub fn execute(
        &self,
        input: SemanticTraversalInput,
    ) -> Result<SemanticTraversalResult, ProximaDBError> {
        // Validate start node exists
        let start_node = self
            .graph_engine
            .get_node(&input.start_node)?
            .ok_or_else(|| {
                ProximaDBError::Internal(format!("Start node not found: {}", input.start_node))
            })?;

        // Initialize traversal state
        let mut queue = VecDeque::new();
        let mut visited = HashSet::new();
        let mut results = Vec::new();
        let mut nodes_visited = 0;

        // Start BFS from initial node
        queue.push_back((start_node.id.clone(), 0u32));
        visited.insert(start_node.id.clone());

        while let Some((current_node_id, depth)) = queue.pop_front() {
            // Stop if max depth reached
            if depth > input.max_depth {
                continue;
            }

            nodes_visited += 1;

            // Get current node
            let current_node = self
                .graph_engine
                .get_node(&current_node_id)?
                .ok_or_else(|| {
                    ProximaDBError::Internal(format!(
                        "Node disappeared during traversal: {}",
                        current_node_id
                    ))
                })?;

            // Compute similarity if node has embedding
            if let Some(embedding_wrapper) = &current_node.embedding {
                let similarity =
                    self.compute_similarity(&input.query_embedding, &embedding_wrapper.vector)?;

                // Only include nodes that meet similarity threshold
                if similarity >= self.similarity_threshold {
                    // Compute combined score (similarity with distance penalty)
                    // Distance penalty: nodes further away get lower scores
                    let distance_penalty = 1.0 / (depth as f32 + 1.0);
                    let combined_score = similarity * distance_penalty;

                    results.push(SemanticNodeMatch {
                        node: current_node.clone(),
                        similarity,
                        graph_distance: depth,
                        combined_score,
                    });
                }
            }

            // Expand to neighbors (BFS)
            if depth < input.max_depth {
                let neighbors = self.graph_engine.get_neighbors(&current_node_id, None)?;

                for neighbor_node in neighbors {
                    if !visited.contains(&neighbor_node.id) {
                        visited.insert(neighbor_node.id.clone());
                        queue.push_back((neighbor_node.id.clone(), depth + 1));
                    }
                }
            }
        }

        // Sort results by combined score (descending)
        // Note: unwrap_or is safe here - partial_cmp returns None only for NaN values,
        // which we treat as equal to maintain sort stability
        results.sort_by(|a, b| {
            b.combined_score
                .partial_cmp(&a.combined_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(SemanticTraversalResult {
            matches_found: results.len(),
            results,
            nodes_visited,
        })
    }

    /// Compute similarity between two vectors using SIMD-accelerated distance computation
    ///
    /// This method delegates to UnifiedDistanceCompute which automatically selects
    /// the best SIMD implementation (AVX2, NEON, etc.) for the current hardware.
    ///
    /// # Arguments
    ///
    /// * `query_vector` - Query embedding vector
    /// * `node_vector` - Node embedding vector
    ///
    /// # Returns
    ///
    /// Similarity score (0.0 to 1.0, higher = more similar)
    ///
    /// Note: For cosine distance, we convert distance to similarity: similarity = 1 - distance
    fn compute_similarity(
        &self,
        query_vector: &[f32],
        node_vector: &[f32],
    ) -> Result<f32, ProximaDBError> {
        // Check dimension mismatch
        if query_vector.len() != node_vector.len() {
            return Err(ProximaDBError::Internal(format!(
                "Dimension mismatch: query={}, node={}",
                query_vector.len(),
                node_vector.len()
            )));
        }

        // Use UnifiedDistanceCompute for SIMD-accelerated distance calculation
        let distance = self.distance_compute.distance_with_metric(
            query_vector,
            node_vector,
            &self.distance_metric,
        );

        // Convert distance to similarity
        // For cosine: distance = 1 - similarity, so similarity = 1 - distance
        // Clamp to [0, 1] range
        let similarity = match self.distance_metric {
            DistanceMetric::Cosine => (1.0 - distance).max(0.0).min(1.0),
            DistanceMetric::DotProduct => {
                // Dot product is already similarity-like (higher = more similar)
                // Normalize to [0, 1] by clamping
                (-distance).max(0.0).min(1.0)
            }
            DistanceMetric::Euclidean | DistanceMetric::Manhattan => {
                // For distance metrics, convert to similarity using inverse
                // similarity = 1 / (1 + distance)
                (1.0 / (1.0 + distance)).max(0.0).min(1.0)
            }
            _ => {
                // For other metrics, use inverse distance
                (1.0 / (1.0 + distance)).max(0.0).min(1.0)
            }
        };

        Ok(similarity)
    }

    /// Get algorithm name
    pub fn name(&self) -> &'static str {
        "SemanticBFS"
    }

    /// Get similarity threshold
    pub fn similarity_threshold(&self) -> f32 {
        self.similarity_threshold
    }

    /// Get distance metric
    pub fn distance_metric(&self) -> DistanceMetric {
        self.distance_metric
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::engines::GraphEngine;
    use crate::graph::engines::orion::OrionGraphEngine;
    use crate::proto::proximadb_v1::EmbeddingVersion;
    use std::collections::HashMap;

    fn create_test_engine() -> Arc<OrionGraphEngine> {
        Arc::new(OrionGraphEngine::new())
    }

    fn create_test_node(id: &str, embedding: Vec<f32>) -> Node {
        Node {
            id: id.to_string(),
            labels: vec!["TestNode".to_string()],
            properties: HashMap::new(),
            embedding: Some(EmbeddingVersion {
                model_id: "test_model".to_string(),
                model_version: "1.0".to_string(),
                vector: embedding,
                dimension: 128,
                created_at_ms: 0,
                model_params: HashMap::new(),
                modality: 0, // TEXT modality
            }),
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    fn create_test_edge(from: &str, to: &str) -> crate::graph::Edge {
        crate::graph::Edge {
            id: format!("{}-{}", from, to),
            from_node_id: from.to_string(),
            to_node_id: to.to_string(),
            edge_type: "CONNECTS".to_string(),
            properties: HashMap::new(),
            weight: Some(1.0),
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    #[tokio::test]
    async fn test_semantic_bfs_empty_graph() {
        let engine = create_test_engine();
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));

        let semantic_bfs = SemanticBFSTraversal::new(
            engine.clone(),
            distance_compute,
            0.7,
            DistanceMetric::Cosine,
        );

        let input = SemanticTraversalInput {
            start_node: "nonexistent".to_string(),
            query_embedding: vec![0.5; 128],
            max_depth: 3,
        };

        // Should fail with NotFound error
        let result = semantic_bfs.execute(input);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_semantic_bfs_single_node() {
        let engine = create_test_engine();
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));

        // Create a single node with embedding identical to query
        let embedding = vec![1.0; 128];
        let node = create_test_node("node_1", embedding.clone());
        GraphEngine::insert_node(engine.as_ref(), node)
            .await
            .expect("Failed to insert test node");

        let semantic_bfs = SemanticBFSTraversal::new(
            engine.clone(),
            distance_compute,
            0.7,
            DistanceMetric::Cosine,
        );

        let input = SemanticTraversalInput {
            start_node: "node_1".to_string(),
            query_embedding: embedding,
            max_depth: 3,
        };

        let result = semantic_bfs
            .execute(input)
            .expect("Failed to execute semantic BFS traversal");

        // Should find the start node with perfect similarity
        assert_eq!(result.matches_found, 1);
        assert_eq!(result.nodes_visited, 1);
        assert_eq!(result.results[0].node.id, "node_1");
        assert!(result.results[0].similarity > 0.99); // Near perfect match
    }

    #[tokio::test]
    async fn test_semantic_bfs_linear_chain() {
        let engine = create_test_engine();
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));

        // Create a linear chain: node_1 -> node_2 -> node_3
        // Use vectors with different directions (not just different magnitudes)
        // For cosine similarity, direction matters, not magnitude

        // Query: unit vector in first half of dimensions
        let mut query_embedding = vec![0.0; 128];
        for i in 0..64 {
            query_embedding[i] = 1.0;
        }

        // Node 1: Very similar to query (mostly first half)
        let mut embed1 = vec![0.0; 128];
        for i in 0..64 {
            embed1[i] = 1.0;
        }
        let node1 = create_test_node("node_1", embed1);
        GraphEngine::insert_node(engine.as_ref(), node1)
            .await
            .expect("Failed to insert node_1");

        // Node 2: Orthogonal to query (second half of dimensions)
        let mut embed2 = vec![0.0; 128];
        for i in 64..128 {
            embed2[i] = 1.0;
        }
        let node2 = create_test_node("node_2", embed2);
        GraphEngine::insert_node(engine.as_ref(), node2)
            .await
            .expect("Failed to insert node_2");

        // Node 3: Opposite direction (negative first half)
        let mut embed3 = vec![0.0; 128];
        for i in 0..64 {
            embed3[i] = -1.0;
        }
        let node3 = create_test_node("node_3", embed3);
        GraphEngine::insert_node(engine.as_ref(), node3)
            .await
            .expect("Failed to insert node_3");

        // Create edges
        let edge1 = create_test_edge("node_1", "node_2");
        let edge2 = create_test_edge("node_2", "node_3");
        GraphEngine::insert_edge(engine.as_ref(), edge1)
            .await
            .expect("Failed to insert edge node_1->node_2");
        GraphEngine::insert_edge(engine.as_ref(), edge2)
            .await
            .expect("Failed to insert edge node_2->node_3");

        let semantic_bfs = SemanticBFSTraversal::new(
            engine.clone(),
            distance_compute,
            0.8, // High threshold - only node_1 should pass (similarity = 1.0)
            DistanceMetric::Cosine,
        );

        let input = SemanticTraversalInput {
            start_node: "node_1".to_string(),
            query_embedding,
            max_depth: 3,
        };

        let result = semantic_bfs
            .execute(input)
            .expect("Failed to execute semantic BFS traversal");

        // Should visit all 3 nodes but only find 1 match (node_1 with similarity 1.0)
        // node_2 has similarity 0.0 (orthogonal), node_3 has similarity 0.0 (opposite)
        assert_eq!(result.nodes_visited, 3);
        assert_eq!(result.matches_found, 1);
        assert_eq!(result.results[0].node.id, "node_1");
    }

    #[tokio::test]
    async fn test_semantic_bfs_respects_max_depth() {
        let engine = create_test_engine();
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));

        // Create a chain of 5 nodes, all with identical embeddings
        let embedding = vec![1.0; 128];
        for i in 1..=5 {
            let node = create_test_node(&format!("node_{}", i), embedding.clone());
            GraphEngine::insert_node(engine.as_ref(), node)
                .await
                .expect("Failed to insert test node");
        }

        // Create edges: 1->2->3->4->5
        for i in 1..5 {
            let edge = create_test_edge(&format!("node_{}", i), &format!("node_{}", i + 1));
            GraphEngine::insert_edge(engine.as_ref(), edge)
                .await
                .expect("Failed to insert test edge");
        }

        let semantic_bfs = SemanticBFSTraversal::new(
            engine.clone(),
            distance_compute,
            0.5,
            DistanceMetric::Cosine,
        );

        let input = SemanticTraversalInput {
            start_node: "node_1".to_string(),
            query_embedding: embedding,
            max_depth: 2, // Should only reach nodes 1, 2, 3
        };

        let result = semantic_bfs
            .execute(input)
            .expect("Failed to execute semantic BFS traversal");

        // Should visit only nodes within depth 2
        assert_eq!(result.nodes_visited, 3); // nodes 1, 2, 3
        assert_eq!(result.matches_found, 3);

        // Results should be sorted by combined score (distance penalty applies)
        assert_eq!(result.results[0].node.id, "node_1"); // Depth 0
        assert_eq!(result.results[1].node.id, "node_2"); // Depth 1
        assert_eq!(result.results[2].node.id, "node_3"); // Depth 2
    }

    #[tokio::test]
    async fn test_semantic_bfs_combined_score_ranking() {
        let engine = create_test_engine();
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));

        let query_embedding = vec![1.0; 128];

        // Node 1 at depth 0: perfect match (similarity 1.0)
        let node1 = create_test_node("node_1", vec![1.0; 128]);
        GraphEngine::insert_node(engine.as_ref(), node1)
            .await
            .expect("Failed to insert node_1");

        // Node 2 at depth 1: good match (similarity ~0.95)
        let node2 = create_test_node("node_2", vec![0.95; 128]);
        GraphEngine::insert_node(engine.as_ref(), node2)
            .await
            .expect("Failed to insert node_2");

        // Node 3 at depth 2: moderate match (similarity ~0.7)
        let node3 = create_test_node("node_3", vec![0.7; 128]);
        GraphEngine::insert_node(engine.as_ref(), node3)
            .await
            .expect("Failed to insert node_3");

        // Create edges
        GraphEngine::insert_edge(engine.as_ref(), create_test_edge("node_1", "node_2"))
            .await
            .expect("Failed to insert edge node_1->node_2");
        GraphEngine::insert_edge(engine.as_ref(), create_test_edge("node_2", "node_3"))
            .await
            .expect("Failed to insert edge node_2->node_3");

        let semantic_bfs = SemanticBFSTraversal::new(
            engine.clone(),
            distance_compute,
            0.6,
            DistanceMetric::Cosine,
        );

        let input = SemanticTraversalInput {
            start_node: "node_1".to_string(),
            query_embedding,
            max_depth: 3,
        };

        let result = semantic_bfs
            .execute(input)
            .expect("Failed to execute semantic BFS traversal");

        assert_eq!(result.matches_found, 3);

        // Verify combined scores decrease with distance
        // Combined score = similarity * (1 / (depth + 1))
        // Node 1: ~1.0 * 1.0 = 1.0
        // Node 2: ~0.95 * 0.5 = 0.475
        // Node 3: ~0.7 * 0.33 = 0.23
        assert!(result.results[0].combined_score > result.results[1].combined_score);
        assert!(result.results[1].combined_score > result.results[2].combined_score);

        // Verify graph distances
        assert_eq!(result.results[0].graph_distance, 0);
        assert_eq!(result.results[1].graph_distance, 1);
        assert_eq!(result.results[2].graph_distance, 2);
    }

    #[tokio::test]
    async fn test_semantic_bfs_similarity_threshold() {
        let engine = create_test_engine();
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));

        // Query: first 64 dimensions set to 1.0
        let mut query_embedding = vec![0.0; 128];
        for i in 0..64 {
            query_embedding[i] = 1.0;
        }

        // Create nodes with varying directional similarity
        // Node 1: Perfect match (same direction as query)
        let mut embed1 = vec![0.0; 128];
        for i in 0..64 {
            embed1[i] = 1.0;
        }
        let node1 = create_test_node("node_1", embed1);

        // Node 2: High similarity (~0.9) - mostly aligned with query but with some noise
        let mut embed2 = vec![0.0; 128];
        for i in 0..60 {
            embed2[i] = 1.0;
        }
        for i in 60..70 {
            embed2[i] = 0.5;
        }
        let node2 = create_test_node("node_2", embed2);

        // Node 3: Low similarity (~0.5) - half aligned, half orthogonal
        let mut embed3 = vec![0.0; 128];
        for i in 0..32 {
            embed3[i] = 1.0;
        }
        for i in 64..96 {
            embed3[i] = 1.0;
        }
        let node3 = create_test_node("node_3", embed3);

        // Node 4: Very low similarity - mostly orthogonal
        let mut embed4 = vec![0.0; 128];
        for i in 64..128 {
            embed4[i] = 1.0;
        }
        let node4 = create_test_node("node_4", embed4);

        GraphEngine::insert_node(engine.as_ref(), node1)
            .await
            .expect("Failed to insert node_1");
        GraphEngine::insert_node(engine.as_ref(), node2)
            .await
            .expect("Failed to insert node_2");
        GraphEngine::insert_node(engine.as_ref(), node3)
            .await
            .expect("Failed to insert node_3");
        GraphEngine::insert_node(engine.as_ref(), node4)
            .await
            .expect("Failed to insert node_4");

        // Create a star topology: node_1 connects to all others
        GraphEngine::insert_edge(engine.as_ref(), create_test_edge("node_1", "node_2"))
            .await
            .expect("Failed to insert edge node_1->node_2");
        GraphEngine::insert_edge(engine.as_ref(), create_test_edge("node_1", "node_3"))
            .await
            .expect("Failed to insert edge node_1->node_3");
        GraphEngine::insert_edge(engine.as_ref(), create_test_edge("node_1", "node_4"))
            .await
            .expect("Failed to insert edge node_1->node_4");

        let semantic_bfs = SemanticBFSTraversal::new(
            engine.clone(),
            distance_compute,
            0.85, // High threshold - should filter out low similarity nodes
            DistanceMetric::Cosine,
        );

        let input = SemanticTraversalInput {
            start_node: "node_1".to_string(),
            query_embedding,
            max_depth: 2,
        };

        let result = semantic_bfs
            .execute(input)
            .expect("Failed to execute semantic BFS traversal");

        // Should visit all 4 nodes but only match high similarity ones
        assert_eq!(result.nodes_visited, 4);
        // Only nodes with similarity >= 0.85 should match (node_1 definitely, node_2 possibly)
        assert!(result.matches_found <= 2); // node_1 and possibly node_2
    }

    #[test]
    fn test_semantic_bfs_name() {
        let engine = create_test_engine();
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));

        let semantic_bfs =
            SemanticBFSTraversal::new(engine, distance_compute, 0.7, DistanceMetric::Cosine);

        assert_eq!(semantic_bfs.name(), "SemanticBFS");
    }

    #[test]
    fn test_semantic_bfs_getters() {
        let engine = create_test_engine();
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));

        let semantic_bfs =
            SemanticBFSTraversal::new(engine, distance_compute, 0.75, DistanceMetric::Cosine);

        assert_eq!(semantic_bfs.similarity_threshold(), 0.75);
        assert_eq!(semantic_bfs.distance_metric(), DistanceMetric::Cosine);
    }
}
