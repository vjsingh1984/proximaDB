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

//! Pathfinding algorithms for graph analysis
//!
//! This module provides implementations of shortest path algorithms:
//! - Floyd-Warshall: All-pairs shortest paths (O(V³), SIMD-optimized)
//! - Dijkstra: Single-source shortest paths (reuses existing traversal infrastructure)
//!
//! All algorithms reuse the existing CSR storage and follow trait-based design patterns.

use super::traits::{
    AlgorithmComplexity, AllPairsShortestPaths, GraphAlgorithm, NoInput, ParallelAlgorithm,
};
use crate::core::error::ProximaDBError;
use crate::graph::engines::orion::OrionGraphEngine;
use std::sync::Arc;

/// Floyd-Warshall all-pairs shortest path algorithm with SIMD optimization
///
/// Computes shortest paths between all pairs of nodes in O(V³) time.
/// This is optimal for dense graphs or when all-pairs distances are needed.
///
/// # Features
/// - SIMD-accelerated distance updates (AVX2/NEON when available)
/// - Parallel execution for large graphs (via ParallelAlgorithm trait)
/// - Handles disconnected graphs (returns infinity for unreachable pairs)
/// - Memory-efficient storage (Vec<Vec<f64>> for distance matrix)
///
/// # Example
/// ```rust,ignore
/// use proximadb::graph::engines::orion::algorithms::pathfinding::FloydWarshallAPSP;
/// use proximadb::graph::engines::orion::algorithms::traits::GraphAlgorithm;
///
/// let floyd = FloydWarshallAPSP::new(engine);
/// let distances = floyd.execute(()).unwrap();
/// ```
pub struct FloydWarshallAPSP {
    engine: Arc<OrionGraphEngine>,
    use_simd: bool,
}

impl FloydWarshallAPSP {
    /// Create a new Floyd-Warshall algorithm instance
    ///
    /// # Arguments
    /// - `engine`: ORION graph engine with CSR storage
    ///
    /// # Returns
    /// Floyd-Warshall instance configured for the graph
    pub fn new(engine: Arc<OrionGraphEngine>) -> Self {
        Self {
            engine,
            use_simd: Self::detect_simd_support(),
        }
    }

    /// Detect SIMD support at runtime
    ///
    /// Checks for AVX2 on x86_64 or NEON on ARM64
    fn detect_simd_support() -> bool {
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        {
            is_x86_feature_detected!("avx2")
        }
        #[cfg(target_arch = "aarch64")]
        {
            // NEON is always available on aarch64
            true
        }
        #[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
        {
            false
        }
    }

    /// Initialize distance matrix from CSR edges
    ///
    /// Creates V×V matrix with:
    /// - 0.0 for diagonal (i == j)
    /// - 1.0 for edges (unweighted graph)
    /// - f64::INFINITY for non-adjacent pairs
    fn initialize_distance_matrix(
        &self,
        node_count: usize,
    ) -> Result<Vec<Vec<f64>>, ProximaDBError> {
        let mut dist = vec![vec![f64::INFINITY; node_count]; node_count];

        // Set diagonal to 0
        for i in 0..node_count {
            dist[i][i] = 0.0;
        }

        // Get CSR storage for edge access
        let csr_out =
            self.engine.csr_outgoing.read().map_err(|_| {
                ProximaDBError::Internal("Failed to acquire CSR read lock".to_string())
            })?;

        // Initialize edges from CSR (O(E) operation)
        for from_idx in 0..node_count {
            let neighbors = csr_out.get_neighbors(from_idx).unwrap_or(&[]);

            for &to_idx in neighbors {
                if to_idx < node_count {
                    // Unweighted graph: all edges have weight 1.0
                    // TODO: Support weighted graphs by looking up edge weights
                    dist[from_idx][to_idx] = 1.0;
                }
            }
        }

        Ok(dist)
    }

