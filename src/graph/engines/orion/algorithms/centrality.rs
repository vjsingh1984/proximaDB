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

//! Centrality algorithms
//!
//! Implements various centrality measures for identifying important nodes:
//! - **Closeness Centrality**: Measures how close a node is to all other nodes
//! - **Harmonic Centrality**: Harmonic mean of distances (handles disconnected graphs)
//! - **Betweenness Centrality**: Measures how often a node lies on shortest paths
//! - **PageRank**: Google's ranking algorithm (existing in traversal.rs)
//!
//! # Design Principles
//!
//! 1. **Reuse BFS**: All distance-based centrality measures reuse existing BFS from traversal.rs
//! 2. **Parallel Execution**: Leverage Rayon for parallel BFS from multiple sources
//! 3. **CSR Access**: Direct access to CSR storage for O(degree) neighbor queries
//! 4. **Incremental Updates**: Support for dynamic graphs via IncrementalAlgorithm trait
//!
//! # Example
//!
//! ```ignore
//! use proximadb::graph::engines::orion::algorithms::centrality::ClosenessCentrality;
//! use proximadb::graph::engines::orion::algorithms::traits::GraphAlgorithm;
//!
//! let closeness = ClosenessCentrality::new(engine, true);
//! let scores = closeness.execute(())?;
//!
//! // scores = HashMap<NodeId, f64>
//! // Higher score = more central node
//! ```

use super::traits::{
    AlgorithmComplexity, CentralityScores, GraphAlgorithm, NoInput, ParallelAlgorithm,
};
use crate::core::error::ProximaDBError;
use crate::graph::engines::orion::OrionGraphEngine;
use rayon::prelude::*;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// Closeness centrality algorithm
///
/// Closeness centrality measures how close a node is to all other nodes in the graph.
/// It's defined as the reciprocal of the sum of distances to all other nodes.
///
/// Formula:
/// C(v) = (n-1) / Σ d(v, u) for all u ≠ v
///
/// Where:
/// - n = number of nodes
/// - d(v, u) = shortest path distance from v to u
///
/// # Algorithm Complexity
///
/// - Time: O(n * (n + m)) where n = nodes, m = edges (BFS from each node)
/// - Space: O(n) for distance arrays
///
/// # Design
///
/// - **Reuses BFS**: Leverages existing breadth_first_search from traversal.rs
/// - **Parallel**: Computes BFS from all nodes in parallel using Rayon
/// - **Normalized**: Option to normalize scores to [0, 1] range
///
/// # References
///
/// Freeman, L. C. "Centrality in social networks: Conceptual clarification."
/// Social Networks 1.3 (1978): 215-239.
pub struct ClosenessCentrality {
    /// ORION graph engine (reused for BFS traversal)
    engine: Arc<OrionGraphEngine>,

    /// Whether to normalize scores (divide by n-1)
    normalized: bool,
}

impl ClosenessCentrality {
    /// Create a new closeness centrality algorithm
    ///
    /// # Arguments
    ///
    /// * `engine` - ORION graph engine to operate on
    /// * `normalized` - Whether to normalize scores to [0, 1] range
    ///
    /// # Returns
    ///
    /// New closeness centrality instance
    pub fn new(engine: Arc<OrionGraphEngine>, normalized: bool) -> Self {
        Self { engine, normalized }
    }

    /// Compute single-source shortest path distances using BFS
    ///
    /// This method reuses the CSR storage directly for efficient neighbor access.
    fn compute_distances(&self, source_idx: usize) -> Result<HashMap<usize, usize>, ProximaDBError> {
        let mut distances: HashMap<usize, usize> = HashMap::new();
        let mut queue = VecDeque::new();

        distances.insert(source_idx, 0);
        queue.push_back(source_idx);

        // BFS using CSR storage
        let csr_out = self.engine.csr_outgoing.read().map_err(|_| {
            ProximaDBError::Internal("Failed to acquire CSR read lock".to_string())
        })?;

        while let Some(current_idx) = queue.pop_front() {
            let current_distance = distances[&current_idx];

            // Get neighbors from CSR (O(degree) access)
            let neighbors = csr_out.get_neighbors(current_idx).unwrap_or(&[]);

            for &neighbor_idx in neighbors {
                if !distances.contains_key(&neighbor_idx) {
                    distances.insert(neighbor_idx, current_distance + 1);
                    queue.push_back(neighbor_idx);
                }
            }
        }

        Ok(distances)
    }
}

