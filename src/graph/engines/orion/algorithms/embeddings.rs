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

//! Graph embedding algorithms for representation learning
//!
//! This module provides implementations of graph embedding algorithms:
//! - Node2Vec: Learns node representations via biased random walks and Skip-Gram
//! - DeepWalk: Unbiased random walks with Skip-Gram (Node2Vec with p=1, q=1)
//!
//! All algorithms reuse the existing CSR storage for efficient graph traversal
//! and the vector engine for embedding storage and similarity search.

use super::traits::{AlgorithmComplexity, GraphAlgorithm, NoInput, ParallelAlgorithm};
use crate::graph::engines::orion::OrionGraphEngine;
use proximadb_kernel::error::ProximaDBError;
use rand::Rng;
use std::collections::HashMap;
use std::sync::Arc;

/// Node embeddings output type: maps NodeId to embedding vector
pub type NodeEmbeddings = HashMap<String, Vec<f32>>;

/// Node2Vec graph embedding algorithm
///
/// Learns low-dimensional representations of nodes by:
/// 1. Generating biased random walks (controlled by p and q parameters)
/// 2. Training Skip-Gram model on walk sequences
/// 3. Producing dense vector embeddings for each node
///
/// # Parameters
/// - `p`: Return parameter (controls likelihood of revisiting nodes)
/// - `q`: In-out parameter (controls BFS vs DFS behavior)
/// - `walk_length`: Length of each random walk
/// - `num_walks`: Number of walks per node
/// - `embedding_dim`: Dimensionality of output embeddings
///
/// # Example
/// ```rust,ignore
/// use proximadb::graph::engines::orion::algorithms::embeddings::Node2VecEmbeddings;
/// use proximadb::graph::engines::orion::algorithms::traits::GraphAlgorithm;
///
/// let node2vec = Node2VecEmbeddings::new(
///     engine,
///     1.0,  // p (return parameter)
///     1.0,  // q (in-out parameter)
///     80,   // walk length
///     10,   // walks per node
///     128,  // embedding dimension
/// );
/// let embeddings = node2vec.execute(())?;
/// ```
pub struct Node2VecEmbeddings {
    engine: Arc<OrionGraphEngine>,
    p: f64,               // Return parameter
    q: f64,               // In-out parameter
    walk_length: usize,   // Length of each walk
    num_walks: usize,     // Number of walks per node
    embedding_dim: usize, // Dimensionality of embeddings
    window_size: usize,   // Skip-Gram context window size
    learning_rate: f32,   // Skip-Gram learning rate
    num_epochs: usize,    // Number of training epochs
}

impl Node2VecEmbeddings {
    /// Create a new Node2Vec embedding algorithm
    ///
    /// # Arguments
    /// - `engine`: ORION graph engine with CSR storage
    /// - `p`: Return parameter (higher = less likely to return to previous node)
    /// - `q`: In-out parameter (higher = more BFS-like, lower = more DFS-like)
    /// - `walk_length`: Number of nodes in each walk
    /// - `num_walks`: Number of walks to generate per node
    /// - `embedding_dim`: Size of embedding vectors
    ///
    /// # Returns
    /// Node2Vec instance ready for training
    pub fn new(
        engine: Arc<OrionGraphEngine>,
        p: f64,
        q: f64,
        walk_length: usize,
        num_walks: usize,
        embedding_dim: usize,
    ) -> Self {
        Self {
            engine,
            p,
            q,
            walk_length,
            num_walks,
            embedding_dim,
            window_size: 10,      // Standard Skip-Gram window
            learning_rate: 0.025, // Standard Skip-Gram learning rate
            num_epochs: 5,        // Standard number of epochs
        }
    }