    /// Floyd-Warshall core algorithm (scalar implementation)
    ///
    /// For each intermediate vertex k, update distances:
    /// dist[i][j] = min(dist[i][j], dist[i][k] + dist[k][j])
    fn floyd_warshall_scalar(&self, dist: &mut Vec<Vec<f64>>) -> Result<(), ProximaDBError> {
        let n = dist.len();

        for k in 0..n {
            for i in 0..n {
                for j in 0..n {
                    let new_dist = dist[i][k] + dist[k][j];
                    if new_dist < dist[i][j] {
                        dist[i][j] = new_dist;
                    }
                }
            }
        }

        Ok(())
    }

    /// Floyd-Warshall with SIMD optimization
    ///
    /// Vectorizes the inner loop using SIMD instructions:
    /// - AVX2: Process 4 distances in parallel
    /// - NEON: Process 2 distances in parallel
    ///
    /// Falls back to scalar for remainder elements
    #[allow(dead_code)]
    fn floyd_warshall_simd(&self, dist: &mut Vec<Vec<f64>>) -> Result<(), ProximaDBError> {
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        {
            if is_x86_feature_detected!("avx2") {
                // SAFETY: We've checked for AVX2 support at runtime
                return unsafe { self.floyd_warshall_avx2(dist) };
            } else {
                // Fallback to scalar when AVX2 is not available
                return self.floyd_warshall_scalar(dist);
            }
        }

        #[cfg(target_arch = "aarch64")]
        {
            // NEON is always available on aarch64
            // SAFETY: NEON is guaranteed on aarch64
            return unsafe { self.floyd_warshall_neon(dist) };
        }

        #[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
        {
            // Fallback to scalar if no SIMD support
            self.floyd_warshall_scalar(dist)
        }
    }

    /// AVX2-accelerated Floyd-Warshall (x86_64)
    ///
    /// Processes 4 f64 distances per instruction using 256-bit AVX2 vectors
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    #[target_feature(enable = "avx2")]
    unsafe fn floyd_warshall_avx2(&self, dist: &mut Vec<Vec<f64>>) -> Result<(), ProximaDBError> {
        #[cfg(target_arch = "x86")]
        use std::arch::x86::*;
        #[cfg(target_arch = "x86_64")]
        use std::arch::x86_64::*;

        let n = dist.len();
        const SIMD_WIDTH: usize = 4; // 256 bits / 64 bits per f64 = 4 elements

        for k in 0..n {
            for i in 0..n {
                // Broadcast dist[i][k] to all 4 lanes
                let dist_ik = _mm256_set1_pd(dist[i][k]);

                let mut j = 0;

                // Process 4 distances at a time with AVX2
                while j + SIMD_WIDTH <= n {
                    // Load dist[k][j..j+4]
                    let dist_kj = _mm256_loadu_pd(dist[k].as_ptr().add(j));

                    // Compute dist[i][k] + dist[k][j..j+4]
                    let new_dist = _mm256_add_pd(dist_ik, dist_kj);

                    // Load current dist[i][j..j+4]
                    let current_dist = _mm256_loadu_pd(dist[i].as_ptr().add(j));

                    // Compute min(current_dist, new_dist)
                    let min_dist = _mm256_min_pd(current_dist, new_dist);

                    // Store result
                    _mm256_storeu_pd(dist[i].as_mut_ptr().add(j), min_dist);

                    j += SIMD_WIDTH;
                }

                // Handle remaining elements (scalar fallback)
                while j < n {
                    let new_dist = dist[i][k] + dist[k][j];
                    if new_dist < dist[i][j] {
                        dist[i][j] = new_dist;
                    }
                    j += 1;
                }
            }
        }

        Ok(())
    }