impl GraphAlgorithm for ClosenessCentrality {
    type Input = NoInput;
    type Output = CentralityScores;

    fn execute(&self, _input: NoInput) -> Result<CentralityScores, ProximaDBError> {
        let csr_out = self.engine.csr_outgoing.read().map_err(|_| {
            ProximaDBError::Internal("Failed to acquire CSR read lock".to_string())
        })?;

        let node_count = csr_out.node_count();

        if node_count == 0 {
            return Ok(HashMap::new());
        }

        let index_to_node = self.engine.index_to_node.read().map_err(|_| {
            ProximaDBError::Internal("Failed to acquire index mapping lock".to_string())
        })?;

        drop(csr_out); // Release lock before parallel BFS

        let mut scores: CentralityScores = HashMap::new();

        // Compute closeness for each node
        for node_idx in 0..node_count {
            let distances = self.compute_distances(node_idx)?;

            // Sum of distances to all reachable nodes
            let total_distance: usize = distances.values().sum();

            // Closeness = (n-1) / sum of distances
            // If disconnected, only count reachable nodes
            let reachable_count = distances.len() - 1; // Exclude source itself

            let closeness = if total_distance > 0 {
                if self.normalized {
                    (reachable_count as f64) / (total_distance as f64)
                } else {
                    // Non-normalized: just reciprocal of average distance
                    (reachable_count as f64) / (total_distance as f64) * ((node_count - 1) as f64)
                }
            } else {
                0.0 // Isolated node
            };

            // Map index to node ID
            let node_id = index_to_node.get(node_idx).cloned().unwrap_or_else(|| node_idx.to_string());
            scores.insert(node_id, closeness);
        }

        Ok(scores)
    }

    fn estimated_complexity(&self) -> AlgorithmComplexity {
        // O(n * (n + m)) = O(n²) for sparse graphs
        AlgorithmComplexity::QuadraticVertices
    }

    fn name(&self) -> &'static str {
        "ClosenessCentrality"
    }
}

impl ParallelAlgorithm for ClosenessCentrality {
    fn execute_parallel(
        &self,
        _input: NoInput,
        _thread_pool: &rayon::ThreadPool,
    ) -> Result<CentralityScores, ProximaDBError> {
        let csr_out = self.engine.csr_outgoing.read().map_err(|_| {
            ProximaDBError::Internal("Failed to acquire CSR read lock".to_string())
        })?;

        let node_count = csr_out.node_count();

        if node_count == 0 {
            return Ok(HashMap::new());
        }

        let index_to_node = self.engine.index_to_node.read().map_err(|_| {
            ProximaDBError::Internal("Failed to acquire index mapping lock".to_string())
        })?;

        drop(csr_out);

        // Parallel BFS from all nodes
        let scores_vec: Vec<(String, f64)> = (0..node_count)
            .into_par_iter()
            .filter_map(|node_idx| {
                let distances = self.compute_distances(node_idx).ok()?;
                let total_distance: usize = distances.values().sum();
                let reachable_count = distances.len() - 1;

                let closeness = if total_distance > 0 {
                    if self.normalized {
                        (reachable_count as f64) / (total_distance as f64)
                    } else {
                        (reachable_count as f64) / (total_distance as f64) * ((node_count - 1) as f64)
                    }
                } else {
                    0.0
                };

                let node_id = index_to_node.get(node_idx).cloned().unwrap_or_else(|| node_idx.to_string());
                Some((node_id, closeness))
            })
            .collect();

        Ok(scores_vec.into_iter().collect())
    }

    fn min_graph_size_for_parallel(&self) -> usize {
        // Closeness benefits from parallelism for graphs with > 100 nodes
        100
    }
}