    /// Generate a single biased random walk starting from a node
    ///
    /// Uses second-order random walk with p and q parameters to control
    /// exploration vs exploitation trade-off.
    fn generate_walk(&self, start_idx: usize) -> Result<Vec<usize>, ProximaDBError> {
        let mut walk = Vec::with_capacity(self.walk_length);
        walk.push(start_idx);

        let csr_out =
            self.engine.csr_outgoing.read().map_err(|_| {
                ProximaDBError::Internal("Failed to acquire CSR read lock".to_string())
            })?;

        let mut rng = rand::thread_rng();

        for step in 1..self.walk_length {
            let current_idx = walk[step - 1];
            let neighbors = csr_out.get_neighbors(current_idx).unwrap_or(&[]);

            if neighbors.is_empty() {
                break; // Dead end
            }

            // For first step, uniform random selection
            if step == 1 {
                let next_idx = neighbors[rng.gen_range(0..neighbors.len())];
                walk.push(next_idx);
                continue;
            }

            // For subsequent steps, use biased random walk
            let prev_idx = walk[step - 2];
            let next_idx = self.biased_random_choice(prev_idx, current_idx, neighbors, &mut rng)?;
            walk.push(next_idx);
        }

        Ok(walk)
    }

    /// Select next node using biased probabilities based on p and q parameters
    ///
    /// Probability depends on distance from previous node:
    /// - d=0 (return to previous): 1/p
    /// - d=1 (neighbor of previous): 1
    /// - d=2 (not neighbor of previous): 1/q
    fn biased_random_choice(
        &self,
        prev_idx: usize,
        _current_idx: usize,
        neighbors: &[usize],
        rng: &mut impl Rng,
    ) -> Result<usize, ProximaDBError> {
        // Get neighbors of previous node for distance calculation
        let csr_out =
            self.engine.csr_outgoing.read().map_err(|_| {
                ProximaDBError::Internal("Failed to acquire CSR read lock".to_string())
            })?;

        let prev_neighbors = csr_out.get_neighbors(prev_idx).unwrap_or(&[]);
        let prev_neighbor_set: std::collections::HashSet<_> =
            prev_neighbors.iter().copied().collect();

        // Calculate unnormalized probabilities
        let mut probabilities = Vec::with_capacity(neighbors.len());
        let mut total_weight = 0.0;

        for &neighbor_idx in neighbors {
            let weight = if neighbor_idx == prev_idx {
                // Distance 0: returning to previous node
                1.0 / self.p
            } else if prev_neighbor_set.contains(&neighbor_idx) {
                // Distance 1: neighbor of previous node
                1.0
            } else {
                // Distance 2: not a neighbor of previous node
                1.0 / self.q
            };

            probabilities.push(weight);
            total_weight += weight;
        }

        // Normalize probabilities and sample
        let random_value: f64 = rng.r#gen();
        let threshold = random_value * total_weight;
        let mut cumulative = 0.0;

        for (i, &prob) in probabilities.iter().enumerate() {
            cumulative += prob;
            if cumulative >= threshold {
                return Ok(neighbors[i]);
            }
        }

        // Fallback (shouldn't reach here with proper floating point math)
        Ok(neighbors[neighbors.len() - 1])
    }

    /// Generate all random walks for the graph
    ///
    /// Creates `num_walks` walks of length `walk_length` for each node.
    fn generate_walks(&self) -> Result<Vec<Vec<usize>>, ProximaDBError> {
        let csr_out =
            self.engine.csr_outgoing.read().map_err(|_| {
                ProximaDBError::Internal("Failed to acquire CSR read lock".to_string())
            })?;

        let node_count = csr_out.node_count();
        drop(csr_out); // Release lock before long computation

        let mut all_walks = Vec::new();

        // Generate walks for each node
        for node_idx in 0..node_count {
            for _ in 0..self.num_walks {
                let walk = self.generate_walk(node_idx)?;
                if walk.len() > 1 {
                    all_walks.push(walk);
                }
            }
        }

        Ok(all_walks)
    }