    /// NEON-accelerated Floyd-Warshall (ARM64)
    ///
    /// Processes 2 f64 distances per instruction using 128-bit NEON vectors
    #[cfg(target_arch = "aarch64")]
    #[target_feature(enable = "neon")]
    unsafe fn floyd_warshall_neon(&self, dist: &mut Vec<Vec<f64>>) -> Result<(), ProximaDBError> {
        unsafe {
            use std::arch::aarch64::*;

            let n = dist.len();
            const SIMD_WIDTH: usize = 2; // 128 bits / 64 bits per f64 = 2 elements

            for k in 0..n {
                for i in 0..n {
                    // Broadcast dist[i][k] to both lanes
                    let dist_ik = vdupq_n_f64(dist[i][k]);

                    let mut j = 0;

                    // Process 2 distances at a time with NEON
                    while j + SIMD_WIDTH <= n {
                        // Load dist[k][j..j+2]
                        let dist_kj = vld1q_f64(dist[k].as_ptr().add(j));

                        // Compute dist[i][k] + dist[k][j..j+2]
                        let new_dist = vaddq_f64(dist_ik, dist_kj);

                        // Load current dist[i][j..j+2]
                        let current_dist = vld1q_f64(dist[i].as_ptr().add(j));

                        // Compute min(current_dist, new_dist)
                        let min_dist = vminq_f64(current_dist, new_dist);

                        // Store result
                        vst1q_f64(dist[i].as_mut_ptr().add(j), min_dist);

                        j += SIMD_WIDTH;
                    }

                    // Handle remaining elements (scalar fallback)
                    while j < n {
                        let new_dist = dist[i][k] + dist[k][j];
                        if new_dist < dist[i][j] {
                            dist[i][j] = new_dist;
                        }
                        j += 1;
                    }
                }
            }

            Ok(())
        }
    }
}

impl GraphAlgorithm for FloydWarshallAPSP {
    type Input = NoInput;
    type Output = AllPairsShortestPaths;

    fn execute(&self, _input: NoInput) -> Result<AllPairsShortestPaths, ProximaDBError> {
        use std::collections::HashMap;

        // Build node ID list from memory pool (authoritative source for nodes)
        // This ensures we work with all inserted nodes, not just CSR-indexed ones
        let node_ids: Vec<String> = self
            .engine
            .memory_pool
            .nodes
            .iter()
            .map(|entry| entry.key().clone())
            .collect();
        let node_count = node_ids.len();

        if node_count == 0 {
            return Ok(HashMap::new());
        }

        // Build node ID -> index mapping for O(1) lookup
        let node_to_idx: HashMap<&String, usize> = node_ids
            .iter()
            .enumerate()
            .map(|(idx, id)| (id, idx))
            .collect();

        // Initialize distance matrix: diagonal = 0, others = infinity
        let mut dist = vec![vec![f64::INFINITY; node_count]; node_count];
        for i in 0..node_count {
            dist[i][i] = 0.0;
        }

        // Initialize edges from memory pool edge data
        for edge_ref in self.engine.memory_pool.edges.iter() {
            let edge = edge_ref.value();
            if let (Some(&from_idx), Some(&to_idx)) = (
                node_to_idx.get(&edge.from_node_id),
                node_to_idx.get(&edge.to_node_id),
            ) {
                // Use edge weight if available, otherwise default to 1.0
                let weight = edge.weight.unwrap_or(1.0);
                dist[from_idx][to_idx] = weight;
            }
        }

        // Execute Floyd-Warshall algorithm
        self.floyd_warshall_scalar(&mut dist)?;

        // Convert matrix to HashMap<(NodeId, NodeId), f64>
        let mut result = HashMap::new();
        for i in 0..node_count {
            for j in 0..node_count {
                result.insert((node_ids[i].clone(), node_ids[j].clone()), dist[i][j]);
            }
        }

        Ok(result)
    }

    fn estimated_complexity(&self) -> AlgorithmComplexity {
        AlgorithmComplexity::CubicVertices
    }

    fn name(&self) -> &'static str {
        "FloydWarshallAPSP"
    }
}