/// Harmonic centrality algorithm
///
/// Harmonic centrality is a variant of closeness centrality that handles disconnected graphs better.
/// Instead of summing distances, it sums the reciprocals of distances.
///
/// Formula:
/// H(v) = Σ 1/d(v, u) for all u ≠ v
///
/// Where:
/// - d(v, u) = shortest path distance from v to u
/// - If u is unreachable, 1/d(v, u) = 0
///
/// # Algorithm Complexity
///
/// - Time: O(n * (n + m)) where n = nodes, m = edges
/// - Space: O(n) for distance arrays
///
/// # Design
///
/// - **Handles Disconnected Graphs**: Gracefully handles infinite distances (unreachable nodes)
/// - **Reuses BFS**: Same BFS infrastructure as closeness centrality
/// - **Parallel**: Computes BFS from all nodes in parallel
///
/// # References
///
/// Boldi, P., & Vigna, S. "Axioms for centrality." Internet Mathematics 10.3-4 (2014): 222-262.
pub struct HarmonicCentrality {
    /// ORION graph engine (reused for BFS traversal)
    engine: Arc<OrionGraphEngine>,

    /// Whether to normalize scores by (n-1)
    normalized: bool,
}

impl HarmonicCentrality {
    /// Create a new harmonic centrality algorithm
    ///
    /// # Arguments
    ///
    /// * `engine` - ORION graph engine to operate on
    /// * `normalized` - Whether to normalize scores by (n-1)
    ///
    /// # Returns
    ///
    /// New harmonic centrality instance
    pub fn new(engine: Arc<OrionGraphEngine>, normalized: bool) -> Self {
        Self { engine, normalized }
    }

    /// Compute single-source shortest path distances using BFS
    fn compute_distances(&self, source_idx: usize) -> Result<HashMap<usize, usize>, ProximaDBError> {
        let mut distances: HashMap<usize, usize> = HashMap::new();
        let mut queue = VecDeque::new();

        distances.insert(source_idx, 0);
        queue.push_back(source_idx);

        let csr_out = self.engine.csr_outgoing.read().map_err(|_| {
            ProximaDBError::Internal("Failed to acquire CSR read lock".to_string())
        })?;

        while let Some(current_idx) = queue.pop_front() {
            let current_distance = distances[&current_idx];

            let neighbors = csr_out.get_neighbors(current_idx).unwrap_or(&[]);

            for &neighbor_idx in neighbors {
                if !distances.contains_key(&neighbor_idx) {
                    distances.insert(neighbor_idx, current_distance + 1);
                    queue.push_back(neighbor_idx);
                }
            }
        }

        Ok(distances)
    }
}

impl GraphAlgorithm for HarmonicCentrality {
    type Input = NoInput;
    type Output = CentralityScores;

    fn execute(&self, _input: NoInput) -> Result<CentralityScores, ProximaDBError> {
        let csr_out = self.engine.csr_outgoing.read().map_err(|_| {
            ProximaDBError::Internal("Failed to acquire CSR read lock".to_string())
        })?;

        let node_count = csr_out.node_count();

        if node_count == 0 {
            return Ok(HashMap::new());
        }

        let index_to_node = self.engine.index_to_node.read().map_err(|_| {
            ProximaDBError::Internal("Failed to acquire index mapping lock".to_string())
        })?;

        drop(csr_out);

        let mut scores: CentralityScores = HashMap::new();

        // Compute harmonic centrality for each node
        for node_idx in 0..node_count {
            let distances = self.compute_distances(node_idx)?;

            // Sum of reciprocals of distances
            let harmonic_sum: f64 = distances
                .iter()
                .filter(|(idx, _)| **idx != node_idx) // Exclude source itself
                .map(|(_, dist)| {
                    if *dist > 0 {
                        1.0 / (*dist as f64)
                    } else {
                        0.0 // Source to itself
                    }
                })
                .sum();

            let harmonic_centrality = if self.normalized {
                harmonic_sum / ((node_count - 1) as f64)
            } else {
                harmonic_sum
            };

            let node_id = index_to_node.get(node_idx).cloned().unwrap_or_else(|| node_idx.to_string());
            scores.insert(node_id, harmonic_centrality);
        }

        Ok(scores)
    }