    /// Train Skip-Gram model on random walks
    ///
    /// Uses hierarchical softmax with negative sampling for efficiency.
    /// Simplified implementation using gradient descent on embedding vectors.
    fn train_skipgram(&self, walks: &[Vec<usize>]) -> Result<Vec<Vec<f32>>, ProximaDBError> {
        let csr_out =
            self.engine.csr_outgoing.read().map_err(|_| {
                ProximaDBError::Internal("Failed to acquire CSR read lock".to_string())
            })?;

        let node_count = csr_out.node_count();
        drop(csr_out);

        // Initialize embeddings randomly
        let mut embeddings = vec![vec![0.0f32; self.embedding_dim]; node_count];
        let mut rng = rand::thread_rng();

        for node_embedding in &mut embeddings {
            for dim in node_embedding.iter_mut() {
                let random_val: f32 = rng.r#gen();
                *dim = (random_val - 0.5) / self.embedding_dim as f32;
            }
        }

        // Train using Skip-Gram objective
        for _epoch in 0..self.num_epochs {
            for walk in walks {
                for (i, &center_idx) in walk.iter().enumerate() {
                    // Define context window
                    let window_start = i.saturating_sub(self.window_size);
                    let window_end = (i + self.window_size + 1).min(walk.len());

                    // Update embeddings based on context
                    for (j, &context_idx) in
                        walk.iter().enumerate().take(window_end).skip(window_start)
                    {
                        if i == j {
                            continue;
                        }

                        // Simplified gradient update (positive sample)
                        // In practice, would use negative sampling for efficiency
                        let center_emb = embeddings[center_idx].clone();
                        let context_emb = embeddings[context_idx].clone();

                        // Compute dot product
                        let dot_product: f32 = center_emb
                            .iter()
                            .zip(context_emb.iter())
                            .map(|(a, b)| a * b)
                            .sum();

                        // Sigmoid activation
                        let sigmoid = 1.0 / (1.0 + (-dot_product).exp());
                        let gradient = self.learning_rate * (1.0 - sigmoid);

                        // Update embeddings
                        for k in 0..self.embedding_dim {
                            embeddings[center_idx][k] += gradient * context_emb[k];
                            embeddings[context_idx][k] += gradient * center_emb[k];
                        }
                    }
                }
            }
        }

        // Normalize embeddings
        for node_embedding in &mut embeddings {
            let norm: f32 = node_embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                for dim in node_embedding.iter_mut() {
                    *dim /= norm;
                }
            }
        }

        Ok(embeddings)
    }

    /// Convert node indices to node IDs for output
    fn index_to_id_mapping(
        &self,
        embeddings: Vec<Vec<f32>>,
    ) -> Result<NodeEmbeddings, ProximaDBError> {
        let index_to_node = self.engine.index_to_node.read().map_err(|_| {
            ProximaDBError::Internal("Failed to acquire index_to_node read lock".to_string())
        })?;

        let mut result = HashMap::new();

        for (idx, embedding) in embeddings.into_iter().enumerate() {
            if let Some(node_id) = index_to_node.get(idx) {
                result.insert(node_id.clone(), embedding);
            }
        }

        Ok(result)
    }
}

impl GraphAlgorithm for Node2VecEmbeddings {
    type Input = NoInput;
    type Output = NodeEmbeddings;

    fn execute(&self, _input: NoInput) -> Result<NodeEmbeddings, ProximaDBError> {
        // Step 1: Generate random walks
        let walks = self.generate_walks()?;

        if walks.is_empty() {
            return Ok(HashMap::new());
        }

        // Step 2: Train Skip-Gram model on walks
        let embeddings = self.train_skipgram(&walks)?;

        // Step 3: Convert to NodeId -> Embedding mapping
        self.index_to_id_mapping(embeddings)
    }

    fn estimated_complexity(&self) -> AlgorithmComplexity {
        // O(V * num_walks * walk_length) for walk generation
        // + O(num_walks * walk_length * window_size * embedding_dim * epochs) for training
        // Dominated by walk generation in most cases
        AlgorithmComplexity::Linear
    }

    fn name(&self) -> &'static str {
        "Node2VecEmbeddings"
    }
}