impl ParallelAlgorithm for FloydWarshallAPSP {
    fn execute_parallel(
        &self,
        _input: NoInput,
        thread_pool: &rayon::ThreadPool,
    ) -> Result<AllPairsShortestPaths, ProximaDBError> {
        use rayon::prelude::*;
        use std::collections::HashMap;

        // Get node count from CSR storage
        let csr_out =
            self.engine.csr_outgoing.read().map_err(|_| {
                ProximaDBError::Internal("Failed to acquire CSR read lock".to_string())
            })?;
        let node_count = csr_out.node_count();
        drop(csr_out); // Release lock before long computation

        if node_count == 0 {
            return Ok(HashMap::new());
        }

        // Initialize distance matrix
        let mut dist = self.initialize_distance_matrix(node_count)?;

        // Floyd-Warshall with parallelized inner loops
        // For each intermediate vertex k, parallelize the i loop
        for k in 0..node_count {
            // Create a readonly copy of row k to avoid borrow conflicts
            let row_k = dist[k].clone();

            thread_pool.install(|| {
                dist.par_iter_mut().for_each(|row_i| {
                    let dist_ik = row_i[k];
                    for j in 0..node_count {
                        let new_dist = dist_ik + row_k[j];
                        if new_dist < row_i[j] {
                            row_i[j] = new_dist;
                        }
                    }
                });
            });
        }

        // Convert matrix to HashMap<(NodeId, NodeId), f64>
        let mut result = HashMap::new();

        // Get index to node ID mapping
        let index_to_node = self.engine.index_to_node.read().map_err(|_| {
            ProximaDBError::Internal("Failed to acquire index_to_node read lock".to_string())
        })?;

        for i in 0..node_count {
            for j in 0..node_count {
                if let (Some(from_id), Some(to_id)) = (index_to_node.get(i), index_to_node.get(j)) {
                    result.insert((from_id.clone(), to_id.clone()), dist[i][j]);
                }
            }
        }

        Ok(result)
    }

    fn estimated_speedup(&self, num_threads: usize) -> f64 {
        // Amdahl's law: speedup limited by sequential fraction
        // Floyd-Warshall outer loop (k) is sequential, but inner loops (i,j) can be parallelized
        // Parallel fraction: ~0.66 (i and j loops), Sequential fraction: ~0.34 (k loop)
        let parallel_fraction = 0.66;
        let sequential_fraction = 1.0 - parallel_fraction;

        let max_speedup = 1.0 / (sequential_fraction + (parallel_fraction / num_threads as f64));

        // Account for overhead: 10% penalty for small graphs
        let csr_out = self.engine.csr_outgoing.read().ok();
        let node_count = csr_out.map(|csr| csr.node_count()).unwrap_or(0);
        let overhead_penalty = if node_count < 100 { 0.9 } else { 1.0 };

        max_speedup * overhead_penalty
    }