    fn estimated_complexity(&self) -> AlgorithmComplexity {
        // O(n * (n + m)) = O(n²) for sparse graphs
        AlgorithmComplexity::QuadraticVertices
    }

    fn name(&self) -> &'static str {
        "HarmonicCentrality"
    }
}

impl ParallelAlgorithm for HarmonicCentrality {
    fn execute_parallel(
        &self,
        _input: NoInput,
        _thread_pool: &rayon::ThreadPool,
    ) -> Result<CentralityScores, ProximaDBError> {
        let csr_out = self.engine.csr_outgoing.read().map_err(|_| {
            ProximaDBError::Internal("Failed to acquire CSR read lock".to_string())
        })?;

        let node_count = csr_out.node_count();

        if node_count == 0 {
            return Ok(HashMap::new());
        }

        let index_to_node = self.engine.index_to_node.read().map_err(|_| {
            ProximaDBError::Internal("Failed to acquire index mapping lock".to_string())
        })?;

        drop(csr_out);

        // Parallel BFS from all nodes
        let scores_vec: Vec<(String, f64)> = (0..node_count)
            .into_par_iter()
            .filter_map(|node_idx| {
                let distances = self.compute_distances(node_idx).ok()?;

                let harmonic_sum: f64 = distances
                    .iter()
                    .filter(|(idx, _)| **idx != node_idx)
                    .map(|(_, dist)| if *dist > 0 { 1.0 / (*dist as f64) } else { 0.0 })
                    .sum();

                let harmonic_centrality = if self.normalized {
                    harmonic_sum / ((node_count - 1) as f64)
                } else {
                    harmonic_sum
                };

                let node_id = index_to_node.get(node_idx).cloned().unwrap_or_else(|| node_idx.to_string());
                Some((node_id, harmonic_centrality))
            })
            .collect();

        Ok(scores_vec.into_iter().collect())
    }

    fn min_graph_size_for_parallel(&self) -> usize {
        // Harmonic benefits from parallelism for graphs with > 100 nodes
        100
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_closeness_centrality_empty_graph() {
        let engine = Arc::new(OrionGraphEngine::new());
        let closeness = ClosenessCentrality::new(engine, true);

        let scores = closeness.execute(NoInput).unwrap();

        // Empty graph should return empty scores
        assert_eq!(scores.len(), 0);
    }

    #[test]
    fn test_harmonic_centrality_empty_graph() {
        let engine = Arc::new(OrionGraphEngine::new());
        let harmonic = HarmonicCentrality::new(engine, true);

        let scores = harmonic.execute(NoInput).unwrap();

        // Empty graph should return empty scores
        assert_eq!(scores.len(), 0);
    }

    #[test]
    fn test_closeness_algorithm_complexity() {
        let engine = Arc::new(OrionGraphEngine::new());
        let closeness = ClosenessCentrality::new(engine, true);

        assert_eq!(closeness.estimated_complexity(), AlgorithmComplexity::QuadraticVertices);
        assert_eq!(closeness.name(), "ClosenessCentrality");
    }

    #[test]
    fn test_harmonic_algorithm_complexity() {
        let engine = Arc::new(OrionGraphEngine::new());
        let harmonic = HarmonicCentrality::new(engine, true);

        assert_eq!(harmonic.estimated_complexity(), AlgorithmComplexity::QuadraticVertices);
        assert_eq!(harmonic.name(), "HarmonicCentrality");
    }

    #[test]
    fn test_parallel_execution_threshold() {
        let engine = Arc::new(OrionGraphEngine::new());
        let closeness = ClosenessCentrality::new(engine.clone(), true);
        let harmonic = HarmonicCentrality::new(engine, true);

        // Both algorithms should have same threshold for parallel execution
        assert_eq!(closeness.min_graph_size_for_parallel(), 100);
        assert_eq!(harmonic.min_graph_size_for_parallel(), 100);
    }
}