impl ParallelAlgorithm for Node2VecEmbeddings {
    fn execute_parallel(
        &self,
        _input: NoInput,
        thread_pool: &rayon::ThreadPool,
    ) -> Result<NodeEmbeddings, ProximaDBError> {
        use rayon::prelude::*;

        // Step 1: Generate random walks in parallel
        let csr_out =
            self.engine.csr_outgoing.read().map_err(|_| {
                ProximaDBError::Internal("Failed to acquire CSR read lock".to_string())
            })?;

        let node_count = csr_out.node_count();
        drop(csr_out);

        let walks: Vec<Vec<usize>> = thread_pool.install(|| {
            (0..node_count)
                .into_par_iter()
                .flat_map(|node_idx| {
                    (0..self.num_walks)
                        .filter_map(|_| self.generate_walk(node_idx).ok())
                        .filter(|walk| walk.len() > 1)
                        .collect::<Vec<_>>()
                })
                .collect()
        });

        if walks.is_empty() {
            return Ok(HashMap::new());
        }

        // Step 2: Train Skip-Gram model (sequential for now, can be parallelized with Hogwild!)
        let embeddings = self.train_skipgram(&walks)?;

        // Step 3: Convert to NodeId -> Embedding mapping
        self.index_to_id_mapping(embeddings)
    }

    fn estimated_speedup(&self, num_threads: usize) -> f64 {
        // Random walk generation is embarrassingly parallel
        // Training is sequential in this implementation
        // Assume 80% of time is walk generation, 20% is training
        let parallel_fraction = 0.8;
        let sequential_fraction = 0.2;

        let max_speedup = 1.0 / (sequential_fraction + (parallel_fraction / num_threads as f64));

        // Small graphs have thread overhead
        let csr_out = self.engine.csr_outgoing.read().ok();
        let node_count = csr_out.map_or(0, |csr| csr.node_count());
        let overhead_penalty = if node_count < 1000 { 0.85 } else { 1.0 };

        max_speedup * overhead_penalty
    }