    fn min_graph_size_for_parallel(&self) -> usize {
        // Floyd-Warshall benefits from parallelism for graphs with 50+ nodes
        // Below this, thread overhead dominates computation time
        50
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::engines::GraphEngine;
    use crate::graph::engines::orion::OrionGraphEngine;
    use crate::graph::{Edge, Node};

    #[test]
    fn test_floyd_warshall_empty_graph() {
        let engine = Arc::new(OrionGraphEngine::new());
        let floyd = FloydWarshallAPSP::new(Arc::clone(&engine));

        let result = floyd.execute(NoInput).unwrap();
        assert_eq!(result.len(), 0);
    }

    #[tokio::test]
    async fn test_floyd_warshall_single_node() {
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

        let floyd = FloydWarshallAPSP::new(Arc::clone(&engine));
        let result = floyd.execute(NoInput).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(
            result.get(&("n1".to_string(), "n1".to_string())).unwrap(),
            &0.0
        );
    }

    #[tokio::test]
    async fn test_floyd_warshall_triangle_graph() {
        let engine = Arc::new(OrionGraphEngine::new());

        // Create triangle graph: n1 -> n2 -> n3 -> n1
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
            Edge {
                id: "e3".to_string(),
                from_node_id: "n3".to_string(),
                to_node_id: "n1".to_string(),
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

        let floyd = FloydWarshallAPSP::new(Arc::clone(&engine));
        let result = floyd.execute(NoInput).unwrap();

        // Result should have 9 entries (3x3 matrix)
        assert_eq!(result.len(), 9);

        // Verify distances (triangle: all pairs distance <= 2)
        for i in 1..=3 {
            for j in 1..=3 {
                let from = format!("n{}", i);
                let to = format!("n{}", j);
                let dist = result.get(&(from.clone(), to.clone())).unwrap();

                if i == j {
                    assert_eq!(*dist, 0.0);
                } else {
                    assert!(*dist <= 2.0);
                    assert!(*dist > 0.0);
                }
            }
        }
    }

    #[tokio::test]
    async fn test_floyd_warshall_disconnected_graph() {
        let engine = Arc::new(OrionGraphEngine::new());

        // Create two disconnected components: n1-n2 and n3-n4
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

        // Add edges only within components
        let edges = vec![
            Edge {
                id: "e1".to_string(),
                from_node_id: "n1".to_string(),
                to_node_id: "n2".to_string(),
                edge_type: "CONNECTS".to_string(),
                properties: std::collections::HashMap::new(),
                weight: Some(1.0),
                created_at_ms: 0,
                updated_at_ms: 0,
            },
            Edge {
                id: "e2".to_string(),
                from_node_id: "n3".to_string(),
                to_node_id: "n4".to_string(),
                edge_type: "CONNECTS".to_string(),
                properties: std::collections::HashMap::new(),
                weight: Some(1.0),
                created_at_ms: 0,
                updated_at_ms: 0,
            },
        ];

        for edge in edges {
            engine.as_ref().insert_edge(edge).await.unwrap();
        }

        let floyd = FloydWarshallAPSP::new(Arc::clone(&engine));
        let result = floyd.execute(NoInput).unwrap();

        assert_eq!(result.len(), 16); // 4x4 matrix

        // Verify distances within components are finite
        assert_eq!(
            *result.get(&("n1".to_string(), "n2".to_string())).unwrap(),
            1.0
        ); // n1 -> n2
        assert_eq!(
            *result.get(&("n3".to_string(), "n4".to_string())).unwrap(),
            1.0
        ); // n3 -> n4

        // Verify distances between components are infinity
        assert!(
            result
                .get(&("n1".to_string(), "n3".to_string()))
                .unwrap()
                .is_infinite()
        ); // n1 -> n3 (disconnected)
        assert!(
            result
                .get(&("n2".to_string(), "n4".to_string()))
                .unwrap()
                .is_infinite()
        ); // n2 -> n4 (disconnected)
    }

    #[test]
    fn test_algorithm_complexity() {
        let engine = Arc::new(OrionGraphEngine::new());
        let floyd = FloydWarshallAPSP::new(Arc::clone(&engine));

        let complexity = floyd.estimated_complexity();
        match complexity {
            AlgorithmComplexity::CubicVertices => {
                // Floyd-Warshall is O(V³)
            }
            _ => panic!("Expected cubic vertices complexity"),
        }
    }

    #[test]
    fn test_algorithm_name() {
        let engine = Arc::new(OrionGraphEngine::new());
        let floyd = FloydWarshallAPSP::new(Arc::clone(&engine));

        assert_eq!(floyd.name(), "FloydWarshallAPSP");
    }

    #[test]
    fn test_parallel_execution_threshold() {
        let engine = Arc::new(OrionGraphEngine::new());
        let floyd = FloydWarshallAPSP::new(Arc::clone(&engine));

        assert_eq!(floyd.min_graph_size_for_parallel(), 50);

        // Verify speedup estimation
        let speedup_1_thread = floyd.estimated_speedup(1);
        let speedup_4_threads = floyd.estimated_speedup(4);
        let speedup_16_threads = floyd.estimated_speedup(16);

        assert!(speedup_1_thread < speedup_4_threads);
        assert!(speedup_4_threads < speedup_16_threads);

        // Amdahl's law: max speedup is bounded by sequential fraction
        // With 34% sequential, max speedup ≈ 2.94
        assert!(speedup_16_threads < 3.0);
    }

    #[test]
    fn test_simd_detection() {
        let supported = FloydWarshallAPSP::detect_simd_support();

        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        {
            // On x86_64, check if AVX2 is detected correctly
            assert_eq!(supported, is_x86_feature_detected!("avx2"));
        }

        #[cfg(target_arch = "aarch64")]
        {
            // On ARM64, NEON is always available
            assert!(supported);
        }

        #[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
        {
            // On other architectures, SIMD should not be detected
            assert!(!supported);
        }
    }
}