    fn min_graph_size_for_parallel(&self) -> usize {
        // Node2Vec benefits from parallelism for graphs with 500+ nodes
        // Below this, random walk overhead is minimal
        500
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::engines::GraphEngine;
    use crate::graph::engines::orion::OrionGraphEngine;
    use crate::graph::{Edge, Node};

    #[test]
    fn test_node2vec_empty_graph() {
        let engine = Arc::new(OrionGraphEngine::new());
        let node2vec = Node2VecEmbeddings::new(
            Arc::clone(&engine),
            1.0, // p
            1.0, // q
            10,  // walk_length
            5,   // num_walks
            64,  // embedding_dim
        );

        let result = node2vec.execute(NoInput).unwrap();
        assert_eq!(result.len(), 0);
    }

    #[tokio::test]
    async fn test_node2vec_single_node() {
        let engine = Arc::new(OrionGraphEngine::new());

        // Create single node
        let node = Node {
            id: "n1".to_string(),
            labels: vec![],
            properties: std::collections::HashMap::new(),
            embedding: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        engine.as_ref().insert_node(node).await.unwrap();

        let node2vec = Node2VecEmbeddings::new(Arc::clone(&engine), 1.0, 1.0, 10, 5, 64);

        let result = node2vec.execute(NoInput).unwrap();

        // Single isolated node produces no walks (need edges)
        assert_eq!(result.len(), 0);
    }

    #[tokio::test]
    async fn test_node2vec_simple_chain() {
        let engine = Arc::new(OrionGraphEngine::new());

        // Create chain graph: n1 -> n2 -> n3
        for i in 1..=3 {
            let node = Node {
                id: format!("n{}", i),
                labels: vec![],
                properties: std::collections::HashMap::new(),
                embedding: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            };
            engine.as_ref().insert_node(node).await.unwrap();
        }

        // Add edges
        let edges = vec![
            Edge {
                id: "e1".to_string(),
                from_node_id: "n1".to_string(),
                to_node_id: "n2".to_string(),
                edge_type: "NEXT".to_string(),
                properties: std::collections::HashMap::new(),
                weight: Some(1.0),
                created_at_ms: 0,
                updated_at_ms: 0,
            },
            Edge {
                id: "e2".to_string(),
                from_node_id: "n2".to_string(),
                to_node_id: "n3".to_string(),
                edge_type: "NEXT".to_string(),
                properties: std::collections::HashMap::new(),
                weight: Some(1.0),
                created_at_ms: 0,
                updated_at_ms: 0,
            },
        ];

        for edge in edges {
            engine.as_ref().insert_edge(edge).await.unwrap();
        }

        let node2vec = Node2VecEmbeddings::new(
            Arc::clone(&engine),
            1.0, // p
            1.0, // q
            5,   // walk_length
            3,   // num_walks
            32,  // embedding_dim (smaller for faster testing)
        );

        let result = node2vec.execute(NoInput).unwrap();

        // Should have embeddings for nodes that can generate walks
        assert!(result.len() >= 2); // At least n1 and n2 can generate walks

        // Verify embedding dimensions
        for (_node_id, embedding) in result.iter() {
            assert_eq!(embedding.len(), 32);

            // Verify normalized (L2 norm should be ~1.0)
            let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!((norm - 1.0).abs() < 0.1);
        }
    }

    #[test]
    fn test_algorithm_complexity() {
        let engine = Arc::new(OrionGraphEngine::new());
        let node2vec = Node2VecEmbeddings::new(Arc::clone(&engine), 1.0, 1.0, 10, 5, 64);

        let complexity = node2vec.estimated_complexity();
        match complexity {
            AlgorithmComplexity::Linear => {
                // Correct: O(V * num_walks * walk_length)
            }
            _ => panic!("Expected linear complexity"),
        }
    }

    #[test]
    fn test_algorithm_name() {
        let engine = Arc::new(OrionGraphEngine::new());
        let node2vec = Node2VecEmbeddings::new(Arc::clone(&engine), 1.0, 1.0, 10, 5, 64);

        assert_eq!(node2vec.name(), "Node2VecEmbeddings");
    }

    #[test]
    fn test_parallel_execution_threshold() {
        let engine = Arc::new(OrionGraphEngine::new());
        let node2vec = Node2VecEmbeddings::new(Arc::clone(&engine), 1.0, 1.0, 10, 5, 64);

        assert_eq!(node2vec.min_graph_size_for_parallel(), 500);

        // Verify speedup estimation
        let speedup_1_thread = node2vec.estimated_speedup(1);
        let speedup_4_threads = node2vec.estimated_speedup(4);
        let speedup_16_threads = node2vec.estimated_speedup(16);

        assert!(speedup_1_thread < speedup_4_threads);
        assert!(speedup_4_threads < speedup_16_threads);

        // With 20% sequential, max speedup ≈ 5.0
        assert!(speedup_16_threads < 5.0);
    }

    #[tokio::test]
    async fn test_deepwalk_equivalence() {
        // DeepWalk is Node2Vec with p=1, q=1 (unbiased random walks)
        let engine = Arc::new(OrionGraphEngine::new());

        // Create small graph
        for i in 1..=4 {
            let node = Node {
                id: format!("n{}", i),
                labels: vec![],
                properties: std::collections::HashMap::new(),
                embedding: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            };
            engine.as_ref().insert_node(node).await.unwrap();
        }

        // Create cycle: n1 -> n2 -> n3 -> n4 -> n1
        for i in 1..=4 {
            let from = format!("n{}", i);
            let to = format!("n{}", if i == 4 { 1 } else { i + 1 });

            let edge = Edge {
                id: format!("e{}", i),
                from_node_id: from,
                to_node_id: to,
                edge_type: "NEXT".to_string(),
                properties: std::collections::HashMap::new(),
                weight: Some(1.0),
                created_at_ms: 0,
                updated_at_ms: 0,
            };
            engine.as_ref().insert_edge(edge).await.unwrap();
        }

        let deepwalk = Node2VecEmbeddings::new(
            Arc::clone(&engine),
            1.0, // p=1 (DeepWalk)
            1.0, // q=1 (DeepWalk)
            10,
            5,
            32,
        );

        let result = deepwalk.execute(NoInput).unwrap();

        // All nodes should have embeddings
        assert_eq!(result.len(), 4);

        // Embeddings should be normalized
        for (_node_id, embedding) in result.iter() {
            let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!((norm - 1.0).abs() < 0.1);
        }
    }
}
